//! # 连接配置持久化存储
//!
//! 负责将连接列表读写到本地 JSON 文件（见 `PROJECT_PLAN.md` 9.1 决策）。
//! - 文件位于系统应用数据目录下 `connections.json`。
//! - 写入采用「临时文件 + rename」保证原子性，避免写坏导致配置丢失。
//!
//! ## 密码存储（DEVELOPMENT.md §2.4「密码改存系统密钥链」）
//! 密码**不落盘**：`save_all` 把每条连接的密码转存系统密钥链（macOS Keychain /
//! Windows Credential Manager / Linux Secret Service），`connections.json` 里不出现
//! `password` 字段；`load` 时从密钥链把密码读回内存（编辑连接时要能回显）。
//! 密钥链不可用（CI / Linux headless 无 D-Bus 等）时自动降级为旧行为 —— 密码
//! 照旧写进 JSON，功能不受影响，只是回到明文落盘。
//!
//! ## 注意
//! 本模块是**同步阻塞 IO**：async 上下文（Tauri 命令、`async fn`）里请用
//! [`ConnectionRepo::load_async`] / [`ConnectionRepo::save_all_async`] —— 它们把文件读写
//! 丢进 [`tauri::async_runtime::spawn_blocking`] 的阻塞线程池，不占用 async 工作线程。
//! 同步版本（[`ConnectionRepo::load`] / [`ConnectionRepo::save_all`]）给测试与阻塞上下文用。

use std::path::PathBuf;

use crate::error::{AppError, AppResult};
use crate::models::Connection;

/// 密钥链服务名（macOS Keychain 的 Service / Secret Service 的 label 前缀）。
const KEYRING_SERVICE: &str = "maidi-cache";

/// 连接配置仓库，负责 `connections.json` 的读写与密码的密钥链存取。
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
    /// - 密码处理：文件里的明文密码**自动迁移**到密钥链后从文件抹除；
    ///   内存中的密码一律从密钥链读回（密钥链不可用时保留文件里的明文，即降级）。
    pub fn load(&self) -> AppResult<Vec<Connection>> {
        if !self.file.exists() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(&self.file)?;
        let mut conns: Vec<Connection> = serde_json::from_str(&content)?;

        let mut migrated = false;
        for conn in &mut conns {
            normalize_password(conn);
            if let Some(pw) = conn.password.clone() {
                // 存量明文密码：写入密钥链成功才算迁完，随后从文件抹除；
                // 写不进去（无后端）就保留明文，功能不受影响
                if store_password(&conn.id, &pw) {
                    migrated = true;
                }
            } else {
                // 密码本就不在文件里：从密钥链读回内存（读不到就留空，不报错）
                conn.password = read_password(&conn.id);
            }
        }

        if migrated {
            // 只抹除「密钥链里已是同一密码」的条目；迁移失败的那条随文件原样保留，
            // 下次加载会再试。写回失败也不阻断加载。
            let stripped: Vec<Connection> = conns
                .iter()
                .cloned()
                .map(|mut c| {
                    if c.password
                        .as_ref()
                        .is_some_and(|pw| read_password(&c.id).as_deref() == Some(pw.as_str()))
                    {
                        c.password = None;
                    }
                    c
                })
                .collect();
            let _ = self.save_all(&stripped);
        }
        Ok(conns)
    }

    /// 原子保存全部连接（**同步阻塞**，async 上下文用 [`Self::save_all_async`]）。
    ///
    /// 先写临时文件，再 rename 覆盖，避免写坏；目录不存在时一并创建。
    /// 密码转存密钥链：成功的条目序列化时抹掉 `password` 字段；密钥链不可用
    /// 时该条密码保留在 JSON 里（降级为明文落盘，与旧版本行为一致）。
    pub fn save_all(&self, conns: &[Connection]) -> AppResult<()> {
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let storable: Vec<Connection> = conns
            .iter()
            .cloned()
            .map(|mut c| {
                normalize_password(&mut c);
                if let Some(pw) = c.password.clone() {
                    if store_password(&c.id, &pw) {
                        c.password = None;
                    }
                }
                c
            })
            .collect();
        let tmp = self.file.with_extension("json.tmp");
        let content = serde_json::to_string_pretty(&storable)?;
        std::fs::write(&tmp, content)?;
        std::fs::rename(tmp, &self.file).map_err(AppError::Io)?;
        Ok(())
    }

    /// 删除一个连接在密钥链里的密码条目（删除连接时调用，尽力而为）。
    pub fn delete_password(&self, conn_id: &str) {
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, conn_id) {
            let _ = entry.delete_credential();
        }
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

