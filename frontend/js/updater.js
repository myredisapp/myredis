/**
 * 静默更新：检查新版本、后台下载、进度条与两个弹窗、安装重启。
 *
 * 配合后端四个命令：check_update / start_update_download / get_update_progress /
 * install_update_and_restart。下载跑在 Rust 侧的后台任务里（Update::download 自带
 * 签名校验），这里只轮询进度画底部进度条，所以下载期间应用照常可用，
 * 甚至刷新页面也不会中断 —— 进度和已下好的安装包都存在后端状态里。
 */
import { invoke } from './api.js';
import { escapeHtml, fmtSize } from './util.js';
import { showToast } from './ui.js';

const UPDATE_POLL_MS = 500;
// 更新失败提示的停留时长：8 秒后自动收起，不一直杵在页面底部
const UPDATE_NOTICE_MS = 8000;
// 同一个版本只提示一次「发现新版本」，避免每次启动都打扰
const UPDATE_NOTICE_KEY = 'mc_cache_update_notified';
// 标题栏的更新按钮默认不显示，只有查到新版本才出现；
// 所以启动后要隔一阵再静默查一次，否则挂机久了发新版也看不到入口
const UPDATE_RECHECK_MS = 30 * 60 * 1000;

const updateBar = document.getElementById('updateBar');
const updateBarFill = document.getElementById('updateBarFill');
const updateBarIcon = document.getElementById('updateBarIcon');
const updateBarText = document.getElementById('updateBarText');
const updateBarPct = document.getElementById('updateBarPct');
const updateBarRestart = document.getElementById('updateBarRestart');
const updateBarDismiss = document.getElementById('updateBarDismiss');
const updateModal = document.getElementById('updateModal');
const updateModalTitle = document.getElementById('updateModalTitle');
const updateModalBody = document.getElementById('updateModalBody');
const updateModalOk = document.getElementById('updateModalOk');
const updateModalCancel = document.getElementById('updateModalCancel');
const updateDot = document.getElementById('updateDot');
const btnCheckUpdate = document.getElementById('btnCheckUpdate');

let updatePollTimer = null;
// 弹窗确认按钮该做什么：发现新版本时是「立即更新」，已就绪时是「立即重启」
let updateModalAction = null;
// 用户手动收起了进度条（新版本开始下载时重置）
let updateBarHidden = false;
// 本轮下载是否已弹过「更新已就绪」，防止重复弹窗
let updateReadyNotified = false;
// 失败提示的自动收起定时器
let updateFailTimer = null;

function showUpdateBar() {
    if (!updateBarHidden) updateBar.classList.add('open');
}

function clearUpdateFailTimer() {
    if (updateFailTimer) {
        clearTimeout(updateFailTimer);
        updateFailTimer = null;
    }
}

function hideUpdateBar() {
    clearUpdateFailTimer();
    updateBar.classList.remove('open', 'indeterminate', 'done', 'failed');
}

// 把后端返回的进度快照画到进度条上
function renderUpdateProgress(p) {
    // 任何一次新渲染都作废上一次失败提示的自动收起
    clearUpdateFailTimer();
    const known = p.total && p.total > 0;
    const pct = known ? Math.min(100, Math.round((p.downloaded / p.total) * 100)) : 0;

    updateBar.classList.toggle('indeterminate', p.stage === 'downloading' && !known);
    updateBar.classList.toggle('done', p.stage === 'ready');
    updateBar.classList.toggle('failed', p.stage === 'failed');

    if (p.stage === 'downloading') {
        showUpdateBar();
        updateBarIcon.className = 'fas fa-circle-notch fa-spin';
        updateBarIcon.style.color = '';
        updateBarText.textContent = '正在后台下载更新 v' + (p.version || '') + '…';
        updateBarPct.textContent = known
            ? pct + '%（' + fmtSize(p.downloaded) + ' / ' + fmtSize(p.total) + '）'
            : fmtSize(p.downloaded);
        updateBarFill.style.width = known ? pct + '%' : '';
        updateBarRestart.style.display = 'none';
        updateBarDismiss.style.display = 'none';
        return;
    }

    if (p.stage === 'ready') {
        showUpdateBar();
        updateBarIcon.className = 'fas fa-check-circle';
        updateBarIcon.style.color = 'var(--green)';
        updateBarText.textContent = '更新 v' + (p.version || '') + ' 已下载完成，重启后生效';
        updateBarPct.textContent = '';
        updateBarFill.style.width = '100%';
        updateBarRestart.style.display = '';
        updateBarDismiss.style.display = '';
        return;
    }

    if (p.stage === 'failed') {
        showUpdateBar();
        updateBarIcon.className = 'fas fa-exclamation-circle';
        updateBarIcon.style.color = 'var(--danger, #e5484d)';
        updateBarText.textContent = p.error || '更新失败';
        updateBarPct.textContent = '';
        updateBarFill.style.width = '100%';
        updateBarRestart.style.display = 'none';
        updateBarDismiss.style.display = '';
        // 失败原因不用一直留在页面上：8 秒后自动收起，也可以点 × 立刻收起。
        // 想重试就再点标题栏的检查更新按钮，会重新走一遍检查流程。
        updateFailTimer = setTimeout(hideUpdateBar, UPDATE_NOTICE_MS);
        return;
    }

    if (p.stage === 'installing') {
        // 正常路径下重启很快，这里主要覆盖「安装途中刷新了页面」的情况，
        // 让进度条继续说明现在在做什么，而不是突然消失
        showUpdateBar();
        updateBarIcon.className = 'fas fa-circle-notch fa-spin';
        updateBarIcon.style.color = '';
        updateBarText.textContent = '正在安装更新并重启…';
        updateBarPct.textContent = '';
        updateBarFill.style.width = '100%';
        updateBarRestart.style.display = 'none';
        updateBarDismiss.style.display = 'none';
        return;
    }

    hideUpdateBar();
}

