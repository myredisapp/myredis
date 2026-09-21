//! Redis 测试连接集成测试。
//!
//! 需先启动 Redis（默认连接 127.0.0.1:6379）。
//! 由于涉及外部服务，默认通过 `#[ignore]` 忽略，需显式运行：
//!
//! ```bash
//! cargo test --test test_connection -- --ignored --nocapture
//! ```

use maidi_cache_lib::connection_pool::Pool;
use maidi_cache_lib::models::{ConnType, Connection};

#[tokio::test]
#[ignore]
async fn test_connection_returns_pong() {
    let conn = Connection {
        id: "test_conn_1".into(),
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
    let pong = Pool::new()
        .test(&conn)
        .await
        .expect("测试连接失败，请确认 Redis 已启动");
    assert_eq!(pong, "PONG");
    println!("测试连接返回: {}", pong);
}

#[tokio::test]
#[ignore]
async fn test_connection_fails_on_wrong_port() {
    let conn = Connection {
        id: "test_conn_2".into(),
        name: "integration".into(),
        host: "127.0.0.1".into(),
        port: 1,
        conn_type: ConnType::Single,
        readonly: false,
        separator: ":".into(),
        db: 0,
        username: None,
        password: None,
    };
    // 连不上的端口必须在连接超时预算内失败，而不是永久等待
    let started = std::time::Instant::now();
    let result = Pool::new().test(&conn).await;
    assert!(result.is_err(), "错误端口应当返回连接失败");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "连接失败应当有界，实际耗时 {:?}",
        started.elapsed()
    );
}
