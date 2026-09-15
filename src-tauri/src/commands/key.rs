//! # Key 操作命令
//!
//! 提供对 Redis key 的基本操作命令：列出、新建、删除。
//! 所有命令都依赖连接池中已建立的连接（`conn_id` 对应连接池中的 key）。

use serde::Serialize;

use crate::connection_pool::{Conn, Pool};
use crate::error::command_error_message;
use crate::models::{ConnType, Connection};

/// 列表中的单个 key。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyEntry {
    /// key 名称
    pub key: String,
    /// 类型：string / hash / list / set / zset / none
    #[serde(rename = "type")]
    pub type_: String,
    /// 剩余存活时间（秒），-1 表示永不过期
    pub ttl: i64,
}

/// 分页请求的默认页大小（调用方未指定 `count` 时使用）。
const DEFAULT_PAGE_SIZE: u32 = 100;

/// 单页允许的最大 key 数，避免调用方传入过大的 `count` 让服务器长时间被 `SCAN` 占用。
const MAX_PAGE_SIZE: u32 = 1000;

/// 单页内最多执行的 `SCAN` 轮次。
///
/// `SCAN` 只保证「所有 key 最终都会被返回」，单次调用可能返回空批次；带 `MATCH`
/// 过滤时命中率还可能很低。该上限让单次请求的工作量有界：命中不足时先把已经扫到的
/// key 返回，游标交给前端滚动时继续拉下一页，而不是把整个 keyspace 一次扫完。
const MAX_SCAN_ROUNDS: usize = 32;

/// 一次分页扫描的结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyPage {
    /// 本页的 key（已附带类型与 TTL）
    pub keys: Vec<KeyEntry>,
    /// 下一页游标；`"0"` 表示当前 keyspace 已扫描完
    pub next_cursor: String,
}

/// 分页列出 key。
///
/// 使用 `SCAN` 游标遍历（**不用 `KEYS *`**，避免阻塞生产实例），并附带每个 key 的
/// 类型与 TTL。参数说明：
///
/// - `cursor`：上一页返回的 `next_cursor`，首页传 `"0"`。
/// - `count`：期望的单页 key 数（`0` 表示用 [`DEFAULT_PAGE_SIZE`]），上限 [`MAX_PAGE_SIZE`]。
/// - `pattern`：用户输入的**原始搜索文本**，按「包含匹配」语义处理（见 [`to_match_pattern`]）；
///   为空或 `None` 表示不过滤。
///
/// 集群模式下游标是 `节点序号:节点内游标` 的复合值（一页可能跨多个节点），对调用方
/// 始终是不透明字符串。redis-rs 对集群连接的 `SCAN` 不做多节点聚合，只会路由到随机
/// 一个节点（见 redis-rs `cluster_routing` 中 SCAN 的路由定义），因此这里用
/// `CLUSTER NODES` 枚举所有主节点、逐节点扫描，避免结果时有时无。
#[tauri::command]
pub async fn list_keys(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    cursor: String,
    count: u32,
    pattern: Option<String>,
) -> Result<KeyPage, String> {
    list_keys_inner(&pool, &conn_id, &cursor, count, pattern.as_deref()).await
}

