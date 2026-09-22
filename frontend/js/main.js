/**
 * 入口：装配各模块、跑初始化、注册后端事件。
 *
 * 这里能看到全局的启动顺序与「谁订阅了谁」：
 * - `onKeysChanged` → Key 树 + 详情区一起重绘（改数据的模块只管发通知，不直接调对方）；
 * - 后端菜单事件（主题 / 检查更新）与监控事件在这里与具体模块接上。
 */
import { invoke, listenEvent } from './api.js';
import { getAllKeys, getCurrentConn, getKeyData, onKeysChanged } from './state.js';
import { initUi } from './ui.js';
import { initTheme, setThemeFromMenu } from './theme.js';
import { applyLayout, initLayout } from './layout.js';
import { appendTerminal, initTerminal } from './terminal.js';
import { handleMonitorEnd, handleMonitorLines, initMonitor } from './monitor.js';
import { initKeys, renderKeyTree } from './keys.js';
import { renderDetail } from './detail.js';
import { initKeyOps } from './keyops.js';
import { initAddKey } from './addkey.js';
import { initConnections, loadConnections, renderConnections } from './connections.js';
import { initServerStatus, refreshDbSelector, renderServerInfo, updateStatus } from './server-status.js';
import { initConnForm } from './conn-form.js';
import { initImportExport } from './import-export.js';
import { checkUpdate, initUpdate } from './updater.js';

// ---------- 后端事件 ----------

function registerBackendEvents() {
    listenEvent('menu:set-theme', (e) => setThemeFromMenu(e && e.payload));
    listenEvent('menu:check-update', () => checkUpdate(false));
    // 实时监控：成批的命令行 + 结束通知（见 src-tauri/src/commands/monitor.rs）
    listenEvent('monitor:lines', handleMonitorLines);
    listenEvent('monitor:end', handleMonitorEnd);
}

// ---------- 初始化 ----------

function init() {
    // 视图之间的订阅：改 Key 数据的模块（详情 / 重命名 / 新增）只管发通知，
    // 由这里决定要重绘哪两块
    onKeysChanged(() => {
        renderKeyTree();
        renderDetail();
    });

    initUi();
    initLayout();
    initTheme();
    initTerminal();
    initMonitor();
    initKeys();
    initKeyOps();
    initAddKey();
    initConnForm();
    initConnections();
    initServerStatus();
    initImportExport();

    // 首屏：先把布局与空态摆好，再去后端拉连接列表
    applyLayout();
    refreshDbSelector();
    renderConnections();
    renderKeyTree();
    renderDetail();
    loadConnections();
    updateStatus();
    initUpdate();

    registerBackendEvents();

    setTimeout(() => {
        appendTerminal('欢迎使用 麦地缓存', 'ok');
        appendTerminal('输入 HELP 查看可用命令', '');
    }, 300);

    // 定时刷新服务器信息（仅当已连接时）
    setInterval(() => {
        const conn = getCurrentConn();
        if (conn && conn.id && conn.online) {
            invoke('get_server_info', { connId: conn.id })
                .then(renderServerInfo)
                .catch(() => { /* 忽略轮询错误，下次重试 */ });
        }
    }, 5000);

    // 调试/自动化钩子：allKeys 与 KEY_DATA 会被分页重置重新赋值，这里用取值器
    // （scripts/frontend-smoke.mjs 也用这个口子做断言）
    window.__app = {
        get allKeys() { return getAllKeys(); },
        get KEY_DATA() { return getKeyData(); },
        renderKeyTree,
        renderDetail,
    };
}

init();
