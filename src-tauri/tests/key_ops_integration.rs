//! Key 重命名 / 复制的集成用例（需本机 Redis，`#[ignore]`）。
//!
//! 默认连 `127.0.0.1:6379`，可用 `MYREDIS_TEST_PORT` 指向别的实例
//! （CI 的 Redis 版本矩阵靠它把同一批用例指向不同版本，见 DEVELOPMENT.md §2.4）。
//!
//! `COPY` 需要 Redis 6.2+，因此这里按**服务器版本**分别断言：
//! 6.2 及以上要求复制成功，更低版本要求给出「不支持 COPY」的明确提示 ——
//! 两种结果都是合格的，静默失败才是 bug（本项目支持 Redis 2.8+，不能假设服务器够新）。

use maidi_cache_lib::commands::key::{copy_key_inner, rename_key_inner};
use maidi_cache_lib::connection_pool::Pool;
use maidi_cache_lib::models::{ConnType, Connection};

fn test_port() -> u16 {
    std::env::var("MYREDIS_TEST_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6379)
}

fn conn_config(id: &str, port: u16) -> Connection {
    Connection {
        id: id.into(),
        name: "key ops integration".into(),
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

fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}", nanos)
}

/// 启动一个指向被测实例的连接池。
async fn test_pool(pool: &Pool, conn_id: &str) -> Result<Connection, String> {
    let cfg = conn_config(conn_id, test_port());
    pool.connect(&cfg)
        .await
        .map_err(|e| format!("连接 Redis 失败（127.0.0.1:{}）: {e}", test_port()))?;
    Ok(cfg)
}

async fn get(pool: &Pool, conn_id: &str, key: &str) -> Result<Option<String>, String> {
    let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;
    con.query(redis::cmd("GET").arg(key))
        .await
        .map_err(|e| e.to_string())
}

async fn set(pool: &Pool, conn_id: &str, key: &str, value: &str) -> Result<(), String> {
    let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;
    con.query::<()>(redis::cmd("SET").arg(key).arg(value))
        .await
        .map_err(|e| e.to_string())
}

async fn ttl(pool: &Pool, conn_id: &str, key: &str) -> Result<i64, String> {
    let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;
    con.query(redis::cmd("TTL").arg(key))
        .await
        .map_err(|e| e.to_string())
}

async fn del(pool: &Pool, conn_id: &str, keys: &[&String]) -> Result<(), String> {
    let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;
    con.query::<i64>(redis::cmd("DEL").arg(keys))
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 服务器是否支持 `COPY`（Redis 6.2+）。解析不出（如后续改名）时按支持处理，
/// 免得断言被悄悄跳过。
async fn server_supports_copy(pool: &Pool, conn_id: &str) -> Result<bool, String> {
    let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;
    let info: redis::Value = con
        .query(&redis::cmd("INFO"))
        .await
        .map_err(|e| e.to_string())?;
    let text = match info {
        redis::Value::Data(d) => String::from_utf8_lossy(&d).into_owned(),
        other => format!("{other:?}"),
    };
    let version = text
        .lines()
        .find_map(|line| line.strip_prefix("redis_version:"))
        .unwrap_or("")
        .trim()
        .to_string();
    let mut parts = version.split('.');
    let major: u64 = parts
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(u64::MAX);
    let minor: u64 = parts
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(u64::MAX);
    Ok((major, minor) >= (6, 2))
}

/// 重命名：默认**不覆盖**已存在的目标（RENAMENX），显式勾选覆盖才替换；键都带 TTL 时 TTL 跟着走。
#[tokio::test]
#[ignore]
async fn rename_moves_value_and_never_silently_overwrites() -> Result<(), String> {
    let pool = Pool::new();
    let conn_id = format!("rename_key_test:{}", unique_suffix());
    let cfg = test_pool(&pool, &conn_id).await?;
    let conn_id = cfg.id.as_str();

    let suffix = unique_suffix();
    let src = format!("maidi:test:rename:src:{suffix}");
    let dst = format!("maidi:test:rename:dst:{suffix}");

    set(&pool, conn_id, &src, "from-source").await?;
    set(&pool, conn_id, &dst, "from-dest").await?;

    // 目标已存在且没勾「覆盖」：必须报错，且两个 key 都不能被动过
    let err = rename_key_inner(&pool, conn_id, &src, &dst, false)
        .await
        .expect_err("目标已存在时默认不应覆盖");
    assert!(err.contains("已存在"), "错误应说明目标已存在: {err}");
    assert!(err.contains("覆盖"), "错误应指出怎么继续: {err}");
    assert_eq!(get(&pool, conn_id, &src).await?, Some("from-source".into()));
    assert_eq!(get(&pool, conn_id, &dst).await?, Some("from-dest".into()));

    // 勾选覆盖：原地改名，源消失、目标变成源的值
    rename_key_inner(&pool, conn_id, &src, &dst, true).await?;
    assert_eq!(get(&pool, conn_id, &src).await?, None, "源 key 应已被移走");
    assert_eq!(get(&pool, conn_id, &dst).await?, Some("from-source".into()));

    // 目标名首尾空白会被去掉（从别处复制来的名字常带空白）
    let padded = format!("  {dst}:trimmed  ");
    rename_key_inner(&pool, conn_id, &dst, &padded, false).await?;
    assert_eq!(
        get(&pool, conn_id, &format!("{dst}:trimmed")).await?,
        Some("from-source".into())
    );

    // 带 TTL 的重命名：TTL 跟着 key 走（RENAME 是移动而不是复制）
    let with_ttl = format!("maidi:test:rename:ttl:{suffix}");
    {
        let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;
        con.query::<()>(redis::cmd("SET").arg(&with_ttl).arg("v").arg("EX").arg(120))
            .await
            .map_err(|e| e.to_string())?;
    }
    let renamed = format!("{with_ttl}:renamed");
    rename_key_inner(&pool, conn_id, &with_ttl, &renamed, false).await?;
    let ttl_after = ttl(&pool, conn_id, &renamed).await?;
    assert!(
        ttl_after > 0 && ttl_after <= 120,
        "重命名后 TTL 应保留，实际 {ttl_after}"
    );

    // 改成同名：空操作而不是报错
    rename_key_inner(&pool, conn_id, &renamed, &renamed, false).await?;
    assert_eq!(get(&pool, conn_id, &renamed).await?, Some("v".into()));

    // 参数与源不存在
    assert!(rename_key_inner(&pool, conn_id, &renamed, "   ", false)
        .await
        .expect_err("空目标名应被拦下")
        .contains("不能为空"));
    let err = rename_key_inner(
        &pool,
        conn_id,
        &format!("{suffix}:no-such-key"),
        "dst",
        false,
    )
    .await
    .expect_err("源不存在应报错");
    assert!(err.contains("不存在"), "错误应说明源 key 不存在: {err}");

    del(&pool, conn_id, &[&dst, &format!("{dst}:trimmed"), &renamed]).await?;
    Ok(())
}

/// 复制：源保持不变、目标拿到同样的值与 TTL；目标已存在时默认不覆盖，`REPLACE` 才覆盖。
///
/// Redis 6.2 以下没有 COPY，此时要求给出「需要 6.2」的明确提示（不是一句 unknown command）。
#[tokio::test]
#[ignore]
async fn copy_duplicates_value_with_version_aware_fallback() -> Result<(), String> {
    let pool = Pool::new();
    let conn_id = format!("copy_key_test:{}", unique_suffix());
    let cfg = test_pool(&pool, &conn_id).await?;
    let conn_id = cfg.id.as_str();

    let suffix = unique_suffix();
    let src = format!("maidi:test:copy:src:{suffix}");
    let dst = format!("maidi:test:copy:dst:{suffix}");
    let other = format!("maidi:test:copy:other:{suffix}");
    let hash_src = format!("maidi:test:copy:hash:{suffix}");

    set(&pool, conn_id, &src, "copied-value").await?;
    {
        let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;
        // 带 TTL 的源：COPY 的语义是连 TTL 一起搬（Redis 文档：复制的是当前值）
        con.query::<()>(
            redis::cmd("SET")
                .arg(&src)
                .arg("copied-value")
                .arg("EX")
                .arg(120),
        )
        .await
        .map_err(|e| e.to_string())?;
        con.query::<i64>(redis::cmd("HSET").arg(&hash_src).arg("f").arg("v"))
            .await
            .map_err(|e| e.to_string())?;
    }

    if !server_supports_copy(&pool, conn_id).await? {
        // 低版本：必须给出版本提示与替代做法，而不是把服务端原文甩给用户
        let err = copy_key_inner(&pool, conn_id, &src, &dst, false)
            .await
            .expect_err("低版本服务器上 COPY 应当返回明确提示");
        assert!(err.contains("6.2"), "应说明版本要求: {err}");
        assert_eq!(
            get(&pool, conn_id, &src).await?,
            Some("copied-value".into())
        );
        assert_eq!(
            get(&pool, conn_id, &dst).await?,
            None,
            "复制失败不应产生目标 key"
        );
        del(&pool, conn_id, &[&src, &hash_src]).await?;
        return Ok(());
    }

    // 正常复制：源不动，目标拿到同样的值与 TTL
    copy_key_inner(&pool, conn_id, &src, &dst, false).await?;
    assert_eq!(
        get(&pool, conn_id, &src).await?,
        Some("copied-value".into())
    );
    assert_eq!(
        get(&pool, conn_id, &dst).await?,
        Some("copied-value".into())
    );
    let ttl_after = ttl(&pool, conn_id, &dst).await?;
    assert!(
        ttl_after > 0 && ttl_after <= 120,
        "复制出来的 key 应带同样的 TTL，实际 {ttl_after}"
    );

    // 非 string 类型整份复制
    let hash_dst = format!("{hash_src}:copy");
    copy_key_inner(&pool, conn_id, &hash_src, &hash_dst, false).await?;
    {
        let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;
        let field: Option<String> = con
            .query(redis::cmd("HGET").arg(&hash_dst).arg("f"))
            .await
            .map_err(|e| e.to_string())?;
        assert_eq!(field, Some("v".into()), "hash 应当整份复制过去");
    }

    // 目标已存在且未勾覆盖：报错并指出目标已存在（不是笼统的「复制失败」）
    let err = copy_key_inner(&pool, conn_id, &src, &dst, false)
        .await
        .expect_err("目标已存在时默认不应覆盖");
    assert!(err.contains("已存在"), "错误应说明目标已存在: {err}");
    assert_eq!(
        get(&pool, conn_id, &dst).await?,
        Some("copied-value".into())
    );

    // 勾选覆盖：目标被源的值替换
    set(&pool, conn_id, &other, "to-be-replaced").await?;
    copy_key_inner(&pool, conn_id, &src, &other, true).await?;
    assert_eq!(
        get(&pool, conn_id, &other).await?,
        Some("copied-value".into())
    );

    // 源不存在：明确提示刷新（COPY 对不存在的源也返回 0，得靠 EXISTS 才能说清）
    let err = copy_key_inner(
        &pool,
        conn_id,
        &format!("{suffix}:no-such-key"),
        "dst",
        false,
    )
    .await
    .expect_err("源不存在应报错");
    assert!(err.contains("不存在"), "错误应说明源 key 不存在: {err}");

    // 同名复制：直接拦下，不必往返一次
    assert!(copy_key_inner(&pool, conn_id, &src, &src, false)
        .await
        .expect_err("同名复制应被拦下")
        .contains("同名"));

    del(&pool, conn_id, &[&src, &dst, &other, &hash_src, &hash_dst]).await?;
    Ok(())
}
