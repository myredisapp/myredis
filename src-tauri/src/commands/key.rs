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
    let mut con = pool.manager(&conn_id).map_err(|e| e.to_string())?;

    let mut cmd = redis::cmd("SET");
    cmd.arg(&key).arg(&value);
    if ttl > 0 {
        cmd.arg("EX").arg(ttl);
    }
    let ok: String = cmd
        .query_async(&mut con)
        .await
        .map_err(|e: redis::RedisError| e.to_string())?;

    Ok(ok)
}

#[cfg(test)]
mod tests {
    use crate::commands::key::set_key_inner;
    use crate::connection_pool::Pool;
    use crate::models::{ConnType, Connection};

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

    #[tokio::test]
    #[ignore]
    async fn set_key_without_ttl() -> Result<(), String> {
        let pool = test_pool("key_test_no_ttl").await?;
        let key = "maidi:test:no_ttl".to_string();
        let value = "hello".to_string();

        let result = set_key_inner(&pool, "key_test_no_ttl".into(), key.clone(), value, -1).await;
        assert!(result.is_ok(), "SET 应当成功: {:?}", result.err());

        let ttl: i64 = redis::cmd("TTL")
            .arg(&key)
            .query_async(&mut pool.manager("key_test_no_ttl").unwrap())
            .await
            .map_err(|e| e.to_string())?;
        assert_eq!(ttl, -1, "未设置 TTL 的 key 应当永不过期");
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn set_key_with_ttl_sets_expiry() -> Result<(), String> {
        let pool = test_pool("key_test_ttl").await?;
        let key = "maidi:test:with_ttl".to_string();
        let value = "world".to_string();

        let result = set_key_inner(&pool, "key_test_ttl".into(), key.clone(), value, 10).await;
        assert!(result.is_ok(), "SET 应当成功: {:?}", result.err());

        let ttl: i64 = redis::cmd("TTL")
            .arg(&key)
            .query_async(&mut pool.manager("key_test_ttl").unwrap())
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
    async fn set_key_rejects_invalid_ttl() {
        let pool = Pool::new();
        let key = "maidi:test:invalid_ttl".to_string();
        let value = "x".to_string();

        for invalid_ttl in [0, -2, -100] {
            let result = set_key_inner(
                &pool,
                "key_test_invalid_ttl".into(),
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
