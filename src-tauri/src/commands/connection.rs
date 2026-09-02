//! # 连接管理命令
//!
//! 提供连接配置的增删查改，以及建立/断开 Redis 连接的能力。

use tauri::State;

use crate::connection_pool::{ConnInfo, Pool};
use crate::models::Connection;
use crate::AppState;

/// 建立到 Redis 的连接。
#[tauri::command]
pub async fn connect(pool: State<'_, Pool>, conn: Connection) -> Result<ConnInfo, String> {
    pool.connect(&conn).await.map_err(|e| e.to_string())
}

/// 断开一个 Redis 连接。
#[tauri::command]
pub async fn disconnect(pool: State<'_, Pool>, conn_id: String) -> Result<bool, String> {
    Ok(pool.disconnect(&conn_id))
}

/// 列出所有已保存的连接配置。
#[tauri::command]
pub async fn list_connections(state: State<'_, AppState>) -> Result<Vec<Connection>, String> {
    let repo = state.repo()?;
    repo.load().map_err(|e| e.to_string())
}

/// 保存（新增或更新）一个连接配置，持久化到本地。
#[tauri::command]
pub async fn save_connection(state: State<'_, AppState>, conn: Connection) -> Result<(), String> {
    let repo = state.repo()?;
    let mut all = repo.load().map_err(|e| e.to_string())?;
    if let Some(existing) = all.iter_mut().find(|c| c.id == conn.id) {
        *existing = conn;
    } else {
        all.push(conn);
    }
    repo.save_all(&all).map_err(|e| e.to_string())
}

/// 删除一个连接配置，返回是否删除成功。
#[tauri::command]
pub async fn delete_connection(
    state: State<'_, AppState>,
    conn_id: String,
) -> Result<bool, String> {
    let repo = state.repo()?;
    let mut all = repo.load().map_err(|e| e.to_string())?;
    let before = all.len();
    all.retain(|c| c.id != conn_id);
    repo.save_all(&all).map_err(|e| e.to_string())?;
    Ok(all.len() != before)
}

/// 测试连接参数是否可用（不保存、不缓存连接）。
#[tauri::command]
pub async fn test_connection(conn: Connection) -> Result<String, String> {
    Pool::test(&conn).await.map_err(|e| e.to_string())
}
