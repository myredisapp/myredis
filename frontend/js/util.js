/**
 * 纯工具函数：不碰 DOM 状态、不碰共享状态、不发请求。
 * 放这里的都是「给定输入就有确定输出」的转换 —— 便于单独推理，也便于将来加测试。
 */

/** HTML 转义（拼接 innerHTML 前必须过一道） */
export function escapeHtml(str) {
    if (!str) return '';
    return String(str).replace(/[&<>"']/g, function (c) {
        return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
    });
}

// ---------- JSON ----------

/** 尝试把字符串解析成 JSON，解析失败返回原值 */
function tryParseJson(str) {
    if (typeof str !== 'string' || !str) return str;
    const s = str.trim();
    if (!(s.startsWith('{') || s.startsWith('['))) return str;
    try {
        return JSON.parse(s);
    } catch (e) {
        return str;
    }
}

/** 格式化 JSON 文本：JSON 字符串原样解析并美化，否则尝试解析后美化，仍失败则原样返回 */
export function formatJsonText(text) {
    if (typeof text !== 'string') {
        try { return JSON.stringify(text, null, 2); } catch (e) { return String(text); }
    }
    const trimmed = text.trim();
    if (!trimmed) return text;
    const parsed = tryParseJson(trimmed);
    if (typeof parsed !== 'string') {
        try { return JSON.stringify(parsed, null, 2); } catch (e) { return text; }
    }
    const inner = tryParseJson(parsed);
    if (typeof inner !== 'string') {
        try { return JSON.stringify(inner, null, 2); } catch (e) { return parsed; }
    }
    return text;
}

// ---------- 数值 / 时间格式 ----------

/** 字节数 → 可读大小（B / KB / MB / GB，一位小数） */
export function formatAppBytes(bytes) {
    if (!bytes) return '0B';
    const u = ['B', 'KB', 'MB', 'GB'];
    let i = 0, v = Number(bytes);
    while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
    return (i === 0 ? v : v.toFixed(1)) + u[i];
}

/** 百分比（保留 1 位小数） */
export function formatDiskPercent(p) {
    const n = Number(p);
    return (isFinite(n) ? Math.round(n * 10) / 10 : n) + '%';
}

/** 更新包大小：B 取整，KB 以上一位小数（与 formatAppBytes 的显示习惯略有不同，保留原样） */
export function fmtSize(bytes) {
    if (!bytes || bytes < 1024) return (bytes || 0) + ' B';
    const units = ['KB', 'MB', 'GB'];
    let value = bytes / 1024;
    let i = 0;
    while (value >= 1024 && i < units.length - 1) { value /= 1024; i++; }
    return value.toFixed(1) + ' ' + units[i];
}

/** 文件名用的日期戳（YYYY-MM-DD） */
export function todayStamp() {
    return new Date().toISOString().slice(0, 10);
}

// ---------- 输入校验 ----------

/**
 * 解析 TTL 输入框：只接受 -1（永不过期）或正整数。
 * 返回 `{ ok, ttl }` 或 `{ ok: false, message }`，由调用方决定怎么提示。
 */
export function parseTtl(value) {
    if (value === '' || value === null || value === undefined) {
        return { ok: false, message: '请输入 TTL' };
    }
    const trimmed = String(value).trim();
    if (!/^-?\d+$/.test(trimmed)) {
        return { ok: false, message: 'TTL 必须为整数' };
    }
    const n = parseInt(trimmed, 10);
    if (n === -1) {
        return { ok: true, ttl: -1 };
    }
    if (n <= 0) {
        return { ok: false, message: 'TTL 必须为 -1 或正整数' };
    }
    return { ok: true, ttl: n };
}

// ---------- Stream 分页 ----------

/** XRANGE 起点是闭区间：翻页起点 = 最后一条 id 的序列号 + 1 */
export function nextStreamId(id) {
    const i = id.lastIndexOf('-');
    if (i < 0) return id + '-1';
    const seq = parseInt(id.slice(i + 1), 10);
    return id.slice(0, i + 1) + (Number.isFinite(seq) ? seq + 1 : 1);
}

// ---------- 浏览器能力 ----------

/** 触发一次「另存为」下载（导出连接配置 / Key 用） */
export function downloadJson(filename, text) {
    const blob = new Blob([text], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
}

/** 剪贴板降级方案：临时 textarea + execCommand（navigator.clipboard 不可用时） */
export function fallbackCopy(text, done) {
    try {
        const ta = document.createElement('textarea');
        ta.value = text;
        ta.style.position = 'fixed';
        ta.style.opacity = '0';
        document.body.appendChild(ta);
        ta.select();
        document.execCommand('copy');
        document.body.removeChild(ta);
        if (done) done();
    } catch (e) {
        if (typeof done === 'function') { /* 保留占位，失败时静默 */ }
    }
}