/// [`list_keys`] 的核心实现，抽成独立函数以便集成测试直接调用。
pub async fn list_keys_inner(
    pool: &Pool,
    conn_id: &str,
    cursor: &str,
    count: u32,
    pattern: Option<&str>,
) -> Result<KeyPage, String> {
    let conn_cfg = pool.get(conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;

    let match_pattern = to_match_pattern(pattern.unwrap_or(""));
    let page_size = match count {
        0 => DEFAULT_PAGE_SIZE,
        n => n.min(MAX_PAGE_SIZE),
    };
    let page_size = u64::from(page_size);

    let (key_names, next_cursor) = if conn_cfg.conn_type == ConnType::Cluster {
        scan_page_cluster(
            &conn_cfg,
            &mut con,
            cursor,
            page_size,
            match_pattern.as_deref(),
        )
        .await?
    } else {
        scan_page_single(&mut con, cursor, page_size, match_pattern.as_deref()).await?
    };

    let mut keys: Vec<KeyEntry> = Vec::with_capacity(key_names.len());
    for key in &key_names {
        let ktype: String = redis::cmd("TYPE")
            .arg(key)
            .query_async(&mut con)
            .await
            .unwrap_or_else(|_| "none".to_string());

        let ttl: i64 = redis::cmd("TTL")
            .arg(key)
            .query_async(&mut con)
            .await
            .unwrap_or(-1);

        keys.push(KeyEntry {
            key: key.clone(),
            ttl: if ttl < 0 { -1 } else { ttl },
            type_: ktype,
        });
    }

    keys.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(KeyPage { keys, next_cursor })
}

/// 把用户输入的原始搜索文本转成 `SCAN` 的 `MATCH` 模式。
///
/// 搜索框的语义是「**不区分大小写的全文包含匹配**」（见 `web/docs` 的搜索说明），而
/// `SCAN` 的 `MATCH` 吃的是大小写敏感的 glob：两者只在输入含 glob 元字符时才等价。因此这里
/// 做两件事：
///
/// 1. 对 `* ? [ ] \` 做转义，再把整串用 `*` 包起来 —— 输入 `a*b` 匹配的就是字面量 `a*b`；
/// 2. 每个 ASCII 字母展开成字符类 `[aA]`，用 glob 的字符类还原「不区分大小写」。
///    （Redis 的 glob 没有 locale 感知的大小写折叠，非 ASCII 字母仍按原样匹配。）
///
/// 返回 `None` 表示不过滤（空串或纯空白）。
fn to_match_pattern(search: &str) -> Option<String> {
    let trimmed = search.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut pattern = String::with_capacity(trimmed.len() * 4 + 2);
    pattern.push('*');
    for ch in trimmed.chars() {
        if ch.is_ascii_alphabetic() {
            pattern.push('[');
            pattern.extend(ch.to_lowercase());
            pattern.extend(ch.to_uppercase());
            pattern.push(']');
        } else {
            if matches!(ch, '*' | '?' | '[' | ']' | '\\') {
                pattern.push('\\');
            }
            pattern.push(ch);
        }
    }
    pattern.push('*');
    Some(pattern)
}

/// 执行一次 `SCAN`，返回（下一个游标，本批 key）。
///
/// `cursor` 为 `0` 且返回游标也为 `0` 时表示扫描结束。
async fn scan_once<C: redis::aio::ConnectionLike>(
    con: &mut C,
    cursor: u64,
    count: u64,
    pattern: Option<&str>,
) -> Result<(u64, Vec<String>), String> {
    let mut cmd = redis::cmd("SCAN");
    cmd.arg(cursor).arg("COUNT").arg(count);
    if let Some(p) = pattern {
        cmd.arg("MATCH").arg(p);
    }
    cmd.query_async(con)
        .await
        .map_err(|e: redis::RedisError| command_error_message(&e, None))
}

/// 单机分页：从 `cursor` 开始扫描，累积到至少 `page_size` 个 key 或游标归零为止。
///
/// 不做「凑满 `page_size` 就丢弃多余 key」的裁剪 —— `SCAN` 的一批 key 无法退还，
/// 丢掉的 key 就再也不会出现在后续分页里。因此单页大小是「至少 `page_size`」。
async fn scan_page_single<C: redis::aio::ConnectionLike>(
    con: &mut C,
    cursor: &str,
    page_size: u64,
    pattern: Option<&str>,
) -> Result<(Vec<String>, String), String> {
    let mut current: u64 = cursor
        .parse()
        .map_err(|_| format!("游标不合法: {cursor}"))?;
    let mut keys: Vec<String> = Vec::new();

    for _ in 0..MAX_SCAN_ROUNDS {
        let (next, batch) = scan_once(con, current, page_size, pattern).await?;
        keys.extend(batch);
        current = next;
        if current == 0 || keys.len() as u64 >= page_size {
            break;
        }
    }

    Ok((keys, current.to_string()))
}

/// 集群分页游标：下次要扫的节点地址 + 该节点内的 SCAN 游标。
///
/// 用**节点地址**而不是节点序号，是因为序号会随 `CLUSTER NODES` 的输出顺序漂移，
/// 翻页途中一旦漂移就会漏读或重复；地址能稳定指向同一个节点。
#[derive(Debug, PartialEq)]
struct ClusterCursor {
    /// `None` 表示从头开始（首页）
    addr: Option<String>,
    /// 节点内的 SCAN 游标
    inner: u64,
}

/// 解析集群游标；`"0"` 或空串表示首页。
fn parse_cluster_cursor(cursor: &str) -> Result<ClusterCursor, String> {
    if cursor.is_empty() || cursor == "0" {
        return Ok(ClusterCursor {
            addr: None,
            inner: 0,
        });
    }
    let invalid = || format!("集群游标不合法: {cursor}");
    // 地址自身含冒号（host:port），因此从右侧切一刀
    let (addr, inner) = cursor.rsplit_once(':').ok_or_else(invalid)?;
    if addr.is_empty() {
        return Err(invalid());
    }
    Ok(ClusterCursor {
        addr: Some(addr.to_string()),
        inner: inner.parse().map_err(|_| invalid())?,
    })
}

/// 组装集群游标（与 [`parse_cluster_cursor`] 互为逆运算）。
fn format_cluster_cursor(addr: &str, inner: u64) -> String {
    format!("{addr}:{inner}")
}

/// 按原连接配置（含认证信息）连接集群中的某个主节点。
async fn connect_cluster_node(
    conn_cfg: &Connection,
    addr: &str,
) -> Result<redis::aio::MultiplexedConnection, String> {
    // 调用方已保证 `host:port` 格式
    let (host, port) = addr.rsplit_once(':').expect("主节点地址格式异常");
    let port: u16 = port
        .parse()
        .map_err(|_| format!("主节点端口无法解析: {addr}"))?;
    let node_cfg = Connection {
        host: host.to_string(),
        port,
        ..conn_cfg.clone()
    };
    let client = redis::Client::open(node_cfg.to_connection_url())
        .map_err(|e: redis::RedisError| format!("连接集群节点 {addr} 失败: {e}"))?;
    client
        .get_multiplexed_async_connection()
        .await
        .map_err(|e: redis::RedisError| format!("连接集群节点 {addr} 失败: {e}"))
}

/// 集群分页：从游标继续扫描，一页可以跨多个主节点。
///
/// 一个节点扫完后自动跳到下一个节点；所有节点都扫完时返回 `"0"`。
async fn scan_page_cluster(
    conn_cfg: &Connection,
    con: &mut Conn,
    cursor: &str,
    page_size: u64,
    pattern: Option<&str>,
) -> Result<(Vec<String>, String), String> {
    let cursor = parse_cluster_cursor(cursor)?;

    let nodes: String = redis::cmd("CLUSTER")
        .arg("NODES")
        .query_async(&mut *con)
        .await
        .map_err(|e: redis::RedisError| e.to_string())?;

    let addrs = parse_master_addrs(&nodes, &conn_cfg.host);
    if addrs.is_empty() {
        return Err("集群中未找到可用的主节点".into());
    }

    let mut node_idx = match &cursor.addr {
        None => 0,
        // 游标指向的节点已不是主节点（扩容/缩容/主从切换）：报错让用户刷新，
        // 而不是猜一个位置继续翻页（那会静默漏读或重复）
        Some(addr) => addrs
            .iter()
            .position(|a| a == addr)
            .ok_or_else(|| format!("集群结构已变化（{addr} 已不是主节点），请刷新 Key 列表"))?,
    };
    let mut node_cursor = cursor.inner;

    let mut keys: Vec<String> = Vec::new();
    let mut rounds = 0usize;

    'nodes: while node_idx < addrs.len() {
        let mut node_con = connect_cluster_node(conn_cfg, &addrs[node_idx]).await?;
        loop {
            if rounds >= MAX_SCAN_ROUNDS || keys.len() as u64 >= page_size {
                break 'nodes;
            }
            rounds += 1;
            let (next, batch) = scan_once(&mut node_con, node_cursor, page_size, pattern).await?;
            keys.extend(batch);
            node_cursor = next;
            if node_cursor == 0 {
                // 当前节点扫完，接着扫下一个节点
                node_idx += 1;
                node_cursor = 0;
                break;
            }
        }
    }

    let next_cursor = match addrs.get(node_idx) {
        Some(addr) => format_cluster_cursor(addr, node_cursor),
        None => "0".to_string(),
    };
    Ok((keys, next_cursor))
}

