/**
 * 实时命令监控（MONITOR）+ 底部面板的双标签切换。
 *
 * 后端一条专用连接读 MONITOR 流，成批 emit `monitor:lines`；这里负责渲染、
 * 过滤、自动滚动、行数上限与会话状态。集群模式不支持（MONITOR 是节点级命令）。
 * 详见 `src-tauri/src/commands/monitor.rs` 与 DEVELOPMENT.md §2.4。
 */
import { invoke } from './api.js';
import { escapeHtml } from './util.js';
import { getCurrentConn } from './state.js';
import { showToast } from './ui.js';
import { appendTerminal, isTerminalCollapsed, setTerminalCollapsed } from './terminal.js';

// 显示上限：长时间盯着高 QPS 实例时 DOM 行数必须有界（超出的从最旧的行开始丢）
const MONITOR_MAX_ROWS = 2000;

const monitorBody = document.getElementById('monitorBody');
const monitorToggleBtn = document.getElementById('monitorToggle');
const monitorStateEl = document.getElementById('monitorState');
const monitorCountEl = document.getElementById('monitorCount');
const monitorFilterInput = document.getElementById('monitorFilter');
const monitorFollowBox = document.getElementById('monitorFollow');
const terminalPanel = document.getElementById('terminalPanel');

// 已收到的监控行（过滤、重绘都基于这份数据，因此它同样受上限约束）
let monitorLines = [];
// 因显示上限 / 后端缓冲上限被省略的行数
let monitorOmitted = 0;
// 当前监控会话：connId 是会话所属连接，notice 是最近一次结束原因 / 启动失败原因
let monitorSession = { connId: null, running: false, notice: '', noticeError: false };
// 程序触发的滚动不参与「用户滚上去就暂停跟随」的判断
let monitorAutoScroll = false;

// 监控视图的占位提示（无匹配行、已清空时显示）
function ensureMonitorPlaceholder(text) {
    if (monitorBody.querySelector('.mon-empty-node')) return;
    const el = document.createElement('div');
    el.className = 'monitor-empty mon-empty-node';
    el.textContent = text;
    monitorBody.appendChild(el);
}

function removeMonitorPlaceholder() {
    const el = monitorBody.querySelector('.mon-empty-node');
    if (el) el.remove();
}

function setMonitorState(text, kind) {
    monitorStateEl.className = 'monitor-state' + (kind ? ' ' + kind : '');
    const icon = kind === 'running' ? 'fa-circle-play' : (kind === 'error' ? 'fa-triangle-exclamation' : 'fa-circle-info');
    monitorStateEl.innerHTML = `<i class="fas ${icon}"></i> ${escapeHtml(text)}`;
}

