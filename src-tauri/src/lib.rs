//! # 麦地缓存 - Redis 图形化管理客户端
//!
//! 库入口，定义 Tauri 应用并注册所有命令与状态。

pub mod commands;
mod config;
pub mod connection_pool;
pub mod error;
/// 菜单栏仅在 macOS 上定制：其他平台原本就没有原生菜单栏，保持系统默认行为。
#[cfg(target_os = "macos")]
mod menu;
pub mod models;
mod storage;

use std::path::PathBuf;

use tauri::Manager;

use crate::config::AppConfig;
use crate::connection_pool::Pool;
use crate::storage::ConnectionRepo;

/// 应用全局状态，注入 Tauri。
pub struct AppState {
    /// 连接配置持久化目录
    config_dir: PathBuf,
}

impl AppState {
    /// 创建一个新的应用状态。
    pub fn new(config_dir: PathBuf) -> Self {
        Self { config_dir }
    }

    /// 返回连接配置仓库（懒创建，目录不存在自动建）。
    pub fn repo(&self) -> Result<ConnectionRepo, String> {
        ConnectionRepo::new(self.config_dir.clone()).map_err(|e| e.to_string())
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default().plugin(tauri_plugin_updater::Builder::new().build());

    // macOS 菜单栏：Window / Settings / Help（编辑快捷键用的标准编辑项挂在应用菜单里，见 menu 模块）。
    #[cfg(target_os = "macos")]
    let builder = builder.menu(menu::build).on_menu_event(menu::on_event);

    builder
        .setup(|app| {
            let config_dir = app
                .path()
                .app_config_dir()
                .unwrap_or_else(|_| std::env::temp_dir());
            app.manage(AppState::new(config_dir));
            // 连接池作为独立的状态，便于命令按需借用
            app.manage(Pool::new());
            // 更新状态：后台下载的进度与已下好的安装包都放这里，供三个更新命令共享
            app.manage(commands::update::UpdateState::default());
            // 初始化配置（目前仅保底引入，避免 dead_code 告警，后续用于默认值/迁移）
            let _ = AppConfig::default();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::connection::connect,
            commands::connection::disconnect,
            commands::connection::list_connections,
            commands::connection::save_connection,
            commands::connection::delete_connection,
            commands::connection::test_connection,
            commands::connection::export_connections,
            commands::connection::import_connections,
            commands::server::ping,
            commands::server::get_server_info,
            commands::server::select_db,
            commands::key::list_keys,
            commands::key::set_key,
            commands::key::del_key,
            commands::key::get_string,
            commands::key_content::get_hash,
            commands::key_content::get_list,
            commands::key_content::get_set,
            commands::key_content::get_zset,
            commands::key_content::set_key_ttl,
            commands::key_content::hash_set_field,
            commands::key_content::hash_del_fields,
            commands::key_content::list_set_element,
            commands::key_content::list_del_element,
            commands::key_content::list_push_element,
            commands::key_content::set_add_member,
            commands::key_content::set_del_member,
            commands::key_content::zset_add_member,
            commands::key_content::zset_del_member,
            commands::terminal::execute_command,
            commands::import_export::export_keys,
            commands::import_export::import_keys,
            commands::update::check_update,
            commands::update::start_update_download,
            commands::update::get_update_progress,
            commands::update::install_update_and_restart,
        ])
        .run(tauri::generate_context!())
        .expect("启动麦地缓存失败");
}
