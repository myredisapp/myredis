//! Redis 集群（Cluster）连接集成测试。
//!
//! 需先启动 Redis 集群（默认连接 127.0.0.1:7001，可通过 `MYREDIS_CLUSTER_PORT`
//! 环境变量指定任一集群节点端口）。
//! 由于涉及外部服务，默认通过 `#[ignore]` 忽略，需显式运行：
//!
//! ```bash
//! cargo test --test cluster_integration -- --ignored --nocapture
//! ```

use maidi_cache_lib::commands::key::{list_keys_inner, KeyEntry};
use maidi_cache_lib::connection_pool::Pool;
use maidi_cache_lib::models::{ConnType, Connection};

/// 按游标翻完所有页，返回全部 key（等价于分页之前「一次列出全部」的结果）。
async fn list_all_keys(pool: &Pool, conn_id: &str) -> Vec<KeyEntry> {
    let mut cursor = "0".to_string();
    let mut keys = Vec::new();
    loop {
        let page = list_keys_inner(pool, conn_id, &cursor, 100, None)
            .await
            .expect("list_keys 失败");
        keys.extend(page.keys);
        if page.next_cursor == "0" {
            break;
        }
        cursor = page.next_cursor;
    }
    keys
}

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
        tls: false,
        tls_insecure: false,
        connect_timeout_secs: None,
        command_timeout_secs: None,
    }
}

#[tokio::test]
#[ignore]
async fn cluster_test_returns_pong() {
    let pong = Pool::new()
        .test(&cluster_conn("cluster_test"))
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

    let _: String = con
        .query(redis::cmd("SET").arg(key).arg("hello-cluster"))
        .await
        .expect("集群 SET 失败");

    let val: String = con
        .query(redis::cmd("GET").arg(key))
        .await
        .expect("集群 GET 失败");
    assert_eq!(val, "hello-cluster");

    let deleted: i64 = con
        .query(redis::cmd("DEL").arg(key))
        .await
        .expect("集群 DEL 失败");
    assert_eq!(deleted, 1);

    let after: Option<String> = con
        .query(redis::cmd("GET").arg(key))
        .await
        .expect("集群 GET 失败");
    assert!(after.is_none(), "删除后不应再取到值");

    // SCAN 在集群连接上会被路由到随机一个节点（redis-rs 不做聚合），
    // 这里仅验证命令本身不报错；完整性与稳定性由 cluster_list_keys 用例覆盖。
    let _: redis::Value = con
        .query(redis::cmd("SCAN").arg(0).arg("COUNT").arg(10))
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
    let _: String = con
        .query(redis::cmd("SET").arg("myredis:cluster:scan:probe").arg("1"))
        .await
        .expect("集群 SET 失败");

    let listed = list_all_keys(&pool, "cluster_info").await;
    assert!(
        listed.iter().any(|k| k.key == "myredis:cluster:scan:probe"),
        "list_keys 应包含刚写入的探针 key"
    );
    println!("list_keys 共列出 {} 个 key", listed.len());

    let _: i64 = con
        .query(redis::cmd("DEL").arg("myredis:cluster:scan:probe"))
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
        let _: String = con
            .query(redis::cmd("SET").arg(&key).arg("1"))
            .await
            .expect("集群 SET 失败");
        probes.push(key);
    }

    // 反复列出多次：每次都必须包含所有探针 key，且结果完全一致
    let mut snapshots = Vec::new();
    for _ in 0..5 {
        let entries = list_all_keys(&pool, "cluster_list_keys").await;
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
        let _: i64 = con
            .query(redis::cmd("DEL").arg(probe))
            .await
            .expect("清理失败");
    }

    pool.disconnect("cluster_list_keys");
}

/// 集群分页：小页大小强制游标在多个主节点之间接力，结果必须完整且不重复。
#[tokio::test]
#[ignore]
async fn cluster_list_keys_pages_span_nodes_without_loss() {
    let pool = Pool::new();
    let conn = cluster_conn("cluster_paging");
    pool.connect(&conn).await.expect("连接失败");
    let mut con = pool.conn("cluster_paging").unwrap();

    // 300 个 key，用 3 个不同的 hash tag 把它们打散到 3 个主节点
    let prefix = "myredis:cluster:page";
    let total = 300usize;
    let mut written = Vec::new();
    for i in 0..total {
        let key = format!("{prefix}:{{t{}}}:{i:04}", i % 3);
        let _: String = con
            .query(redis::cmd("SET").arg(&key).arg("1"))
            .await
            .expect("集群 SET 失败");
        written.push(key);
    }

    // 每页 25 个：必然翻多页，且游标必须跨节点接力
    let mut cursor = "0".to_string();
    let mut names = Vec::new();
    let mut visited_nodes = std::collections::HashSet::new();
    let mut pages = 0usize;
    loop {
        let page = list_keys_inner(&pool, "cluster_paging", &cursor, 25, Some(prefix))
            .await
            .expect("集群分页失败");
        names.extend(page.keys.into_iter().map(|k| k.key));
        pages += 1;
        assert!(pages < 100, "分页未收敛，游标处理可能有误");
        if page.next_cursor == "0" {
            break;
        }
        // 集群游标形如 `地址:游标`，地址部分应始终是主节点
        let (addr, _) = page
            .next_cursor
            .rsplit_once(':')
            .expect("集群游标应带节点地址");
        visited_nodes.insert(addr.to_string());
        cursor = page.next_cursor;
    }

    assert!(pages > 1, "25 个/页、{total} 个 key，至少应分 2 页");
    assert!(
        visited_nodes.len() > 1,
        "游标应在多个主节点之间接力，实际只经过 {visited_nodes:?}"
    );

    let mut expected = written.clone();
    let mut actual = names;
    expected.sort();
    actual.sort();
    assert_eq!(actual, expected, "集群分页必须覆盖全部 key 且不重复");

    for key in &written {
        let _: i64 = con
            .query(redis::cmd("DEL").arg(key))
            .await
            .expect("清理失败");
    }

    pool.disconnect("cluster_paging");
}

