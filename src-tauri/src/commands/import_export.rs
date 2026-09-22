//! # Key 导入 / 导出
//!
//! 把当前数据库中的 key 导出为一份 JSON 文档（含类型、TTL、内容），
//! 也可把这份文档重新导入（覆盖或跳过已存在的同名 key）。
//!
//! 导出格式（`version` 预留，当前为 1）：
//!
//! ```json
//! {
//!   "app": "maidi-cache",
//!   "version": 1,
//!   "exportedAt": "2026-09-14T08:00:00Z",
//!   "keys": [
//!     { "key": "s", "type": "string", "ttl": -1, "value": "..." },
//!     { "key": "h", "type": "hash",   "ttl": -1, "value": { "f": "v" } },
//!     { "key": "l", "type": "list",   "ttl": -1, "value": ["a", "b"] },
//!     { "key": "t", "type": "set",    "ttl": -1, "value": ["m"] },
//!     { "key": "z", "type": "zset",   "ttl": -1, "value": [{ "member": "m", "score": 1.0 }] },
//!     { "key": "x", "type": "stream", "ttl": -1, "value": [{ "id": "1-1", "fields": { "f": "v" } }] }
//!   ]
//! }
//! ```
//!
//! 逐 key 用独立命令写入（非 `MSET`），保证集群模式下不触发跨 slot 错误。

use serde::Deserialize;
use serde::Serialize;

use crate::connection_pool::Pool;
use crate::connection_pool::PooledConn;
use crate::error::command_error_text;
use crate::error::AppError;

/// 导入单条失败的原因。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportFailure {
    /// 出错的 key
    pub key: String,
    /// 错误信息
    pub error: String,
}

/// 导入结果统计。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    /// 成功导入的 key 数
    pub imported: u64,
    /// 因「已存在且不覆盖」而跳过的 key 数
    pub skipped: u64,
    /// 导入失败（类型不支持 / 数据非法 / Redis 报错）
    pub failed: Vec<ImportFailure>,
}

/// 导出文档中的单条 key。
#[derive(Debug, Deserialize)]
struct ExportEntry {
    key: String,
    #[serde(rename = "type")]
    type_: String,
    #[serde(default)]
    ttl: i64,
    value: serde_json::Value,
}

/// 导出结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    /// 导出的 JSON 文档文本
    pub content: String,
    /// 实际导出的 key 数（读取期间消失或被跳过的类型不计入）
    pub count: usize,
}

/// 导出指定 keys 的完整内容，返回 JSON 文档文本与实际导出条数。
///
/// `keys` 省略或为空时，导出当前 keyspace 的**全部** key：前端分页浏览时只持有已加载
/// 的那部分 key，导出不能只看那一部分，所以全量枚举放在后端做。
///
/// 读取期间被删除或类型变为 `none` 的 key 会被静默跳过。
#[tauri::command]
pub async fn export_keys(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    keys: Option<Vec<String>>,
) -> Result<ExportResult, String> {
    export_keys_inner(&pool, &conn_id, keys).await
}

