//! # 工作区设置
//!
//! 「工作区」是导出文件的落盘目录：Key 导出与连接配置导出都写到这里。
//!
//! - **默认工作区**是当前用户的主目录（`$HOME`），不需要任何配置就能用；
//! - 用户可以在设置下拉框里**选择任意文件夹**作为工作区，选择结果持久化到应用配置目录下的
//!   `workspace.json`，所以重开窗口、重启应用后仍是同一个目录；导出完成后界面会提示完整路径，
//!   解决「导出后不知道文件保存到哪」的问题；
//! - 除当前工作区外还保留最近选过的若干目录（见 [`MAX_RECENT`]），方便一键切回。
//!
//! 与 `storage.rs` 一样是同步阻塞 IO：读写的是几百字节的小文件，调用方在命令层直接用。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// 最近工作区列表最多保留的目录数（不含默认工作区）。
pub const MAX_RECENT: usize = 8;

/// 同名文件自动加序号的尝试次数，超过后改用时间戳兜底（仍旧不覆盖）。
const MAX_NAME_RETRY: u32 = 1000;

/// 工作区设置，持久化为 `workspace.json`。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSettings {
    /// 当前工作区；`None` 表示使用默认工作区（用户主目录）。
    #[serde(default)]
    pub current: Option<PathBuf>,
    /// 用户选择过的目录，最近的在前，不含默认工作区。
    #[serde(default)]
    pub recent: Vec<PathBuf>,
}

impl WorkspaceSettings {
    /// 丢弃已经不存在的目录，当前工作区不存在时回落到默认工作区。
    ///
    /// 启动时执行：目录可能被删除、外置磁盘可能没挂上，留着它们只会让导出时才失败。
    fn sanitize(&mut self, default_dir: &Path) {
        self.recent
            .retain(|dir| dir.is_dir() && dir.as_path() != default_dir);
        let current_alive = self
            .current
            .as_deref()
            .map(|dir| dir.is_dir() && dir != default_dir)
            .unwrap_or(false);
        if !current_alive {
            self.current = None;
        }
    }
}

/// 工作区仓库：当前工作区 + 最近列表，由它统一读写 `workspace.json`。
pub struct WorkspaceStore {
    /// 设置文件路径（配置目录下的 `workspace.json`）。
    file: PathBuf,
    /// 默认工作区，即用户主目录。
    default_dir: PathBuf,
    /// 内存中的设置快照，落盘成功后才会更新。
    settings: Mutex<WorkspaceSettings>,
}

impl WorkspaceStore {
    /// 从配置目录加载设置。
    ///
    /// 设置文件缺失或损坏都退回默认工作区，不让一个坏文件拖住启动。
    /// `default_dir` 会尽力规范化（软链接、`/var` 这类别名统一），失败时按原样使用。
    pub fn load(config_dir: PathBuf, default_dir: PathBuf) -> Self {
        let default_dir = std::fs::canonicalize(&default_dir).unwrap_or(default_dir);
        let file = config_dir.join("workspace.json");
        let mut settings = read_settings(&file);
        settings.sanitize(&default_dir);
        Self {
            file,
            default_dir,
            settings: Mutex::new(settings),
        }
    }

    /// 默认工作区（用户主目录）。
    pub fn default_dir(&self) -> PathBuf {
        self.default_dir.clone()
    }

    /// 当前工作区绝对路径：用户选过就是那个目录，否则是默认工作区。
    pub fn current_dir(&self) -> PathBuf {
        self.settings()
            .current
            .unwrap_or_else(|| self.default_dir.clone())
    }

    /// 当前设置快照。
    pub fn settings(&self) -> WorkspaceSettings {
        self.lock().clone()
    }

    /// 设置当前工作区，`dir` 为 `None` 时恢复默认工作区。
    ///
    /// 目录会先规范化（展开 `~`、要求存在、解析软链接），选中默认目录等同于恢复默认。
    /// 成功后目录进入最近列表（最近的在前、去重、上限 [`MAX_RECENT`]）。
    pub fn set_current(&self, dir: Option<PathBuf>) -> AppResult<WorkspaceSettings> {
        let normalized = match dir {
            Some(dir) => {
                let dir = normalize_dir(&dir, &self.default_dir)?;
                // 选中的就是主目录时按「默认工作区」记，界面上不会出现两个等价的条目
                if dir == self.default_dir {
                    None
                } else {
                    Some(dir)
                }
            }
            None => None,
        };

        let mut guard = self.lock();
        let mut next = guard.clone();
        next.current = normalized.clone();
        if let Some(dir) = normalized {
            push_recent(&mut next.recent, dir);
        }
        self.persist(&mut guard, next)
    }

