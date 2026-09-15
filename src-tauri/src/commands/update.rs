//! # 静默更新
//!
//! 对应前端标题栏的更新入口与窗口底部的进度条。流程刻意拆成三步：
//!
//! 1. [`check_update`] —— 查最新版本（应用启动时静默跑一次，用户也可手动触发）；
//! 2. [`start_update_download`] —— 后台下载安装包后立即返回，前端靠
//!    [`get_update_progress`] 轮询进度画进度条，下载期间应用照常可用；
//! 3. [`install_update_and_restart`] —— 用户点「立即重启」时才落盘安装并重启。
//!
//! 之所以敢把「下载」和「安装」拆开：`Update::download` 内部已完成签名校验，
//! 校验不过不会返回字节，所以不存在「下到一半的坏包被装上」的窗口期 ——
//! 真正的写盘动作被压缩到用户点重启的那一刻，下好之后哪怕一直不重启也是安全的。
//!
//! 更新源是 `tauri.conf.json` 里 `plugins.updater.endpoints` 指向的 `latest.json`，
//! 校验用的公钥同在该文件；私钥只存在于 CI（`.github/workflows/release.yml`）。

use std::sync::{Mutex, MutexGuard};

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::error::{AppError, AppResult};

/// 更新流程所处的阶段，前端据此决定进度条怎么显示。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateStage {
    /// 尚无待下载的更新（初始态，或检查完发现已是最新）
    #[default]
    Idle,
    /// 正在后台下载
    Downloading,
    /// 下载并校验完成，等用户点重启
    Ready,
    /// 正在安装并重启
    Installing,
    /// 下载或安装失败，`UpdateProgress::error` 里有原因
    Failed,
}

/// 版本信息，对应「发现新版本」弹窗。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    /// 是否有可用更新
    pub available: bool,
    /// 最新版本号；无更新时为 `None`
    pub version: Option<String>,
    /// 当前运行的版本号（取自安装包，不是仓库里的占位版本）
    pub current_version: String,
    /// 更新说明（`latest.json` 的 `notes`）
    pub notes: Option<String>,
    /// 发布时间（`latest.json` 的 `pub_date`）
    pub date: Option<String>,
}

/// 下载进度快照，前端轮询它来画底部进度条。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgress {
    /// 当前阶段
    pub stage: UpdateStage,
    /// 已下载字节数
    pub downloaded: u64,
    /// 总字节数；服务端未给 `Content-Length` 时为 `None`，前端显示不确定进度
    pub total: Option<u64>,
    /// 正在下载/已就绪的版本号
    pub version: Option<String>,
    /// 失败原因，仅 `Failed` 阶段有值
    pub error: Option<String>,
}

/// 更新状态内部数据。
#[derive(Default)]
struct UpdateInner {
    stage: UpdateStage,
    /// 上一次检查到、尚未安装的更新句柄
    update: Option<Update>,
    /// 已下载并通过签名校验的安装包字节
    bytes: Option<Vec<u8>>,
    downloaded: u64,
    total: Option<u64>,
    error: Option<String>,
}

/// 更新全局状态，由 `lib.rs` 注入 Tauri。
#[derive(Default)]
pub struct UpdateState(Mutex<UpdateInner>);

impl UpdateState {
    /// 取内部数据的锁。
    ///
    /// 临界区只做几次字段读写、绝不跨 `await`，所以用标准库互斥锁即可；
    /// 万一有线程在持锁时 panic，这里直接取回内部值继续 —— 更新失败不该让整个应用不可用。
    ///
    /// 名字不叫 `inner`：`tauri::State` 自带一个 `inner()` 返回 `&T`，
    /// 同名方法会优先命中它，拿到的就不是 `MutexGuard` 了。
    fn guard(&self) -> MutexGuard<'_, UpdateInner> {
        self.0.lock().unwrap_or_else(|err| err.into_inner())
    }
}

/// 把更新句柄转成前端要展示的信息。
fn info_of(update: &Update) -> UpdateInfo {
    UpdateInfo {
        available: true,
        version: Some(update.version.clone()),
        current_version: update.current_version.clone(),
        notes: update.body.clone(),
        // 不引 `time` 的格式化特性，直接从原始 JSON 里取，取不到就不展示发布日期
        date: update
            .raw_json
            .get("pub_date")
            .and_then(|value| value.as_str())
            .map(str::to_string),
    }
}

