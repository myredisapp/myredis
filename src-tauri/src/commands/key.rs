//! # Key 操作命令
//!
//! 提供对 Redis key 的基本操作命令：列出、新建、删除。
//! 所有命令都依赖连接池中已建立的连接（`conn_id` 对应连接池中的 key）。

use serde::Serialize;

use crate::connection_pool::Pool;

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

/// 列出数据库中的所有 key。
///
/// 使用 `SCAN` 游标遍历（非阻塞），并附带每个 key 的类型与 TTL。
#[tauri::command]
pub async fn list_keys(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
) -> Result<Vec<KeyEntry>, String> {
    let mut con = pool.manager(&conn_id).map_err(|e| e.to_string())?;

    let mut cursor = 0i64;
    let mut keys: Vec<KeyEntry> = Vec::new();
    loop {
        let (next_cursor, batch): (i64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("COUNT")
            .arg(500)
            .query_async(&mut con)
            .await
            .map_err(|e: redis::RedisError| e.to_string())?;
        cursor = next_cursor;

        for key in &batch {
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

        if cursor == 0 {
            break;
        }
    }

    keys.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(keys)
}

/// 新建或覆盖一个字符串类型的 key（等价于 `SET key value`）。
#[tauri::command]
pub async fn set_key(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    key: String,
    value: String,
) -> Result<String, String> {
    pool.ensure_writable(&conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.manager(&conn_id).map_err(|e| e.to_string())?;
    let ok: String = redis::cmd("SET")
        .arg(&key)
        .arg(&value)
        .query_async(&mut con)
        .await
        .map_err(|e: redis::RedisError| e.to_string())?;
    Ok(ok)
}

/// 删除一个或多个 key，返回实际删除的 key 数量。
#[tauri::command]
pub async fn del_key(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    keys: Vec<String>,
) -> Result<i64, String> {
    pool.ensure_writable(&conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.manager(&conn_id).map_err(|e| e.to_string())?;
    let mut cmd = redis::cmd("DEL");
    for k in &keys {
        cmd.arg(k);
    }
    let n: i64 = cmd
        .query_async(&mut con)
        .await
        .map_err(|e: redis::RedisError| e.to_string())?;
    Ok(n)
}

/// 获取一个字符串类型 key 的值。若 key 不存在或类型不是 string，返回错误信息。
#[tauri::command]
pub async fn get_string(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    key: String,
) -> Result<Option<String>, String> {
    let mut con = pool.manager(&conn_id).map_err(|e| e.to_string())?;
    let val: Option<String> = redis::cmd("GET")
        .arg(&key)
        .query_async(&mut con)
        .await
        .map_err(|e: redis::RedisError| e.to_string())?;
    Ok(val)
}
