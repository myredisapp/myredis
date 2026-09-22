/**
 * 连接对话框：新建 / 编辑 / 复制（预填）/ 测试连接。
 *
 * 保存成功后要「重新加载连接列表 + 若是当前连接则重连」，那是 connections.js 的事，
 * 所以这里不 import 它，而是由调用方传 `onSaved` 回调进来（避免两个模块互相依赖）。
 */
import { invoke } from './api.js';
import { showToast } from './ui.js';

const modal = document.getElementById('connectionModal');
const modalTitle = document.getElementById('modalTitle');
const connName = document.getElementById('connName');
const connHost = document.getElementById('connHost');
const connPort = document.getElementById('connPort');
const connUser = document.getElementById('connUser');
const connPass = document.getElementById('connPass');
const connReadonly = document.getElementById('connReadonly');
const connCluster = document.getElementById('connCluster');
const connSeparator = document.getElementById('connSeparator');
const connTls = document.getElementById('connTls');
const connTlsInsecure = document.getElementById('connTlsInsecure');
const connConnectTimeout = document.getElementById('connConnectTimeout');
const connCommandTimeout = document.getElementById('connCommandTimeout');

// 正在编辑的连接 id；null 表示「新建」（保存时会生成新 id）
let editingConnId = null;
// 保存成功后的回调（由 connections.js 注入）
let onSavedCallback = null;

function closeModal() {
    modal.classList.remove('open');
    showTestStatus('', '');
}

// 连接对话框里 TLS / 超时两个可选项的收集：空或非法一律回落默认（传 null）
function collectConnExtra() {
    const num = (id) => {
        const v = parseInt(document.getElementById(id).value, 10);
        return Number.isFinite(v) && v > 0 ? v : null;
    };
    return {
        tls: connTls.checked,
        tls_insecure: connTlsInsecure.checked,
        connect_timeout_secs: num('connConnectTimeout'),
        command_timeout_secs: num('connCommandTimeout'),
    };
}

// 只有勾选 TLS 才显示「跳过证书校验」
function syncTlsInsecureVisibility() {
    document.getElementById('connTlsInsecureWrap').style.visibility =
        connTls.checked ? 'visible' : 'hidden';
}

// 主机字段里填了带协议的地址（如 rediss://host）时给出提示。
// 后端在保存 / 导入 / 建连三处也会拦（校验在 models/connection.rs），
// 这里先拦一次只是省一趟往返、并把提示显示在用户正看着的位置。
function hostSchemeError(host) {
    const m = /^([A-Za-z][A-Za-z0-9+.-]*):\/\//.exec((host || '').trim());
    if (!m) return '';
    const scheme = m[1].toLowerCase();
    if (scheme === 'rediss' || scheme === 'tls' || scheme === 'ssl') {
        // 勾选 TLS 后由后端剥掉前缀；带账号 / 端口 / 路径的仍要拦（后端也会拦）
        if (connTls.checked) {
            const rest = m.input.slice(m[0].length);
            if (!rest.includes('@') && !rest.includes('/') && !rest.includes(':')) return '';
        }
        return '主机字段检测到 rediss:// 前缀：如需 TLS 加密连接，请勾选「TLS 加密」，主机字段只需填主机名';
    }
    return `主机字段只需填主机名（如 127.0.0.1），不要带 ${scheme}:// 前缀`;
}

function showTestStatus(msg, type) {
    const el = document.getElementById('testStatus');
    el.textContent = msg;
    el.className = type === 'success' ? 'success' : type === 'error' ? 'error' : '';
}

/**
 * 打开连接对话框。
 *
 * - `conn`：编辑已有连接；为 null 时是新建。
 * - `prefill`：新建时预填部分字段（「复制」菜单项用）。
 * - `onSaved`：保存成功后的回调（重新加载连接列表 / 重连当前连接）。
 */