function stopUpdatePolling() {
    if (updatePollTimer) {
        clearInterval(updatePollTimer);
        updatePollTimer = null;
    }
}

function pollUpdateProgress() {
    return invoke('get_update_progress').then(p => {
        renderUpdateProgress(p);
        if (p.stage === 'ready') {
            stopUpdatePolling();
            if (!updateReadyNotified) {
                updateReadyNotified = true;
                openUpdateReadyModal(p.version);
            }
        } else if (p.stage === 'failed') {
            stopUpdatePolling();
        }
        return p;
    }).catch(err => {
        // 单次轮询失败不打断下载，下一轮继续
        console.warn('读取更新进度失败', err);
    });
}

function startUpdatePolling() {
    stopUpdatePolling();
    pollUpdateProgress();
    updatePollTimer = setInterval(pollUpdateProgress, UPDATE_POLL_MS);
}

function closeUpdateModal() {
    updateModal.classList.remove('open');
    updateModalAction = null;
}

function showUpdateModal(title, bodyHtml, okText, action) {
    updateModalTitle.textContent = title;
    updateModalBody.innerHTML = bodyHtml;
    updateModalOk.textContent = okText;
    updateModalAction = action;
    updateModal.classList.add('open');
}

// 「发现新版本」弹窗：版本号 + 更新说明
function openUpdateAvailableModal(info) {
    let html = '<div>新版本 <b style="color: var(--accent);">v' + escapeHtml(info.version) + '</b>'
        + ' 已发布，当前版本 v' + escapeHtml(info.currentVersion) + '。</div>';
    if (info.date) {
        const when = new Date(info.date);
        // 日期解析失败就不显示，不影响更新本身
        if (!isNaN(when.getTime())) {
            html += '<div style="margin-top: 6px; color: var(--text-muted); font-size: 12px;">'
                + '发布于 ' + escapeHtml(when.toLocaleString('zh-CN')) + '</div>';
        }
    }
    if (info.notes) {
        html += '<div class="update-notes">' + escapeHtml(info.notes) + '</div>';
    }
    html += '<div style="margin-top: 12px; color: var(--text-muted);">'
        + '更新包会在后台下载，期间可以继续使用；下载完成后重启即可生效。</div>';
    showUpdateModal('发现新版本', html, '立即更新', startUpdateDownload);
}

// 「更新已就绪」弹窗：下载完成后的提示，重启即生效
function openUpdateReadyModal(version) {
    const html = '<div>新版本 <b style="color: var(--green);">v' + escapeHtml(version || '') + '</b>'
        + ' 已下载完成，重启应用即可完成更新。</div>'
        + '<div style="margin-top: 10px; color: var(--text-muted);">'
        + '重启不影响已保存的连接配置，正在编辑的内容请先保存。</div>';
    showUpdateModal('更新已就绪', html, '立即重启', installAndRestart);
}