/// 集群下的类型/TTL 富化：类型与 TTL 必须挂在正确的 key 上，且能跨节点把所有 key 都富化到。
///
/// 分两轮验证：
/// 1. **每页 1 个 key** —— 游标逐节点推进，每个节点各自跑一遍富化 pipeline；
/// 2. **一页跨多节点** —— 不同 slot 的 key 落在同一页里，富化必须按节点拆开 pipeline：
///    若实现把整页的 key 塞进同一条 pipeline（集群连接会按第一个 key 路由），
///    服务端会直接判 CROSSSLOT，本用例随即失败。
///
/// 类型与 TTL 错位（pipeline 的返回按命令顺序排列）同样会被断言挡住。
#[tokio::test]
#[ignore]
async fn cluster_list_keys_reports_types_across_nodes() {
    let pool = Pool::new();
    let conn = cluster_conn("cluster_types");
    pool.connect(&conn).await.expect("连接失败");
    let mut con = pool.conn("cluster_types").unwrap();

    let prefix = "myredis:cluster:types";
    // 3 个 hash tag 把它们分散到 3 个主节点：string / hash / 带 TTL 的 list
    let string_key = format!("{prefix}:{{t0}}:string");
    let hash_key = format!("{prefix}:{{t1}}:hash");
    let list_key = format!("{prefix}:{{t2}}:expiring-list");
    let _: String = con
        .query(redis::cmd("SET").arg(&string_key).arg("v"))
        .await
        .expect("集群 SET 失败");
    let _: i64 = con
        .query(redis::cmd("HSET").arg(&hash_key).arg("f").arg("v"))
        .await
        .expect("集群 HSET 失败");
    let _: i64 = con
        .query(redis::cmd("RPUSH").arg(&list_key).arg("a"))
        .await
        .expect("集群 RPUSH 失败");
    let _: i64 = con
        .query(redis::cmd("EXPIRE").arg(&list_key).arg(120))
        .await
        .expect("集群 EXPIRE 失败");

    // 每页 1 个 key：游标必然逐节点推进，每个节点的富化 pipeline 都会被单独执行
    let mut cursor = "0".to_string();
    let mut visited_nodes = std::collections::HashSet::new();
    let mut found: std::collections::HashMap<String, (String, i64)> =
        std::collections::HashMap::new();
    let mut pages = 0usize;
    loop {
        let page = list_keys_inner(&pool, "cluster_types", &cursor, 1, Some(prefix))
            .await
            .expect("集群列表富化失败（跨节点的 key 混进同一条 pipeline 会报 CROSSSLOT）");
        for entry in page.keys {
            found.insert(entry.key, (entry.type_, entry.ttl));
        }
        pages += 1;
        assert!(pages < 100, "分页未收敛，游标处理可能有误");
        if page.next_cursor == "0" {
            break;
        }
        let (addr, _) = page
            .next_cursor
            .rsplit_once(':')
            .expect("集群游标应带节点地址");
        visited_nodes.insert(addr.to_string());
        cursor = page.next_cursor;
    }

    assert_eq!(
        found.get(&string_key).map(|(t, _)| t.as_str()),
        Some("string"),
        "string key 的类型应当正确: {found:?}"
    );
    assert_eq!(
        found.get(&hash_key).map(|(t, _)| t.as_str()),
        Some("hash"),
        "hash key 的类型应当正确: {found:?}"
    );
    assert_eq!(
        found.get(&list_key).map(|(t, _)| t.as_str()),
        Some("list"),
        "list key 的类型应当正确: {found:?}"
    );
    assert_eq!(
        found.get(&string_key).map(|(_, ttl)| *ttl),
        Some(-1),
        "未设过期的 key 应报 -1"
    );
    let ttl = found.get(&list_key).map(|(_, ttl)| *ttl).unwrap_or(-2);
    assert!(ttl > 0 && ttl <= 120, "带过期的 key 应报正数 TTL: {ttl}");
    assert!(
        visited_nodes.len() > 1,
        "探针 key 应分布在多个主节点上，实际只经过 {visited_nodes:?}"
    );

    // 一页跨多节点：不同 slot 的 key 同页返回，富化必须按节点各自成批
    let page = list_keys_inner(&pool, "cluster_types", "0", 50, Some(prefix))
        .await
        .expect("一页跨多节点的富化失败（整页混进同一条 pipeline 会被判 CROSSSLOT）");
    let page_keys: std::collections::HashMap<&str, &KeyEntry> =
        page.keys.iter().map(|e| (e.key.as_str(), e)).collect();
    for (key, expected_type) in [
        (&string_key, "string"),
        (&hash_key, "hash"),
        (&list_key, "list"),
    ] {
        let entry = page_keys
            .get(key.as_str())
            .unwrap_or_else(|| panic!("{key} 应当出现在同一页里"));
        assert_eq!(entry.type_, expected_type, "跨节点一页里的类型应当正确");
    }

    for key in [&string_key, &hash_key, &list_key] {
        let _: i64 = con
            .query(redis::cmd("DEL").arg(key))
            .await
            .expect("清理失败");
    }

    pool.disconnect("cluster_types");
}
