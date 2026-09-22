/**
 * 通用界面反馈：Toast 提示、确认对话框、只读闸门。
 * 三者都被多个模块调用（连接管理、Key 编辑、导入导出、更新…），且不持有业务状态。
 */

// ---------- Toast ----------

/**
 * 轻提示。duration 默认 2.2 秒；更新失败的提示要留足阅读时间，
 * 调用方传 `UPDATE_NOTICE_MS`（见 updater.js）。
 */
export function showToast(msg, type = 'info', duration = 2200) {
    const existing = document.querySelector('.toast-notification');
    if (existing) existing.remove();
    const toast = document.createElement('div');
    toast.className = 'toast-notification';
    const colors = {
        success: 'var(--green)',
        error: 'var(--accent)',
        info: 'var(--blue)',
    };
    toast.style.cssText = `
        position: fixed; bottom: 30px; left: 50%; transform: translateX(-50%);
        background: var(--bg-secondary); border: 1px solid var(--border-color);
        padding: 10px 28px; border-radius: var(--radius);
        font-size: 14px; color: var(--text-primary);
        box-shadow: var(--shadow); z-index: 2000;
        border-left: 4px solid ${colors[type] || colors.info};
        animation: fadeIn 0.2s ease;
        max-width: 90vw;
    `;
    toast.textContent = msg;
    document.body.appendChild(toast);
    setTimeout(() => {
        toast.style.opacity = '0';
        toast.style.transition = 'opacity 0.3s';
        setTimeout(() => toast.remove(), 300);
    }, duration);
}

// ---------- 通用确认对话框（替代原生 confirm，避免 WebView 兼容问题） ----------

let _confirmResolve = null;
// 确认框里可选复选框的当前状态（showConfirm 传入 checkbox 时才有意义）
let _confirmOptionChecked = false;

/**
 * 弹出确认框，返回 Promise<boolean>。
 * `options.checkbox` 会显示一个可选复选框，调用方用 `getConfirmOption()` 读结果。
 */
export function showConfirm(message, options = {}) {
    const cModal = document.getElementById('confirmModal');
    document.getElementById('confirmTitle').textContent = options.title || '提示';
    document.getElementById('confirmMessage').textContent = message;
    const okBtn = document.getElementById('confirmOk');
    okBtn.textContent = options.okText || '删除';
    if (options.danger !== false) {
        okBtn.style.background = 'var(--danger, #e5484d)';
        okBtn.style.borderColor = 'var(--danger, #e5484d)';
    } else {
        okBtn.style.background = '';
        okBtn.style.borderColor = '';
    }
    const optionRow = document.getElementById('confirmOption');
    const optionBox = document.getElementById('confirmOptionBox');
    if (options.checkbox && options.checkbox.label) {
        _confirmOptionChecked = !!options.checkbox.checked;
        optionBox.checked = _confirmOptionChecked;
        document.getElementById('confirmOptionLabel').textContent = options.checkbox.label;
        optionRow.style.display = 'flex';
    } else {
        _confirmOptionChecked = false;
        optionRow.style.display = 'none';
    }
    cModal.classList.add('open');
    return new Promise((resolve) => {
        _confirmResolve = resolve;
    });
}

/** 读取最近一次确认框里复选框的状态 */
export function getConfirmOption() {
    return _confirmOptionChecked;
}

function closeConfirm(result) {
    const cModal = document.getElementById('confirmModal');
    cModal.classList.remove('open');
    if (result) _confirmOptionChecked = document.getElementById('confirmOptionBox').checked;
    if (_confirmResolve) {
        _confirmResolve(result);
        _confirmResolve = null;
    }
}

export function initUi() {
    document.getElementById('confirmOk').addEventListener('click', () => closeConfirm(true));
    document.getElementById('confirmCancel').addEventListener('click', () => closeConfirm(false));
    document.getElementById('confirmModal').addEventListener('click', (e) => {
        if (e.target.id === 'confirmModal') closeConfirm(false);
    });
}

/** 只读闸门：按当前连接是否为只读，启用/禁用写操作入口 */
export function setWriteActionsDisabled(readonly) {
    const ids = ['btnAddKey', 'btnFlush', 'detailSave'];
    ids.forEach(id => {
        const btn = document.getElementById(id);
        if (btn) {
            btn.disabled = readonly;
            btn.title = readonly ? '只读连接，禁止写操作' : btn.getAttribute('data-orig-title') || '';
            btn.style.cursor = readonly ? 'not-allowed' : '';
        }
    });
}
