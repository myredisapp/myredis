/**
 * 连接管理：连接列表、建连 / 切换 / 断开 / 删除、服务端信息与状态栏、数据库选择。
 *
 * 「连接」在前端是两件事：一条本地配置（CONNECTIONS，来自后端 list_connections）
 * 与一个已建立的会话（后端连接池里的句柄，由 connect / disconnect 维护）。
 * `onlineConns` 记录哪些配置当前在线；多个连接可以同时在线，但只有 `currentConn`
 * 是界面正在展示的那个。
 */
import { invoke } from './api.js';
import { formatAppBytes } from './util.js';
import {
    findConnection, getConnections, getCurrentConn, isOnline, markOffline, markOnline,
    notifyKeysChanged, setConnections, setCurrentConn, setSelectedKey,
} from './state.js';
import { showConfirm, showToast } from './ui.js';
import { appendTerminal } from './terminal.js';
import { loadKeys, resetKeyList } from './keys.js';
import { resetMonitorSession, stopMonitorForOtherConn, syncMonitorControls } from './monitor.js';
import { openModal } from './conn-form.js';
import { refreshDbSelector, renderServerInfo, updateStatus } from './server-status.js';

const connectionListEl = document.getElementById('connectionList');

// ---------- 渲染：连接列表 ----------