/// [`export_keys`] 的核心实现，抽成独立函数以便集成测试直接调用。
pub async fn export_keys_inner(
    pool: &Pool,
    conn_id: &str,
    keys: Option<Vec<String>>,
) -> Result<ExportResult, String> {
    let conn_cfg = pool.get(conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;

    let keys = match keys {
        Some(list) if !list.is_empty() => list,
        _ => crate::commands::key::collect_all_key_names(&conn_cfg, &mut con).await?,
    };

    let mut entries = Vec::with_capacity(keys.len());
    for key in &keys {
        let ktype: String = con
            .query(redis::cmd("TYPE").arg(key))
            .await
            .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
        if ktype == "none" {
            continue;
        }
        let ttl: i64 = con.query(redis::cmd("TTL").arg(key)).await.unwrap_or(-1);

        let value = match ktype.as_str() {
            "string" => {
                // 读 TYPE 到读 GET 之间 key 可能已被删除或过期：nil 说明它已经不在了，
                // 按「读取期间消失的 key 静默跳过」处理（否则会报一条看不懂的类型错误）
                let v: Option<String> = con
                    .query(redis::cmd("GET").arg(key))
                    .await
                    .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
                match v {
                    Some(v) => serde_json::Value::String(v),
                    None => continue,
                }
            }
            "hash" => {
                let pairs: Vec<(String, String)> = con
                    .query(redis::cmd("HGETALL").arg(key))
                    .await
                    .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
                pairs
                    .into_iter()
                    .map(|(f, v)| (f, serde_json::Value::String(v)))
                    .collect::<serde_json::Map<String, serde_json::Value>>()
                    .into()
            }
            "list" => {
                let items: Vec<String> = con
                    .query(redis::cmd("LRANGE").arg(key).arg(0).arg(-1))
                    .await
                    .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
                serde_json::Value::Array(items.into_iter().map(serde_json::Value::String).collect())
            }
            "set" => {
                let mut members: Vec<String> = con
                    .query(redis::cmd("SMEMBERS").arg(key))
                    .await
                    .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
                members.sort();
                serde_json::Value::Array(
                    members.into_iter().map(serde_json::Value::String).collect(),
                )
            }
            "zset" => {
                let items: Vec<(String, f64)> = con
                    .query(
                        redis::cmd("ZRANGE")
                            .arg(key)
                            .arg(0)
                            .arg(-1)
                            .arg("WITHSCORES"),
                    )
                    .await
                    .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
                serde_json::Value::Array(
                    items
                        .into_iter()
                        .map(|(member, score)| {
                            serde_json::json!({ "member": member, "score": score })
                        })
                        .collect(),
                )
            }
            "stream" => {
                // 全量导出（无 COUNT 限制）。超大 Stream 可能超过命令超时，
                // 需要更长的读取时间请在连接设置里调大命令超时（DEVELOPMENT.md §2.4）。
                // XRANGE 的嵌套条目结构用共享辅助函数解析（见 key_content）
                let raw: Vec<redis::Value> = con
                    .query(redis::cmd("XRANGE").arg(key).arg("-").arg("+"))
                    .await
                    .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
                let items = crate::commands::key_content::parse_xrange_entries(raw)
                    .map_err(|e| command_error_text(&AppError::msg(e), Some(&conn_cfg)))?;
                serde_json::Value::Array(
                    items
                        .into_iter()
                        .map(|(id, flat)| {
                            let obj: serde_json::Map<String, serde_json::Value> = flat
                                .chunks_exact(2)
                                .map(|pair| {
                                    (pair[0].clone(), serde_json::Value::String(pair[1].clone()))
                                })
                                .collect();
                            serde_json::json!({ "id": id, "fields": serde_json::Value::Object(obj) })
                        })
                        .collect(),
                )
            }
            // 未知类型整体跳过，并在结果里可见（导出为 null 会让导入产生歧义）
            _ => continue,
        };

        entries.push(serde_json::json!({
            "key": key,
            "type": ktype,
            "ttl": if ttl < 0 { -1 } else { ttl },
            "value": value,
        }));
    }

    let doc = serde_json::json!({
        "app": "maidi-cache",
        "version": 1,
        "exportedAt": chrono_like_now(),
        "keys": entries,
    });
    let content = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    Ok(ExportResult {
        content,
        count: entries.len(),
    })
}

/// 生成 ISO-8601 风格的 UTC 时间戳（不引入 chrono 依赖，手写最小实现）。
pub(crate) fn chrono_like_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_unix_time(secs)
}