/// 在单个连接上完整执行一轮 `SCAN`，返回所有 key 名（导出等需要全量的场景使用）。
async fn scan_key_names<C: redis::aio::ConnectionLike>(con: &mut C) -> Result<Vec<String>, String> {
    let mut cursor = 0u64;
    let mut keys: Vec<String> = Vec::new();
    loop {
        let (next_cursor, batch) = scan_once(con, cursor, 500, None).await?;
        cursor = next_cursor;
        keys.extend(batch);
        if cursor == 0 {
            break;
        }
    }
    Ok(keys)
}

/// 解析 `CLUSTER NODES` 输出中的主节点地址列表（`ip:port`）。
///
/// - 跳过非 master 行（slave / handshake 等）与处于 fail 状态的节点。
/// - 节点未公布地址（`ip` 为空）时回退到 `fallback_host`。
fn parse_master_addrs(cluster_nodes: &str, fallback_host: &str) -> Vec<String> {
    cluster_nodes
        .lines()
        .filter_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 3 {
                return None;
            }
            let flags = cols[2];
            if !flags.split(',').any(|f| f == "master") || flags.contains("fail") {
                return None;
            }
            // 地址形如 `ip:port@cport` 或 `ip:port@cport,hostname`
            let host_port = cols[1].split(',').next()?.split('@').next()?;
            let (host, port) = host_port.rsplit_once(':')?;
            if port.is_empty() {
                return None;
            }
            let host = if host.is_empty() { fallback_host } else { host };
            Some(format!("{host}:{port}"))
        })
        .collect()
}