export function renderConnections() {
    const CONNECTIONS = getConnections();
    const currentConn = getCurrentConn();
    if (!CONNECTIONS || CONNECTIONS.length === 0) {
        connectionListEl.innerHTML = `
            <div style="padding:20px 12px;color:var(--text-muted);text-align:center;font-size:13px;">
                <i class="fas fa-plug" style="display:block;font-size:22px;margin-bottom:8px;opacity:.4;"></i>
                暂无连接<br/>点击右上角「新建连接」
            </div>`;
        return;
    }
    connectionListEl.innerHTML = CONNECTIONS.map(conn => {
        const isActive = currentConn && conn.id === currentConn.id;
        const online = isOnline(conn.id);
        return `
        <div class="connection-item ${isActive ? 'active' : ''}" data-id="${conn.id}">
            <div class="icon"><i class="fas fa-server"></i></div>
            <div class="info">
                <div class="name">${conn.name}</div>
                <div class="host">${conn.host}:${conn.port}</div>
            </div>
            <span class="badge ${online ? 'online' : ''}">${online ? '● 在线' : '○ 离线'}</span>
            <div class="dropdown-container">
                <div class="actions">
                    <button class="icon-btn refresh-conn" data-id="${conn.id}" title="刷新"><i class="fas fa-sync-alt"></i></button>
                    <button class="icon-btn conn-menu-btn" data-id="${conn.id}" title="更多操作"><i class="fas fa-ellipsis-v"></i></button>
                </div>
                <div class="conn-dropdown-menu" data-id="${conn.id}">
                    <button class="dd-item" data-action="edit"><i class="fas fa-pen"></i>编辑</button>
                    <button class="dd-item" data-action="duplicate"><i class="fas fa-copy"></i>复制</button>
                    <button class="dd-item" data-action="disconnect" ${online ? '' : 'disabled'} style="${online ? '' : 'opacity:.4;cursor:not-allowed;'}"><i class="fas fa-plug"></i>关闭连接</button>
                    <button class="dd-item" data-action="delete"><i class="fas fa-trash-alt"></i>删除</button>
                </div>
            </div>
        </div>`;
    }).join('');

    // 事件绑定
    connectionListEl.querySelectorAll('.connection-item').forEach(el => {
        el.addEventListener('click', (e) => {
            if (e.target.closest('.actions')) return;
            const id = el.dataset.id;
            const conn = findConnection(id);
            if (conn) switchConnection(conn);
        });
    });
    // 刷新图标
    connectionListEl.querySelectorAll('.refresh-conn').forEach(btn => {
        btn.addEventListener('click', (e) => {
            e.stopPropagation();
            const id = btn.dataset.id;
            const conn = findConnection(id);
            if (conn) switchConnection(conn);
        });
    });
    // 菜单按钮：点击切换下拉
    connectionListEl.querySelectorAll('.conn-menu-btn').forEach(btn => {
        btn.addEventListener('click', (e) => {
            e.stopPropagation();
            const id = btn.dataset.id;
            // 关闭其他下拉
            document.querySelectorAll('.conn-dropdown-menu.show').forEach(m => {
                if (m.dataset.id !== id) m.classList.remove('show');
            });
            const menu = connectionListEl.querySelector(`.conn-dropdown-menu[data-id="${id}"]`);
            if (menu) menu.classList.toggle('show');
        });
    });
    // 下拉菜单按钮
    connectionListEl.querySelectorAll('.conn-dropdown-menu .dd-item').forEach(item => {
        item.addEventListener('click', (e) => {
            e.stopPropagation();
            const menu = item.closest('.conn-dropdown-menu');
            if (menu) menu.classList.remove('show');
            const id = menu.dataset.id;
            const conn = findConnection(id);
            const action = item.dataset.action;
            if (!conn) return;
            if (action === 'edit') {
                openConnectionModal(conn);
            } else if (action === 'duplicate') {
                // 复制：预填源连接的可编辑字段，保存时生成新 id
                openConnectionModal(null, conn);
            } else if (action === 'disconnect') {
                if (!isOnline(id)) return; // 已离线，忽略
                invoke('disconnect', { connId: id }).then(() => {
                    markOffline(id);
                    const current = getCurrentConn();
                    if (current && current.id === id) {
                        current.online = false;
                        resetKeyList();
                        setSelectedKey(null);
                        notifyKeysChanged();
                        // 断开时后端会停掉该连接的监控：界面同步复位
                        resetMonitorSession('未连接。连接 Redis 后可开始监控');
                    }
                    renderConnections();
                    updateStatus();
                    refreshDbSelector();
                    appendTerminal(`已关闭连接 ${conn.name}`, '');
                }).catch(err => {
                    showToast('关闭连接失败: ' + err, 'error');
                });
            } else if (action === 'delete') {
                showConfirm(`确定要删除连接「${conn.name}」吗？`, { title: '删除连接' }).then((ok) => {
                    if (!ok) return;
                    invoke('delete_connection', { connId: id }).then((deleted) => {
                        if (deleted === false) {
                            showToast('未找到该连接，可能已被删除', 'error');
                        }
                        markOffline(id);
                        const current = getCurrentConn();
                        if (current && current.id === id) {
                            setCurrentConn(null);
                            // 删除连接时后端也会停掉该连接的监控（密钥链条目同理）
                            resetMonitorSession('未连接。连接 Redis 后可开始监控');
                        }
                        return loadConnections();
                    }).catch(err => {
                        showToast('删除失败: ' + err, 'error');
                    });
                });
            }
        });
    });
    // 点击空白处关闭下拉
    if (!window._connMenuListener) {
        window._connMenuListener = true;
        document.addEventListener('click', () => {
            document.querySelectorAll('.conn-dropdown-menu.show').forEach(m => m.classList.remove('show'));
        });
    }
}

// ---------- 连接对话框（保存后：刷新列表 + 重连当前连接） ----------

/**
 * 打开连接对话框。
 * @param {object|null} conn 编辑已有连接；null 为新建
 * @param {object|null} prefill 新建时预填字段（「复制连接」用）
 */
function openConnectionModal(conn = null, prefill = null) {
    openModal(conn, prefill, () => {
        loadConnections().then(() => {
            const current = getCurrentConn();
            if (current) {
                const updated = findConnection(current.id);
                if (updated) switchConnection(updated);
            }
        });
    });
}

// ---------- 建连 / 切换 ----------