    /// 忘记一个最近目录；若它就是当前工作区，则一并回落到默认工作区。
    pub fn forget(&self, dir: &Path) -> AppResult<WorkspaceSettings> {
        let mut guard = self.lock();
        let mut next = guard.clone();
        next.recent.retain(|d| d != dir);
        if next.current.as_deref() == Some(dir) {
            next.current = None;
        }
        self.persist(&mut guard, next)
    }

    /// 取设置锁；上一次持锁期间出错（panic）不阻塞后续读写，直接沿用中毒前的数据。
    fn lock(&self) -> MutexGuard<'_, WorkspaceSettings> {
        self.settings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 落盘成功后更新内存快照；写盘失败时内存保持原样，避免界面与磁盘不一致。
    fn persist(
        &self,
        guard: &mut WorkspaceSettings,
        next: WorkspaceSettings,
    ) -> AppResult<WorkspaceSettings> {
        write_settings(&self.file, &next)?;
        *guard = next.clone();
        Ok(next)
    }
}

/// 校验并规范化一个工作区目录：展开 `~`、要求目录存在、解析成绝对路径。
///
/// 目录不存在时直接报错，让问题在「选择工作区」时就暴露，而不是等导出时才失败。
pub fn normalize_dir(dir: &Path, home: &Path) -> AppResult<PathBuf> {
    let expanded = expand_tilde(dir, home);
    if !expanded.exists() {
        return Err(AppError::msg(format!("目录不存在：{}", expanded.display())));
    }
    if !expanded.is_dir() {
        return Err(AppError::msg(format!("不是文件夹：{}", expanded.display())));
    }
    std::fs::canonicalize(&expanded).map_err(AppError::Io)
}

/// 展开路径开头的 `~`（只支持 `~` 与 `~/...`，`~user` 写法不处理）。
fn expand_tilde(dir: &Path, home: &Path) -> PathBuf {
    let text = dir.to_string_lossy();
    if text == "~" {
        return home.to_path_buf();
    }
    match text.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => dir.to_path_buf(),
    }
}

/// 把 `content` 写进工作区目录，返回实际落盘的绝对路径。
///
/// 同名文件已存在时自动加序号（`a.json` → `a-1.json`），不覆盖用户已有的导出文件。
pub fn save_to_workspace(dir: &Path, file_name: &str, content: &str) -> AppResult<PathBuf> {
    let name = safe_file_name(file_name)?;
    std::fs::create_dir_all(dir)?;
    let path = unique_path(dir, &name);
    std::fs::write(&path, content)?;
    Ok(path)
}

/// 只接受纯文件名：带路径分隔符或 `..` 一律拒绝，避免写到工作区之外。
fn safe_file_name(file_name: &str) -> AppResult<String> {
    let name = file_name.trim();
    let bare = Path::new(name)
        .file_name()
        .map(|n| n == std::ffi::OsStr::new(name))
        .unwrap_or(false);
    if !bare {
        return Err(AppError::msg(format!("文件名不合法：{file_name}")));
    }
    Ok(name.to_string())
}

/// 在 `dir` 下找一个不冲突的文件名：`a.json` 已存在就用 `a-1.json`、`a-2.json`……
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }

    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string());
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned());
    let renamed = |suffix: &str| match &ext {
        Some(ext) => dir.join(format!("{stem}-{suffix}.{ext}")),
        None => dir.join(format!("{stem}-{suffix}")),
    };

    for n in 1..=MAX_NAME_RETRY {
        let candidate = renamed(&n.to_string());
        if !candidate.exists() {
            return candidate;
        }
    }
    // 同名文件上千个的极端情况：改用时间戳兜底，仍然不覆盖已有文件
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let candidate = renamed(&nanos.to_string());
    if candidate.exists() {
        return dir.join(match &ext {
            Some(ext) => format!("{stem}-{nanos}-{}.{ext}", std::process::id()),
            None => format!("{stem}-{nanos}-{}", std::process::id()),
        });
    }
    candidate
}

