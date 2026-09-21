//! # Key 操作命令
//!
//! 提供对 Redis key 的基本操作命令：列出、新建、删除。
//! 所有命令都依赖连接池中已建立的连接（`conn_id` 对应连接池中的 key）。

use serde::Serialize;

use crate::config::ConnectionTimeout;
use crate::connection_pool::{Conn, Pool, PooledConn};
use crate::error::command_error_text;
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
/// 类型与 TTL（一次 pipeline 取回，见 [`enrich_keys`]）。参数说明：
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

    // 两条路径都返回已富化的条目：类型与 TTL 的往返次数与扫描方式绑定，
    // 放在各自内部做才能各自压到最少（见两个函数的说明）
    let (mut keys, next_cursor) = if conn_cfg.conn_type == ConnType::Cluster {
        scan_page_cluster(
            &conn_cfg,
            &mut con,
            cursor,
            page_size,
            match_pattern.as_deref(),
        )
        .await?
    } else {
        // 单机模式的「直连节点」就是这条连接：出错时把节点地址带上（MOVED 提示要用）
        scan_page_single(
            &conn_cfg,
            &mut con,
            cursor,
            page_size,
            match_pattern.as_deref(),
        )
        .await?
    };

    keys.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(KeyPage { keys, next_cursor })
}

/// 富化一批 key：用**一次 pipeline** 取回全部 TYPE 与 TTL。
///
/// 命令按 `TYPE k1, TTL k1, TYPE k2, TTL k2, …` 排队，因此返回的 `2n` 个值与 key
/// 顺序一一对应。原先每个 key 各发两次命令（一页 100 个 key 就是 200 次往返），
/// 现在整批只占一次往返；单条 pipeline 最多 [`MAX_PAGE_SIZE`] × 2 = 2000 条命令。
///
/// 调用方需保证 `con` **能直接服务这批 key**：集群里一条 pipeline 只能发给一个节点，
/// 跨节点的 key 混在一起会被服务端判 `CROSSSLOT`，所以集群路径按节点分组后各自调用。
///
/// 服务端对不存在的 key 也照常应答（TYPE → `none`、TTL → `-2`），不会整批失败；
/// 只有整批拿不到响应时才返回错误 —— 连接不可用时把整页当成 `none` / `-1` 返回，
/// 反而是在骗用户。
///
/// `direct` 与 [`command_error_text`] 的第二个参数同义：单机模式传当前连接配置，
/// 让迁移中的 key 报 MOVED 时能点明「当前直连的是谁」，集群模式传 `None`。
async fn enrich_keys(
    con: &mut PooledConn,
    keys: &[String],
    direct: Option<&Connection>,
) -> Result<Vec<KeyEntry>, String> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }

    let mut pipe = redis::pipe();
    for key in keys {
        pipe.cmd("TYPE").arg(key).cmd("TTL").arg(key);
    }
    let values: Vec<redis::Value> = con
        .query_pipeline(&pipe)
        .await
        .map_err(|e| command_error_text(&e, direct))?;

    let expected = keys.len() * 2;
    if values.len() != expected {
        // 正常情况下驱动会严格按命令数返回；数量不符说明协议层出了问题，
        // 此时按下标取值会把类型和 TTL 错位配到别的 key 上，宁可报错
        return Err(format!(
            "TYPE/TTL 批量查询返回了 {} 个结果，期望 {expected} 个",
            values.len()
        ));
    }

    let entries = keys
        .iter()
        .enumerate()
        .map(|(i, key)| {
            // 单条解析失败按「查不到」处理（与逐条查询时 unwrap_or 的兜底一致）
            let type_ = redis::from_redis_value::<String>(&values[i * 2])
                .unwrap_or_else(|_| "none".to_string());
            let ttl = redis::from_redis_value::<i64>(&values[i * 2 + 1]).unwrap_or(-1);
            KeyEntry {
                key: key.clone(),
                type_,
                ttl: if ttl < 0 { -1 } else { ttl },
            }
        })
        .collect();
    Ok(entries)
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
async fn scan_once(
    con: &mut PooledConn,
    cursor: u64,
    count: u64,
    pattern: Option<&str>,
) -> Result<(u64, Vec<String>), String> {
    let mut cmd = redis::cmd("SCAN");
    cmd.arg(cursor).arg("COUNT").arg(count);
    if let Some(p) = pattern {
        cmd.arg("MATCH").arg(p);
    }
    con.query(&cmd)
        .await
        .map_err(|e| command_error_text(&e, None))
}

