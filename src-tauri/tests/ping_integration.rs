//! Redis 连接集成测试。
//!
//! 需先启动 Redis（默认连接 127.0.0.1:6379）。
//! 由于涉及外部服务，默认通过 `#[ignore]` 忽略，需显式运行：
//!
//! ```bash
//! cargo test --test ping_integration -- --ignored --nocapture
//! ```

use maidi_cache_lib::connection_pool::Pool;
use maidi_cache_lib::models::{ConnType, Connection};

#[tokio::test]
#[ignore]
async fn redis_ping_returns_pong() {
    let pool = Pool::new();
    let conn = Connection {
        id: "itest1".into(),
        name: "integration".into(),
        host: "127.0.0.1".into(),
        port: 6379,
        conn_type: ConnType::Single,
        readonly: false,
        separator: ":".into(),
        db: 0,
        username: None,
        password: None,
    };
    pool.connect(&conn)
        .await
        .expect("连接 Redis 失败，请确认服务已启动");
    let pong = pool.ping("itest1").await.expect("PING 失败");
    assert_eq!(pong, "PONG");
    println!("PING 返回: {}", pong);
}

/// `rediss://` 主机在入口就被拦下，并给出可操作的提示（不依赖 Redis：校验发生在建连之前）。
///
/// 只断言「报错」是不够的 —— 旧实现拼出畸形 URL 也算报错，这正是本用例要防的回归，
/// 所以这里断言的是文案（见 `Connection::check_supported_scheme`）。
#[tokio::test]
async fn tls_not_supported() {
    let pool = Pool::new();
    let conn = Connection {
        id: "tls".into(),
        name: "tls".into(),
        host: "rediss://example.com".into(),
        port: 6380,
        conn_type: ConnType::Single,
        readonly: false,
        separator: ":".into(),
        db: 0,
        username: None,
        password: None,
    };
    let err = pool.connect(&conn).await.expect_err("TLS 连接应当被拒绝");
    assert!(
        err.to_string().contains("暂不支持 TLS 加密连接"),
        "应给出明确提示，而不是通用 URL 解析错误: {err}"
    );
}

#[test]
fn connection_json_deserializes_from_frontend_payload() {
    // 前端 save_connection 发送的精确 JSON 结构
    let payload = r#"{"id":"conn_123","name":"本地开发","host":"127.0.0.1","port":6379,"type":"single","password":null}"#;
    let c: Result<maidi_cache_lib::models::Connection, _> = serde_json::from_str(payload);
    assert!(c.is_ok(), "反序列化失败: {:?}", c.err());
    let c = c.unwrap();
    assert_eq!(c.name, "本地开发");
    assert_eq!(c.host, "127.0.0.1");
    assert_eq!(c.port, 6379);
    assert!(c.password.is_none());
}

#[test]
#[ignore]
fn get_server_info_parses_real_redis() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (raw, db_size) = rt.block_on(async {
        let client = redis::Client::open("redis://127.0.0.1:6379").unwrap();
        let mut con = client.get_connection_manager().await.unwrap();
        let info: String = redis::cmd("INFO")
            .arg("server")
            .arg("memory")
            .arg("clients")
            .query_async(&mut con)
            .await
            .unwrap();
        let db_size: u64 = redis::cmd("DBSIZE").query_async(&mut con).await.unwrap();
        (info, db_size)
    });
    println!("=== INFO 原始输出 ===\n{}\n=== END ===", raw);
    assert!(raw.contains("redis_version"));
    assert!(raw.contains("used_memory"));
    for line in raw.lines().filter(|l| l.starts_with("redis_version:")) {
        println!(
            "服务端 Redis 版本: {}",
            line.split(':').nth(1).unwrap_or("?")
        );
    }
    println!("DBSIZE = {}", db_size);
    let _ = db_size;
}
