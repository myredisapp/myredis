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
        tls: false,
        tls_insecure: false,
        connect_timeout_secs: None,
        command_timeout_secs: None,
    };
    pool.connect(&conn)
        .await
        .expect("连接 Redis 失败，请确认服务已启动");
    let pong = pool.ping("itest1").await.expect("PING 失败");
    assert_eq!(pong, "PONG");
    println!("PING 返回: {}", pong);
}

/// 主机字段填了 `rediss://` 但**未勾选 TLS**：在入口就被拦下，并给出可操作的提示
///（不依赖 Redis：校验发生在建连之前）。
///
/// 只断言「报错」是不够的 —— 旧实现拼出畸形 URL 也算报错，这正是本用例要防的回归，
/// 所以这里断言的是文案（见 `Connection::check_supported_scheme`）。
#[tokio::test]
async fn rediss_host_without_tls_flag_is_rejected() {
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
        tls: false,
        tls_insecure: false,
        connect_timeout_secs: None,
        command_timeout_secs: None,
    };
    let err = pool.connect(&conn).await.expect_err("未开 TLS 应被拦下");
    assert!(
        err.to_string().contains("TLS 加密"),
        "应给出明确提示，而不是通用 URL 解析错误: {err}"
    );
}

/// TLS 开关已接线到 URL：勾选后拼接 `rediss://`，自签名模式追加 `#insecure`（纯单测，不建连）。
#[test]
fn tls_flag_builds_rediss_url() {
    let conn = Connection {
        id: "tls-url".into(),
        name: "tls".into(),
        host: "redis.example.com".into(),
        port: 6379,
        conn_type: ConnType::Single,
        readonly: false,
        separator: ":".into(),
        db: 0,
        username: Some("u".into()),
        password: Some("p".into()),
        tls: true,
        tls_insecure: false,
        connect_timeout_secs: None,
        command_timeout_secs: None,
    };
    assert_eq!(
        conn.to_connection_url(),
        "rediss://u:p@redis.example.com:6379/0"
    );

    let mut insecure = conn.clone();
    insecure.tls_insecure = true;
    assert_eq!(
        insecure.to_connection_url(),
        "rediss://u:p@redis.example.com:6379/0#insecure"
    );
}

/// 真实 TLS 建连：跳过证书校验（自签名）应连通本机 TLS Redis（需按 DEVELOPMENT.md §7
/// 用 `redis-server --tls-port 6390 ...` 启动，否则 #[ignore] 跳过）。
#[tokio::test]
#[ignore]
async fn tls_connect_with_insecure_flag() {
    let pool = Pool::new();
    let conn = Connection {
        id: "tls-itest".into(),
        name: "tls".into(),
        host: "127.0.0.1".into(),
        port: 6390,
        conn_type: ConnType::Single,
        readonly: false,
        separator: ":".into(),
        db: 0,
        username: None,
        password: None,
        tls: true,
        tls_insecure: true,
        connect_timeout_secs: None,
        command_timeout_secs: None,
    };
    pool.connect(&conn)
        .await
        .expect("TLS(跳过校验) 连接失败，请确认 127.0.0.1:6390 的 TLS Redis 已启动");
    let pong = pool.ping("tls-itest").await.expect("PING 失败");
    assert_eq!(pong, "PONG");
}

/// 真实 TLS 建连：校验证书时自签名证书必须被拒（而不是静默成功或报看不懂的错误）。
#[tokio::test]
#[ignore]
async fn tls_connect_rejects_self_signed_without_insecure_flag() {
    let pool = Pool::new();
    let conn = Connection {
        id: "tls-verify".into(),
        name: "tls".into(),
        host: "127.0.0.1".into(),
        port: 6390,
        conn_type: ConnType::Single,
        readonly: false,
        separator: ":".into(),
        db: 0,
        username: None,
        password: None,
        tls: true,
        tls_insecure: false,
        connect_timeout_secs: None,
        command_timeout_secs: None,
    };
    let err = pool
        .connect(&conn)
        .await
        .expect_err("自签名证书在校验模式下应被拒");
    let text = err.to_string();
    assert!(
        text.contains("certificate") || text.contains("证书") || text.contains("tls"),
        "应是证书校验类错误: {text}"
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
        // 不带 section 参数的裸 INFO（与 server.rs 的生产用法一致）：单 section 语法
        // 是 Redis 7.0 才有的，6.x（如 ubuntu-22.04 apt 源）会回 syntax error
        let info: String = redis::cmd("INFO").query_async(&mut con).await.unwrap();
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
