/**
 * 布局状态：侧栏折叠 + 三处尺寸拖拽（侧栏 / Key 树 / 底部面板）持久化。
 * 尺寸写成 CSS 变量（`--sidebar-width` 等），样式只在 styles.css 里读变量。
 */

const LAYOUT_KEY = 'myredis.layout';
const LAYOUT_DEFAULTS = {
    sidebarCollapsed: false,
    sidebarWidth: 270,
    treeWidth: 320,
    terminalHeight: 200,
};

const $ = (sel) => document.getElementById(sel);

let layoutState = { ...LAYOUT_DEFAULTS };
try {
    const saved = JSON.parse(localStorage.getItem(LAYOUT_KEY) || '{}');
    layoutState = { ...LAYOUT_DEFAULTS, ...saved };
} catch (e) { /* 存档损坏时回退默认值 */ }

let layoutSaveTimer = null;

function saveLayout() {
    clearTimeout(layoutSaveTimer);
    layoutSaveTimer = setTimeout(() => {
        try { localStorage.setItem(LAYOUT_KEY, JSON.stringify(layoutState)); } catch (e) {}
    }, 300);
}

function updateSidebarToggleIcon() {
    $('sidebarToggleIcon').className =
        layoutState.sidebarCollapsed ? 'fas fa-bars-staggered' : 'fas fa-bars';
}

export function applyLayout() {
    const rootStyle = document.documentElement.style;
    rootStyle.setProperty('--sidebar-width', layoutState.sidebarWidth + 'px');
    rootStyle.setProperty('--tree-width', layoutState.treeWidth + 'px');
    rootStyle.setProperty('--terminal-height', layoutState.terminalHeight + 'px');
    $('sidebar').classList.toggle('collapsed', layoutState.sidebarCollapsed);
    // 折叠态侧栏宽度锁定为 52px，此时拖拽无意义，隐藏对应手柄
    $('splitterSidebar').classList.toggle('hidden', layoutState.sidebarCollapsed);
    updateSidebarToggleIcon();
    saveLayout();
}

function setLayoutSize(key, value) {
    layoutState[key] = value;
    applyLayout();
}

// 通用边界拖拽：pointerdown 起拖，pointermove 按轴向改写对应尺寸
function makeSplitter(handleId, options) {
    const handle = $(handleId);
    if (!handle) return;
    handle.addEventListener('pointerdown', (e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        const startPos = options.axis === 'x' ? e.clientX : e.clientY;
        // 以实际渲染尺寸为基准，避免状态值与渲染值不一致导致起拖跳变
        const startSize = options.get();
        const target = options.resizeTarget ? options.resizeTarget() : null;
        handle.classList.add('dragging');
        document.body.classList.add(options.axis === 'x' ? 'resizing-h' : 'resizing-v');
        if (target) target.classList.add('no-transition');
        // 手柄仅 5px 宽，不做指针捕获时指针一旦移出就会丢失 pointermove
        try { handle.setPointerCapture(e.pointerId); } catch (err) {}

        function onMove(ev) {
            const pos = options.axis === 'x' ? ev.clientX : ev.clientY;
            let size = Math.round(startSize + (pos - startPos) * (options.invert ? -1 : 1));
            if (options.maxSize) {
                size = Math.min(size, options.maxSize());
            } else if (typeof options.max === 'number') {
                size = Math.min(size, options.max);
            }
            options.set(Math.max(options.min, size));
        }
        function onUp() {
            handle.classList.remove('dragging');
            document.body.classList.remove('resizing-h', 'resizing-v');
            handle.removeEventListener('pointermove', onMove);
            handle.removeEventListener('pointerup', onUp);
            handle.removeEventListener('pointercancel', onUp);
            try { handle.releasePointerCapture(e.pointerId); } catch (err) {}
            saveLayout();
            // 松手后稍等再恢复过渡，避免尺寸跳变产生动画闪烁
            if (target) setTimeout(() => target.classList.remove('no-transition'), 50);
        }
        handle.addEventListener('pointermove', onMove);
        handle.addEventListener('pointerup', onUp);
        handle.addEventListener('pointercancel', onUp);
    });
}

export function initLayout() {
    makeSplitter('splitterSidebar', {
        axis: 'x', min: 180, max: 480,
        get: () => $('sidebar').offsetWidth,
        set: (v) => setLayoutSize('sidebarWidth', v),
        resizeTarget: () => $('sidebar'),
    });
    makeSplitter('splitterTree', {
        axis: 'x', min: 180, max: 480,
        get: () => $('keyTree').offsetWidth,
        set: (v) => setLayoutSize('treeWidth', v),
    });
    makeSplitter('splitterTerminal', {
        axis: 'y', min: 100, invert: true,
        maxSize: () => Math.round(window.innerHeight * 0.5),
        get: () => $('terminalPanel').offsetHeight,
        set: (v) => setLayoutSize('terminalHeight', v),
        resizeTarget: () => $('terminalPanel'),
    });
    $('btnSidebarToggle').addEventListener('click', () => {
        layoutState.sidebarCollapsed = !layoutState.sidebarCollapsed;
        applyLayout();
    });
}