/// 空串密码视为未设置（前端表单清掉密码框会传空串）。
fn normalize_password(conn: &mut Connection) {
    if conn.password.as_deref().is_some_and(|p| p.is_empty()) {
        conn.password = None;
    }
}

/// 把密码写入系统密钥链，返回是否成功（失败 = 无可用后端，调用方走降级）。
fn store_password(conn_id: &str, password: &str) -> bool {
    match keyring::Entry::new(KEYRING_SERVICE, conn_id) {
        Ok(entry) => entry.set_password(password).is_ok(),
        Err(_) => false,
    }
}

/// 从系统密钥链读密码；无条目或后端不可用都返回 `None`。
fn read_password(conn_id: &str) -> Option<String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, conn_id).ok()?;
    entry.get_password().ok()
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
            tls: false,
            tls_insecure: false,
            connect_timeout_secs: None,
            command_timeout_secs: None,
        }
    }

    /// 目录不存在也能保存（首次保存时按需创建），且 load / save 往返内容一致。
    /// 密码无论密钥链是否可用都要能读回来（可用 → 经密钥链；不可用 → 降级为文件明文）。
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

        repo.delete_password("c1");
        repo.delete_password("c2");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 密钥链可用时密码不落盘（JSON 里没有 password 字段），内存里能读回；
    /// 密钥链不可用时降级为明文落盘。两种环境各自断言成立。
    #[tokio::test]
    async fn password_persists_via_keychain_or_file_fallback() {
        let dir = temp_dir("secret");
        let repo = ConnectionRepo::new(dir.clone());
        repo.save_all_async(&[sample("sec1")])
            .await
            .expect("保存应当成功");

        let on_disk = std::fs::read_to_string(dir.join("connections.json")).unwrap();
        if keychain_works() {
            assert!(
                !on_disk.contains("\"password\""),
                "密钥链可用时密码不应落盘: {on_disk}"
            );
        } else {
            assert!(
                on_disk.contains("\"password\""),
                "密钥链不可用时密码应降级写进文件: {on_disk}"
            );
        }

        let loaded = repo.load_async().await.expect("加载应当成功");
        assert_eq!(
            loaded[0].password.as_deref(),
            Some("pw"),
            "密码必须能读回（密钥链或降级文件）"
        );

        // 清理密钥链里的测试条目，避免污染真实钥匙串
        repo.delete_password("sec1");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 手写一份带明文密码的旧版文件：加载时自动迁移（密码进密钥链、文件抹除）。
    /// 密钥链不可用的环境下文件保持明文（降级），密码同样可读回。
    #[tokio::test]
    async fn load_migrates_plaintext_password_to_keychain() {
        let dir = temp_dir("migrate");
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        let legacy = r#"[{"id":"old1","name":"旧连接","host":"127.0.0.1","port":6379,"type":"single","separator":":","db":0,"password":"legacy-pw"}]"#;
        std::fs::write(dir.join("connections.json"), legacy).expect("写入旧版文件失败");

        let repo = ConnectionRepo::new(dir.clone());
        let loaded = repo.load_async().await.expect("加载应当成功");
        assert_eq!(loaded.len(), 1);
        assert_eq!(
            loaded[0].password.as_deref(),
            Some("legacy-pw"),
            "迁移后内存中应有密码"
        );

        if keychain_works() {
            let on_disk = std::fs::read_to_string(dir.join("connections.json")).unwrap();
            assert!(
                !on_disk.contains("legacy-pw"),
                "迁移成功后文件里的明文密码应被抹除: {on_disk}"
            );
        }

        repo.delete_password("old1");
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

    /// 探测当前环境密钥链是否可用（用随机条目试写，随即删除）。
    fn keychain_works() -> bool {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let probe = format!("probe-{}-{nanos}", std::process::id());
        let ok = super::store_password(&probe, "x");
        if let Ok(entry) = keyring::Entry::new(super::KEYRING_SERVICE, &probe) {
            let _ = entry.delete_credential();
        }
        ok
    }
}
