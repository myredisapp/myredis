/**
 * Key 树：后端分页（SCAN 游标）+ 虚拟滚动 + 折叠文件夹 + 搜索下推。
 *
 * 数据在 state.js（KEY_DATA / allKeys），这里只负责「怎么显示、怎么翻页」：
 * 每页向后端要 100 个 key，滚到接近底部就预取下一页；文件夹按连接的分隔符折叠，
 * 展开状态按连接分别记住（`expandedCache`）。
 */
import { invoke } from './api.js';
import { escapeHtml } from './util.js';
import {
    clearKeyEntries, getAllKeys, getCurrentConn, getKeyData, getSelectedKey, setSelectedKey, upsertKeyEntry,
} from './state.js';
import { showToast } from './ui.js';
import { appendTerminal } from './terminal.js';
import { renderDetail } from './detail.js';

const keyTreeEl = document.getElementById('keyTree');
const keySearch = document.getElementById('keySearch');

// 每页向后端请求的 key 数（后端把它当作 SCAN 的 COUNT 提示，实际每页可能略多）
const KEY_PAGE_SIZE = 100;
// 虚拟滚动的行高，需与 CSS 里 .vt-row 的 height 一致
let KEY_ROW_HEIGHT = 28;
// 视口上下各多渲染几行，减少快速滚动时出现白屏
const KEY_OVERSCAN = 8;
// 距底部还有这么多行时就预取下一页
const KEY_PREFETCH_ROWS = 10;

// 分页状态：cursor 是后端返回的不透明游标，done 表示已扫描完当前 keyspace
const keyPaging = {
    cursor: '0',
    done: false,
    loading: false,
    searchTerm: '',
    error: '',
    token: 0,
};
// 扁平化后的可见行（文件夹 + key），虚拟滚动的数据源
let keyRows = [];
// 每个连接已展开的文件夹路径集合，Map<connId, Set<folderPath>>
let expandedCache = {};

function sepCacheKey() {
    const conn = getCurrentConn();
    return (conn && conn.id) || 'default';
}

function getExpandedSet() {
    const k = sepCacheKey();
    if (!expandedCache[k]) expandedCache[k] = new Set();
    return expandedCache[k];
}

function getSeparator() {
    const conn = getCurrentConn();
    const sep = conn && conn.separator;
    return (typeof sep === 'string' && sep.length > 0) ? sep : ':';
}

// 渲染叶子 key 行
function renderKeyRowHtml(key, depth) {
    const item = getKeyData()[key] || {};
    const selectedKey = getSelectedKey();
    return '<div class="tree-node">' +
        '<div class="node-label node-key-select' + (selectedKey === key ? ' active' : '') + '" data-key="' + escapeHtml(key) + '" title="' + escapeHtml(key) + '" style="padding-left:' + (8 + depth * 16) + 'px">' +
            '<span class="toggle" style="visibility:hidden;"><i class="fas fa-chevron-right"></i></span>' +
            '<i class="fas fa-key icon-key"></i>' +
            '<span class="key-name">' + escapeHtml(key) + '</span>' +
            '<span class="key-type ' + (item.type || 'string') + '">' + (item.type || 'string') + '</span>' +
        '</div></div>';
}

// 渲染文件夹行
function renderFolderRowHtml(label, path, isOpen, depth) {
    return '<div class="tree-node"><div class="node-label node-folder-select' + (isOpen ? ' open' : '') + '" data-folder="' + escapeHtml(path) + '" style="padding-left:' + (8 + depth * 16) + 'px">' +
        '<span class="toggle"><i class="fas fa-chevron-right' + (isOpen ? ' open' : '') + '"></i></span>' +
        '<i class="fas fa-folder' + (isOpen ? '-open' : '') + ' icon-folder"></i>' +
        '<span class="key-name">' + escapeHtml(label) + '</span>' +
    '</div></div>';
}

// 文件夹模式：keys 已排序。按 sep 组织成目录树，展开的目录输出后续行。
function buildFolderRows(keys, sep, expanded, depth, dirPath, out) {
    const prefix = dirPath ? dirPath + sep : '';
    const childDirs = new Set();
    const leafKeys = [];
    keys.forEach(function (k) {
        if (!k.startsWith(prefix)) return;
        const rest = k.slice(prefix.length);
        if (rest === '') return;
        const idx = rest.indexOf(sep);
        if (idx === -1) {
            leafKeys.push(k); // 直接子 key（完整路径）
        } else {
            childDirs.add(prefix + rest.slice(0, idx)); // 直接子目录
        }
    });
    // 先文件件（按路径），再 key —— 与折叠前的展示顺序一致
    const dirList = Array.from(childDirs).sort();
    for (let i = 0; i < dirList.length; i++) {
        const d = dirList[i];
        const isOpen = expanded.has(d);
        out.push({ kind: 'folder', path: d, label: d.split(sep).pop(), isOpen: isOpen, depth: depth });
        if (isOpen) buildFolderRows(keys, sep, expanded, depth + 1, d, out);
    }
    const leafList = leafKeys.sort();
    for (let j = 0; j < leafList.length; j++) {
        out.push({ kind: 'key', key: leafList[j], depth: depth });
    }
}