// 服务器时间戳（秒.微秒）→ 本机时区的 HH:MM:SS.mmm（完整值在行的 title 里）
function formatMonitorTime(raw) {
    const dot = raw.indexOf('.');
    const seconds = parseFloat(dot > 0 ? raw.slice(0, dot) : raw);
    if (!isFinite(seconds)) return raw;
    const d = new Date(seconds * 1000);
    const pad = n => String(n).padStart(2, '0');
    const ms = dot > 0 ? raw.slice(dot + 1, dot + 4) : '000';
    return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${ms}`;
}

// 一行监控输出的 DOM（time / 客户端 / db / 命令 / 参数）
function buildMonitorRow(line) {
    const row = document.createElement('div');
    row.className = 'mon-row';
    row.title = line.raw;
    const cell = (cls, text) => {
        const span = document.createElement('span');
        span.className = cls;
        span.textContent = text;
        return span;
    };
    row.appendChild(cell('mon-time', formatMonitorTime(line.time)));
    row.appendChild(cell('mon-client', line.client));
    row.appendChild(cell('mon-db', 'db' + line.db));
    row.appendChild(cell('mon-cmd', line.command));
    row.appendChild(cell('mon-args', (line.args || []).join(' ')));
    return row;
}

// 过滤条件：对**原始行**做不区分大小写的包含匹配（key / 命令 / 客户端都能命中）
function monitorLineMatches(line) {
    const term = monitorFilterInput.value.trim().toLowerCase();
    if (!term) return true;
    return String(line.raw || '').toLowerCase().includes(term);
}

function updateMonitorCount() {
    const total = monitorLines.length + monitorOmitted;
    const shown = monitorFilterInput.value.trim()
        ? monitorLines.filter(monitorLineMatches).length
        : monitorLines.length;
    const omitted = monitorOmitted > 0 ? `（已省略 ${monitorOmitted} 行）` : '';
    monitorCountEl.textContent = `已显示 ${shown} / ${total} 行${omitted}`;
}

function scrollMonitorToBottom() {
    monitorAutoScroll = true;
    monitorBody.scrollTop = monitorBody.scrollHeight;
    requestAnimationFrame(() => { monitorAutoScroll = false; });
}

// 按当前过滤条件整体重绘（切过滤、清空后调用）
function renderMonitor(emptyText) {
    monitorBody.innerHTML = '';
    const matched = monitorLines.filter(monitorLineMatches);
    if (matched.length === 0) {
        ensureMonitorPlaceholder(emptyText || '没有匹配的命令');
    } else {
        const fragment = document.createDocumentFragment();
        matched.forEach(line => fragment.appendChild(buildMonitorRow(line)));
        monitorBody.appendChild(fragment);
    }
    updateMonitorCount();
    if (monitorFollowBox.checked) scrollMonitorToBottom();
}

// 追加一批行：只动新增的 DOM（2000 行时整块重绘太贵）
function appendMonitorLines(lines, dropped) {
    if (dropped > 0) monitorOmitted += dropped;
    const fragment = document.createDocumentFragment();
    for (const line of lines) {
        monitorLines.push(line);
        if (!monitorLineMatches(line)) continue;
        fragment.appendChild(buildMonitorRow(line));
    }
    if (fragment.childNodes.length > 0) {
        removeMonitorPlaceholder();
        monitorBody.appendChild(fragment);
    }
    // 超出上限：最旧的行从数据与 DOM 里一起丢掉（DOM 节点跟着数据走，
    // 因此这里不需要按过滤后的行数另算一次）
    const excess = monitorLines.length - MONITOR_MAX_ROWS;
    if (excess > 0) {
        monitorLines = monitorLines.slice(excess);
        monitorOmitted += excess;
        renderMonitor('没有匹配的命令');
        return;
    }
    if (monitorBody.childNodes.length === 0) ensureMonitorPlaceholder('没有匹配的命令');
    updateMonitorCount();
    if (monitorFollowBox.checked) scrollMonitorToBottom();
}

/** 清空监控输出（开始新会话、切连接、点清空时调用） */
function resetMonitorOutput(emptyText) {
    monitorLines = [];
    monitorOmitted = 0;
    monitorBody.innerHTML = '';
    ensureMonitorPlaceholder(emptyText || '监控中…新命令会实时出现在这里');
    updateMonitorCount();
}

// 本界面记录的会话是否就属于当前连接
function monitorOwnsCurrentConn() {
    const conn = getCurrentConn();
    return !!(conn && conn.id && monitorSession.connId === conn.id);
}

/** 按连接状态与会话状态刷新按钮 / 状态文案 */
export function syncMonitorControls() {
    const conn = getCurrentConn();
    const online = !!(conn && conn.id && conn.online);
    const cluster = !!(conn && conn.type === 'cluster');
    const running = monitorOwnsCurrentConn() && monitorSession.running;

    monitorToggleBtn.disabled = !online || cluster;
    monitorToggleBtn.title = !online
        ? '请先连接 Redis 服务器'
        : (cluster ? '集群模式不支持实时监控（MONITOR 是节点级命令），请用单机模式直连要观察的节点' : '');
    monitorToggleBtn.innerHTML = running
        ? '<i class="fas fa-stop"></i> 停止监控'
        : '<i class="fas fa-play"></i> 开始监控';
    monitorToggleBtn.classList.toggle('primary', !running);

    if (running) {
        setMonitorState('监控中', 'running');
    } else if (monitorOwnsCurrentConn() && monitorSession.notice) {
        setMonitorState(monitorSession.notice, monitorSession.noticeError ? 'error' : '');
    } else if (!online) {
        setMonitorState('未连接', '');
    } else if (cluster) {
        setMonitorState('集群模式不支持', '');
    } else {
        setMonitorState('未开始', '');
    }
}

/** 把界面上的监控会话复位（断开 / 删除 / 切换连接时调用；后端会一并停掉监控） */
export function resetMonitorSession(emptyText) {
    monitorSession = { connId: null, running: false, notice: '', noticeError: false };
    resetMonitorOutput(emptyText);
    syncMonitorControls();
}

// 开始 / 停止监控（同一个按钮）
function toggleMonitor() {
    const conn = getCurrentConn();
    if (!conn || !conn.id || !conn.online) {
        showToast('请先连接 Redis 服务器', 'error');
        return;
    }
    const connId = conn.id;
    const running = monitorOwnsCurrentConn() && monitorSession.running;
    monitorToggleBtn.disabled = true;
    const done = () => {
        monitorToggleBtn.disabled = false;
        syncMonitorControls();
    };

    if (running) {
        // 停止：不发「已停止」通知，结束后端会推 monitor:end（stoppedByUser）
        invoke('stop_monitor', { connId })
            .then((stopped) => {
                monitorSession.running = false;
                monitorSession.notice = '已停止监控';
                monitorSession.noticeError = false;
                appendTerminal(stopped ? '已停止实时监控' : '监控会话已自行结束（连接可能已断开）', stopped ? '' : 'error');
            })
            .catch(err => {
                monitorSession.notice = '停止失败: ' + err;
                monitorSession.noticeError = true;
                showToast('停止监控失败: ' + err, 'error');
            })
            .finally(done);
        return;
    }

    // 开始：先清空上一段输出，避免新旧会话的行混在一起
    resetMonitorOutput('监控中…新命令会实时出现在这里');
    monitorSession = { connId, running: false, notice: '', noticeError: false };
    invoke('start_monitor', { connId })
        .then(() => {
            monitorSession = { connId, running: true, notice: '', noticeError: false };
            appendTerminal(`开始监控 ${connId} 上执行的命令（MONITOR）`, 'ok');
            showToast('已开始实时监控', 'success');
        })
        .catch(err => {
            monitorSession = { connId, running: false, notice: '启动失败: ' + err, noticeError: true };
            ensureMonitorPlaceholder('启动失败: ' + err);
            showToast('开始监控失败: ' + err, 'error');
        })
        .finally(done);
}

// 以**后端**为准恢复会话状态（切回监控标签、刚连上时调用）：
// 界面刷新或切走再切回来时，后端可能仍在监控
function refreshMonitorStatus() {
    const conn = getCurrentConn();
    if (!conn || !conn.id || !conn.online) {
        syncMonitorControls();
        return Promise.resolve();
    }
    const connId = conn.id;
    return invoke('monitor_status', { connId })
        .then((running) => {
            const now = getCurrentConn();
            if (!now || now.id !== connId) return;
            // 后端在跑但界面没有这份会话（例如刷新过界面）：接上它，别让按钮说谎
            if (running && !monitorOwnsCurrentConn()) {
                monitorSession = { connId, running: true, notice: '', noticeError: false };
            } else if (!running && monitorOwnsCurrentConn()) {
                monitorSession.running = false;
            }
            syncMonitorControls();
        })
        .catch(() => { syncMonitorControls(); });
}

// 后端推来的一批监控行
function handleMonitorLines(event) {
    const payload = event && event.payload;
    const conn = getCurrentConn();
    if (!payload || !conn || payload.connId !== conn.id) return;
    if (!monitorOwnsCurrentConn()) return;
    appendMonitorLines(payload.lines || [], payload.dropped || 0);
}

// 监控结束（用户停止 / 连接断开 / 读流中断）
function handleMonitorEnd(event) {
    const payload = event && event.payload;
    if (!payload || !monitorOwnsCurrentConn() || payload.connId !== monitorSession.connId) return;
    monitorSession.running = false;
    monitorSession.notice = payload.reason || (payload.stoppedByUser ? '已停止监控' : '监控已结束');
    monitorSession.noticeError = !payload.stoppedByUser;
    appendTerminal('监控结束: ' + monitorSession.notice, payload.stoppedByUser ? '' : 'error');
    if (!payload.stoppedByUser) showToast('监控已结束: ' + monitorSession.notice, 'error');
    syncMonitorControls();
}

/**
 * 切到别的连接前，把还在跑的监控会话停掉
 *（后端会推 monitor:end，但那时界面已复位，事件会被忽略 —— 正是我们要的）
 */
export function stopMonitorForOtherConn(nextConnId) {
    if (!monitorSession.running || !monitorSession.connId) return;
    if (monitorSession.connId === nextConnId) return;
    invoke('stop_monitor', { connId: monitorSession.connId })
        .catch(err => console.warn('停止上一个连接的监控失败:', err));
}

// 切换底部面板的标签页（终端 / 实时监控）
function setBottomTab(tab) {
    const isMonitor = tab === 'monitor';
    terminalPanel.classList.toggle('monitor-active', isMonitor);
    document.getElementById('tabTerminal').classList.toggle('active', !isMonitor);
    document.getElementById('tabMonitor').classList.toggle('active', isMonitor);
    if (!isMonitor) return;
    // 折叠状态下切到监控看不出内容，顺手展开
    if (isTerminalCollapsed()) setTerminalCollapsed(false);
    syncMonitorControls();
    refreshMonitorStatus();
}

export function initMonitor() {
    // 标签页在标题栏里，而标题栏整体是「折叠/展开」的点击区，
    // 因此这里要拦掉冒泡（与折叠按钮同样的处理）
    document.getElementById('tabTerminal').addEventListener('click', (e) => {
        e.stopPropagation();
        setBottomTab('terminal');
    });
    document.getElementById('tabMonitor').addEventListener('click', (e) => {
        e.stopPropagation();
        setBottomTab('monitor');
    });
    monitorToggleBtn.addEventListener('click', toggleMonitor);
    document.getElementById('monitorClear').addEventListener('click', () => {
        resetMonitorOutput('已清空。继续监控中，新命令会实时出现');
    });
    monitorFilterInput.addEventListener('input', () => {
        renderMonitor('没有匹配的命令');
    });
    monitorFollowBox.addEventListener('change', () => {
        if (monitorFollowBox.checked) scrollMonitorToBottom();
    });
    // 用户往上翻看历史时自动暂停跟随（滚回底部再自动恢复）：
    // 否则新行进来会把正在看的内容顶走
    monitorBody.addEventListener('scroll', () => {
        if (monitorAutoScroll) return;
        const atBottom = monitorBody.scrollHeight - monitorBody.scrollTop - monitorBody.clientHeight < 24;
        if (!atBottom && monitorFollowBox.checked) {
            monitorFollowBox.checked = false;
        } else if (atBottom && !monitorFollowBox.checked) {
            monitorFollowBox.checked = true;
        }
    });
    // 首屏就把监控按钮摆到正确状态（未连接时禁用）
    syncMonitorControls();
}

/** 后端事件（monitor:lines / monitor:end）由 main.js 用 listenEvent 注册到这里 */
export { handleMonitorLines, handleMonitorEnd };