/// 把 Unix 秒格式化为 `YYYY-MM-DDTHH:MM:SSZ`（UTC）。
fn format_unix_time(secs: u64) -> String {
    let days = secs / 86400;
    let rem = secs % 86400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // civil-from-days 算法（Howard Hinnant），1970-01-01 起算
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mth = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mth <= 2 { y + 1 } else { y };
    format!("{y:04}-{mth:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// 导入 JSON 文档中的 keys。
///
/// - `overwrite` 为 `false` 时，已存在的同名 key 直接跳过；
///   为 `true` 时先 `DEL` 再写入。
/// - 每条 key 独立写入（string → `SET`，hash → `HSET`，list → `RPUSH`，
///   set → `SADD`，zset → `ZADD`），TTL 为正数时追加 `EXPIRE`。
/// - 单条失败不影响其它 key，失败原因收集在 [`ImportResult::failed`] 中返回。
#[tauri::command]
pub async fn import_keys(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    content: String,
    overwrite: bool,
) -> Result<ImportResult, String> {
    import_keys_inner(&pool, &conn_id, &content, overwrite).await
}

/// [`import_keys`] 的核心实现，抽成独立函数以便集成测试直接调用。
pub async fn import_keys_inner(
    pool: &Pool,
    conn_id: &str,
    content: &str,
    overwrite: bool,
) -> Result<ImportResult, String> {
    pool.ensure_writable(conn_id).map_err(|e| e.to_string())?;
    let conn_cfg = pool.get(conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;

    let doc: serde_json::Value =
        serde_json::from_str(content).map_err(|e| format!("导入文件不是有效的 JSON: {e}"))?;
    let items = extract_entries(&doc).ok_or("导入文件格式不正确：缺少 keys 数组")?;

    let mut result = ImportResult::default();
    for item in items {
        let outcome = import_one(&mut con, &conn_cfg, &item, overwrite).await;
        match outcome {
            Ok(ImportOutcome::Imported) => result.imported += 1,
            Ok(ImportOutcome::Skipped) => result.skipped += 1,
            Err(msg) => result.failed.push(ImportFailure {
                key: item.key.clone(),
                error: msg,
            }),
        }
    }
    Ok(result)
}

/// 单条导入的结果。
enum ImportOutcome {
    Imported,
    Skipped,
}

/// 从导入文档中取出 `keys` 数组（兼容裸数组格式）。
fn extract_entries(doc: &serde_json::Value) -> Option<Vec<ExportEntry>> {
    if let Ok(entries) = serde_json::from_value::<Vec<ExportEntry>>(doc.clone()) {
        return Some(entries);
    }
    doc.get("keys")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

/// 导入单条 key，返回其结果。
async fn import_one(
    con: &mut PooledConn,
    conn_cfg: &crate::models::Connection,
    item: &ExportEntry,
    overwrite: bool,
) -> Result<ImportOutcome, String> {
    let exists: bool = con
        .query(redis::cmd("EXISTS").arg(&item.key))
        .await
        .map_err(|e| command_error_text(&e, Some(conn_cfg)))?;
    if exists {
        if !overwrite {
            return Ok(ImportOutcome::Skipped);
        }
        con.query::<()>(redis::cmd("DEL").arg(&item.key))
            .await
            .map_err(|e| command_error_text(&e, Some(conn_cfg)))?;
    }

    write_value(con, conn_cfg, item).await?;

    if item.ttl > 0 {
        con.query::<()>(redis::cmd("EXPIRE").arg(&item.key).arg(item.ttl))
            .await
            .map_err(|e| command_error_text(&e, Some(conn_cfg)))?;
    }
    Ok(ImportOutcome::Imported)
}

/// 按类型把 `value` 写入 Redis（调用前已按需 `DEL` 清理旧值）。
async fn write_value(
    con: &mut PooledConn,
    conn_cfg: &crate::models::Connection,
    item: &ExportEntry,
) -> Result<(), String> {
    let key = &item.key;
    match item.type_.as_str() {
        "string" => {
            let value = item
                .value
                .as_str()
                .ok_or("string 类型的 value 必须是字符串")?;
            con.query::<()>(redis::cmd("SET").arg(key).arg(value)).await
        }
        "hash" => {
            let obj = item
                .value
                .as_object()
                .ok_or("hash 类型的 value 必须是对象")?;
            if obj.is_empty() {
                return Err("空 Hash 无法导入（Redis 不支持空 key）".into());
            }
            let mut cmd = redis::cmd("HSET");
            cmd.arg(key);
            for (f, v) in obj {
                let s = v.as_str().ok_or("hash 字段值必须是字符串")?;
                cmd.arg(f).arg(s);
            }
            con.query::<()>(&cmd).await
        }
        "list" => {
            let arr = item
                .value
                .as_array()
                .ok_or("list 类型的 value 必须是数组")?;
            if arr.is_empty() {
                return Err("空 List 无法导入（Redis 不支持空 key）".into());
            }
            let mut cmd = redis::cmd("RPUSH");
            cmd.arg(key);
            for v in arr {
                let s = v.as_str().ok_or("list 元素必须是字符串")?;
                cmd.arg(s);
            }
            con.query::<()>(&cmd).await
        }
        "set" => {
            let arr = item.value.as_array().ok_or("set 类型的 value 必须是数组")?;
            if arr.is_empty() {
                return Err("空 Set 无法导入（Redis 不支持空 key）".into());
            }
            let mut cmd = redis::cmd("SADD");
            cmd.arg(key);
            for v in arr {
                let s = v.as_str().ok_or("set 成员必须是字符串")?;
                cmd.arg(s);
            }
            con.query::<()>(&cmd).await
        }
        "zset" => {
            let arr = item
                .value
                .as_array()
                .ok_or("zset 类型的 value 必须是数组")?;
            if arr.is_empty() {
                return Err("空 ZSet 无法导入（Redis 不支持空 key）".into());
            }
            let mut cmd = redis::cmd("ZADD");
            cmd.arg(key);
            for v in arr {
                let member = v
                    .get("member")
                    .and_then(|m| m.as_str())
                    .ok_or("zset 元素缺少 member 字段")?;
                let score = v
                    .get("score")
                    .and_then(|s| s.as_f64())
                    .ok_or("zset 元素缺少 score 字段")?;
                cmd.arg(score).arg(member);
            }
            con.query::<()>(&cmd).await
        }
        "stream" => {
            let arr = item
                .value
                .as_array()
                .ok_or("stream 类型的 value 必须是数组")?;
            if arr.is_empty() {
                return Err("空 Stream 无法导入（Redis 不支持空 key）".into());
            }
            // 逐条 XADD 并保留原始 entry id（覆盖模式已先 DEL，id 单调递增不会被拒）。
            // 条目 id 是导入后消费组的起点，静默重排会让下游消费者重复/漏读。
            for entry in arr {
                let id = entry
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or("stream 条目缺少 id 字段")?;
                let fields = entry
                    .get("fields")
                    .and_then(|v| v.as_object())
                    .ok_or("stream 条目的 fields 必须是对象")?;
                if fields.is_empty() {
                    return Err("stream 条目至少需要一个字段".into());
                }
                let mut cmd = redis::cmd("XADD");
                cmd.arg(key).arg(id);
                for (f, v) in fields {
                    let s = v.as_str().ok_or("stream 字段值必须是字符串")?;
                    cmd.arg(f).arg(s);
                }
                con.query::<()>(&cmd)
                    .await
                    .map_err(|e| command_error_text(&e, Some(conn_cfg)))?;
            }
            return Ok(());
        }
        other => return Err(format!("不支持的类型: {other}")),
    }
    .map_err(|e| command_error_text(&e, Some(conn_cfg)))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{export_keys_inner, format_unix_time, import_keys_inner};
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
            tls: false,
            tls_insecure: false,
            connect_timeout_secs: None,
            command_timeout_secs: None,
        };
        pool.connect(&conn)
            .await
            .map_err(|e| format!("连接 Redis 失败，请确认服务已启动: {e}"))?;
        Ok(pool)
    }

    #[test]
    fn format_unix_time_epoch() {
        assert_eq!(format_unix_time(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn format_unix_time_known_date() {
        // 2026-09-14 08:00:00 UTC
        let secs = 1_789_372_800u64;
        assert_eq!(format_unix_time(secs), "2026-09-14T08:00:00Z");
    }

    // 以下测试依赖本地 Redis，默认标记为 #[ignore]；
    // 启动 Redis 后可通过 `cargo test -- --ignored` 运行。

    #[tokio::test]
    #[ignore]
    async fn export_import_roundtrip() -> Result<(), String> {
        let conn_id = format!("impexp:{}", unique_id());
        let pool = test_pool(&conn_id).await?;
        let mut con = pool.conn(&conn_id).unwrap();

        // 造数据：四种类型各一个
        con.query::<()>(redis::cmd("SET").arg("maidi:ie:s").arg("v1"))
            .await
            .unwrap();
        con.query::<()>(redis::cmd("HSET").arg("maidi:ie:h").arg("f").arg("v"))
            .await
            .unwrap();
        con.query::<()>(redis::cmd("RPUSH").arg("maidi:ie:l").arg("a").arg("b"))
            .await
            .unwrap();
        con.query::<()>(redis::cmd("SADD").arg("maidi:ie:t").arg("m"))
            .await
            .unwrap();
        con.query::<()>(redis::cmd("ZADD").arg("maidi:ie:z").arg(1.5).arg("zm"))
            .await
            .unwrap();

        let keys = vec![
            "maidi:ie:s".to_string(),
            "maidi:ie:h".to_string(),
            "maidi:ie:l".to_string(),
            "maidi:ie:t".to_string(),
            "maidi:ie:z".to_string(),
        ];
        let doc = export_keys_inner(&pool, &conn_id, Some(keys.clone())).await?;
        assert_eq!(doc.count, 5, "5 个 key 应全部导出: {doc:?}");

        // 删掉后重新导入（覆盖模式）
        con.query::<()>(redis::cmd("DEL").arg(&keys)).await.unwrap();
        let result = import_keys_inner(&pool, &conn_id, &doc.content, true).await?;
        assert_eq!(result.imported, 5, "5 个 key 应全部导入: {result:?}");
        assert!(result.failed.is_empty(), "不应有失败: {result:?}");

        // 再导一次（不覆盖）应全部跳过
        let result = import_keys_inner(&pool, &conn_id, &doc.content, false).await?;
        assert_eq!(result.skipped, 5);
        assert_eq!(result.imported, 0);

        // 抽查值
        let v: String = con
            .query(redis::cmd("GET").arg("maidi:ie:s"))
            .await
            .unwrap();
        assert_eq!(v, "v1");
        let len: i64 = con
            .query(redis::cmd("LLEN").arg("maidi:ie:l"))
            .await
            .unwrap();
        assert_eq!(len, 2);
        let score: f64 = con
            .query(redis::cmd("ZSCORE").arg("maidi:ie:z").arg("zm"))
            .await
            .unwrap();
        assert_eq!(score, 1.5);

        con.query::<()>(redis::cmd("DEL").arg(&keys)).await.unwrap();
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn stream_export_import_roundtrip() -> Result<(), String> {
        let conn_id = format!("impexp_stream:{}", unique_id());
        let pool = test_pool(&conn_id).await?;
        let key = format!("maidi:ie:x:{}", unique_id());
        let mut con = pool.conn(&conn_id).unwrap();

        // 造数据：两个条目，各自带不同数量的字段
        con.query::<()>(
            redis::cmd("XADD")
                .arg(&key)
                .arg("1000-1")
                .arg("f1")
                .arg("v1")
                .arg("f2")
                .arg("v2"),
        )
        .await
        .map_err(|e| e.to_string())?;
        con.query::<()>(redis::cmd("XADD").arg(&key).arg("1000-2").arg("g").arg("w"))
            .await
            .map_err(|e| e.to_string())?;

        let doc = export_keys_inner(&pool, &conn_id, Some(vec![key.clone()])).await?;
        assert_eq!(doc.count, 1, "Stream 应被导出: {doc:?}");
        assert!(
            doc.content.contains("stream") && doc.content.contains("1000-1"),
            "导出文档应含 stream 类型与原始 entry id: {doc:?}"
        );

        con.query::<()>(redis::cmd("DEL").arg(&key)).await.unwrap();
        let result = import_keys_inner(&pool, &conn_id, &doc.content, true).await?;
        assert_eq!(result.imported, 1, "Stream 应能导入: {result:?}");
        assert!(result.failed.is_empty(), "不应有失败: {result:?}");

        // entry id 必须原样保留（导入后 id 递增才不会被拒，消费组也不受影响）
        let raw: Vec<redis::Value> = con
            .query(redis::cmd("XRANGE").arg(&key).arg("-").arg("+"))
            .await
            .map_err(|e| e.to_string())?;
        let entries = crate::commands::key_content::parse_xrange_entries(raw)?;
        let ids: Vec<String> = entries.into_iter().map(|(id, _)| id).collect();
        assert_eq!(
            ids,
            vec!["1000-1", "1000-2"],
            "entry id 应原样还原: {ids:?}"
        );

        con.query::<()>(redis::cmd("DEL").arg(&key)).await.unwrap();
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn import_invalid_json_returns_error() -> Result<(), String> {
        let conn_id = format!("impexp_bad:{}", unique_id());
        let pool = test_pool(&conn_id).await?;
        let result = import_keys_inner(&pool, &conn_id, "not json", true).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("JSON"));
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn export_without_keys_scans_whole_keyspace() -> Result<(), String> {
        let conn_id = format!("impexp_all:{}", unique_id());
        let pool = test_pool(&conn_id).await?;
        let key = format!("maidi:ie:all:{}", unique_id());
        {
            let mut con = pool.conn(&conn_id).unwrap();
            con.query::<()>(redis::cmd("SET").arg(&key).arg("v"))
                .await
                .map_err(|e| e.to_string())?;
        }

        // 不传 keys：后端自行全量枚举，不受前端「只加载了一页」的限制
        let exported = export_keys_inner(&pool, &conn_id, None).await?;
        assert!(exported.count >= 1, "全量导出至少应包含刚写入的 key");
        assert!(
            exported.content.contains(&key),
            "全量导出应包含刚写入的 key: {key}"
        );

        let mut con = pool.conn(&conn_id).unwrap();
        con.query::<()>(redis::cmd("DEL").arg(&key))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}