/** silent=true 时只在标题栏显示更新入口并轻提示一次（启动时的静默检查） */
export function checkUpdate(silent) {
    return invoke('check_update').then(info => {
        // 没有新版本就不显示更新按钮，标题栏只留刷新 / 清库 / 新建连接
        btnCheckUpdate.classList.toggle('show', !!info.available);
        updateDot.classList.toggle('show', !!info.available);
        btnCheckUpdate.title = info.available
            ? '发现新版本 v' + info.version + '，点击更新'
            : '检查更新';
        if (info.available) {
            notifyUpdateOnce(info);
        } else if (!silent) {
            showToast('已是最新版本 v' + info.currentVersion, 'success');
        }
        return info;
    }).catch(err => {
        if (!silent) showToast('检查更新失败: ' + err, 'error', UPDATE_NOTICE_MS);
        else console.warn('静默检查更新失败', err);
        return null;
    });
}

function notifyUpdateOnce(info) {
    try {
        if (localStorage.getItem(UPDATE_NOTICE_KEY) === info.version) return;
        localStorage.setItem(UPDATE_NOTICE_KEY, info.version);
    } catch (e) { /* localStorage 不可用时每次都提示，可以接受 */ }
    showToast('发现新版本 v' + info.version + '，点击右上角下载', 'info');
}

function startUpdateDownload() {
    closeUpdateModal();
    updateBarHidden = false;
    updateReadyNotified = false;
    showUpdateBar();
    updateBarIcon.className = 'fas fa-circle-notch fa-spin';
    updateBarIcon.style.color = '';
    updateBarText.textContent = '正在准备下载…';
    invoke('start_update_download')
        .then(() => startUpdatePolling())
        .catch(err => {
            hideUpdateBar();
            showToast('开始下载失败: ' + err, 'error', UPDATE_NOTICE_MS);
        });
}

function installAndRestart() {
    closeUpdateModal();
    updateBarRestart.disabled = true;
    updateBarRestart.textContent = '正在重启…';
    updateBarText.textContent = '正在安装更新并重启…';
    invoke('install_update_and_restart').catch(err => {
        // 成功时进程会被新版本接管、不会有回调；能走到这说明安装失败
        updateBarRestart.disabled = false;
        updateBarRestart.textContent = '立即重启';
        // 后端已把阶段置为 failed，拉一次进度让进度条自己把失败原因显示出来
        //（失败提示 8 秒后自动收起，见 renderUpdateProgress）
        pollUpdateProgress();
        showToast('更新安装失败: ' + err, 'error', UPDATE_NOTICE_MS);
    });
}

export function initUpdate() {
    btnCheckUpdate.addEventListener('click', () => {
        btnCheckUpdate.disabled = true;
        // 安装包已经下好时直接给重启入口，不必再走一遍「发现新版本 → 立即更新」
        invoke('get_update_progress').catch(() => null).then(p => {
            if (p && p.stage === 'ready') {
                openUpdateReadyModal(p.version);
                return;
            }
            return checkUpdate(false).then(info => {
                if (info && info.available) openUpdateAvailableModal(info);
            });
        }).finally(() => { btnCheckUpdate.disabled = false; });
    });
    updateBarRestart.addEventListener('click', installAndRestart);
    updateBarDismiss.addEventListener('click', () => {
        updateBarHidden = true;
        hideUpdateBar();
    });
    updateModalOk.addEventListener('click', () => {
        const action = updateModalAction;
        closeUpdateModal();
        if (action) action();
    });
    updateModalCancel.addEventListener('click', closeUpdateModal);

    // 标题栏版本号取自安装包（不依赖网络），拿不到就保留占位符
    invoke('plugin:app|version')
        .then(v => { document.getElementById('appVersion').textContent = 'v' + v; })
        .catch(() => {});

    // 恢复进度条：下载可能在上一次页面加载时就开始了，进度存在后端
    invoke('get_update_progress').then(p => {
        renderUpdateProgress(p);
        if (p.stage === 'downloading') {
            startUpdatePolling();
        } else if (p.stage === 'ready') {
            // 本次启动前就下好了，保留进度条上的重启入口，不再弹窗打扰
            updateReadyNotified = true;
        }
    }).catch(() => {});

    // 启动后静默查一次：有新版只显示更新入口并轻提示，不弹窗挡操作
    setTimeout(() => checkUpdate(true), 1500);

    // 之后定时静默复查：入口平时是藏起来的，只有复查到新版本才会出现。
    // 下载中 / 已就绪时后端直接返回缓存结果，不会重复打网络请求。
    setInterval(() => checkUpdate(true), UPDATE_RECHECK_MS);
}