// 由已加载的 key 重建扁平行列表（文件夹展开状态变化、数据变化后调用）
function rebuildKeyRows() {
    const list = Object.keys(getKeyData()).sort((a, b) => a.localeCompare(b));
    const sep = getSeparator();
    const rows = [];
    if (keyPaging.searchTerm || !sep) {
        // 搜索时不折叠文件夹：命中的是 key 本身，折叠起来反而看不到结果
        for (let i = 0; i < list.length; i++) {
            rows.push({ kind: 'key', key: list[i], depth: 0 });
        }
    } else {
        buildFolderRows(list, sep, getExpandedSet(), 0, '', rows);
    }
    keyRows = rows;
}

// Key 树的空态 / 占位提示
function keyTreeHintHtml(icon, text, loading) {
    return '<div style="padding:24px 16px;color:var(--text-muted);text-align:center;font-size:13px;">' +
        '<i class="fas ' + (loading ? 'fa-spinner fa-spin' : icon) + '" style="font-size:22px;display:block;margin-bottom:8px;opacity:.4;"></i>' +
        escapeHtml(text) + '</div>';
}

// 虚拟滚动需要的骨架：占位层 + 底部状态条
function ensureKeyTreeSkeleton() {
    if (document.getElementById('keyTreeSpacer')) return;
    keyTreeEl.innerHTML = '<div class="vt-spacer" id="keyTreeSpacer"></div>' +
        '<div class="vt-status" id="keyTreeStatus"></div>';
}

function updateKeyTreeStatus() {
    const el = document.getElementById('keyTreeStatus');
    if (!el) return;
    if (keyPaging.loading) {
        el.innerHTML = '<i class="fas fa-spinner fa-spin"></i> 加载中…';
        return;
    }
    const scope = keyPaging.searchTerm ? '匹配 ' : '已加载 ';
    const n = getAllKeys().length;
    if (keyPaging.done) {
        el.textContent = scope + n + ' 个 Key（已全部加载）';
        return;
    }
    // 文件夹折叠起来时列表可能撑不满视口，滚不动就永远触发不了加载；
    // 所以除了滚动预取，这里再给一个显式入口
    el.innerHTML = scope + n + ' 个 Key · <button type="button" class="vt-more">加载更多</button>';
}

// 只渲染视口内的行（虚拟滚动）
function renderKeyWindow() {
    const spacer = document.getElementById('keyTreeSpacer');
    if (!spacer) return;
    const total = keyRows.length;
    spacer.style.height = (total * KEY_ROW_HEIGHT) + 'px';
    if (total === 0) {
        spacer.innerHTML = '';
        updateKeyTreeStatus();
        return;
    }
    const viewHeight = keyTreeEl.clientHeight || 400;
    const first = Math.max(0, Math.floor(keyTreeEl.scrollTop / KEY_ROW_HEIGHT) - KEY_OVERSCAN);
    const last = Math.min(total, Math.ceil((keyTreeEl.scrollTop + viewHeight) / KEY_ROW_HEIGHT) + KEY_OVERSCAN);
    let html = '';
    for (let i = first; i < last; i++) {
        const row = keyRows[i];
        const inner = row.kind === 'folder'
            ? renderFolderRowHtml(row.label, row.path, row.isOpen, row.depth)
            : renderKeyRowHtml(row.key, row.depth);
        html += '<div class="vt-row" style="top:' + (i * KEY_ROW_HEIGHT) + 'px">' + inner + '</div>';
    }
    spacer.innerHTML = html;
    // 行高自愈：CSS 与 JS 常量不一致时按真实高度重算一次
    const probe = spacer.firstElementChild;
    if (probe && probe.offsetHeight > 0 && probe.offsetHeight !== KEY_ROW_HEIGHT) {
        KEY_ROW_HEIGHT = probe.offsetHeight;
        renderKeyWindow();
        return;
    }
    updateKeyTreeStatus();
}

// 滚动接近底部时预取下一页
function maybeLoadNextPage() {
    if (keyPaging.done || keyPaging.loading) return;
    const remain = keyTreeEl.scrollHeight - keyTreeEl.scrollTop - keyTreeEl.clientHeight;
    if (remain <= KEY_ROW_HEIGHT * KEY_PREFETCH_ROWS) loadKeyPage();
}

/** 清空本地列表与分页状态（断开连接、切换连接、重新加载前调用） */
export function resetKeyList() {
    keyPaging.token += 1;
    keyPaging.cursor = '0';
    keyPaging.done = false;
    keyPaging.loading = false;
    keyPaging.searchTerm = keySearch ? keySearch.value.trim() : '';
    keyPaging.error = '';
    clearKeyEntries();
    keyRows = [];
    keyTreeEl.scrollTop = 0;
    // 注意：expandedCache 按连接记住展开状态，这里**不清**（重新搜索 / 刷新后
    // 用户展开过的文件夹仍然保持展开）
}