/// 在系统文件管理器里打开目录（Finder / 资源管理器 / 默认文件管理器）。
pub fn open_in_file_manager(dir: &Path) -> AppResult<()> {
    #[cfg(target_os = "macos")]
    let (program, args) = ("open", vec![dir.as_os_str()]);
    #[cfg(target_os = "windows")]
    let (program, args) = ("explorer", vec![dir.as_os_str()]);
    #[cfg(all(unix, not(target_os = "macos")))]
    let (program, args) = ("xdg-open", vec![dir.as_os_str()]);

    std::process::Command::new(program)
        .args(args)
        .spawn()
        .map_err(|e| AppError::msg(format!("打开目录失败：{e}")))?;
    Ok(())
}

/// 把一个目录放到最近列表最前面：去重、上限 [`MAX_RECENT`]。
fn push_recent(recent: &mut Vec<PathBuf>, dir: PathBuf) {
    recent.retain(|d| d != &dir);
    recent.insert(0, dir);
    recent.truncate(MAX_RECENT);
}

/// 读取 `workspace.json`；缺失或损坏都退回默认设置并打日志。
fn read_settings(file: &Path) -> WorkspaceSettings {
    match std::fs::read_to_string(file) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
            eprintln!(
                "工作区设置解析失败（{}），改用默认工作区：{err}",
                file.display()
            );
            WorkspaceSettings::default()
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => WorkspaceSettings::default(),
        Err(err) => {
            eprintln!(
                "工作区设置读取失败（{}），改用默认工作区：{err}",
                file.display()
            );
            WorkspaceSettings::default()
        }
    }
}

/// 原子写入设置（临时文件 + rename），与 `storage.rs` 的连接配置同一套写法。
fn write_settings(file: &Path, settings: &WorkspaceSettings) -> AppResult<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(settings)?)?;
    std::fs::rename(tmp, file).map_err(AppError::Io)
}