/// 集群模式下枚举所有主节点并逐节点 `SCAN`，合并去重后返回全部 key 名。
async fn scan_cluster_key_names(
    conn_cfg: &Connection,
    con: &mut Conn,
) -> Result<Vec<String>, String> {
    let nodes: String = redis::cmd("CLUSTER")
        .arg("NODES")
        .query_async(&mut *con)
        .await
        .map_err(|e: redis::RedisError| e.to_string())?;

    let addrs = parse_master_addrs(&nodes, &conn_cfg.host);
    if addrs.is_empty() {
        return Err("集群中未找到可用的主节点".into());
    }

    let mut keys: Vec<String> = Vec::new();
    for addr in addrs {
        let mut c = connect_cluster_node(conn_cfg, &addr).await?;
        keys.extend(scan_key_names(&mut c).await?);
    }

    keys.sort();
    keys.dedup();
    Ok(keys)
}

/// 枚举当前 keyspace 的全部 key 名（不分页）。
///
/// 供导出这类必须拿到完整列表的场景使用；交互式的 key 列表走 [`list_keys`] 分页，
/// 避免一次性把整个 keyspace 拉进内存。集群模式下逐主节点扫描后合并去重。
pub(crate) async fn collect_all_key_names(
    conn_cfg: &Connection,
    con: &mut Conn,
) -> Result<Vec<String>, String> {
    if conn_cfg.conn_type == ConnType::Cluster {
        scan_cluster_key_names(conn_cfg, con).await
    } else {
        scan_key_names(con).await
    }
}

/// 新建或覆盖一个字符串类型的 key（等价于 `SET key value [EX ttl]`）。
///
/// `ttl` 为 `-1` 表示不设置过期时间；为正整数时表示过期秒数。
/// 其他值（如 `0`、负数且不等于 `-1`）会返回参数错误。
#[tauri::command]
pub async fn set_key(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    key: String,
    value: String,
    ttl: i64,
) -> Result<String, String> {
    set_key_inner(&pool, conn_id, key, value, ttl).await
}

