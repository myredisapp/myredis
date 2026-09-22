//! # Key 内容命令
//!
//! Hash / List / Set / ZSet / Stream 五种集合类型的**内容读取**与**字段级编辑**。
//!
//! 读取命令（`get_hash` / `get_list` / `get_set` / `get_zset` / `get_stream`）按类型返回
//! 结构化数据，供前端详情区渲染；编辑命令一次只改一个字段 / 元素 / 条目，成功后由前端
//! 重新拉取内容。所有写命令都先经 [`Pool::ensure_writable`] 拦截只读连接。

use serde::Deserialize;
use serde::Serialize;

use crate::connection_pool::Pool;
use crate::error::command_error_text;

/// Hash 的一个字段。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HashField {
    /// 字段名
    pub field: String,
    /// 字段值
    pub value: String,
}

/// List 的一个元素（带下标，`LSET` 按它定位）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListItem {
    /// 元素下标（从 0 开始）
    pub index: i64,
    /// 元素值
    pub value: String,
}

/// ZSet 的一个成员。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZSetItem {
    /// 成员
    pub member: String,
    /// 分值
    pub score: f64,
}

/// 读取 Hash 的全部字段（`HGETALL`）。
///
/// key 不存在时返回空列表；类型不匹配时返回错误。
#[tauri::command]
pub async fn get_hash(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    key: String,
) -> Result<Vec<HashField>, String> {
    let conn_cfg = pool.get(&conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.conn(&conn_id).map_err(|e| e.to_string())?;
    let raw: Vec<(String, String)> = con
        .query(redis::cmd("HGETALL").arg(&key))
        .await
        .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
    Ok(raw
        .into_iter()
        .map(|(field, value)| HashField { field, value })
        .collect())
}

/// 读取 List 的全部元素（`LRANGE 0 -1`），带下标。
#[tauri::command]
pub async fn get_list(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    key: String,
) -> Result<Vec<ListItem>, String> {
    let conn_cfg = pool.get(&conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.conn(&conn_id).map_err(|e| e.to_string())?;
    let raw: Vec<String> = con
        .query(redis::cmd("LRANGE").arg(&key).arg(0).arg(-1))
        .await
        .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
    Ok(raw
        .into_iter()
        .enumerate()
        .map(|(i, value)| ListItem {
            index: i as i64,
            value,
        })
        .collect())
}

/// 读取 Set 的全部成员（`SMEMBERS`）。
#[tauri::command]
pub async fn get_set(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    key: String,
) -> Result<Vec<String>, String> {
    let conn_cfg = pool.get(&conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.conn(&conn_id).map_err(|e| e.to_string())?;
    let mut members: Vec<String> = con
        .query(redis::cmd("SMEMBERS").arg(&key))
        .await
        .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
    members.sort();
    Ok(members)
}

/// 读取 ZSet 的全部成员与分值（`ZRANGE 0 -1 WITHSCORES`）。
#[tauri::command]
pub async fn get_zset(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    key: String,
) -> Result<Vec<ZSetItem>, String> {
    let conn_cfg = pool.get(&conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.conn(&conn_id).map_err(|e| e.to_string())?;
    let raw: Vec<(String, f64)> = con
        .query(
            redis::cmd("ZRANGE")
                .arg(&key)
                .arg(0)
                .arg(-1)
                .arg("WITHSCORES"),
        )
        .await
        .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
    Ok(raw
        .into_iter()
        .map(|(member, score)| ZSetItem { member, score })
        .collect())
}

/// Stream 条目的一个字段。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamField {
    /// 字段名
    pub field: String,
    /// 字段值
    pub value: String,
}

/// Stream 的一个条目（entry id 天然是主键，`XDEL` 按它定位，无下标漂移问题）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamEntry {
    /// 条目 id（`毫秒序号-序列号`，如 `1726884000000-0`）
    pub id: String,
    /// 字段列表（一个条目可含多个字段，保持服务器返回顺序）
    pub fields: Vec<StreamField>,
}

/// 读取 Stream 的一段条目（`XRANGE key start + COUNT count`）。
///
/// - `start` 传 `"-"` 表示从头开始；翻页时传上一页最后一条 id 的**下一个** id
///   （前端负责递增序列号，`XRANGE` 的起点是闭区间）。
/// - key 不存在时返回空列表；类型不匹配时返回错误。
/// - 大 Stream 请用 `count` 分页读取；一次性全量（导出用）见
///   [`crate::commands::import_export`]，注意命令超时配置（DEVELOPMENT.md §2.4）。
#[tauri::command]
pub async fn get_stream(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    key: String,
    start: String,
    count: i64,
) -> Result<Vec<StreamEntry>, String> {
    get_stream_inner(&pool, &conn_id, &key, &start, count).await
}

/// [`get_stream`] 的核心实现，抽成独立函数以便集成测试直接调用。
pub async fn get_stream_inner(
    pool: &Pool,
    conn_id: &str,
    key: &str,
    start: &str,
    count: i64,
) -> Result<Vec<StreamEntry>, String> {
    let conn_cfg = pool.get(conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;
    let raw: Vec<redis::Value> = con
        .query(
            redis::cmd("XRANGE")
                .arg(key)
                .arg(start)
                .arg("+")
                .arg("COUNT")
                .arg(count),
        )
        .await
        .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
    Ok(parse_xrange_entries(raw)?
        .into_iter()
        .map(|(id, flat)| StreamEntry {
            id,
            fields: pair_fields(flat),
        })
        .collect())
}

/// 向 Stream 追加一个条目（`XADD`），返回服务器分配的条目 id。
///
/// `entry_id` 为 `None` 时由服务器自动生成（`*`）；传显式 id 可精确复原
/// （导入导出用， id 必须大于当前最后一条，否则服务器报错）。
#[tauri::command]
pub async fn stream_add_entry(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    key: String,
    entry_id: Option<String>,
    fields: Vec<StreamFieldInput>,
) -> Result<String, String> {
    stream_add_entry_inner(&pool, &conn_id, &key, entry_id.as_deref(), &fields).await
}

/// 前端传入的 Stream 字段。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamFieldInput {
    pub field: String,
    pub value: String,
}

/// 把 XRANGE 的原始应答解析成「条目 id + 扁平字段串」列表。
///
/// 不能直接用 `Vec<(String, Vec<String>)>`：redis-rs 对 Vec<元组> 期望的是
/// HGETALL 那样的**扁平**键值对，而 XRANGE 返回的是每条目一层嵌套数组，
/// 直接转换会报「Bulk response of wrong dimension」。key 不存在时 XRANGE 返回
/// 空数组；读取期间 key 消失也可能回 nil，都视作空。
pub(crate) fn parse_xrange_entries(
    items: Vec<redis::Value>,
) -> Result<Vec<(String, Vec<String>)>, String> {
    items
        .into_iter()
        .map(|entry| {
            let mut pair = match entry {
                redis::Value::Bulk(parts) if parts.len() == 2 => parts.into_iter(),
                other => {
                    return Err(format!("XRANGE 条目格式异常: {other:?}"));
                }
            };
            // unwrap 安全：上面已确认恰有 2 个元素
            let id_raw = pair.next().unwrap();
            let fields_raw = pair.next().unwrap();
            let id: String = redis::from_redis_value(&id_raw)
                .map_err(|e| format!("XRANGE 条目 id 解析失败: {e}"))?;
            let fields: Vec<String> = redis::from_redis_value(&fields_raw)
                .map_err(|e| format!("XRANGE 条目字段解析失败: {e}"))?;
            Ok((id, fields))
        })
        .collect()
}

/// 扁平字段串两两配成字段列表；奇数长度说明服务器应答异常，丢掉落单的。
fn pair_fields(flat: Vec<String>) -> Vec<StreamField> {
    flat.as_chunks::<2>()
        .0
        .iter()
        .map(|pair| StreamField {
            field: pair[0].clone(),
            value: pair[1].clone(),
        })
        .collect()
}

/// [`stream_add_entry`] 的核心实现，抽成独立函数以便集成测试直接调用。
pub async fn stream_add_entry_inner(
    pool: &Pool,
    conn_id: &str,
    key: &str,
    entry_id: Option<&str>,
    fields: &[StreamFieldInput],
) -> Result<String, String> {
    if fields.is_empty() {
        return Err("Stream 条目至少需要一个字段".into());
    }
    pool.ensure_writable(conn_id).map_err(|e| e.to_string())?;
    let conn_cfg = pool.get(conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;
    let mut cmd = redis::cmd("XADD");
    cmd.arg(key).arg(entry_id.unwrap_or("*"));
    for f in fields {
        cmd.arg(&f.field).arg(&f.value);
    }
    let id: String = con
        .query(&cmd)
        .await
        .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
    Ok(id)
}

/// 删除 Stream 中的一个或多个条目（`XDEL`），返回实际删除的条目数。
///
/// 按 entry id 定位，id 不存在时返回 0（不算错误）。
#[tauri::command]
pub async fn stream_del_entry(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    key: String,
    entry_ids: Vec<String>,
) -> Result<i64, String> {
    stream_del_entry_inner(&pool, &conn_id, &key, &entry_ids).await
}

/// [`stream_del_entry`] 的核心实现，抽成独立函数以便集成测试直接调用。
pub async fn stream_del_entry_inner(
    pool: &Pool,
    conn_id: &str,
    key: &str,
    entry_ids: &[String],
) -> Result<i64, String> {
    if entry_ids.is_empty() {
        return Err("请选择要删除的条目".into());
    }
    pool.ensure_writable(conn_id).map_err(|e| e.to_string())?;
    let conn_cfg = pool.get(conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;
    let mut cmd = redis::cmd("XDEL");
    cmd.arg(key);
    for id in entry_ids {
        cmd.arg(id);
    }
    let n: i64 = con
        .query(&cmd)
        .await
        .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
    Ok(n)
}

/// 修改 key 的 TTL（`-1` 表示移除过期时间，正整数为过期秒数）。
///
/// 对任意类型可用：`-1` 走 `PERSIST`，否则走 `EXPIRE`。
/// String 类型的「保存」仍走 [`crate::commands::key::set_key`] 的 `SET EX`，
/// 本命令主要服务集合类型的 TTL 修改。
#[tauri::command]
pub async fn set_key_ttl(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    key: String,
    ttl: i64,
) -> Result<(), String> {
    set_key_ttl_inner(&pool, &conn_id, &key, ttl).await
}

/// [`set_key_ttl`] 的核心实现，抽成独立函数以便集成测试直接调用。
pub async fn set_key_ttl_inner(
    pool: &Pool,
    conn_id: &str,
    key: &str,
    ttl: i64,
) -> Result<(), String> {
    if ttl != -1 && ttl <= 0 {
        return Err("TTL 必须为 -1 或正整数".into());
    }
    pool.ensure_writable(conn_id).map_err(|e| e.to_string())?;
    let conn_cfg = pool.get(conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;

    let mut cmd = redis::cmd(if ttl == -1 { "PERSIST" } else { "EXPIRE" });
    cmd.arg(key);
    if ttl > 0 {
        cmd.arg(ttl);
    }
    con.query::<()>(&cmd)
        .await
        .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
    Ok(())
}

/// 新增或修改 Hash 的一个字段（`HSET`）。
#[tauri::command]
pub async fn hash_set_field(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    key: String,
    field: String,
    value: String,
) -> Result<(), String> {
    hash_set_field_inner(&pool, &conn_id, &key, &field, &value).await
}

/// [`hash_set_field`] 的核心实现，抽成独立函数以便集成测试直接调用。
pub async fn hash_set_field_inner(
    pool: &Pool,
    conn_id: &str,
    key: &str,
    field: &str,
    value: &str,
) -> Result<(), String> {
    pool.ensure_writable(conn_id).map_err(|e| e.to_string())?;
    let conn_cfg = pool.get(conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;
    con.query::<()>(redis::cmd("HSET").arg(key).arg(field).arg(value))
        .await
        .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
    Ok(())
}

/// 删除 Hash 的一个或多个字段（`HDEL`），返回实际删除的字段数。
#[tauri::command]
pub async fn hash_del_fields(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    key: String,
    fields: Vec<String>,
) -> Result<i64, String> {
    hash_del_fields_inner(&pool, &conn_id, &key, &fields).await
}

/// [`hash_del_fields`] 的核心实现，抽成独立函数以便集成测试直接调用。
pub async fn hash_del_fields_inner(
    pool: &Pool,
    conn_id: &str,
    key: &str,
    fields: &[String],
) -> Result<i64, String> {
    pool.ensure_writable(conn_id).map_err(|e| e.to_string())?;
    let conn_cfg = pool.get(conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;
    let mut cmd = redis::cmd("HDEL");
    cmd.arg(key);
    for f in fields {
        cmd.arg(f);
    }
    let n: i64 = con
        .query(&cmd)
        .await
        .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
    Ok(n)
}

/// 按下标修改 List 元素（`LSET`）。
#[tauri::command]
pub async fn list_set_element(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    key: String,
    index: i64,
    value: String,
) -> Result<(), String> {
    list_set_element_inner(&pool, &conn_id, &key, index, &value).await
}

/// [`list_set_element`] 的核心实现，抽成独立函数以便集成测试直接调用。
pub async fn list_set_element_inner(
    pool: &Pool,
    conn_id: &str,
    key: &str,
    index: i64,
    value: &str,
) -> Result<(), String> {
    pool.ensure_writable(conn_id).map_err(|e| e.to_string())?;
    let conn_cfg = pool.get(conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;
    con.query::<()>(redis::cmd("LSET").arg(key).arg(index).arg(value))
        .await
        .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
    Ok(())
}

/// 删除 List 中值等于 `value` 的第一个元素（`LREM 1`）。
///
/// 返回实际删除的元素数（0 或 1）。List 允许重复值，
/// 按值删除比按下标删除更不容易误删（下标会在并发修改后漂移）。
#[tauri::command]
pub async fn list_del_element(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    key: String,
    value: String,
) -> Result<i64, String> {
    list_del_element_inner(&pool, &conn_id, &key, &value).await
}

/// [`list_del_element`] 的核心实现，抽成独立函数以便集成测试直接调用。
pub async fn list_del_element_inner(
    pool: &Pool,
    conn_id: &str,
    key: &str,
    value: &str,
) -> Result<i64, String> {
    pool.ensure_writable(conn_id).map_err(|e| e.to_string())?;
    let conn_cfg = pool.get(conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;
    let n: i64 = con
        .query(redis::cmd("LREM").arg(key).arg(1).arg(value))
        .await
        .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
    Ok(n)
}

/// 向 List 左端或右端插入一个元素（`LPUSH` / `RPUSH`）。
#[tauri::command]
pub async fn list_push_element(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    key: String,
    value: String,
    left: bool,
) -> Result<i64, String> {
    list_push_element_inner(&pool, &conn_id, &key, &value, left).await
}

/// [`list_push_element`] 的核心实现，抽成独立函数以便集成测试直接调用。
pub async fn list_push_element_inner(
    pool: &Pool,
    conn_id: &str,
    key: &str,
    value: &str,
    left: bool,
) -> Result<i64, String> {
    pool.ensure_writable(conn_id).map_err(|e| e.to_string())?;
    let conn_cfg = pool.get(conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;
    let name = if left { "LPUSH" } else { "RPUSH" };
    let n: i64 = con
        .query(redis::cmd(name).arg(key).arg(value))
        .await
        .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
    Ok(n)
}

/// 向 Set 添加一个成员（`SADD`），返回实际新增的成员数（0 或 1）。
#[tauri::command]
pub async fn set_add_member(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    key: String,
    member: String,
) -> Result<i64, String> {
    set_add_member_inner(&pool, &conn_id, &key, &member).await
}

/// [`set_add_member`] 的核心实现，抽成独立函数以便集成测试直接调用。
pub async fn set_add_member_inner(
    pool: &Pool,
    conn_id: &str,
    key: &str,
    member: &str,
) -> Result<i64, String> {
    pool.ensure_writable(conn_id).map_err(|e| e.to_string())?;
    let conn_cfg = pool.get(conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;
    let n: i64 = con
        .query(redis::cmd("SADD").arg(key).arg(member))
        .await
        .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
    Ok(n)
}

/// 从 Set 删除一个成员（`SREM`），返回实际删除的成员数。
#[tauri::command]
pub async fn set_del_member(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    key: String,
    member: String,
) -> Result<i64, String> {
    set_del_member_inner(&pool, &conn_id, &key, &member).await
}

/// [`set_del_member`] 的核心实现，抽成独立函数以便集成测试直接调用。
pub async fn set_del_member_inner(
    pool: &Pool,
    conn_id: &str,
    key: &str,
    member: &str,
) -> Result<i64, String> {
    pool.ensure_writable(conn_id).map_err(|e| e.to_string())?;
    let conn_cfg = pool.get(conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;
    let n: i64 = con
        .query(redis::cmd("SREM").arg(key).arg(member))
        .await
        .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
    Ok(n)
}

/// 向 ZSet 新增成员或修改已有成员的分值（`ZADD`）。
#[tauri::command]
pub async fn zset_add_member(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    key: String,
    member: String,
    score: f64,
) -> Result<i64, String> {
    zset_add_member_inner(&pool, &conn_id, &key, &member, score).await
}

/// [`zset_add_member`] 的核心实现，抽成独立函数以便集成测试直接调用。
pub async fn zset_add_member_inner(
    pool: &Pool,
    conn_id: &str,
    key: &str,
    member: &str,
    score: f64,
) -> Result<i64, String> {
    pool.ensure_writable(conn_id).map_err(|e| e.to_string())?;
    let conn_cfg = pool.get(conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;
    let n: i64 = con
        .query(redis::cmd("ZADD").arg(key).arg(score).arg(member))
        .await
        .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
    Ok(n)
}

/// 从 ZSet 删除一个成员（`ZREM`），返回实际删除的成员数。
#[tauri::command]
pub async fn zset_del_member(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    key: String,
    member: String,
) -> Result<i64, String> {
    zset_del_member_inner(&pool, &conn_id, &key, &member).await
}

/// [`zset_del_member`] 的核心实现，抽成独立函数以便集成测试直接调用。
pub async fn zset_del_member_inner(
    pool: &Pool,
    conn_id: &str,
    key: &str,
    member: &str,
) -> Result<i64, String> {
    pool.ensure_writable(conn_id).map_err(|e| e.to_string())?;
    let conn_cfg = pool.get(conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.conn(conn_id).map_err(|e| e.to_string())?;
    let n: i64 = con
        .query(redis::cmd("ZREM").arg(key).arg(member))
        .await
        .map_err(|e| command_error_text(&e, Some(&conn_cfg)))?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::{
        get_stream_inner, hash_del_fields_inner, hash_set_field_inner, list_push_element_inner,
        list_set_element_inner, set_add_member_inner, set_del_member_inner, stream_add_entry_inner,
        stream_del_entry_inner, zset_add_member_inner, zset_del_member_inner, StreamFieldInput,
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

    // 以下测试依赖本地 Redis，默认标记为 #[ignore]；
    // 启动 Redis 后可通过 `cargo test -- --ignored` 运行。

    #[tokio::test]
    #[ignore]
    async fn hash_field_roundtrip() -> Result<(), String> {
        let conn_id = format!("content_hash:{}", unique_id());
        let pool = test_pool(&conn_id).await?;
        let key = unique_key("hash");

        hash_set_field_inner(&pool, &conn_id, &key, "f1", "v1").await?;
        let fields: Vec<(String, String)> = pool
            .conn(&conn_id)
            .unwrap()
            .query(redis::cmd("HGETALL").arg(&key))
            .await
            .map_err(|e| e.to_string())?;
        assert!(
            fields.iter().any(|(f, v)| f == "f1" && v == "v1"),
            "HGETALL 应包含刚写入的字段: {fields:?}"
        );

        let n = hash_del_fields_inner(&pool, &conn_id, &key, &["f1".to_string()]).await?;
        assert_eq!(n, 1);
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn list_element_roundtrip() -> Result<(), String> {
        let conn_id = format!("content_list:{}", unique_id());
        let pool = test_pool(&conn_id).await?;
        let key = unique_key("list");

        list_push_element_inner(&pool, &conn_id, &key, "a", false).await?;
        list_push_element_inner(&pool, &conn_id, &key, "b", false).await?;
        list_push_element_inner(&pool, &conn_id, &key, "first", true).await?;
        let items: Vec<String> = pool
            .conn(&conn_id)
            .unwrap()
            .query(redis::cmd("LRANGE").arg(&key).arg(0).arg(-1))
            .await
            .map_err(|e| e.to_string())?;
        assert_eq!(items, vec!["first", "a", "b"]);

        list_set_element_inner(&pool, &conn_id, &key, 1, "A").await?;
        let items: Vec<String> = pool
            .conn(&conn_id)
            .unwrap()
            .query(redis::cmd("LRANGE").arg(&key).arg(0).arg(-1))
            .await
            .map_err(|e| e.to_string())?;
        assert_eq!(items, vec!["first", "A", "b"]);
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn set_member_roundtrip() -> Result<(), String> {
        let conn_id = format!("content_set:{}", unique_id());
        let pool = test_pool(&conn_id).await?;
        let key = unique_key("set");

        assert_eq!(set_add_member_inner(&pool, &conn_id, &key, "m1").await?, 1);
        // 重复添加返回 0
        assert_eq!(set_add_member_inner(&pool, &conn_id, &key, "m1").await?, 0);
        assert_eq!(set_del_member_inner(&pool, &conn_id, &key, "m1").await?, 1);
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn zset_member_roundtrip() -> Result<(), String> {
        let conn_id = format!("content_zset:{}", unique_id());
        let pool = test_pool(&conn_id).await?;
        let key = unique_key("zset");

        zset_add_member_inner(&pool, &conn_id, &key, "m1", 1.5).await?;
        let score: f64 = pool
            .conn(&conn_id)
            .unwrap()
            .query(redis::cmd("ZSCORE").arg(&key).arg("m1"))
            .await
            .map_err(|e| e.to_string())?;
        assert_eq!(score, 1.5);

        assert_eq!(zset_del_member_inner(&pool, &conn_id, &key, "m1").await?, 1);
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn stream_entry_roundtrip() -> Result<(), String> {
        let conn_id = format!("content_stream:{}", unique_id());
        let pool = test_pool(&conn_id).await?;
        let key = unique_key("stream");

        // 追加两个条目（自动 id），分页读取应全部拿到且顺序一致
        let id1 = stream_add_entry_inner(
            &pool,
            &conn_id,
            &key,
            None,
            &[
                StreamFieldInput {
                    field: "f1".into(),
                    value: "v1".into(),
                },
                StreamFieldInput {
                    field: "f2".into(),
                    value: "v2".into(),
                },
            ],
        )
        .await?;
        let id2 = stream_add_entry_inner(
            &pool,
            &conn_id,
            &key,
            None,
            &[StreamFieldInput {
                field: "g".into(),
                value: "w".into(),
            }],
        )
        .await?;

        let page1 = get_stream_inner(&pool, &conn_id, &key, "-", 1).await?;
        assert_eq!(page1.len(), 1, "COUNT 1 应只返回一条: {page1:?}");
        assert_eq!(page1[0].id, id1);
        assert_eq!(page1[0].fields.len(), 2);

        // 翻页：从 id1 的下一个 id 开始（XRANGE 起点闭区间），应拿到第二条
        let next = next_id_for_test(&id1);
        let page2 = get_stream_inner(&pool, &conn_id, &key, &next, 100).await?;
        assert_eq!(page2.len(), 1, "第二页应只有一条: {page2:?}");
        assert_eq!(page2[0].id, id2);
        assert_eq!(page2[0].fields[0].field, "g");

        // 删除两条；已删除的 id 再删返回 0（不算错误）
        assert_eq!(
            stream_del_entry_inner(&pool, &conn_id, &key, &[id1.clone(), id2.clone()]).await?,
            2
        );
        assert_eq!(
            stream_del_entry_inner(&pool, &conn_id, &key, &[id1]).await?,
            0
        );

        // 空字段被拒
        let err = stream_add_entry_inner(&pool, &conn_id, &key, None, &[])
            .await
            .expect_err("空字段应报错");
        assert!(err.contains("至少需要一个字段"), "{err}");
        Ok(())
    }

    /// `XRANGE` 起点是闭区间，翻页要把上一页最后一条的 id 递增 1。
    fn next_id_for_test(id: &str) -> String {
        match id.split_once('-') {
            Some((ms, seq)) => format!("{ms}-{}", seq.parse::<u64>().unwrap_or(0) + 1),
            None => format!("{id}-1"),
        }
    }
}
