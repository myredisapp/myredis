/**
 * Key 详情区：类型徽章、值编辑器（string）、TTL、保存 / 刷新 / 重命名 / 复制 / 删除。
 * 集合类型的字段级表格在 collections.js；重命名 / 复制对话框在 keyops.js。
 *
 * 详情区的 DOM 每次渲染整块重建（bindings 跟着重建），所以这里没有 init 绑定，
 * 只有 `renderDetail()` 一个入口。
 */
import { invoke } from './api.js';
import { escapeHtml, fallbackCopy, formatJsonText, parseTtl } from './util.js';
import {
    getAllKeys, getCurrentConn, getSelectedKey, notifyKeysChanged,
    patchKeyEntry, removeKeyEntry, setSelectedKey,
} from './state.js';
import { showConfirm, showToast } from './ui.js';
import { appendTerminal } from './terminal.js';
import { getCachedContent, wireCollView } from './collections.js';
import { openKeyOpModal, serverSupportsCopy } from './keyops.js';

// 为编辑器生成格式化按钮（JSON 支持直接点击，其它类型用复制代替）
function buildValueEditorActions(typeClass) {
    const actions = [];
    if (typeClass === 'string') {
        actions.push('<button class="format" id="editorFormat" type="button" title="格式化 JSON"><i class="fas fa-magic"></i> 格式化</button>');
    }
    actions.push('<button class="copy" id="editorCopy" type="button" title="复制内容"><i class="far fa-copy"></i> 复制</button>');
    return actions.join('');
}

