/**
 * 跨模块共享状态：连接列表、当前连接、在线连接、选中的 Key、Key 列表数据。
 *
 * 拆分前这些是 28 个散在同一个闭包里的 `let`（`selectedKey` 被 6 个分区写），
 * 谁改了我只能靠翻代码。这里把它们收进一个模块，只暴露读写函数、不暴露变量本身，
 * 跨模块的写入因此都能在 diff 里被看见。
 *
 * 只放**真正跨模块**的状态：分页游标 / 虚拟滚动窗口 / 展开的文件夹属于 Key 树自己的
 * 内部状态，留在 keys.js；监控会话留在 monitor.js；更新进度留在 updater.js。
 */

// ---------- 连接 ----------

/** 连接配置列表（由后端 list_connections 加载） */
let connections = [];
/** 当前选中（正在使用）的连接；未连接时为 null 或 online=false 的对象 */
let currentConn = null;
/** 已成功建立连接的 connId 集合（多个连接可同时在线） */
const onlineConns = new Set();

export function getConnections() {
    return connections;
}

export function setConnections(list) {
    connections = list || [];
}

export function findConnection(id) {
    return connections.find(c => c.id === id) || null;
}

export function getCurrentConn() {
    return currentConn;
}

export function setCurrentConn(conn) {
    currentConn = conn;
}

export function isOnline(id) {
    return onlineConns.has(id);
}

export function markOnline(id) {
    if (id) onlineConns.add(id);
}

export function markOffline(id) {
    onlineConns.delete(id);
}

// ---------- 选中的 Key ----------

/** 当前选中的 Key 全名（详情区展示的对象） */
let selectedKey = null;

export function getSelectedKey() {
    return selectedKey;
}

export function setSelectedKey(key) {
    selectedKey = key;
}

// ---------- Key 列表数据 ----------

/** Key 元数据：key 全名 → { key, type, ttl, value } */
let keyData = {};
/** 与 keyData 同一批对象的数组视图（搜索、详情查找用） */
let allKeys = [];

export function getKeyData() {
    return keyData;
}

export function getAllKeys() {
    return allKeys;
}

/** 取一条 key 的元数据（不存在返回 undefined） */
export function getKeyEntry(key) {
    return keyData[key];
}

/**
 * 本地插入或更新一个 key（新增成功、分页结果合并时调用）。
 * KEY_DATA 与 allKeys 共用同一个对象，两处状态不会再走偏。
 */
export function upsertKeyEntry(key, type, ttl) {
    const existing = keyData[key];
    if (existing) {
        // SCAN 在键空间变动时可能重复返回同一个 key：更新而不重复入列
        existing.type = type;
        existing.ttl = ttl;
        return existing;
    }
    const entry = { key: key, type: type, ttl: ttl, value: '' };
    keyData[key] = entry;
    allKeys.push(entry);
    return entry;
}

/** 就地修改一条 key 的字段（保存值 / TTL 后调用），返回是否命中 */
export function patchKeyEntry(key, patch) {
    const entry = keyData[key];
    if (!entry) return false;
    Object.assign(entry, patch);
    return true;
}

/** 本地移除一个 key（删除 / 重命名后调用，避免整表重扫） */
export function removeKeyEntry(key) {
    delete keyData[key];
    const idx = allKeys.findIndex(k => k.key === key);
    if (idx >= 0) allKeys.splice(idx, 1);
}

/** 清空本地 Key 列表（断开连接、切换连接、重新加载前调用） */
export function clearKeyEntries() {
    keyData = {};
    allKeys = [];
}

// ---------- 数据变更通知 ----------

/**
 * Key 列表数据变了 → 通知订阅者重绘。
 *
 * 存在的理由：详情区 / 重命名 / 新增 Key 都会改 KEY_DATA，而重绘 Key 树属于 keys.js。
 * 直接互相 import 会绕成环（keys.js 要调 renderDetail，detail.js 要调 renderKeyTree），
 * 所以这里用一个极小的订阅口：改数据的一方 `notifyKeysChanged()`，
 * 由 main.js 把 `renderKeyTree` 订阅进来。
 */
const keysChangedListeners = [];

export function onKeysChanged(fn) {
    keysChangedListeners.push(fn);
}

export function notifyKeysChanged() {
    for (const fn of keysChangedListeners) fn();
}