/// 检查是否有新版本。
///
/// 已有下载成果（下载中 / 已就绪）时直接复用缓存结果，不再打一次网络请求 ——
/// 此时再查也只会拿到同一个版本，反而会让进度条上的版本号跟已下载的对不上。
#[tauri::command]
pub async fn check_update(app: AppHandle) -> AppResult<UpdateInfo> {
    let state = app.state::<UpdateState>();
    let current_version = app.package_info().version.to_string();

    {
        let inner = state.guard();
        if matches!(inner.stage, UpdateStage::Downloading | UpdateStage::Ready) {
            if let Some(update) = inner.update.as_ref() {
                return Ok(UpdateInfo {
                    current_version,
                    ..info_of(update)
                });
            }
        }
    }

    let update = app
        .updater()
        .map_err(|err| AppError::msg(format!("更新组件初始化失败: {err}")))?
        .check()
        .await
        .map_err(|err| AppError::msg(format!("检查更新失败: {err}")))?;

    let Some(update) = update else {
        return Ok(UpdateInfo {
            available: false,
            version: None,
            current_version,
            notes: None,
            date: None,
        });
    };

    let info = info_of(&update);
    let mut inner = state.guard();
    inner.update = Some(update);
    inner.stage = UpdateStage::Idle;
    inner.error = None;
    Ok(info)
}

/// 后台下载更新包。
///
/// 立刻返回，下载在独立任务里跑；完成后 `get_update_progress` 的阶段变为 `Ready`。
/// 重复调用是幂等的（正在下载或已就绪时直接返回成功），避免连点按钮起多个下载。
#[tauri::command]
pub async fn start_update_download(app: AppHandle) -> AppResult<()> {
    let update = {
        let state = app.state::<UpdateState>();
        let mut inner = state.guard();
        if matches!(
            inner.stage,
            UpdateStage::Downloading | UpdateStage::Ready | UpdateStage::Installing
        ) {
            return Ok(());
        }
        let update = inner
            .update
            .clone()
            .ok_or_else(|| AppError::msg("没有待下载的更新，请先检查更新"))?;
        inner.stage = UpdateStage::Downloading;
        inner.downloaded = 0;
        inner.total = None;
        inner.error = None;
        update
    };

    tauri::async_runtime::spawn(async move {
        let on_chunk = {
            let app = app.clone();
            move |chunk: usize, total: Option<u64>| {
                let state = app.state::<UpdateState>();
                let mut inner = state.guard();
                inner.downloaded += chunk as u64;
                if total.is_some() {
                    inner.total = total;
                }
            }
        };

        let result = update.download(on_chunk, || {}).await;

        let state = app.state::<UpdateState>();
        let mut inner = state.guard();
        match result {
            Ok(bytes) => {
                inner.bytes = Some(bytes);
                inner.stage = UpdateStage::Ready;
            }
            Err(err) => {
                inner.stage = UpdateStage::Failed;
                inner.error = Some(format!("更新包下载失败: {err}"));
            }
        }
    });

    Ok(())
}

/// 读取当前下载进度（前端轮询）。
#[tauri::command]
pub fn get_update_progress(state: tauri::State<UpdateState>) -> UpdateProgress {
    let inner = state.guard();
    UpdateProgress {
        stage: inner.stage,
        downloaded: inner.downloaded,
        total: inner.total,
        version: inner.update.as_ref().map(|update| update.version.clone()),
        error: inner.error.clone(),
    }
}

/// 安装已下载的更新并重启，重启后运行的才是新版本。
///
/// 平台差异：macOS / Linux 的 `install` 只把新版本写到磁盘，当前进程仍是旧版本，
/// 必须重启才生效；Windows 的 `install` 会拉起安装器并结束当前进程，
/// 下面这行重启通常执行不到，新版本由安装器的 `restart_after_install`（默认开）带起来。
#[tauri::command]
pub fn install_update_and_restart(app: AppHandle) -> AppResult<()> {
    let state = app.state::<UpdateState>();
    let (update, bytes) = {
        let mut inner = state.guard();
        let update = inner
            .update
            .clone()
            .ok_or_else(|| AppError::msg("没有待安装的更新"))?;
        let bytes = inner
            .bytes
            .take()
            .ok_or_else(|| AppError::msg("更新包尚未下载完成"))?;
        inner.stage = UpdateStage::Installing;
        (update, bytes)
    };

    if let Err(err) = update.install(&bytes) {
        // 安装失败（比如应用目录不可写）时把字节放回去，用户可以再试一次，不必重新下载
        let mut inner = state.guard();
        inner.bytes = Some(bytes);
        inner.stage = UpdateStage::Failed;
        inner.error = Some(format!("更新安装失败: {err}"));
        return Err(AppError::msg(format!("更新安装失败: {err}")));
    }

    app.restart();
}
