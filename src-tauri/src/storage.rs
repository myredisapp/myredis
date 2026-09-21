//! # 连接配置持久化存储
//!
//! 负责将连接列表读写到本地 JSON 文件（见 `PROJECT_PLAN.md` 9.1 决策）。
//! - 文件位于系统应用数据目录下 `connections.json`。
//! - 写入采用「临时文件 + rename」保证原子性，避免写坏导致配置丢失。
//!
//! ## 注意
//! 本模块是**同步阻塞 IO**：async 上下文（Tauri 命令、`async fn`）里请用
//! [`ConnectionRepo::load_async`] / [`ConnectionRepo::save_all_async`] —— 它们把文件读写
//! 丢进 [`tauri::async_runtime::spawn_blocking`] 的阻塞线程池，不占用 async 工作线程。
//! 同步版本（[`ConnectionRepo::load`] / [`ConnectionRepo::save_all`]）给测试与阻塞上下文用。

use std::path::PathBuf;

use crate::error::{AppError, AppResult};
use crate::models::Connection;

/// 连接配置仓库，负责 `connections.json` 的读写。
///
/// 仓库自身只持有一个路径，构造它**不产生任何 IO**（目录在首次保存时按需创建）；
/// 所有文件读写都是同步阻塞的，见模块注释里的两条调用约定。
#[derive(Clone)]
pub struct ConnectionRepo {
    file: PathBuf,
}

impl ConnectionRepo {
    /// 指向 `dir/connections.json` 的仓库。
    ///
    /// 不检查也不创建目录（构造仓库是每个命令都要做的事，不该带 IO）：
    /// 目录不存在时由 [`Self::save_all`] 按需创建。
    pub fn new(dir: PathBuf) -> Self {
        Self {
            file: dir.join("connections.json"),
        }
    }

    /// 从磁盘加载所有连接（**同步阻塞**，async 上下文用 [`Self::load_async`]）。
    ///
    /// - 文件不存在时返回空 `Vec`（首次启动）。
    /// - 文件损坏时返回错误（不 panic）。
    pub fn load(&self) -> AppResult<Vec<Connection>> {
        if !self.file.exists() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(&self.file)?;
        let conns = serde_json::from_str(&content)?;
        Ok(conns)
    }

    /// 原子保存全部连接（**同步阻塞**，async 上下文用 [`Self::save_all_async`]）。
    ///
    /// 先写临时文件，再 rename 覆盖，避免写坏；目录不存在时一并创建。
    pub fn save_all(&self, conns: &[Connection]) -> AppResult<()> {
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.file.with_extension("json.tmp");
        let content = serde_json::to_string_pretty(conns)?;
        std::fs::write(&tmp, content)?;
        std::fs::rename(tmp, &self.file).map_err(AppError::Io)?;
        Ok(())
    }

    /// [`Self::load`] 的异步封装：文件读取在阻塞线程池里执行。
    pub async fn load_async(&self) -> AppResult<Vec<Connection>> {
        let repo = self.clone();
        spawn_blocking(move || repo.load()).await
    }

    /// [`Self::save_all`] 的异步封装：写盘在阻塞线程池里执行。
    pub async fn save_all_async(&self, conns: &[Connection]) -> AppResult<()> {
        let repo = self.clone();
        let conns = conns.to_vec();
        spawn_blocking(move || repo.save_all(&conns)).await
    }
}

/// 在阻塞线程池里执行一段同步 IO，并把它包进 [`AppResult`]。
///
/// 内层结果原样返回；线程池里的任务 panic（join 失败）属于代码缺陷，
/// 转成一条可展示的错误而不是让命令永远悬着。
async fn spawn_blocking<T, F>(task: F) -> AppResult<T>
where
    F: FnOnce() -> AppResult<T> + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|e| AppError::msg(format!("连接配置读写任务执行失败: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::ConnectionRepo;
    use crate::models::{ConnType, Connection};
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应当可用")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "maidi-storage-{tag}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn sample(id: &str) -> Connection {
        Connection {
            id: id.into(),
            name: "本地".into(),
            host: "127.0.0.1".into(),
            port: 6379,
            conn_type: ConnType::Single,
            readonly: false,
            separator: ":".into(),
            db: 0,
            username: Some("default".into()),
            password: Some("pw".into()),
        }
    }

    /// 目录不存在也能保存（首次保存时按需创建），且 load / save 往返内容一致。
    #[tokio::test]
    async fn save_then_load_roundtrips_through_blocking_pool() {
        let dir = temp_dir("roundtrip");
        let repo = ConnectionRepo::new(dir.clone());
        assert!(!dir.exists(), "构造仓库不应产生 IO，目录要等到保存时才建");

        let conns = vec![sample("c1"), sample("c2")];
        repo.save_all_async(&conns)
            .await
            .expect("保存应当成功（目录不存在时按需创建）");

        let loaded = repo.load_async().await.expect("加载应当成功");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, "c1");
        assert_eq!(loaded[0].password.as_deref(), Some("pw"));
        assert_eq!(loaded[1].id, "c2");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 文件不存在（首次启动）返回空列表，而不是报错。
    #[tokio::test]
    async fn load_returns_empty_when_file_is_absent() {
        let repo = ConnectionRepo::new(temp_dir("absent"));
        let loaded = repo.load_async().await.expect("缺少文件不应报错");
        assert!(loaded.is_empty(), "首次启动应当得到空列表");
    }

    /// 文件损坏时返回可展示的错误，而不是 panic。
    #[tokio::test]
    async fn load_reports_broken_json() {
        let dir = temp_dir("broken");
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        std::fs::write(dir.join("connections.json"), "{ not json").expect("写入坏文件失败");

        let repo = ConnectionRepo::new(dir.clone());
        let err = repo.load_async().await.expect_err("坏文件应当报错");
        assert!(
            err.to_string().contains("配置解析错误"),
            "应提示解析错误: {err}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
