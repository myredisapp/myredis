//! # 工作区命令
//!
//! 设置下拉框（以及 macOS 的 Settings 菜单）背后的落点：读取、选择、切换、移除工作区，
//! 以及把导出内容写进工作区并回报完整路径。

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Runtime, State};
use tauri_plugin_dialog::DialogExt;

use crate::workspace::{
    normalize_dir, open_in_file_manager, save_to_workspace, WorkspaceSettings, WorkspaceStore,
};

/// 返回给前端的工作区视图，字段为 camelCase，前端直接渲染。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceView {
    /// 当前工作区绝对路径
    pub current: String,
    /// 默认工作区（用户主目录）
    pub default: String,
    /// 当前用的就是默认工作区
    pub is_default: bool,
    /// 最近选择过的目录（不含默认工作区），最近的在前
    pub recent: Vec<String>,
}

impl WorkspaceView {
    /// 由一份设置快照与默认目录组装视图。
    fn new(settings: &WorkspaceSettings, default_dir: &std::path::Path) -> Self {
        let current = settings
            .current
            .clone()
            .unwrap_or_else(|| default_dir.to_path_buf());
        Self {
            current: current.display().to_string(),
            default: default_dir.display().to_string(),
            is_default: settings.current.is_none(),
            recent: settings
                .recent
                .iter()
                .map(|dir| dir.display().to_string())
                .collect(),
        }
    }
}

/// 写入工作区后的结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedFile {
    /// 文件名（同名文件已自动加序号，这是最终落盘的名字）
    pub name: String,
    /// 落盘后的绝对路径
    pub path: String,
}

/// 读取当前工作区设置。前端启动时调一次，之后每次改动都返回最新视图。
#[tauri::command]
pub async fn get_workspace(store: State<'_, WorkspaceStore>) -> Result<WorkspaceView, String> {
    let settings = store.settings();
    Ok(WorkspaceView::new(&settings, &store.default_dir()))
}

/// 弹出系统「选择文件夹」对话框，把选中目录设为工作区。
///
/// 用户取消时返回 `None`：取消不是错误，前端保持原状、不提示。
#[tauri::command]
pub async fn choose_workspace<R: Runtime>(
    app: AppHandle<R>,
    store: State<'_, WorkspaceStore>,
) -> Result<Option<WorkspaceView>, String> {
    let picked = app
        .dialog()
        .file()
        .set_title("选择工作区目录")
        // 从当前工作区打开，方便在附近找目录
        .set_directory(store.current_dir())
        .blocking_pick_folder();

    let Some(picked) = picked else {
        return Ok(None);
    };
    let dir = picked
        .into_path()
        .map_err(|e| format!("读取所选目录失败：{e}"))?;

    let settings = store.set_current(Some(dir)).map_err(|e| e.to_string())?;
    Ok(Some(WorkspaceView::new(&settings, &store.default_dir())))
}

/// 切换到指定目录（最近列表点击、或前端已知的路径）。
///
/// `path` 为 `None` 或空串时恢复默认工作区。
#[tauri::command]
pub async fn set_workspace(
    store: State<'_, WorkspaceStore>,
    path: Option<String>,
) -> Result<WorkspaceView, String> {
    let dir = path.filter(|p| !p.trim().is_empty()).map(PathBuf::from);
    let settings = store.set_current(dir).map_err(|e| e.to_string())?;
    Ok(WorkspaceView::new(&settings, &store.default_dir()))
}

/// 从最近列表移除一个目录；若它就是当前工作区，则回落到默认工作区。
#[tauri::command]
pub async fn remove_workspace(
    store: State<'_, WorkspaceStore>,
    path: String,
) -> Result<WorkspaceView, String> {
    // 最近列表里的路径本来就是绝对路径，这里再规范化一次：目录被软链接改写或已删除时仍能对上
    let dir = normalize_dir(Path::new(&path), &store.default_dir())
        .unwrap_or_else(|_| PathBuf::from(&path));
    let settings = store.forget(&dir).map_err(|e| e.to_string())?;
    Ok(WorkspaceView::new(&settings, &store.default_dir()))
}

/// 把导出内容写进工作区，返回落盘后的绝对路径（前端据此提示「保存到哪」）。
#[tauri::command]
pub async fn save_workspace_file(
    store: State<'_, WorkspaceStore>,
    file_name: String,
    content: String,
) -> Result<SavedFile, String> {
    let path =
        save_to_workspace(&store.current_dir(), &file_name, &content).map_err(|e| e.to_string())?;
    Ok(SavedFile {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| file_name.clone()),
        path: path.display().to_string(),
    })
}

/// 在系统文件管理器里打开当前工作区，返回打开的路径。
#[tauri::command]
pub async fn open_workspace(store: State<'_, WorkspaceStore>) -> Result<String, String> {
    let dir = store.current_dir();
    open_in_file_manager(&dir).map_err(|e| e.to_string())?;
    Ok(dir.display().to_string())
}
