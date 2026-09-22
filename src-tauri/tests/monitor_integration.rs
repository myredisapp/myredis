//! MONITOR 实时监控的集成用例（需本机 Redis，`#[ignore]`）。
//!
//! 默认连 `127.0.0.1:6379`，可用 `MYREDIS_TEST_PORT` 指向别的实例 ——
//! CI 的「Redis 版本矩阵」与本地多版本回归就是靠它把同一批用例指向不同版本的
//! Redis（见 DEVELOPMENT.md §2.4 / §2.3），例如 Docker 里的 6.0 / 7.x。
//!
//! 启动方式见 DEVELOPMENT.md §7。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

use maidi_cache_lib::commands::monitor::{run_monitor, MonitorLine};
use maidi_cache_lib::connection_pool::Pool;
use maidi_cache_lib::models::{ConnType, Connection};

/// 被测 Redis 的端口（默认与其它集成用例一致的本机 6379）。
fn test_port() -> u16 {
    std::env::var("MYREDIS_TEST_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6379)
}

fn conn_config(id: &str, port: u16) -> Connection {
    Connection {
        id: id.into(),
        name: "monitor integration".into(),
        host: "127.0.0.1".into(),
        port,
        conn_type: ConnType::Single,
        readonly: false,
        separator: ":".into(),
        db: 0,
        username: None,
        password: None,
        tls: false,
        tls_insecure: false,
        connect_timeout_secs: None,
        command_timeout_secs: None,
    }
}

/// 唯一名字，避免与同一实例上的其它用例（或历史数据）相互干扰。
fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}", nanos)
}

/// 收集到的监控行里，是否已有「第一个参数等于 `key`」的那条。
fn find_line_for(lines: &Arc<Mutex<Vec<MonitorLine>>>, key: &str) -> Option<MonitorLine> {
    let guard = lines.lock().unwrap();
    guard
        .iter()
        .find(|line| line.args.first().map(String::as_str) == Some(key))
        .cloned()
}

/// 在池里的连接上执行一条 `SET`（用来在监控流里制造一行）。
async fn set_via_pool(pool: &Pool, conn_id: &str, key: &str, value: &str) -> Result<(), String> {
    let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;
    con.query::<()>(redis::cmd("SET").arg(key).arg(value))
        .await
        .map_err(|e| e.to_string())
}

/// 监控流的核心行为：**真实的命令会以正确的参数出现在流里**，点停止后**立刻**不再有新行。
///
/// 覆盖三件事：
/// 1. 参数逐字还原 —— 含中文（服务器按字节写成 `\xHH`）与引号 / 反斜杠 / 换行等转义；
/// 2. 形态字段可用 —— 时间戳、db、发起命令的客户端地址都不为空；
/// 3. 停止生效 —— 收到停止信号后，后续命令不再进入收集到的行（而不是「还在读、只是不显示」）。
#[tokio::test]
#[ignore]
async fn monitor_streams_real_commands_and_stops_on_signal() -> Result<(), String> {
    let port = test_port();
    let pool = Pool::new();
    let cfg = conn_config("monitor_test", port);
    pool.connect(&cfg)
        .await
        .map_err(|e| format!("连接 Redis 失败（127.0.0.1:{port}）: {e}"))?;
    let timeout = pool.conn(&cfg.id).map_err(|e| e.to_string())?.timeout();

    let suffix = unique_suffix();
    let plain_key = format!("maidi:test:monitor:{suffix}");
    // 引号 / 反斜杠 / 换行 / 中文：中文在 MONITOR 里是逐字节的 \xHH
    let tricky_key = format!("中文键:{suffix}");
    let tricky_value = "a\"b\\c\nd\t中文";

    let lines: Arc<Mutex<Vec<MonitorLine>>> = Arc::new(Mutex::new(Vec::new()));
    let collected = Arc::clone(&lines);
    let stop = Arc::new(Notify::new());
    let runner = tokio::spawn({
        let cfg = cfg.clone();
        let stop = Arc::clone(&stop);
        async move {
            run_monitor(&cfg, timeout, stop, move |batch, _dropped| {
                collected.lock().unwrap().extend(batch);
            })
            .await
        }
    });

    // MONITOR 是异步生效的（发完命令才有流），所以边发命令边等，最多等 10 秒
    let mut plain_line = None;
    let mut tricky_line = None;
    for _ in 0..50 {
        if plain_line.is_none() {
            set_via_pool(&pool, &cfg.id, &plain_key, "v").await?;
        }
        if tricky_line.is_none() {
            set_via_pool(&pool, &cfg.id, &tricky_key, tricky_value).await?;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        plain_line = find_line_for(&lines, &plain_key);
        tricky_line = find_line_for(&lines, &tricky_key);
        if plain_line.is_some() && tricky_line.is_some() {
            break;
        }
    }

    let plain = plain_line.ok_or("10 秒内没在监控流里看到刚执行的 SET")?;
    assert_eq!(plain.command, "SET", "命令名应与发出的一致");
    assert_eq!(plain.args, vec![plain_key.clone(), "v".to_string()]);
    assert_eq!(plain.db, 0, "默认库命令应报 db 0");
    assert!(
        plain.client.contains(':'),
        "客户端标识应是 host:port 形式，实际: {}",
        plain.client
    );
    assert!(
        plain.time.contains('.'),
        "时间戳应是 秒.微秒 形式，实际: {}",
        plain.time
    );
    assert!(
        plain.raw.contains(&plain_key),
        "raw 应保留服务器原始行: {}",
        plain.raw
    );

    let tricky = tricky_line.ok_or("10 秒内没在监控流里看到带转义参数的命令")?;
    assert_eq!(
        tricky.args,
        vec![tricky_key.clone(), tricky_value.to_string()],
        "转义（引号 / 反斜杠 / 换行 / 制表符）与中文都必须逐字还原"
    );

    // 监控用的是独立连接：池里的连接照常可用（共享句柄一旦被 MONITOR 占住就废了）
    let mut con = pool.conn(&cfg.id).map_err(|e| e.to_string())?;
    let pong: String = con
        .query(&redis::cmd("PING"))
        .await
        .map_err(|e| format!("监控期间普通命令应当照常执行: {e}"))?;
    assert_eq!(pong, "PONG");

    // 停止后必须真的不再读：再执行一条命令，它不应出现在收集到的行里
    stop.notify_one();
    runner
        .await
        .map_err(|e| format!("监控任务应当正常结束，而不是 panic: {e}"))?
        .map_err(|e| format!("监控任务应因停止信号正常退出: {e}"))?;

    let after_key = format!("maidi:test:monitor:after-stop:{suffix}");
    set_via_pool(&pool, &cfg.id, &after_key, "v").await?;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        find_line_for(&lines, &after_key).is_none(),
        "停止后不应再收到监控行"
    );

    // 收尾：清掉本次写入的 key
    let mut con = pool.conn(&cfg.id).map_err(|e| e.to_string())?;
    con.query::<i64>(
        redis::cmd("DEL")
            .arg(&plain_key)
            .arg(&tricky_key)
            .arg(&after_key),
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}
