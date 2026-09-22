//! 「非集群方式登录集群节点」的行为验证测试。
//!
//! 背景：用户期望以单机（Single）模式登录 Redis 集群中的某个节点时，
//! 写入的 key 一定落在被连接的这个节点上（而不是被路由到集群中别的节点）。
//!
//! 本测试用一个真实集群验证三条产品语义：
//! 1. 单机模式写入「属于该节点 slot」的 key → 成功，数据只在该节点，集群也能读到；
//! 2. 单机模式写入「不属于该节点 slot」的 key → 服务器拒绝（MOVED），数据不会写到任何节点；
//! 3. 集群模式写入同一个 key → 成功，且数据不在被直连的节点上（即确实路由到了别的节点）。
//!
//! 需先启动 Redis 集群（默认连接 127.0.0.1:7001，可通过 `MYREDIS_CLUSTER_PORT`
//! 环境变量指定任一集群节点端口）。默认 `#[ignore]`，显式运行：
//!
//! ```bash
//! cargo test --test single_mode_on_cluster -- --ignored --nocapture
//! ```

use maidi_cache_lib::connection_pool::Pool;
use maidi_cache_lib::models::{ConnType, Connection};

fn conn(id: &str, conn_type: ConnType) -> Connection {
    let port = std::env::var("MYREDIS_CLUSTER_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(7001);
    Connection {
        id: id.into(),
        name: "single-on-cluster-itest".into(),
        host: "127.0.0.1".into(),
        port,
        conn_type,
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

/// 解析 `CLUSTER NODES` 中 myself 主节点负责的 slot 区间，返回 (own_ranges, all_ranges)。
fn my_slot_ranges(cluster_nodes: &str) -> Vec<(u16, u16)> {
    let mut ranges = Vec::new();
    for line in cluster_nodes.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 9 || !cols[2].contains("myself") || !cols[2].contains("master") {
            continue;
        }
        for r in &cols[8..] {
            if let Some((a, b)) = r.split_once('-') {
                if let (Ok(a), Ok(b)) = (a.parse(), b.parse()) {
                    ranges.push((a, b));
                }
            } else if let Ok(a) = r.parse() {
                ranges.push((a, a));
            }
        }
    }
    ranges
}

fn slot_in(slot: u16, ranges: &[(u16, u16)]) -> bool {
    ranges.iter().any(|(a, b)| slot >= *a && slot <= *b)
}

/// 找到 slot 落在 / 不落在指定区间内的 key：redis-cli `CLUSTER KEYSLOT` 同名算法。
fn slot_of(key: &str) -> u16 {
    // CRC16(key) mod 16384（XMODEM），与 redis `cluster keyslot` 一致
    let mut crc: u16 = 0;
    for b in key.as_bytes() {
        crc ^= *b as u16;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc & 0x3fff
}

fn find_key(in_my: bool, ranges: &[(u16, u16)]) -> String {
    for i in 0..100000u32 {
        let key = format!("myredis:pin:{}", i);
        if slot_in(slot_of(&key), ranges) == in_my {
            return key;
        }
    }
    panic!("找不到满足条件的 key");
}

#[tokio::test]
#[ignore]
async fn single_mode_write_pins_to_connected_node() {
    let pool = Pool::new();

    // 单机模式直连集群节点
    pool.connect(&conn("single_pin", ConnType::Single))
        .await
        .expect("单机模式连接集群节点失败");

    // 该节点负责的 slot 区间
    let nodes: String = {
        let mut c = pool.conn("single_pin").unwrap();
        c.query(redis::cmd("CLUSTER").arg("NODES"))
            .await
            .expect("CLUSTER NODES 失败")
    };
    let my_ranges = my_slot_ranges(&nodes);
    assert!(!my_ranges.is_empty(), "直连节点应为主节点并持有 slot");

    let own_key = find_key(true, &my_ranges);
    let foreign_key = find_key(false, &my_ranges);
    println!(
        "直连节点 slot 区间: {:?}; own_key={} (slot={}), foreign_key={} (slot={})",
        my_ranges,
        own_key,
        slot_of(&own_key),
        foreign_key,
        slot_of(&foreign_key)
    );

    // 1) 写入属于本节点的 key：应成功
    let mut single = pool.conn("single_pin").unwrap();
    let ok: String = single
        .query(redis::cmd("SET").arg(&own_key).arg("v-own"))
        .await
        .expect("单机模式写入本节点 slot 的 key 应成功");
    assert_eq!(ok, "OK");

    // 数据确实在直连节点上（本节点 GET 不跨节点，成功即证明存储在此）
    let v: String = single
        .query(redis::cmd("GET").arg(&own_key))
        .await
        .expect("本节点应能读到自己 slot 的 key");
    assert_eq!(v, "v-own");

    // 集群模式也能读到（「集群也能看到」）
    pool.connect(&conn("cluster_view", ConnType::Cluster))
        .await
        .expect("集群模式连接失败");
    let mut cluster = pool.conn("cluster_view").unwrap();
    let v: String = cluster
        .query(redis::cmd("GET").arg(&own_key))
        .await
        .expect("集群模式应能读到该 key");
    assert_eq!(v, "v-own");

    // 2) 写入不属于本节点 slot 的 key：服务器必须拒绝（MOVED），不落在任何节点
    let err = single
        .query::<String>(redis::cmd("SET").arg(&foreign_key).arg("v-foreign"))
        .await
        .expect_err("单机模式写入其他节点 slot 的 key 应返回 MOVED 错误")
        .to_string();
    println!("单机模式写 foreign key 的报错: {err}");
    assert!(
        err.to_uppercase().contains("MOVED"),
        "错误应为 MOVED，实际: {err}"
    );
    // 集群视角也应读不到（证明真的没有写入任何节点）
    let v: Option<String> = cluster
        .query(redis::cmd("GET").arg(&foreign_key))
        .await
        .expect("GET 查询失败");
    assert!(v.is_none(), "被拒绝的 key 不应存在于集群任何节点");

    // 3) 集群模式写同一个 foreign key：成功，且数据落在别的节点（本节点读不到 → MOVED）
    let ok: String = cluster
        .query(redis::cmd("SET").arg(&foreign_key).arg("v-foreign"))
        .await
        .expect("集群模式写入任意 key 应成功");
    assert_eq!(ok, "OK");
    let err = single
        .query::<String>(redis::cmd("GET").arg(&foreign_key))
        .await
        .expect_err("foreign key 不应落在直连节点上")
        .to_string();
    assert!(
        err.to_uppercase().contains("MOVED"),
        "错误应为 MOVED，实际: {err}"
    );

    // 清理
    let _: i64 = cluster
        .query(redis::cmd("DEL").arg(&own_key).arg(&foreign_key))
        .await
        .expect("清理失败");
}
