/**
 * Key 重命名 / 复制（共用一个对话框）。
 *
 * 默认**不覆盖**目标：重命名走 `RENAMENX`、复制走不带 `REPLACE` 的 `COPY`，
 * 要覆盖必须用户显式勾选 —— 否则「顺手重命名」会安静地毁掉另一个 key。
 * `COPY` 需要 Redis 6.2+，版本取自连接时拿到的 INFO（拿不到就不拦，交给后端给明确提示）。
 */
import { invoke } from './api.js';
import {
    getCurrentConn, getKeyEntry, getSelectedKey, notifyKeysChanged, removeKeyEntry, setSelectedKey, upsertKeyEntry,
} from './state.js';
import { showToast } from './ui.js';
import { appendTerminal } from './terminal.js';

let keyOpMode = 'rename';

/** 服务器是否支持 COPY（Redis 6.2+） */
export function serverSupportsCopy() {
    const conn = getCurrentConn();
    const version = conn && conn.info && conn.info.redisVersion;
    if (!version) return true;
    const parts = String(version).split('.').map(n => parseInt(n, 10));
    const major = isNaN(parts[0]) ? 0 : parts[0];
    const minor = isNaN(parts[1]) ? 0 : parts[1];
    return major > 6 || (major === 6 && minor >= 2);
}

export function openKeyOpModal(mode) {
    const selectedKey = getSelectedKey();
    const conn = getCurrentConn();
    if (!selectedKey) {
        showToast('请先选择一个 Key', 'error');
        return;
    }
    if (!conn || !conn.online) {
        showToast('请先连接 Redis 服务器', 'error');
        return;
    }
    if (conn.readonly) {
        showToast('只读连接，禁止写操作', 'error');
        return;
    }
    if (mode === 'copy' && !serverSupportsCopy()) {
        const version = (conn.info && conn.info.redisVersion) || '未知版本';
        showToast(`COPY 需要 Redis 6.2 及以上版本（当前 ${version}）`, 'error');
        return;
    }

    keyOpMode = mode;
    const isRename = mode === 'rename';
    document.getElementById('keyOpTitle').textContent = isRename ? '重命名 Key' : '复制 Key';
    document.getElementById('keyOpSource').value = selectedKey;
    const destInput = document.getElementById('keyOpDest');
    destInput.value = isRename ? selectedKey : selectedKey + ':copy';
    document.getElementById('keyOpOverwrite').checked = false;
    document.getElementById('keyOpHint').textContent = isRename
        ? '重命名是原地移动：源 Key 不复存在，TTL 一起带走'
        : '复制到同一个库：源 Key 保持不变（COPY，需要 Redis 6.2+）';
    document.getElementById('keyOpOverwriteHint').textContent = isRename
        ? '不勾选：目标已存在时报错、不做任何修改（RENAMENX）；勾选：目标被覆盖（数据不可恢复）'
        : '不勾选：目标已存在时报错、不做任何修改；勾选：加 REPLACE 覆盖目标';
    document.getElementById('keyOpConfirm').textContent = isRename ? '重命名' : '复制';
    document.getElementById('keyOpModal').classList.add('open');
    destInput.focus();
    destInput.select();
}

function closeKeyOpModal() {
    document.getElementById('keyOpModal').classList.remove('open');
}

function submitKeyOp() {
    const source = getSelectedKey();
    const conn = getCurrentConn();
    const dest = document.getElementById('keyOpDest').value.trim();
    if (!dest) {
        showToast('请输入目标 Key 名称', 'error');
        document.getElementById('keyOpDest').focus();
        return;
    }
    const overwrite = document.getElementById('keyOpOverwrite').checked;
    const isRename = keyOpMode === 'rename';
    const confirmBtn = document.getElementById('keyOpConfirm');
    confirmBtn.disabled = true;

    invoke(isRename ? 'rename_key' : 'copy_key', {
        connId: conn.id,
        key: source,
        newKey: dest,
        overwrite: overwrite,
    }).then(() => {
        closeKeyOpModal();
        const entry = getKeyEntry(source);
        const type = (entry && entry.type) || 'string';
        const ttl = (entry && entry.ttl !== undefined) ? entry.ttl : -1;
        if (isRename) {
            // 本地跟着搬：重扫列表会丢掉已加载的分页与滚动位置，
            // 而且目标名未必落在已加载的页里
            removeKeyEntry(source);
            upsertKeyEntry(dest, type, ttl);
            setSelectedKey(dest);
            appendTerminal(`RENAME ${source} ${dest} → OK`, 'ok');
            showToast(`已重命名为 "${dest}"`, 'success');
        } else {
            upsertKeyEntry(dest, type, ttl);
            appendTerminal(`COPY ${source} ${dest}${overwrite ? ' REPLACE' : ''} → OK`, 'ok');
            showToast(`已复制为 "${dest}"`, 'success');
        }
        // 选中项 / 列表都变了：Key 树与详情区一起重绘
        notifyKeysChanged();
    }).catch((err) => {
        // 目标已存在是「可继续」的错误：把提示留在对话框里，用户勾上覆盖再来一次
        showToast((isRename ? '重命名失败: ' : '复制失败: ') + err, 'error');
        appendTerminal(`${isRename ? 'RENAME' : 'COPY'} ${source} → ${dest} 失败: ${err}`, 'error');
    }).finally(() => {
        confirmBtn.disabled = false;
    });
}

export function initKeyOps() {
    document.getElementById('keyOpCancel').addEventListener('click', closeKeyOpModal);
    document.getElementById('keyOpConfirm').addEventListener('click', submitKeyOp);
    document.getElementById('keyOpDest').addEventListener('keydown', (e) => {
        if (e.key === 'Enter') submitKeyOp();
    });
    document.getElementById('keyOpModal').addEventListener('click', (e) => {
        if (e.target === document.getElementById('keyOpModal')) closeKeyOpModal();
    });
}