function connectTo(conn) {
    if (!conn) return Promise.reject(new Error('无连接信息'));
    appendTerminal(`正在连接 ${conn.name} (${conn.host}:${conn.port}) …`, '');
    // 直接建立连接（connect 内部会替换连接池中同 id 的连接）
    return invoke('connect', {
        conn: {
            id: conn.id,
            name: conn.name,
            host: conn.host,
            port: conn.port,
            type: conn.type || 'single',
            readonly: !!conn.readonly,
            separator: conn.separator || ':',
            db: (conn.db !== undefined && conn.db !== null) ? conn.db : 0,
            username: conn.username || null,
            password: conn.password || null,
            tls: !!conn.tls,
            tls_insecure: !!conn.tlsInsecure,
            connect_timeout_secs: conn.connectTimeoutSecs || null,
            command_timeout_secs: conn.commandTimeoutSecs || null,
        }
    }).then(() => {
        if (conn && conn.id) markOnline(conn.id);
        setCurrentConn({ ...conn, online: true });
        return invoke('get_server_info', { connId: conn.id });
    }).then((info) => {
        getCurrentConn().info = info;
        return info;
    });
}

function switchConnection(conn) {
    if (!conn) return;
    // 上一个连接若正在监控，先停掉：监控只跟随当前连接展示，
    // 留在后台跑既看不到又白占一条服务器连接
    stopMonitorForOtherConn(conn.id);
    // 初始化逻辑数据库编号（旧连接配置可能没有 db 字段）
    if (conn.db === undefined || conn.db === null) conn.db = 0;
    setCurrentConn(conn);
    renderConnections();
    updateStatus();
    // 切换连接时先清掉上一个连接的列表与分页游标，避免串数据
    resetKeyList();
    // 选中的 Key 属于上一个连接，必须一起清掉：否则详情区会拿空列表
    // 去查它，误报「Key 不存在」
    setSelectedKey(null);
    notifyKeysChanged();
    // 监控输出属于上一个连接，一并清掉并复位状态
    resetMonitorSession('点「开始监控」后，这里会实时显示服务器上执行的每条命令（MONITOR）');
    refreshDbSelector();
    connectTo(conn).then((info) => {
        if (info) {
            renderServerInfo(info);
            // 更新连接状态为「已连接」并刷新底栏和左侧徽章
            getCurrentConn().online = true;
            markOnline(conn.id);
            renderConnections();
            updateStatus();
            refreshDbSelector();
            // 以连接时取回的版本信息刷新一次监控按钮（集群模式不支持监控）
            syncMonitorControls();
            const mem = info.usedMemory ? formatAppBytes(Number(info.usedMemory)) : '-';
            const days = info.uptimeSeconds > 0 ? Math.floor(info.uptimeSeconds / 86400) : 0;
            const hrs = info.uptimeSeconds > 0 ? Math.floor((info.uptimeSeconds % 86400) / 3600) : 0;
            appendTerminal(
                `连接成功 · Redis ${info.redisVersion || ''} · 运行 ${days}天${hrs}时 · 内存 ${mem} · 连接 ${info.connectedClients || 0} · Keys ${info.dbKeys || 0}`,
                'ok'
            );
            // 连接成功后加载真实 key 列表
            loadKeys();
        }
    }).catch(err => {
        const current = getCurrentConn();
        if (current) {
            current.online = false;
            markOffline(current.id);
        }
        renderConnections();
        updateStatus();
        appendTerminal(`连接失败: ${err}`, 'error');
        showToast('连接失败: ' + err, 'error');
    });
}

// ---------- 数据加载（从后端） ----------

export function loadConnections() {
    return invoke('list_connections').then(list => {
        setConnections((list || []).map(c => ({
            id: c.id,
            name: c.name,
            host: c.host,
            port: c.port,
            type: c.type || 'single',
            readonly: !!c.readonly,
            separator: c.separator || ':',
            username: c.username || '',
            password: c.password || '',
            tls: !!c.tls,
            tlsInsecure: !!c.tls_insecure,
            connectTimeoutSecs: c.connect_timeout_secs || null,
            commandTimeoutSecs: c.command_timeout_secs || null,
        })));
        const current = getCurrentConn();
        if (current && !findConnection(current.id)) setCurrentConn(null);
        renderConnections();
        updateStatus();
        return getConnections();
    }).catch(err => {
        console.error('加载连接失败', err);
        showToast('加载连接失败: ' + err, 'error');
    });
}

export function initConnections() {
    document.getElementById('addConnectionBtn').addEventListener('click', () => openConnectionModal(null));
    document.getElementById('btnNewConnection').addEventListener('click', () => openConnectionModal(null));
}
