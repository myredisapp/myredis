//! # 服务器命令
//!
//! 提供与 Redis 服务器相关的只读命令（连通性、服务器信息等）。

use std::collections::HashMap;

use serde::Serialize;
use tauri::State;

use crate::connection_pool::Pool;
use crate::error::{AppError, AppResult};

/// 磁盘空间使用情况。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskUsage {
    /// 数据文件所在目录（来自 Redis `CONFIG GET dir`）
    pub path: String,
    /// 磁盘总容量（字节）
    pub total: u64,
    /// 磁盘剩余可用容量（字节）
    pub available: u64,
    /// 已用百分比（0.0 ~ 100.0）
    pub used_percent: f64,
}

/// Redis 服务器信息（解析自 `INFO` / `DBSIZE` 命令）。
///
/// 对应前端状态栏展示的 内存 / 连接数 / Key 总数 / 磁盘等。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    /// Redis 服务器版本（如 `7.2.4`）
    pub redis_version: String,
    /// 操作系统
    pub os: String,
    /// 运行时间（秒）
    pub uptime_seconds: i64,
    /// 已用内存（字节）
    pub used_memory: u64,
    /// 已连接客户端数
    pub connected_clients: u64,
    /// 数据库总 Key 数（来自 `DBSIZE`）
    pub db_keys: u64,
    /// 当前数据库编号
    pub db: i64,
    /// 持久化数据所在磁盘的使用情况
    pub disk: Option<DiskUsage>,
}

impl ServerInfo {
    /// 从 `INFO` 命令的原始输出解析服务器信息。
    ///
    /// `db_keys` 为单独的 `DBSIZE` 结果，不来自 INFO。
    pub fn parse_info(info: &str, db_keys: u64) -> ServerInfo {
        let mut map: HashMap<String, String> = HashMap::new();
        for line in info.lines() {
            if line.is_empty() || line.starts_with('#') || !line.contains(':') {
                continue;
            }
            if let Some((k, v)) = line.split_once(':') {
                map.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        let gstr = |k: &str| map.get(k).cloned().unwrap_or_default();
        let gi64 = |k: &str| map.get(k).and_then(|s| s.parse().ok()).unwrap_or(0);

        ServerInfo {
            redis_version: gstr("redis_version"),
            os: gstr("os"),
            uptime_seconds: gi64("uptime_in_seconds"),
            used_memory: gi64("used_memory") as u64,
            connected_clients: gi64("connected_clients") as u64,
            db_keys,
            db: 0,
            disk: None,
        }
    }
}

/// 获取某一路径所在磁盘的使用情况。
///
/// 遍历系统所有磁盘，选择「挂载点是该路径最长前缀」的那个磁盘进行统计；
/// 找不到匹配磁盘时返回 `None`。
fn disk_usage_for(path: &str) -> Option<DiskUsage> {
    let path_norm = std::path::Path::new(path);
    let disks = sysinfo::Disks::new_with_refreshed_list();

    // 找出能作为 path 前缀的最长挂载点
    let mut best: Option<(std::path::PathBuf, u64, u64)> = None;
    for d in disks.iter() {
        if path_norm.starts_with(d.mount_point()) {
            let candidate = (d.mount_point().to_path_buf(), d.total_space(), d.available_space());
            let better = match &best {
                None => true,
                Some(b) => candidate.0.components().count() > b.0.components().count(),
            };
            if better {
                best = Some(candidate);
            }
        }
    }
    let (_, total, available) = best?;
    if total == 0 {
        return None;
    }
    let used = total.saturating_sub(available);
    let used_percent = (used as f64) / (total as f64) * 100.0;
    Some(DiskUsage {
        path: path.to_string(),
        total,
        available,
        used_percent,
    })
}

/// 获取 Redis 服务器信息。
///
/// 向 Redis 发送 `INFO` 和 `DBSIZE` 命令，解析为 [`ServerInfo`]；
/// 并通过 `CONFIG GET dir` 获取持久化目录，再查询该目录所在磁盘的使用情况。
#[tauri::command]
pub async fn get_server_info(pool: State<'_, Pool>, conn_id: String) -> AppResult<ServerInfo> {
    let mut con = pool.manager(&conn_id)?;

    let info: String = redis::cmd("INFO")
        .query_async(&mut con)
        .await
        .map_err(AppError::from)?;

    let db_size: u64 = redis::cmd("DBSIZE")
        .query_async(&mut con)
        .await
        .map_err(AppError::from)?;

    // 查询 Redis 数据文件所在目录（持久化目录），并尝试获取其磁盘使用情况
    let dir = redis::cmd("CONFIG")
        .arg("GET")
        .arg("dir")
        .query_async::<_, Vec<String>>(&mut con)
        .await
        .ok()
        .and_then(|mut v| {
            if v.len() >= 2 && !v[1].is_empty() {
                Some(v.remove(1))
            } else {
                None
            }
        });

    let mut si = ServerInfo::parse_info(&info, db_size);
    si.disk = dir.and_then(|d| disk_usage_for(&d));
    Ok(si)
}

/// 切换当前连接使用的逻辑数据库（默认 db0）。
///
/// 通过修改数据库编号并重新建立连接生效，成功返回新的数据库编号。
#[tauri::command]
pub async fn select_db(pool: State<'_, Pool>, conn_id: String, db: u64) -> Result<u64, String> {
    // 从池中取回原始连接配置
    let conn = pool.get(&conn_id).map_err(|e| e.to_string())?;
    let mut updated = conn;
    updated.db = db;
    pool.connect(&updated).await.map_err(|e| e.to_string())?;
    Ok(updated.db)
}

/// PING 测试连接。
#[tauri::command]
pub async fn ping(pool: State<'_, Pool>, conn_id: String) -> AppResult<String> {
    pool.ping(&conn_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_info_parses_redis_info() {
        let raw = "# Server\nredis_version:7.4.11\nos:Linux 5.15.0-microsoft-standard-WSL2 x86_64\nuptime_in_seconds:3600\n# Memory\nused_memory:104857600\n# Clients\nconnected_clients:12\n";
        let info = ServerInfo::parse_info(raw, 8);
        assert_eq!(info.redis_version, "7.4.11");
        assert_eq!(info.uptime_seconds, 3600);
        assert_eq!(info.used_memory, 104_857_600);
        assert_eq!(info.connected_clients, 12);
        assert_eq!(info.db_keys, 8);
        assert!(info.os.contains("Linux"));
        assert!(info.disk.is_none());
    }

    #[test]
    fn parse_info_handles_missing_fields() {
        let info = ServerInfo::parse_info("# Only comments\n\n", 0);
        assert_eq!(info.redis_version, "");
        assert_eq!(info.used_memory, 0);
    }

    #[test]
    fn disk_usage_locates_mount_point() {
        // 根目录总容量 > 0，且可用容量 > 0，使用率在 [0,100]
        let usage = disk_usage_for("/").expect("root mount should exist");
        assert!(usage.total > 0);
        assert!(usage.available > 0);
        assert!((0.0..=100.0).contains(&usage.used_percent));
    }
}