/** 清空搜索框（切数据库时调用，让列表回到「全部 key」） */
export function clearKeySearch() {
    if (keySearch) keySearch.value = '';
}

/** 从第一页重新加载（连接切换、搜索、刷新、导入后调用） */
export function loadKeys() {
    const conn = getCurrentConn();
    if (!conn || !conn.id) {
        resetKeyList();
        renderKeyTree();
        return Promise.resolve(getAllKeys());
    }
    resetKeyList();
    return loadKeyPage(true);
}

/**
 * 拉取一页 key 并追加到列表。
 *
 * `reset` 为 true 表示这一页是重置后的首页（此时列表刚被清空）。
 */
function loadKeyPage(reset) {
    const conn = getCurrentConn();
    if (!conn || !conn.id) return Promise.resolve(getAllKeys());
    if (keyPaging.loading && !reset) return Promise.resolve(getAllKeys());
    if (keyPaging.done) return Promise.resolve(getAllKeys());

    const token = keyPaging.token;
    keyPaging.loading = true;
    keyPaging.error = '';
    renderKeyTree();

    return invoke('list_keys', {
        connId: conn.id,
        cursor: keyPaging.cursor,
        count: KEY_PAGE_SIZE,
        pattern: keyPaging.searchTerm || null,
    }).then((page) => {
        // 请求期间连接/搜索词变过：这批结果已经过期，直接丢弃
        if (token !== keyPaging.token) return getAllKeys();
        const items = (page && page.keys) || [];
        for (const it of items) {
            const type = it.type || 'string';
            const ttl = it.ttl !== undefined ? it.ttl : -1;
            upsertKeyEntry(it.key, type, ttl);
        }
        keyPaging.cursor = (page && page.nextCursor) ? page.nextCursor : '0';
        keyPaging.done = keyPaging.cursor === '0';
        renderKeyTree();
        return getAllKeys();
    }).catch((err) => {
        if (token === keyPaging.token) {
            // 保留已加载的部分，只把失败原因记下来（空列表时提示里会显示）
            keyPaging.error = String(err);
            console.error('加载 Key 列表失败', err);
            showToast('加载 Key 列表失败: ' + err, 'error');
        }
        return getAllKeys();
    }).finally(() => {
        if (token === keyPaging.token) {
            keyPaging.loading = false;
            renderKeyTree();
        }
    });
}

/** 渲染 Key 列表：先处理未连接 / 无数据的空态，否则重建行并渲染可视窗口 */
export function renderKeyTree() {
    const conn = getCurrentConn();
    const isConnected = conn && conn.online;
    if (!isConnected) {
        keyRows = [];
        keyTreeEl.innerHTML = keyTreeHintHtml('fa-plug', '请先连接 Redis 服务器', false);
        return;
    }
    rebuildKeyRows();
    if (keyRows.length === 0) {
        const searching = !!keyPaging.searchTerm;
        if (keyPaging.error) {
            keyTreeEl.innerHTML = keyTreeHintHtml('fa-exclamation-triangle', '加载失败：' + keyPaging.error, false);
        } else {
            keyTreeEl.innerHTML = keyTreeHintHtml(
                'fa-inbox',
                searching ? '没有匹配的 Key' : '暂无 Key\n点击「新增 Key」创建',
                keyPaging.loading
            );
        }
        return;
    }
    ensureKeyTreeSkeleton();
    renderKeyWindow();
}

export function initKeys() {
    // 搜索下推到后端 SCAN：输入停顿后再重扫，避免每敲一个字符就扫一遍 keyspace
    let keySearchTimer = null;
    keySearch.addEventListener('input', () => {
        clearTimeout(keySearchTimer);
        keySearchTimer = setTimeout(() => { loadKeys(); }, 300);
    });

    // Key 树的滚动与点击都用事件委托：虚拟滚动会不断替换行元素，
    // 逐元素绑定既浪费又容易漏绑
    keyTreeEl.addEventListener('scroll', () => {
        renderKeyWindow();
        maybeLoadNextPage();
    });
    keyTreeEl.addEventListener('click', (e) => {
        // 状态条上的「加载更多」
        if (e.target.closest('.vt-more')) {
            loadKeyPage();
            return;
        }
        const keyEl = e.target.closest('.node-key-select');
        if (keyEl) {
            setSelectedKey(keyEl.dataset.key);
            renderKeyWindow();
            renderDetail();
            return;
        }
        const folderEl = e.target.closest('.node-folder-select');
        if (!folderEl) return;
        const path = folderEl.dataset.folder;
        const expanded = getExpandedSet();
        if (expanded.has(path)) expanded.delete(path); else expanded.add(path);
        rebuildKeyRows();
        renderKeyWindow();
    });

    document.getElementById('btnRefresh').addEventListener('click', () => {
        loadKeys();
        showToast('已刷新', 'info');
        appendTerminal('刷新 Key 列表', '');
    });
}
