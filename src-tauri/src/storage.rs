//! # 连接配置持久化存储
//!
//! 负责将连接列表读写到本地 JSON 文件（见 `PROJECT_PLAN.md` 9.1 决策）。
//! - 文件位于系统应用数据目录下 `connections.json`。
//! - 写入采用「临时文件 + rename」保证原子性，避免写坏导致配置丢失。
//!
//! ## 注意
//! 本模块为同步阻塞 IO，调用方应放在 `tauri::async_runtime::spawn_blocking` 中执行，避免阻塞主线程。

use std::path::PathBuf;

use crate::error::{AppError, AppResult};

/// 连接配置仓库，负责 `connections.json` 的读写。
pub struct ConnectionRepo {
    file: PathBuf,
}

impl ConnectionRepo {
    /// 使用应用数据目录创建仓库。
    ///
    /// 若目录不存在会尝试创建。
    pub fn new(dir: PathBuf) -> AppResult<Self> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            file: dir.join("connections.json"),
        })
    }

    /// 从磁盘加载所有连接。
    ///
    /// - 文件不存在时返回空 `Vec`（首次启动）。
    /// - 文件损坏时返回错误（不 panic）。
    pub fn load(&self) -> AppResult<Vec<crate::models::Connection>> {
        if !self.file.exists() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(&self.file)?;
        let conns = serde_json::from_str(&content)?;
        Ok(conns)
    }

    /// 原子保存全部连接（先写临时文件，再 rename 覆盖，避免写坏）。
    pub fn save_all(&self, conns: &[crate::models::Connection]) -> AppResult<()> {
        let tmp = self.file.with_extension("json.tmp");
        let content = serde_json::to_string_pretty(conns)?;
        std::fs::write(&tmp, content)?;
        std::fs::rename(tmp, &self.file).map_err(AppError::Io)?;
        Ok(())
    }
}