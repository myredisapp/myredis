//! Redis 集群（Cluster）连接集成测试。
//!
//! 需先启动 Redis 集群（默认连接 127.0.0.1:7001，可通过 `MYREDIS_CLUSTER_PORT`
//! 环境变量指定任一集群节点端口）。
//! 由于涉及外部服务，默认通过 `#[ignore]` 忽略，需显式运行：
//!
//! ```bash
//! cargo test --test cluster_integration -- --ignored --nocapture
//! ```

use maidi_cache_lib::connection_pool::Pool;
use maidi_cache_lib::models::{ConnType, Connection};

fn cluster_conn(id: &str) -> Connection {
    let port = std::env::var("MYREDIS_CLUSTER_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(7001);
    Connection {
        id: id.into(),
        name: "cluster-itest".into(),
        host: "127.0.0.1".into(),
        port,
        conn_type: ConnType::Cluster,
        readonly: false,
        separator: ":".into(),
        db: 0,
        username: None,
        password: None,
    }
}

#[tokio::test]
#[ignore]
async fn cluster_test_returns_pong() {
    let pong = Pool::test(&cluster_conn("cluster_test"))
        .await
        .expect("集群连接测试失败，请确认集群已启动");
    assert_eq!(pong, "PONG");
    println!("集群 PING 返回: {}", pong);
}

#[tokio::test]
#[ignore]
async fn cluster_connect_ping_and_crud() {
    let pool = Pool::new();
    let conn = cluster_conn("cluster_crud");
    pool.connect(&conn)
        .await
        .expect("连接 Redis 集群失败，请确认集群已启动");

    // PING 走统一连接句柄（集群分支）
    let pong = pool.ping("cluster_crud").await.expect("集群 PING 失败");
    assert_eq!(pong, "PONG");

    // 通过 pool.conn() 取句柄执行 SET / GET / DEL（单 key 单 slot，集群支持）
    let mut con = pool.conn("cluster_crud").expect("获取集群句柄失败");
    let key = "myredis:cluster:itest:key";

    let _: String = redis::cmd("SET")
        .arg(key)
        .arg("hello-cluster")
        .query_async(&mut con)
        .await
        .expect("集群 SET 失败");

    let val: String = redis::cmd("GET")
        .arg(key)
        .query_async(&mut con)
        .await
        .expect("集群 GET 失败");
    assert_eq!(val, "hello-cluster");

    let deleted: i64 = redis::cmd("DEL")
        .arg(key)
        .query_async(&mut con)
        .await
        .expect("集群 DEL 失败");
    assert_eq!(deleted, 1);

    let after: Option<String> = redis::cmd("GET")
        .arg(key)
        .query_async(&mut con)
        .await
        .expect("集群 GET 失败");
    assert!(after.is_none(), "删除后不应再取到值");

    // SCAN 在集群连接上会被路由到随机一个节点（redis-rs 不做聚合），
    // 这里仅验证命令本身不报错；完整性与稳定性由 cluster_list_keys 用例覆盖。
    let _: redis::Value = redis::cmd("SCAN")
        .arg(0)
        .arg("COUNT")
        .arg(10)
        .query_async(&mut con)
        .await
        .expect("集群 SCAN 失败");

    pool.disconnect("cluster_crud");
}

#[tokio::test]
#[ignore]
async fn cluster_get_server_info_and_scan() {
    use maidi_cache_lib::commands::server::fetch_server_info;

    let pool = Pool::new();
    let conn = cluster_conn("cluster_info");
    pool.connect(&conn).await.expect("连接失败");

    // get_server_info 核心逻辑：集群下 INFO 返回所有节点信息，应合并成功
    let info = fetch_server_info(&pool, "cluster_info")
        .await
        .expect("获取集群服务器信息失败");
    assert!(!info.redis_version.is_empty(), "应解析出版本号");
    println!(
        "cluster info: version={} os={} uptime={}s mem={} clients={} keys={}",
        info.redis_version,
        info.os,
        info.uptime_seconds,
        info.used_memory,
        info.connected_clients,
        info.db_keys
    );

    // list_keys 应能列出探针 key；修复前 SCAN 只扫随机一个节点，结果时有时无
    let mut con = pool.conn("cluster_info").unwrap();
    let _: String = redis::cmd("SET")
        .arg("myredis:cluster:scan:probe")
        .arg("1")
        .query_async(&mut con)
        .await
        .expect("集群 SET 失败");

    let listed = maidi_cache_lib::commands::key::list_keys_inner(&pool, "cluster_info")
        .await
        .expect("list_keys 失败");
    assert!(
        listed.iter().any(|k| k.key == "myredis:cluster:scan:probe"),
        "list_keys 应包含刚写入的探针 key"
    );
    println!("list_keys 共列出 {} 个 key", listed.len());

    let _: i64 = redis::cmd("DEL")
        .arg("myredis:cluster:scan:probe")
        .query_async(&mut con)
        .await
        .expect("清理失败");

    pool.disconnect("cluster_info");
}

#[tokio::test]
#[ignore]
async fn cluster_list_keys_stable_and_complete() {
    let pool = Pool::new();
    let conn = cluster_conn("cluster_list_keys");
    pool.connect(&conn).await.expect("连接失败");
    let mut con = pool.conn("cluster_list_keys").unwrap();

    // 写入 12 个带不同 hash tag 的 key，强制分布到不同主节点
    let mut probes = Vec::new();
    for i in 0..12 {
        let key = format!("myredis:cluster:lk:{{t{i}}}:probe");
        let _: String = redis::cmd("SET")
            .arg(&key)
            .arg("1")
            .query_async(&mut con)
            .await
            .expect("集群 SET 失败");
        probes.push(key);
    }

    // 反复列出多次：每次都必须包含所有探针 key，且结果完全一致
    let mut snapshots = Vec::new();
    for _ in 0..5 {
        let entries = maidi_cache_lib::commands::key::list_keys_inner(&pool, "cluster_list_keys")
            .await
            .expect("list_keys 失败");
        snapshots.push(entries.iter().map(|e| e.key.clone()).collect::<Vec<_>>());
    }
    for (i, snap) in snapshots.iter().enumerate().skip(1) {
        assert_eq!(
            &snapshots[0], snap,
            "第 {i} 次 list_keys 结果与首次不一致（集群 SCAN 结果不稳定）"
        );
    }
    for probe in &probes {
        assert!(
            snapshots[0].iter().any(|k| k == probe),
            "list_keys 应包含探针 key {probe}"
        );
    }

    // 清理
    for probe in &probes {
        let _: i64 = redis::cmd("DEL")
            .arg(probe)
            .query_async(&mut con)
            .await
            .expect("清理失败");
    }

    pool.disconnect("cluster_list_keys");
}