/// 构造 `SET key value [EX ttl]` 命令。
///
/// 调用方需保证 `ttl` 已经过合法性校验；本函数仅负责命令组装。
fn build_set_cmd(key: &str, value: &str, ttl: i64) -> redis::Cmd {
    let mut cmd = redis::cmd("SET");
    cmd.arg(key).arg(value);
    if ttl > 0 {
        cmd.arg("EX").arg(ttl);
    }
    cmd
}

async fn set_key_inner(
    pool: &Pool,
    conn_id: String,
    key: String,
    value: String,
    ttl: i64,
) -> Result<String, String> {
    if ttl != -1 && ttl <= 0 {
        return Err("TTL 必须为 -1 或正整数".into());
    }

    pool.ensure_writable(&conn_id).map_err(|e| e.to_string())?;
    let conn_cfg = pool.get(&conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.conn(&conn_id).map_err(|e| e.to_string())?;

    let cmd = build_set_cmd(&key, &value, ttl);
    let ok: String = cmd
        .query_async(&mut con)
        .await
        .map_err(|e: redis::RedisError| command_error_message(&e, Some(&conn_cfg)))?;

    Ok(ok)
}

#[cfg(test)]
mod tests {
    use super::{
        build_set_cmd, format_cluster_cursor, list_keys_inner, parse_cluster_cursor,
        parse_master_addrs, set_key_inner, to_match_pattern, ClusterCursor,
    };
    use crate::connection_pool::Pool;
    use crate::models::{ConnType, Connection};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_id() -> String {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        format!("{}_{}", ts, n)
    }

    fn unique_key(prefix: &str) -> String {
        format!("maidi:test:{}:{}", prefix, unique_id())
    }

    async fn test_pool(conn_id: &str) -> Result<Pool, String> {
        let pool = Pool::new();
        let conn = Connection {
            id: conn_id.into(),
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
            .map_err(|e| format!("连接 Redis 失败，请确认服务已启动: {e}"))?;
        Ok(pool)
    }

    #[test]
    fn parse_master_addrs_picks_healthy_masters() {
        let nodes = "abc 127.0.0.1:7001@17001 myself,master - 0 0 1 connected 0-5460\n\
                     def 127.0.0.1:7002@17002 slave abc 0 0 1 connected\n\
                     ghi :7003@17003 master - 0 0 2 connected 5461-10922\n\
                     jkl 127.0.0.1:7004@17004 master,fail? - 0 0 3 connected\n\
                     mno 127.0.0.1:7005@17005,noannounced.host master - 0 0 3 connected";
        let addrs = parse_master_addrs(nodes, "10.0.0.1");
        assert_eq!(
            addrs,
            vec!["127.0.0.1:7001", "10.0.0.1:7003", "127.0.0.1:7005"]
        );
    }

    #[test]
    fn parse_master_addrs_skips_empty_input() {
        assert!(parse_master_addrs("", "127.0.0.1").is_empty());
        assert!(parse_master_addrs("not a nodes reply", "127.0.0.1").is_empty());
    }

    #[test]
    fn match_pattern_wraps_as_contains_and_escapes_glob() {
        // 空搜索不过滤
        assert_eq!(to_match_pattern(""), None);
        assert_eq!(to_match_pattern("   "), None);

        // ASCII 字母展开成字符类：还原搜索框「不区分大小写」的语义
        assert_eq!(
            to_match_pattern("user"),
            Some("*[uU][sS][eE][rR]*".to_string())
        );
        // 前后空白不参与匹配
        assert_eq!(to_match_pattern("  ab  "), Some("*[aA][bB]*".to_string()));
        // 用户输入的 glob 元字符按字面量处理
        assert_eq!(to_match_pattern("a*b"), Some("*[aA]\\*[bB]*".to_string()));
        assert_eq!(
            to_match_pattern("k?[1]\\"),
            Some("*[kK]\\?\\[1\\]\\\\*".to_string())
        );
        // 非 ASCII 字符原样保留（Redis glob 没有大小写折叠）
        assert_eq!(to_match_pattern("用户:1"), Some("*用户:1*".to_string()));
    }

    #[test]
    fn cluster_cursor_parses_and_rejects_garbage() {
        // 首页
        assert_eq!(
            parse_cluster_cursor("0"),
            Ok(ClusterCursor {
                addr: None,
                inner: 0
            })
        );
        assert_eq!(
            parse_cluster_cursor(""),
            Ok(ClusterCursor {
                addr: None,
                inner: 0
            })
        );

        // 地址自带冒号，必须从右侧切开
        assert_eq!(
            parse_cluster_cursor("127.0.0.1:7001:1234"),
            Ok(ClusterCursor {
                addr: Some("127.0.0.1:7001".into()),
                inner: 1234
            })
        );
        // 编解码互逆
        assert_eq!(
            parse_cluster_cursor(&format_cluster_cursor("10.0.0.2:7002", 77)),
            Ok(ClusterCursor {
                addr: Some("10.0.0.2:7002".into()),
                inner: 77
            })
        );

        assert!(parse_cluster_cursor("abc").is_err());
        assert!(parse_cluster_cursor("1:x").is_err());
        assert!(parse_cluster_cursor(":12").is_err());
    }

    #[test]
    fn build_set_cmd_without_ttl() {
        let cmd = build_set_cmd("mykey", "myvalue", -1);
        assert_eq!(
            cmd.get_packed_command(),
            b"*3\r\n$3\r\nSET\r\n$5\r\nmykey\r\n$7\r\nmyvalue\r\n"
        );
    }

    #[test]
    fn build_set_cmd_with_ttl() {
        let cmd = build_set_cmd("mykey", "myvalue", 10);
        assert_eq!(
            cmd.get_packed_command(),
            b"*5\r\n$3\r\nSET\r\n$5\r\nmykey\r\n$7\r\nmyvalue\r\n$2\r\nEX\r\n$2\r\n10\r\n"
        );
    }

    // 以下测试依赖本地 Redis，默认标记为 #[ignore]；
    // 启动 Redis 后可通过 `cargo test -- --ignored` 运行。

    #[tokio::test]
    #[ignore]
    async fn set_key_without_ttl() -> Result<(), String> {
        let conn_id = format!("key_test_no_ttl:{}", unique_id());
        let pool = test_pool(&conn_id).await?;
        let key = unique_key("no_ttl");
        let value = "hello".to_string();

        let result = set_key_inner(&pool, conn_id.clone(), key.clone(), value, -1).await;
        assert!(result.is_ok(), "SET 应当成功: {:?}", result.err());

        let ttl: i64 = redis::cmd("TTL")
            .arg(&key)
            .query_async(&mut pool.conn(&conn_id).unwrap())
            .await
            .map_err(|e| e.to_string())?;
        assert_eq!(ttl, -1, "未设置 TTL 的 key 应当永不过期");
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn set_key_with_ttl_sets_expiry() -> Result<(), String> {
        let conn_id = format!("key_test_ttl:{}", unique_id());
        let pool = test_pool(&conn_id).await?;
        let key = unique_key("with_ttl");
        let value = "world".to_string();

        let result = set_key_inner(&pool, conn_id.clone(), key.clone(), value, 10).await;
        assert!(result.is_ok(), "SET 应当成功: {:?}", result.err());

        let ttl: i64 = redis::cmd("TTL")
            .arg(&key)
            .query_async(&mut pool.conn(&conn_id).unwrap())
            .await
            .map_err(|e| e.to_string())?;
        assert!(
            ttl > 0 && ttl <= 10,
            "TTL 应当被设置为正数且不超过 10 秒: got {}",
            ttl
        );
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn set_key_updates_ttl_of_existing_key() -> Result<(), String> {
        let conn_id = format!("key_test_update_ttl:{}", unique_id());
        let pool = test_pool(&conn_id).await?;
        let key = unique_key("update_ttl");

        set_key_inner(&pool, conn_id.clone(), key.clone(), "first".into(), -1)
            .await
            .map_err(|e| format!("首次 SET 失败: {e}"))?;

        let initial_ttl: i64 = redis::cmd("TTL")
            .arg(&key)
            .query_async(&mut pool.conn(&conn_id).unwrap())
            .await
            .map_err(|e| e.to_string())?;
        assert_eq!(initial_ttl, -1, "未设置 TTL 的 key 应当永不过期");

        set_key_inner(&pool, conn_id.clone(), key.clone(), "second".into(), 10)
            .await
            .map_err(|e| format!("更新 TTL 的 SET 失败: {e}"))?;

        let updated_ttl: i64 = redis::cmd("TTL")
            .arg(&key)
            .query_async(&mut pool.conn(&conn_id).unwrap())
            .await
            .map_err(|e| e.to_string())?;
        assert!(
            updated_ttl > 0 && updated_ttl <= 10,
            "更新后的 TTL 应当为正数且不超过 10 秒: got {}",
            updated_ttl
        );
        Ok(())
    }

    #[tokio::test]
    async fn set_key_rejects_invalid_ttl() {
        let pool = Pool::new();
        let key = unique_key("invalid_ttl");
        let value = "x".to_string();
        let conn_id = format!("key_test_invalid_ttl:{}", unique_id());

        for invalid_ttl in [0, -2, -100] {
            let result = set_key_inner(
                &pool,
                conn_id.clone(),
                key.clone(),
                value.clone(),
                invalid_ttl,
            )
            .await;
            assert!(result.is_err(), "TTL={} 应当返回错误", invalid_ttl);
            assert!(
                result.unwrap_err().contains("TTL"),
                "错误信息应提示 TTL 参数"
            );
        }
    }

    // ---------- 分页（依赖本地 Redis，默认 #[ignore]） ----------

    /// 按游标把所有页拉完，返回（全部 key 名, 页数）。
    async fn page_through(
        pool: &Pool,
        conn_id: &str,
        count: u32,
        pattern: Option<&str>,
    ) -> Result<(Vec<String>, usize), String> {
        let mut cursor = "0".to_string();
        let mut names: Vec<String> = Vec::new();
        let mut pages = 0usize;
        loop {
            let page = list_keys_inner(pool, conn_id, &cursor, count, pattern).await?;
            names.extend(page.keys.into_iter().map(|k| k.key));
            pages += 1;
            assert!(pages < 100, "分页未收敛，游标处理可能有误");
            if page.next_cursor == "0" {
                break;
            }
            cursor = page.next_cursor;
        }
        Ok((names, pages))
    }

    #[tokio::test]
    #[ignore]
    async fn list_keys_pages_cover_every_key_exactly_once() -> Result<(), String> {
        let conn_id = format!("key_test_page:{}", unique_id());
        let pool = test_pool(&conn_id).await?;
        let prefix = unique_key("page");
        let total = 250usize;
        {
            let mut con = pool.conn(&conn_id).unwrap();
            for i in 0..total {
                redis::cmd("SET")
                    .arg(format!("{prefix}:{i:04}"))
                    .arg("v")
                    .query_async::<_, ()>(&mut con)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }

        let (names, pages) = page_through(&pool, &conn_id, 40, Some(&prefix)).await?;
        assert!(
            pages > 1,
            "每页 40 个、共 {total} 个 key，至少应分 2 页返回"
        );

        let mut expected: Vec<String> = (0..total).map(|i| format!("{prefix}:{i:04}")).collect();
        let mut actual = names;
        expected.sort();
        actual.sort();
        // 无遗漏也无重复：排序后逐项相等
        assert_eq!(actual, expected, "分页结果必须覆盖全部 key 且不重复");

        // TYPE / TTL 富化在分页路径上同样生效
        let first = list_keys_inner(&pool, &conn_id, "0", 40, Some(&prefix)).await?;
        assert!(!first.keys.is_empty());
        assert!(
            first
                .keys
                .iter()
                .all(|k| k.type_ == "string" && k.ttl == -1),
            "首页应带正确的类型与 TTL: {:?}",
            first.keys.first()
        );

        let mut con = pool.conn(&conn_id).unwrap();
        redis::cmd("DEL")
            .arg(&expected)
            .query_async::<_, ()>(&mut con)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn list_keys_pattern_miss_returns_nothing() -> Result<(), String> {
        let conn_id = format!("key_test_page_miss:{}", unique_id());
        let pool = test_pool(&conn_id).await?;
        let (names, _) = page_through(&pool, &conn_id, 40, Some("maidi:no-such-key-xyz")).await?;
        assert!(
            names.is_empty(),
            "不存在的搜索文本不应命中任何 key: {names:?}"
        );
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn list_keys_pattern_is_case_insensitive_contains() -> Result<(), String> {
        let conn_id = format!("key_test_case:{}", unique_id());
        let pool = test_pool(&conn_id).await?;
        let prefix = unique_key("case");
        let upper = format!("{prefix}:USER:Alpha");
        let lower = format!("{prefix}:user:beta");
        let star = format!("{prefix}:star*key");
        let star_lookalike = format!("{prefix}:starXkey");
        {
            let mut con = pool.conn(&conn_id).unwrap();
            for key in [&upper, &lower, &star, &star_lookalike] {
                redis::cmd("SET")
                    .arg(key)
                    .arg("v")
                    .query_async::<_, ()>(&mut con)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }

        // 小写搜索应命中大写 key（字符类 [uU]… 生效，保持搜索框原有的大小写不敏感语义）
        let (names, _) = page_through(&pool, &conn_id, 10, Some(&format!("{prefix}:user"))).await?;
        assert!(names.contains(&upper), "小写搜索应命中 {}", upper);
        assert!(names.contains(&lower), "小写搜索应命中 {}", lower);

        // `*` 是字面量而不是通配符：只命中真的含 `*` 的那个 key
        let (names, _) =
            page_through(&pool, &conn_id, 10, Some(&format!("{prefix}:star*key"))).await?;
        assert_eq!(
            names,
            vec![star.clone()],
            "`*` 应按字面量匹配，且不得命中 {}",
            star_lookalike
        );

        let mut con = pool.conn(&conn_id).unwrap();
        for key in [&upper, &lower, &star, &star_lookalike] {
            redis::cmd("DEL")
                .arg(key)
                .query_async::<_, ()>(&mut con)
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn list_keys_rejects_invalid_cursor() {
        let conn_id = format!("key_test_page_bad:{}", unique_id());
        let pool = test_pool(&conn_id).await.expect("连接 Redis 失败");
        let err = list_keys_inner(&pool, &conn_id, "not-a-cursor", 10, None)
            .await
            .expect_err("非法游标应当报错");
        assert!(err.contains("游标"), "错误信息应说明游标问题: {err}");
    }
}

/// 删除一个或多个 key，返回实际删除的 key 数量。
#[tauri::command]
pub async fn del_key(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    keys: Vec<String>,
) -> Result<i64, String> {
    pool.ensure_writable(&conn_id).map_err(|e| e.to_string())?;
    let conn_cfg = pool.get(&conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.conn(&conn_id).map_err(|e| e.to_string())?;
    let mut cmd = redis::cmd("DEL");
    for k in &keys {
        cmd.arg(k);
    }
    let n: i64 = cmd
        .query_async(&mut con)
        .await
        .map_err(|e: redis::RedisError| command_error_message(&e, Some(&conn_cfg)))?;
    Ok(n)
}

/// 获取一个字符串类型 key 的值。若 key 不存在或类型不是 string，返回错误信息。
#[tauri::command]
pub async fn get_string(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    key: String,
) -> Result<Option<String>, String> {
    let conn_cfg = pool.get(&conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.conn(&conn_id).map_err(|e| e.to_string())?;
    let val: Option<String> = redis::cmd("GET")
        .arg(&key)
        .query_async(&mut con)
        .await
        .map_err(|e: redis::RedisError| command_error_message(&e, Some(&conn_cfg)))?;
    Ok(val)
}
