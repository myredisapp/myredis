/**
 * 服务端信息（侧栏统计）、连接状态栏、逻辑数据库选择。
 *
 * 三者都跟着「当前连接」走：连接建立 / 切换 / 断开时由 connections.js 调用这里刷新。
 */
import { invoke } from './api.js';
import { formatAppBytes, formatDiskPercent } from './util.js';
import { getCurrentConn, notifyKeysChanged, setSelectedKey } from './state.js';
import { setWriteActionsDisabled, showToast } from './ui.js';
import { appendTerminal } from './terminal.js';
import { clearKeySearch, loadKeys } from './keys.js';

const statusDot = document.getElementById('statusDot');
const statusLabel = document.getElementById('statusLabel');
const dbSelectorWrap = document.getElementById('dbSelectorWrap');
const dbSelect = document.getElementById('dbSelect');

// Redis 逻辑数据库数量（默认 16 个：db0 - db15）
const DB_COUNT = 16;

/** 把服务端 INFO 的结果画到左侧统计栏 */
export function renderServerInfo(info) {
    if (!info) return;
    const setStat = (id, txt) => {
        const el = document.getElementById(id);
        if (el) el.textContent = txt;
    };

    // Key 总数
    const keys = info.dbKeys !== undefined ? Number(info.dbKeys).toLocaleString() : '-';
    setStat('statKeys', keys);

    // 磁盘空间使用率（Redis 持久化数据所在磁盘）
    const diskWrap = document.getElementById('statDiskItem');
    if (info.disk && info.disk.usedPercent !== undefined && info.disk.usedPercent !== null) {
        const pct = formatDiskPercent(info.disk.usedPercent);
        setStat('statDisk', pct);
        if (diskWrap) {
            diskWrap.title = info.disk.path
                ? `Redis 持久化目录: ${info.disk.path} · 磁盘已用 ${pct}`
                : '磁盘已用 ' + pct;
            diskWrap.style.cursor = 'help';
        }
    } else {
        setStat('statDisk', '-');
    }

    // 内存（B → 可读大小）
    const mem = info.usedMemory !== undefined ? formatAppBytes(Number(info.usedMemory)) : '-';
    setStat('statMem', mem);

    // 连接数
    setStat('statConn', info.connectedClients !== undefined ? String(info.connectedClients) : '-');
}

/** 标题栏的连接状态点 + 文案；只读连接附带「(只读)」并禁掉写入口 */
export function updateStatus() {
    const currentConn = getCurrentConn();
    const online = currentConn && currentConn.online;
    statusDot.className = 'dot' + (online ? '' : ' disconnected');
    let suffix = '';
    if (online && currentConn.readonly) suffix = ' (只读)';
    statusLabel.textContent = online ?
        `已连接 · ${currentConn.host}:${currentConn.port}${suffix}` :
        `未连接 · ${currentConn ? currentConn.host : '无'}`;
    setWriteActionsDisabled(!!(online && currentConn.readonly));
}

// 初始化数据库选择下拉选项（仅创建一次）
function initDbOptions() {
    if (!dbSelect) return;
    dbSelect.innerHTML = '';
    for (let i = 0; i < DB_COUNT; i++) {
        const opt = document.createElement('option');
        opt.value = String(i);
        opt.textContent = 'DB ' + i;
        dbSelect.appendChild(opt);
    }
}

/** 根据当前连接刷新数据库选择框的可用状态与选中值；集群只有 db0，直接隐藏选择器 */
export function refreshDbSelector() {
    const currentConn = getCurrentConn();
    if (!dbSelect) return;
    const isCluster = !!(currentConn && currentConn.type === 'cluster');
    if (dbSelectorWrap) dbSelectorWrap.style.display = isCluster ? 'none' : '';
    if (isCluster) return;
    const online = !!(currentConn && currentConn.online);
    dbSelect.disabled = !online;
    let current = 0;
    if (currentConn && currentConn.db !== undefined && currentConn.db !== null) {
        current = Number(currentConn.db);
    }
    if (dbSelect.value !== String(current)) dbSelect.value = String(current);
}

// 切换数据库（由下拉框 change 触发）
function onDbChange() {
    const currentConn = getCurrentConn();
    if (!currentConn || !currentConn.id) return;
    if (currentConn.type === 'cluster') {
        // 集群只有 db0，不支持切换
        refreshDbSelector();
        return;
    }
    if (!currentConn.online) {
        refreshDbSelector();
        return;
    }
    const db = parseInt(dbSelect.value, 10);
    if (isNaN(db) || (currentConn.db !== undefined && currentConn.db === db)) {
        refreshDbSelector();
        return;
    }
    dbSelect.disabled = true;
    appendTerminal(`正在切换到数据库 Db${db} …`, '');
    invoke('select_db', { connId: currentConn.id, db: db })
        .then((dbIndex) => {
            currentConn.db = dbIndex;
            // 逻辑数据库已更换，清空选中的 Key 与搜索条件
            setSelectedKey(null);
            clearKeySearch();
            notifyKeysChanged();
            appendTerminal(`已切换到数据库 Db${dbIndex}`, 'ok');
            // 刷新 key 列表与服务器信息
            const infoP = invoke('get_server_info', { connId: currentConn.id })
                .then((info) => { currentConn.info = info; renderServerInfo(info); return info; })
                .catch(() => {});
            return Promise.all([loadKeys(), infoP]);
        })
        .catch((err) => {
            appendTerminal(`切换数据库失败: ${err}`, 'error');
            showToast('切换数据库失败: ' + err, 'error');
        })
        .finally(() => {
            refreshDbSelector();
        });
}

export function initServerStatus() {
    initDbOptions();
    if (dbSelect) dbSelect.addEventListener('change', onDbChange);
}
