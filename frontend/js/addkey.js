/**
 * 新增 Key 对话框（只支持 string 类型，与后端 `set_key` 对应）。
 * 创建成功后本地插入这条 key 并选中它 —— 重扫列表会丢掉已加载的分页与滚动位置，
 * 而且新 key 未必落在第一页里。
 */
import { invoke } from './api.js';
import { parseTtl } from './util.js';
import {
    getCurrentConn, getKeyEntry, notifyKeysChanged, patchKeyEntry, setSelectedKey, upsertKeyEntry,
} from './state.js';
import { showToast } from './ui.js';
import { appendTerminal } from './terminal.js';

const addKeyModal = document.getElementById('addKeyModal');
const addKeyNameInput = document.getElementById('addKeyName');
const addKeyValueInput = document.getElementById('addKeyValue');
const addKeyTtlInput = document.getElementById('addKeyTtl');

function openAddKeyModal() {
    const conn = getCurrentConn();
    if (!conn || !conn.id || conn.online !== true) {
        showToast('请先连接 Redis 服务器', 'error');
        return;
    }
    addKeyNameInput.value = '';
    addKeyValueInput.value = '';
    addKeyTtlInput.value = '-1';
    addKeyModal.classList.add('open');
    setTimeout(() => addKeyNameInput.focus(), 50);
}

function closeAddKeyModal() {
    addKeyModal.classList.remove('open');
}

function submitAddKey() {
    const conn = getCurrentConn();
    const trimmed = addKeyNameInput.value.trim();
    if (!trimmed) {
        showToast('请输入 Key 名称', 'error');
        addKeyNameInput.focus();
        return;
    }
    // 查重只看已加载的部分：分页模式下未加载的 key 查不到，
    // 真冲突时 SET 会直接覆盖（与旧行为一致）
    if (getKeyEntry(trimmed)) {
        showToast('Key 已存在', 'error');
        return;
    }
    const ttlResult = parseTtl(addKeyTtlInput.value);
    if (!ttlResult.ok) {
        showToast(ttlResult.message, 'error');
        addKeyTtlInput.focus();
        return;
    }
    const value = addKeyValueInput.value;
    document.getElementById('btnAddKey').disabled = true;
    invoke('set_key', { connId: conn.id, key: trimmed, value: value, ttl: ttlResult.ttl })
        .then(() => {
            closeAddKeyModal();
            showToast(`Key "${trimmed}" 已创建`, 'success');
            const ttlArg = ttlResult.ttl > 0 ? ` EX ${ttlResult.ttl}` : '';
            appendTerminal(`SET ${trimmed} "${value}"${ttlArg} → OK`, 'ok');
            // 本地插入这条新 key：重扫列表会丢掉已加载的分页与滚动位置，
            // 而且新 key 未必落在第一页里
            upsertKeyEntry(trimmed, 'string', ttlResult.ttl);
            patchKeyEntry(trimmed, { value: value });
            setSelectedKey(trimmed);
            notifyKeysChanged();
        })
        .catch(err => {
            showToast('创建失败: ' + err, 'error');
            appendTerminal(`SET ${trimmed} 失败: ${err}`, 'error');
        })
        .finally(() => {
            document.getElementById('btnAddKey').disabled = false;
        });
}

export function initAddKey() {
    document.getElementById('btnAddKey').addEventListener('click', openAddKeyModal);
    document.getElementById('addKeyCancel').addEventListener('click', closeAddKeyModal);
    document.getElementById('addKeySave').addEventListener('click', submitAddKey);
    addKeyNameInput.addEventListener('keydown', (e) => { if (e.key === 'Enter') submitAddKey(); });
    addKeyValueInput.addEventListener('keydown', (e) => { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) submitAddKey(); });
    addKeyTtlInput.addEventListener('keydown', (e) => { if (e.key === 'Enter') submitAddKey(); });
    addKeyModal.addEventListener('click', (e) => { if (e.target === addKeyModal) closeAddKeyModal(); });
}