export function renderDetail() {
    const keyDetailEl = document.getElementById('keyDetail');
    const selectedKey = getSelectedKey();
    if (!selectedKey) {
        keyDetailEl.innerHTML = `
            <div class="empty-state">
                <svg class="empty-state-icon" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="56" height="56"><defs><linearGradient id="md-favicon-gradient" x1="0%" y1="0%" x2="0%" y2="100%"><stop offset="0%" stop-color="#f9c75e" /><stop offset="100%" stop-color="#f5b342" /></linearGradient></defs><ellipse cx="44" cy="28" rx="24" ry="9" fill="url(#md-favicon-gradient)" /><path d="M20 28 L20 70 Q20 78 44 78 Q68 78 68 70 L68 28" fill="url(#md-favicon-gradient)" /><circle cx="72" cy="72" r="20" fill="#ffffff" stroke="#f5b342" stroke-width="4" /><text x="72" y="78" text-anchor="middle" font-family="'Noto Sans SC','PingFang SC','Microsoft YaHei',sans-serif" font-weight="700" font-size="15" fill="#f5b342">MR</text></svg>
                <h3>选择一个 Key</h3>
                <p>从左侧树中点击任意 Key 查看详细内容和值</p>
            </div>
        `;
        return;
    }
    const item = getAllKeys().find(k => k.key === selectedKey);
    if (!item) {
        keyDetailEl.innerHTML = `
            <div class="empty-state">
                <i class="fas fa-exclamation-triangle"></i>
                <h3>Key 不存在</h3>
                <p>该 Key 可能已被删除或不存在</p>
            </div>
        `;
        return;
    }

    const conn = getCurrentConn();
    const typeClass = item.type || 'string';
    const typeLabel = typeClass.toUpperCase();
    const ttl = item.ttl !== undefined ? item.ttl : -1;
    // 只读连接（已连接且配置为只读）时，禁用「保存」按钮
    const readonlyConn = !!(conn && conn.online && conn.readonly);
    // 复制走 COPY，需要 Redis 6.2+；版本取自连接时拿到的 INFO
    const serverVersion = (conn && conn.info && conn.info.redisVersion) || '';
    const copySupported = serverSupportsCopy();

    const valueActions = buildValueEditorActions(typeClass);

    let valueHtml = '';
    if (typeClass === 'string') {
        valueHtml = `
            <textarea id="valueEditor" placeholder="加载中…">${escapeHtml(item.value || '')}</textarea>
        `;
        // 从后端拉取真实值（异步）
        if (conn && conn.id) {
            const thisKey = selectedKey;
            invoke('get_string', { connId: conn.id, key: thisKey })
                .then(v => {
                    if (v === null || v === undefined) {
                        v = '(nil)';
                    }
                    const editor = document.getElementById('valueEditor');
                    if (editor && getSelectedKey() === thisKey) {
                        editor.value = v;
                    }
                })
                .catch(() => { /* 忽略 */ });
        }
    } else if (typeClass === 'hash' || typeClass === 'list' || typeClass === 'set' || typeClass === 'zset' || typeClass === 'stream') {
        // 集合类型：先占位，随后由 wireCollView 从后端拉取内容渲染
        valueHtml = `
            <div id="collView" class="coll-view" data-type="${typeClass}"><div class="coll-empty">加载中…</div></div>
        `;
    } else {
        valueHtml = `
            <div class="coll-empty">暂不支持此类型</div>
        `;
    }

    const ttlDisplay = ttl === -1 ? '永不过期' : `${ttl}s`;
    const ttlDisabled = readonlyConn ? ' disabled' : '';
    // 内容大小：string 取列表值，集合类型取最近一次加载到的内容
    const cachedForSize = getCachedContent(selectedKey, typeClass);
    const sizeSource = typeClass === 'string' ? item.value : cachedForSize;
    const sizeBytes = sizeSource ? JSON.stringify(sizeSource).length : 0;

    keyDetailEl.innerHTML = `
        <div class="detail-header">
            <div class="key-title">
                <span>${selectedKey}</span>
                <span class="type-badge ${typeClass}">${typeLabel}</span>
            </div>
            <span class="ttl"><i class="far fa-clock"></i> <input type="number" id="detailTtl" value="${ttl}" title="TTL（秒），-1 表示永不过期"${ttlDisabled} /></span>
            <div class="detail-actions">
                <button class="primary" id="detailSave"${readonlyConn ? ' disabled' : ''} data-orig-title="保存修改" title="${readonlyConn ? '只读连接，禁止写操作' : '保存修改'}"><i class="fas fa-save"></i> 保存</button>
                <button id="detailRename"${readonlyConn ? ' disabled' : ''} title="${readonlyConn ? '只读连接，禁止写操作' : '重命名该 Key（RENAME，含 TTL 一起移动）'}"><i class="fas fa-i-cursor"></i> 重命名</button>
                <button id="detailCopyKey"${readonlyConn || !copySupported ? ' disabled' : ''} title="${readonlyConn ? '只读连接，禁止写操作' : (copySupported ? '复制该 Key（COPY，同库内）' : 'COPY 需要 Redis 6.2 及以上版本，当前 ' + (serverVersion || '未知版本'))}"><i class="fas fa-clone"></i> 复制</button>
                <button id="detailRefresh"><i class="fas fa-sync-alt"></i> 刷新</button>
                <button class="danger" id="detailDelete"><i class="fas fa-trash-alt"></i> 删除</button>
            </div>
        </div>
        <div class="detail-body">
            <div class="value-editor">
                <div class="editor-toolbar">
                    ${valueActions}
                </div>
                ${valueHtml}
            </div>
            <div class="value-footer">
                <div class="info">
                    <span><i class="far fa-file"></i> 类型: ${typeLabel}</span>
                    <span><i class="far fa-clock"></i> TTL: ${ttlDisplay}</span>
                </div>
                <span>大小: ${sizeBytes} 字节</span>
            </div>
        </div>
    `;

    const saveBtn = document.getElementById('detailSave');
    if (saveBtn) {
        saveBtn.addEventListener('click', () => {
            const conn2 = getCurrentConn();
            const key = getSelectedKey();
            if (!conn2 || !conn2.id) {
                showToast('请先连接 Redis 服务器', 'error');
                return;
            }
            if (conn2.readonly) {
                showToast('只读连接，禁止写操作', 'error');
                appendTerminal(`保存 Key "${key}" 失败: 当前为只读连接，禁止写操作`, 'error');
                return;
            }
            const ttlInput = document.getElementById('detailTtl');
            const ttlResult = parseTtl(ttlInput ? ttlInput.value : '-1');
            if (!ttlResult.ok) {
                showToast(ttlResult.message, 'error');
                if (ttlInput) ttlInput.focus();
                return;
            }
            // 集合类型：字段级修改在表格行内即时保存，顶部「保存」只负责 TTL
            if (typeClass !== 'string') {
                saveBtn.disabled = true;
                invoke('set_key_ttl', { connId: conn2.id, key: key, ttl: ttlResult.ttl })
                    .then(() => {
                        showToast('TTL 已更新', 'success');
                        appendTerminal(ttlResult.ttl > 0 ? `EXPIRE ${key} ${ttlResult.ttl} → OK` : `PERSIST ${key} → OK`, 'ok');
                        patchKeyEntry(key, { ttl: ttlResult.ttl });
                        notifyKeysChanged();
                    })
                    .catch(err => {
                        showToast('TTL 更新失败: ' + err, 'error');
                        appendTerminal(`修改 TTL 失败: ${err}`, 'error');
                    })
                    .finally(() => {
                        saveBtn.disabled = false;
                    });
                return;
            }
            const editor = document.getElementById('valueEditor');
            if (!editor) {
                showToast('此类型不支持直接修改', 'info');
                return;
            }
            const value = editor.value;
            saveBtn.disabled = true;
            invoke('set_key', { connId: conn2.id, key: key, value: value, ttl: ttlResult.ttl })
                .then(() => {
                    const ttlArg = ttlResult.ttl > 0 ? ` EX ${ttlResult.ttl}` : '';
                    appendTerminal(`SET ${key}${ttlArg} → OK`, 'ok');
                    showToast('Key 已保存', 'success');
                    patchKeyEntry(key, { value: value, ttl: ttlResult.ttl });
                    notifyKeysChanged();
                })
                .catch(err => {
                    showToast('保存失败: ' + err, 'error');
                    appendTerminal(`保存 Key "${key}" 失败: ${err}`, 'error');
                })
                .finally(() => {
                    saveBtn.disabled = false;
                });
        });
    }
    const refreshBtn = document.getElementById('detailRefresh');
    if (refreshBtn) {
        refreshBtn.addEventListener('click', () => {
            renderDetail();
            appendTerminal(`刷新 Key "${getSelectedKey()}"`, '');
        });
    }
    const renameBtn = document.getElementById('detailRename');
    if (renameBtn) {
        renameBtn.addEventListener('click', () => openKeyOpModal('rename'));
    }
    const copyKeyBtn = document.getElementById('detailCopyKey');
    if (copyKeyBtn) {
        copyKeyBtn.addEventListener('click', () => openKeyOpModal('copy'));
    }
    const deleteBtn = document.getElementById('detailDelete');
    if (deleteBtn) {
        deleteBtn.addEventListener('click', () => {
            const keyToDelete = getSelectedKey();
            if (!keyToDelete) return;
            showConfirm(`确定要删除 Key「${keyToDelete}」吗？此操作不可恢复。`, { title: '删除 Key' }).then((ok) => {
                if (!ok) return;
                const conn2 = getCurrentConn();
                if (!conn2 || !conn2.id) {
                    showToast('未连接到 Redis', 'error');
                    return;
                }
                invoke('del_key', { connId: conn2.id, keys: [keyToDelete] })
                    .then((n) => {
                        appendTerminal(`删除 Key "${keyToDelete}"${n > 0 ? '' : '（不存在）'}`, n > 0 ? 'ok' : '');
                        showToast(n > 0 ? 'Key 已删除' : 'Key 不存在', n > 0 ? 'success' : 'error');
                        if (getSelectedKey() === keyToDelete) setSelectedKey(null);
                        // 本地摘掉这一条：分页模式下重扫列表会把已加载的页
                        // 和滚动位置一起丢掉
                        removeKeyEntry(keyToDelete);
                        notifyKeysChanged();
                    })
                    .catch(err => {
                        console.error('删除 Key 失败', err);
                        showToast('删除 Key 失败: ' + err, 'error');
                    });
            });
        });
    }

    // 获取当前详情展示的文本值（string 取编辑器内容，集合类型取最近一次加载的内容）
    const currentDetailValue = () => {
        const editor = document.getElementById('valueEditor');
        if (editor) return editor.value;
        const cached = getCachedContent(getSelectedKey(), typeClass);
        return cached != null ? JSON.stringify(cached, null, 2) : '';
    };

    const formatBtn = document.getElementById('editorFormat');
    if (formatBtn) {
        formatBtn.addEventListener('click', () => {
            const editor = document.getElementById('valueEditor');
            if (!editor) return;
            const formatted = formatJsonText(editor.value);
            editor.value = formatted;
            appendTerminal(`格式化 Key "${getSelectedKey()}" 的值`, 'ok');
            showToast('已格式化', 'success');
        });
    }

    const copyBtn = document.getElementById('editorCopy');
    if (copyBtn) {
        copyBtn.addEventListener('click', () => {
            const text = currentDetailValue();
            const done = () => { showToast('已复制到剪贴板', 'success'); };
            if (navigator.clipboard && navigator.clipboard.writeText) {
                navigator.clipboard.writeText(text).then(done).catch(() => fallbackCopy(text, done));
            } else {
                fallbackCopy(text, done);
            }
        });
    }

    // 集合类型：加载并渲染字段级编辑表格
    if (typeClass === 'hash' || typeClass === 'list' || typeClass === 'set' || typeClass === 'zset' || typeClass === 'stream') {
        wireCollView(typeClass);
    }
}