/// 单机分页：从 `cursor` 开始扫描，累积到至少 `page_size` 个 key 或游标归零为止，
/// 再用一趟 pipeline 补齐类型与 TTL。
///
/// 不做「凑满 `page_size` 就丢弃多余 key」的裁剪 —— `SCAN` 的一批 key 无法退还，
/// 丢掉的 key 就再也不会出现在后续分页里。因此单页大小是「至少 `page_size`」。
async fn scan_page_single(
    conn_cfg: &Connection,
    con: &mut PooledConn,
    cursor: &str,
    page_size: u64,
    pattern: Option<&str>,
) -> Result<(Vec<KeyEntry>, String), String> {
    let mut current: u64 = cursor
        .parse()
        .map_err(|_| format!("游标不合法: {cursor}"))?;
    let mut names: Vec<String> = Vec::new();

    for _ in 0..MAX_SCAN_ROUNDS {
        let (next, batch) = scan_once(con, current, page_size, pattern).await?;
        names.extend(batch);
        current = next;
        if current == 0 || names.len() as u64 >= page_size {
            break;
        }
    }

    // 单机连接就是这些 key 的归属连接：整页一次 pipeline 即可
    let keys = enrich_keys(con, &names, Some(conn_cfg)).await?;
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
///
/// 返回带超时配置的直连句柄：节点连接同样受 [`ConnectionTimeout::connect`] 约束，
/// 命令超时沿用调用方句柄的配置（`timeout`）。
async fn connect_cluster_node(
    conn_cfg: &Connection,
    addr: &str,
    timeout: ConnectionTimeout,
) -> Result<PooledConn, String> {
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
        .map_err(|e| format!("连接集群节点 {addr} 失败: {e}"))?;
    let conn = tokio::time::timeout(timeout.connect, client.get_multiplexed_async_connection())
        .await
        .map_err(|_| timeout.connect_timeout_error().to_string())?
        .map_err(|e| format!("连接集群节点 {addr} 失败: {e}"))?;
    Ok(PooledConn::new(Conn::Node(conn), timeout))
}

/// 集群分页：从游标继续扫描，一页可以跨多个主节点。
///
/// 一个节点扫完后自动跳到下一个节点；所有节点都扫完时返回 `"0"`。
/// 类型与 TTL 在**每个节点自己的连接上**就地富化：`SCAN` 只返回该节点负责的 key，
/// 一个节点的 key 刚好能放进同一条 pipeline（跨节点会 CROSSSLOT）；
/// 也省掉了「富化时再按 slot 路由一遍」的往返与重连。
///
/// 代价是这批 key 不再跟随 MOVED / ASK 重定向（集群连接会跟，节点直连不会）：
/// 首页扫过之后槽才迁走的 key（要求此刻正在做 resharding）会让这一页报 MOVED，
/// 提示里带着新的负责节点，刷新一次即可 —— 不做隐式重试，重试也只是把同样的竞态再撞一次。
async fn scan_page_cluster(
    conn_cfg: &Connection,
    con: &mut PooledConn,
    cursor: &str,
    page_size: u64,
    pattern: Option<&str>,
) -> Result<(Vec<KeyEntry>, String), String> {
    let cursor = parse_cluster_cursor(cursor)?;

    let nodes: String = con
        .query(redis::cmd("CLUSTER").arg("NODES"))
        .await
        .map_err(|e| e.to_string())?;

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

    let mut entries: Vec<KeyEntry> = Vec::new();
    let mut rounds = 0usize;

    while node_idx < addrs.len() {
        let mut node_con = connect_cluster_node(conn_cfg, &addrs[node_idx], con.timeout()).await?;
        // 本节点这一批（扫完整个节点，或扫到页满 / 轮次用尽为止）
        let mut names: Vec<String> = Vec::new();
        loop {
            if rounds >= MAX_SCAN_ROUNDS || (entries.len() + names.len()) as u64 >= page_size {
                break;
            }
            rounds += 1;
            let (next, batch) = scan_once(&mut node_con, node_cursor, page_size, pattern).await?;
            names.extend(batch);
            node_cursor = next;
            if node_cursor == 0 {
                // 当前节点扫完，接着扫下一个节点
                node_idx += 1;
                node_cursor = 0;
                break;
            }
        }

        // 扫到一半也要先富化（page_size 已满足时，这批 key 若不返回就再也拿不到了）
        entries.extend(enrich_keys(&mut node_con, &names, None).await?);
        if rounds >= MAX_SCAN_ROUNDS || entries.len() as u64 >= page_size {
            break;
        }
    }

    let next_cursor = match addrs.get(node_idx) {
        Some(addr) => format_cluster_cursor(addr, node_cursor),
        None => "0".to_string(),
    };
    Ok((entries, next_cursor))
}

/// 在单个连接上完整执行一轮 `SCAN`，返回所有 key 名（导出等需要全量的场景使用）。
async fn scan_key_names(con: &mut PooledConn) -> Result<Vec<String>, String> {
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
    con: &mut PooledConn,
) -> Result<Vec<String>, String> {
    let nodes: String = con
        .query(redis::cmd("CLUSTER").arg("NODES"))
        .await
        .map_err(|e| e.to_string())?;

    let addrs = parse_master_addrs(&nodes, &conn_cfg.host);
    if addrs.is_empty() {
        return Err("集群中未找到可用的主节点".into());
    }

    let mut keys: Vec<String> = Vec::new();
    for addr in addrs {
        let mut c = connect_cluster_node(conn_cfg, &addr, con.timeout()).await?;
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
    con: &mut PooledConn,
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
    let ok: String = con
        .query(&cmd)
        .await
        .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;

    Ok(ok)
}

#[cfg(test)]
mod tests {
    use super::{
        build_set_cmd, format_cluster_cursor, list_keys_inner, parse_cluster_cursor,
        parse_master_addrs, set_key_inner, to_match_pattern, ClusterCursor, KeyEntry,
    };
    use crate::config::ConnectionTimeout;
    use crate::connection_pool::Pool;
    use crate::models::{ConnType, Connection};
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread::JoinHandle;
    use std::time::Duration;

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

    fn conn_config(id: &str, port: u16) -> Connection {
        Connection {
            id: id.into(),
            name: "integration".into(),
            host: "127.0.0.1".into(),
            port,
            conn_type: ConnType::Single,
            readonly: false,
            separator: ":".into(),
            db: 0,
            username: None,
            password: None,
        }
    }

    async fn test_pool(conn_id: &str) -> Result<Pool, String> {
        let pool = Pool::new();
        pool.connect(&conn_config(conn_id, 6379))
            .await
            .map_err(|e| format!("连接 Redis 失败，请确认服务已启动: {e}"))?;
        Ok(pool)
    }

    /// 一个最小的 RESP 假服务器，专门验证「整页一次 pipeline」。
    ///
    /// 约定：`SCAN` 回一批固定的 key（游标 `0`，一趟扫完），之后**必须收齐 2n 条
    /// TYPE / TTL 才逐条回包**。这个「不齐不回」本身就是断言 —— 逐 key 往返的实现会
    /// 在等第一条回复时死等，直到命令超时才返回错误；只有把整批命令一次性发出去的
    /// 实现能拿到回复。
    ///
    /// 回复内容按收到的参数生成（TYPE → `type-<key>`，TTL → 该 key 在批次里的序号），
    /// 于是「值有没有配错 key」也能一并断言。
    ///
    /// 线程在测试结束后自行泄漏，进程退出即回收（与 `connection_pool.rs` 的桩服务器一致）。
    fn spawn_batch_stub(keys: Vec<String>) -> (u16, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定本地端口失败");
        let port = listener.local_addr().expect("读取本地端口失败").port();
        let handle = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let keys = keys.clone();
                std::thread::spawn(move || serve_batch(stream, &keys));
            }
        });
        (port, handle)
    }

    /// 见 [`spawn_batch_stub`]：单条连接上的服务循环。
    fn serve_batch(stream: TcpStream, keys: &[String]) {
        let Ok(mut writer) = stream.try_clone() else {
            return;
        };
        let mut reader = BufReader::new(stream);
        let mut scanned = false;
        let mut pending: Vec<String> = Vec::new();

        while let Some((name, args)) = read_command(&mut reader) {
            let name = name.to_ascii_uppercase();
            if !scanned {
                // 建连握手（CLIENT SETINFO 等）照常回 +OK，否则连不上
                let reply = if name == "SCAN" {
                    scanned = true;
                    scan_reply(keys)
                } else {
                    "+OK\r\n".to_string()
                };
                if writer.write_all(reply.as_bytes()).is_err() {
                    return;
                }
                continue;
            }

            // 富化阶段：只收集不回，等整批到齐
            let key = args.last().cloned().unwrap_or_default();
            let index = keys.iter().position(|k| *k == key).unwrap_or(0);
            pending.push(match name.as_str() {
                "TYPE" => format!("+type-{key}\r\n"),
                "TTL" => format!(":{index}\r\n"),
                _ => "+OK\r\n".to_string(),
            });
            if pending.len() == keys.len() * 2 {
                for reply in pending.drain(..) {
                    if writer.write_all(reply.as_bytes()).is_err() {
                        return;
                    }
                }
                if writer.flush().is_err() {
                    return;
                }
            }
        }
    }

    /// `SCAN` 的固定回复：游标 `0` + 给定的一批 key。
    fn scan_reply(keys: &[String]) -> String {
        let mut reply = format!("*2\r\n$1\r\n0\r\n*{}\r\n", keys.len());
        for key in keys {
            reply.push_str(&format!("${}\r\n{key}\r\n", key.len()));
        }
        reply
    }

    /// 从 RESP 请求流里读出一条命令，返回（命令名, 参数列表）；流结束返回 `None`。
    fn read_command(reader: &mut impl BufRead) -> Option<(String, Vec<String>)> {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 {
            return None;
        }
        let argc: usize = header.trim().trim_start_matches('*').parse().ok()?;
        let mut args = Vec::with_capacity(argc);
        for _ in 0..argc {
            let mut len_line = String::new();
            if reader.read_line(&mut len_line).ok()? == 0 {
                return None;
            }
            let len: usize = len_line.trim().trim_start_matches('$').parse().ok()?;
            let mut arg = vec![0u8; len];
            reader.read_exact(&mut arg).ok()?;
            let mut crlf = [0u8; 2];
            reader.read_exact(&mut crlf).ok()?;
            args.push(String::from_utf8_lossy(&arg).into_owned());
        }
        let name = args.first().cloned().unwrap_or_default();
        Some((name, args))
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

    /// 整页的类型与 TTL 走**一次 pipeline**：假服务器要求收齐全部 TYPE/TTL 才回包，
    /// 逐 key 往返的实现在这里会一直等到命令超时（即回到 N+1 的老路，测试立刻红）。
    ///
    /// 顺带断言「值没有配错 key」：假服务器按收到的参数生成回复（类型里带 key 名、
    /// TTL 用该 key 的序号），错位就会立刻暴露。
    #[tokio::test]
    async fn list_keys_enriches_a_page_with_one_pipelined_round_trip() {
        let keys: Vec<String> = ["alpha", "beta", "gamma"]
            .iter()
            .map(|s| format!("maidi:test:pipeline:{s}"))
            .collect();
        let (port, _server) = spawn_batch_stub(keys.clone());
        let timeout = ConnectionTimeout {
            connect: Duration::from_secs(5),
            command: Duration::from_millis(500),
        };
        let pool = Pool::with_timeout(timeout);
        let conn = conn_config("pipeline_stub", port);
        pool.connect(&conn)
            .await
            .expect("假服务器会回握手命令，建连应当成功");

        let page = list_keys_inner(&pool, "pipeline_stub", "0", keys.len() as u32, None)
            .await
            .expect("整页富化应当一次往返完成，而不是逐条等待");
        assert_eq!(page.next_cursor, "0", "假服务器一趟扫完，游标应归零");
        assert_eq!(page.keys.len(), keys.len());

        let by_key: HashMap<&str, &KeyEntry> =
            page.keys.iter().map(|e| (e.key.as_str(), e)).collect();
        for (index, key) in keys.iter().enumerate() {
            let entry = by_key
                .get(key.as_str())
                .unwrap_or_else(|| panic!("{key} 应当出现在结果里"));
            assert_eq!(entry.type_, format!("type-{key}"), "类型配错了 key");
            assert_eq!(entry.ttl, index as i64, "TTL 配错了 key");
        }
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

        let ttl: i64 = pool
            .conn(&conn_id)
            .unwrap()
            .query(redis::cmd("TTL").arg(&key))
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

        let ttl: i64 = pool
            .conn(&conn_id)
            .unwrap()
            .query(redis::cmd("TTL").arg(&key))
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

        let initial_ttl: i64 = pool
            .conn(&conn_id)
            .unwrap()
            .query(redis::cmd("TTL").arg(&key))
            .await
            .map_err(|e| e.to_string())?;
        assert_eq!(initial_ttl, -1, "未设置 TTL 的 key 应当永不过期");

        set_key_inner(&pool, conn_id.clone(), key.clone(), "second".into(), 10)
            .await
            .map_err(|e| format!("更新 TTL 的 SET 失败: {e}"))?;

        let updated_ttl: i64 = pool
            .conn(&conn_id)
            .unwrap()
            .query(redis::cmd("TTL").arg(&key))
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
                con.query::<()>(redis::cmd("SET").arg(format!("{prefix}:{i:04}")).arg("v"))
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
        con.query::<()>(redis::cmd("DEL").arg(&expected))
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

    /// 真实 Redis 上的批量富化：五种类型 + 一个带 TTL 的 key，类型与 TTL 必须各自
    /// 挂在正确的 key 上（pipeline 的返回值按命令顺序排列，错位会张冠李戴）。
    #[tokio::test]
    #[ignore]
    async fn list_keys_reports_type_and_ttl_per_key() -> Result<(), String> {
        let conn_id = format!("key_test_enrich:{}", unique_id());
        let pool = test_pool(&conn_id).await?;
        let prefix = unique_key("enrich");
        let string_key = format!("{prefix}:string");
        let hash_key = format!("{prefix}:hash");
        let list_key = format!("{prefix}:list");
        let set_key = format!("{prefix}:set");
        let zset_key = format!("{prefix}:zset");
        let expiring_key = format!("{prefix}:expiring");

        {
            let mut con = pool.conn(&conn_id).unwrap();
            con.query::<()>(redis::cmd("SET").arg(&string_key).arg("v"))
                .await
                .map_err(|e| e.to_string())?;
            con.query::<i64>(redis::cmd("HSET").arg(&hash_key).arg("f").arg("v"))
                .await
                .map_err(|e| e.to_string())?;
            con.query::<i64>(redis::cmd("RPUSH").arg(&list_key).arg("a").arg("b"))
                .await
                .map_err(|e| e.to_string())?;
            con.query::<i64>(redis::cmd("SADD").arg(&set_key).arg("m"))
                .await
                .map_err(|e| e.to_string())?;
            con.query::<i64>(redis::cmd("ZADD").arg(&zset_key).arg(1.5).arg("m"))
                .await
                .map_err(|e| e.to_string())?;
            con.query::<String>(
                redis::cmd("SET")
                    .arg(&expiring_key)
                    .arg("v")
                    .arg("EX")
                    .arg(120),
            )
            .await
            .map_err(|e| e.to_string())?;
        }

        let page = list_keys_inner(&pool, &conn_id, "0", 100, Some(&prefix)).await?;
        let by_key: HashMap<&str, &KeyEntry> =
            page.keys.iter().map(|e| (e.key.as_str(), e)).collect();
        let type_of = |key: &String| -> String {
            by_key
                .get(key.as_str())
                .unwrap_or_else(|| panic!("{key} 应当出现在结果里"))
                .type_
                .clone()
        };
        assert_eq!(type_of(&string_key), "string");
        assert_eq!(type_of(&hash_key), "hash");
        assert_eq!(type_of(&list_key), "list");
        assert_eq!(type_of(&set_key), "set");
        assert_eq!(type_of(&zset_key), "zset");
        assert_eq!(type_of(&expiring_key), "string");

        // TTL：永不过期的报 -1（服务器回 -1，缺失的 key 回 -2 也该归一成 -1），
        // 设了过期的报正数且不超过设置值
        assert_eq!(by_key[string_key.as_str()].ttl, -1);
        let ttl = by_key[expiring_key.as_str()].ttl;
        assert!(ttl > 0 && ttl <= 120, "过期 key 应带正数 TTL: {ttl}");

        let mut con = pool.conn(&conn_id).unwrap();
        let written = vec![
            string_key,
            hash_key,
            list_key,
            set_key,
            zset_key,
            expiring_key,
        ];
        con.query::<()>(redis::cmd("DEL").arg(&written))
            .await
            .map_err(|e| e.to_string())?;
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
                con.query::<()>(redis::cmd("SET").arg(key).arg("v"))
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
            con.query::<()>(redis::cmd("DEL").arg(key))
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
    let n: i64 = con
        .query(&cmd)
        .await
        .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
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
    let val: Option<String> = con
        .query(redis::cmd("GET").arg(&key))
        .await
        .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
    Ok(val)
}
