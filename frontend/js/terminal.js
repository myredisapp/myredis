/**
 * 底部面板的终端：命令转发 + 输出渲染 + 折叠。
 *
 * 终端面板是「终端 / 实时监控」双标签的容器，标签切换与监控视图在 monitor.js；
 * 本模块只管终端这一半（输出行、输入框、清空、折叠），并把 `appendTerminal`
 * 提供给其它模块（连接、Key 操作、监控都会往终端里写回显）。
 */
import { invoke } from './api.js';
import { escapeHtml } from './util.js';
import { getCurrentConn } from './state.js';

const terminalBody = document.getElementById('terminalBody');
const terminalInput = document.getElementById('terminalInput');
const terminalSend = document.getElementById('terminalSend');
const termToggleBtn = document.getElementById('termToggleBtn');
const termChevron = document.getElementById('termChevron');
const terminalPanel = document.getElementById('terminalPanel');

let terminalCollapsed = false;

/** 往终端追加一行（type: '' | 'ok' | 'error'）；text 由调用方决定是否已转义 */
export function appendTerminal(text, type = '') {
    const line = document.createElement('div');
    line.className = 'line';
    const cls = type === 'ok' ? 'output ok' : type === 'error' ? 'output error' : 'output';
    line.innerHTML = `<span class="prompt">></span><span class="${cls}">${text}</span>`;
    terminalBody.appendChild(line);
    terminalBody.scrollTop = terminalBody.scrollHeight;
}

export function isTerminalCollapsed() {
    return terminalCollapsed;
}

/** 终端折叠时同步隐藏其边界手柄 */
export function setTerminalCollapsed(collapsed) {
    terminalCollapsed = collapsed;
    terminalPanel.classList.toggle('collapsed', collapsed);
    termChevron.className = collapsed ? 'fas fa-chevron-up' : 'fas fa-chevron-down';
    document.getElementById('splitterTerminal').classList.toggle('hidden', collapsed);
}

function executeCommand(cmd) {
    const trimmed = cmd.trim();
    if (!trimmed) return;
    appendTerminal(escapeHtml(trimmed), '');
    terminalInput.value = '';
    if (trimmed.toLowerCase() === 'help') {
        appendTerminal('命令直接转发到已连接的 Redis 服务器执行，例如 PING / GET key / HGETALL key / INFO / DBSIZE。只读连接会拦截写命令。', '');
        terminalInput.focus();
        return;
    }
    const conn = getCurrentConn();
    if (!conn || !conn.id || !conn.online) {
        appendTerminal('未连接 Redis 服务器，请先建立连接', 'error');
        terminalInput.focus();
        return;
    }
    terminalSend.disabled = true;
    invoke('execute_command', { connId: conn.id, line: trimmed })
        .then((text) => {
            appendTerminal(escapeHtml(text || '(空结果)'), '');
        })
        .catch((err) => {
            appendTerminal(escapeHtml(String(err)), 'error');
        })
        .finally(() => {
            terminalSend.disabled = false;
            terminalInput.focus();
        });
}

export function initTerminal() {
    document.getElementById('terminalToggle').addEventListener('click', () => {
        setTerminalCollapsed(!terminalCollapsed);
    });
    termToggleBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        setTerminalCollapsed(!terminalCollapsed);
    });
    terminalSend.addEventListener('click', () => executeCommand(terminalInput.value));
    terminalInput.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') executeCommand(terminalInput.value);
    });
    document.getElementById('termClear').addEventListener('click', () => {
        terminalBody.innerHTML = `
            <div class="line"><span class="prompt">></span><span class="output ok">终端已清空</span></div>
        `;
    });
}