export function openModal(conn = null, prefill = null, onSaved = null) {
    onSavedCallback = onSaved;
    if (conn) {
        editingConnId = conn.id;
        modalTitle.textContent = '编辑连接';
        connName.value = conn.name;
        connHost.value = conn.host;
        connPort.value = conn.port;
        connUser.value = conn.username || '';
        connPass.value = conn.password || '';
        connReadonly.checked = !!conn.readonly;
        connCluster.checked = conn.type === 'cluster';
        connSeparator.value = conn.separator || '';
        connTls.checked = !!conn.tls;
        connTlsInsecure.checked = !!conn.tlsInsecure;
        connConnectTimeout.value = conn.connectTimeoutSecs || '';
        connCommandTimeout.value = conn.commandTimeoutSecs || '';
        syncTlsInsecureVisibility();
    } else {
        editingConnId = null;
        modalTitle.textContent = '新建连接';
        connName.value = '';
        connHost.value = '127.0.0.1';
        connPort.value = '6379';
        connUser.value = '';
        connPass.value = '';
        connReadonly.checked = false;
        connCluster.checked = false;
        connSeparator.value = '';
        connTls.checked = false;
        connTlsInsecure.checked = false;
        connConnectTimeout.value = '';
        connCommandTimeout.value = '';
        syncTlsInsecureVisibility();
        // 「复制连接」：沿用源连接的可编辑字段（id 不复制，保存时生成新的）
        if (prefill) {
            connName.value = prefill.name + ' 副本';
            if (prefill.host) connHost.value = prefill.host;
            if (prefill.port) connPort.value = prefill.port;
            if (prefill.username) connUser.value = prefill.username;
            if (prefill.password) connPass.value = prefill.password;
            if (prefill.readonly) connReadonly.checked = true;
            if (prefill.type === 'cluster') connCluster.checked = true;
            if (prefill.separator) connSeparator.value = prefill.separator;
        }
    }
    showTestStatus('', '');
    modal.classList.add('open');
    connName.focus();
}

function saveConnection() {
    const name = connName.value.trim();
    const host = connHost.value.trim();
    const port = parseInt(connPort.value) || 6379;
    if (!name || !host) {
        showToast('请填写名称和主机', 'error');
        return;
    }
    const hostError = hostSchemeError(host);
    if (hostError) {
        showToast(hostError, 'error');
        return;
    }
    const isEdit = !!editingConnId;
    const id = editingConnId || ('conn_' + Date.now());
    const separator = connSeparator.value;
    const conn = {
        id: id,
        name: name,
        host: host,
        port: port,
        type: connCluster.checked ? 'cluster' : 'single',
        readonly: connReadonly.checked,
        separator: separator || ':',
        username: connUser.value || null,
        password: connPass.value || null,
        ...collectConnExtra(),
    };
    invoke('save_connection', { conn }).then(() => {
        closeModal();
        showToast(isEdit ? '连接已更新' : '连接已创建', 'success');
        if (onSavedCallback) onSavedCallback();
    }).catch(err => {
        showToast('保存失败: ' + err, 'error');
    });
}

function testConnection() {
    const host = connHost.value.trim();
    const port = parseInt(connPort.value) || 0;
    if (!host || !port) {
        showTestStatus('请填写主机和端口', 'error');
        return;
    }
    const hostError = hostSchemeError(host);
    if (hostError) {
        showTestStatus('✗ ' + hostError, 'error');
        return;
    }

    const btn = document.getElementById('modalTest');
    const originalText = btn.textContent;
    btn.disabled = true;
    btn.textContent = '测试中…';
    showTestStatus('正在测试连接…', 'info');

    const conn = {
        id: editingConnId || ('test_' + Date.now()),
        name: connName.value.trim() || '测试连接',
        host: host,
        port: port,
        type: connCluster.checked ? 'cluster' : 'single',
        readonly: connReadonly.checked,
        separator: connSeparator.value || ':',
        username: connUser.value || null,
        password: connPass.value || null,
        ...collectConnExtra(),
    };

    invoke('test_connection', { conn })
        .then(pong => {
            showTestStatus('✓ 连接成功（' + pong + '）', 'success');
        })
        .catch(err => {
            showTestStatus('✗ ' + err, 'error');
        })
        .finally(() => {
            btn.disabled = false;
            btn.textContent = originalText;
        });
}

export function initConnForm() {
    document.getElementById('modalTest').addEventListener('click', testConnection);
    document.getElementById('modalCancel').addEventListener('click', closeModal);
    document.getElementById('modalSave').addEventListener('click', saveConnection);
    modal.addEventListener('click', (e) => {
        if (e.target === modal) closeModal();
    });

    [connName, connHost, connPort, connUser, connPass, connSeparator].forEach(el => {
        if (el) el.addEventListener('input', () => showTestStatus('', ''));
    });
    connReadonly.addEventListener('change', () => showTestStatus('', ''));
    connCluster.addEventListener('change', () => showTestStatus('', ''));
    connTls.addEventListener('change', () => {
        syncTlsInsecureVisibility();
        if (!connTls.checked) connTlsInsecure.checked = false;
        showTestStatus('', '');
    });
    document.addEventListener('keydown', (e) => {
        if (e.key === 'Escape') closeModal();
        if (e.key === 'Enter' && modal.classList.contains('open')) saveConnection();
    });
}