#[cfg(test)]
mod tests {
    use super::{
        expand_tilde, normalize_dir, push_recent, safe_file_name, save_to_workspace, unique_path,
        WorkspaceSettings, WorkspaceStore, MAX_RECENT,
    };
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// 建一个本次测试独占的临时目录，测试结束时由调用方删除。
    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("maidi-ws-{tag}-{nanos}-{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    #[test]
    fn expand_tilde_only_handles_bare_and_prefixed() {
        let home = Path::new("/Users/tester");
        assert_eq!(expand_tilde(Path::new("~"), home), home);
        assert_eq!(
            expand_tilde(Path::new("~/Projects"), home),
            Path::new("/Users/tester/Projects")
        );
        // 其他写法原样保留：`~user` 与中间出现的 `~` 都不是家目录
        assert_eq!(
            expand_tilde(Path::new("~other/x"), home),
            Path::new("~other/x")
        );
        assert_eq!(
            expand_tilde(Path::new("/tmp/~x"), home),
            Path::new("/tmp/~x")
        );
    }

    #[test]
    fn normalize_dir_rejects_missing_and_file() {
        let root = temp_dir("normalize");
        let missing = root.join("nope");
        let err = normalize_dir(&missing, &root).unwrap_err().to_string();
        assert!(
            err.contains("目录不存在"),
            "错误信息应说明目录不存在: {err}"
        );

        let file = root.join("a.json");
        std::fs::write(&file, "{}").unwrap();
        let err = normalize_dir(&file, &root).unwrap_err().to_string();
        assert!(
            err.contains("不是文件夹"),
            "错误信息应说明不是文件夹: {err}"
        );

        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(normalize_dir(&sub, &root).unwrap(), sub);
        // `~/sub` 这种写法会被展开
        assert_eq!(normalize_dir(Path::new("~/sub"), &root).unwrap(), sub);

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn push_recent_dedups_and_caps() {
        let mut recent: Vec<PathBuf> = Vec::new();
        push_recent(&mut recent, PathBuf::from("/a"));
        push_recent(&mut recent, PathBuf::from("/b"));
        push_recent(&mut recent, PathBuf::from("/a"));
        assert_eq!(recent, vec![PathBuf::from("/a"), PathBuf::from("/b")]);

        for n in 0..MAX_RECENT + 3 {
            push_recent(&mut recent, PathBuf::from(format!("/dir{n}")));
        }
        assert_eq!(recent.len(), MAX_RECENT, "最近列表应被截断到上限");
        assert_eq!(recent[0], PathBuf::from(format!("/dir{}", MAX_RECENT + 2)));
    }

    #[test]
    fn save_to_workspace_keeps_existing_files() {
        let root = temp_dir("save");
        let first = save_to_workspace(&root, "export.json", "one").unwrap();
        let second = save_to_workspace(&root, "export.json", "two").unwrap();
        assert_eq!(first.file_name().unwrap(), "export.json");
        assert_eq!(second.file_name().unwrap(), "export-1.json");
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "one");
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "two");

        // 目录不存在时按需创建（用户把工作区目录删掉后仍能导出）
        let nested = root.join("deep/deeper");
        let saved = save_to_workspace(&nested, "a.json", "x").unwrap();
        assert!(saved.starts_with(&nested));

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn safe_file_name_rejects_paths() {
        assert!(safe_file_name("ok.json").is_ok());
        for bad in ["../evil.json", "sub/evil.json", "", "..", "  "] {
            assert!(safe_file_name(bad).is_err(), "应拒绝文件名 {bad:?}");
        }
    }

    #[test]
    fn unique_path_falls_back_to_timestamp() {
        let root = temp_dir("unique");
        // 占满序号空间，逼出时间戳兜底分支
        std::fs::write(root.join("a.json"), "x").unwrap();
        for n in 1..=super::MAX_NAME_RETRY {
            std::fs::write(root.join(format!("a-{n}.json")), "x").unwrap();
        }
        let path = unique_path(&root, "a.json");
        assert_ne!(path, root.join("a.json"));
        assert!(!path.exists(), "兜底路径必须仍是不冲突的新文件");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn store_persists_across_reload() {
        let root = temp_dir("store");
        let config = root.join("config");
        let home = root.join("home");
        let picked = root.join("work space");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&picked).unwrap();

        let store = WorkspaceStore::load(config.clone(), home.clone());
        assert_eq!(store.current_dir(), home, "默认工作区是主目录");
        assert!(store.settings().current.is_none());

        let saved = store.set_current(Some(picked.clone())).unwrap();
        assert_eq!(saved.current.as_deref(), Some(picked.as_path()));
        assert_eq!(store.current_dir(), picked);
        assert_eq!(saved.recent, vec![picked.clone()]);

        // 模拟重开窗口：重新加载后仍是同一个工作区
        let reopened = WorkspaceStore::load(config.clone(), home.clone());
        assert_eq!(reopened.current_dir(), picked);
        assert_eq!(reopened.settings().recent, vec![picked.clone()]);

        // 选中默认目录等于恢复默认
        let reset = reopened.set_current(Some(home.clone())).unwrap();
        assert!(reset.current.is_none());
        assert_eq!(reopened.current_dir(), home);

        // 忘记最近目录：当前工作区被移除时回落到默认
        store.set_current(Some(picked.clone())).unwrap();
        let after = store.forget(&picked).unwrap();
        assert!(after.recent.is_empty());
        assert!(after.current.is_none());
        assert_eq!(store.current_dir(), home);

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn store_drops_dead_dirs_on_load() {
        let root = temp_dir("dead");
        let config = root.join("config");
        let home = root.join("home");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let gone = root.join("gone");
        std::fs::create_dir_all(&gone).unwrap();

        let store = WorkspaceStore::load(config.clone(), home.clone());
        store.set_current(Some(gone.clone())).unwrap();
        std::fs::remove_dir_all(&gone).unwrap();

        let reopened = WorkspaceStore::load(config, home.clone());
        assert_eq!(reopened.current_dir(), home, "目录没了应回落到默认工作区");
        assert!(reopened.settings().recent.is_empty());

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn store_survives_broken_settings_file() {
        let root = temp_dir("broken");
        let config = root.join("config");
        let home = root.join("home");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(config.join("workspace.json"), "{ not json").unwrap();

        let store = WorkspaceStore::load(config, home.clone());
        assert_eq!(store.current_dir(), home);
        // 坏文件不应让后续设置失效
        let picked = root.join("work");
        std::fs::create_dir_all(&picked).unwrap();
        store.set_current(Some(picked.clone())).unwrap();
        assert_eq!(store.current_dir(), picked);

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn settings_sanitize_ignores_default_dir_in_recent() {
        let root = temp_dir("sanitize");
        let mut settings = WorkspaceSettings {
            current: Some(root.clone()),
            recent: vec![root.clone(), root.join("missing")],
        };
        settings.sanitize(&root);
        assert!(
            settings.current.is_none(),
            "当前工作区等于默认目录时按默认处理"
        );
        assert!(
            settings.recent.is_empty(),
            "默认目录与失效目录都从最近列表移除"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
