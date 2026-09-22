/**
 * 集合类型（Hash / List / Set / ZSet / Stream）的字段级编辑。
 *
 * 详情区给一块 `#collView` 容器，这里负责「拉内容 → 渲染可编辑表格 → 单字段操作后重拉」。
 * Stream 与其他类型不同：`XRANGE` 分页（每页 200 条），翻页起点是最后一条 id 的下一个。
 */
import { invoke } from './api.js';
import { escapeHtml, nextStreamId } from './util.js';
import { getCurrentConn, getSelectedKey } from './state.js';
import { showToast } from './ui.js';
import { appendTerminal } from './terminal.js';

// 各集合类型对应的内容读取命令
const COLL_READ_CMD = { hash: 'get_hash', list: 'get_list', set: 'get_set', zset: 'get_zset', stream: 'get_stream' };
// Stream 详情区分页大小（XRANGE COUNT）
const STREAM_PAGE = 200;

// 集合类型最近一次加载的内容缓存（key|type → 数据），用于复制与大小统计
const contentCache = {};

/** 读最近一次加载到的集合内容（详情区算「大小」、复制按钮取文本时用） */
export function getCachedContent(key, type) {
    return contentCache[key + '|' + type];
}

/** 加载并渲染某类型的字段级编辑表格；key 切换后异步返回时做校验 */
export function wireCollView(type) {
    const view = document.getElementById('collView');
    const conn = getCurrentConn();
    if (!view || !conn || !conn.id) return;
    const key = getSelectedKey();
    // Stream 走 XRANGE 分页（start 闭区间，翻页从下一 id 开始），其余类型一次全量
    const args = type === 'stream'
        ? { connId: conn.id, key, start: '-', count: STREAM_PAGE }
        : { connId: conn.id, key };
    invoke(COLL_READ_CMD[type], args)
        .then((data) => {
            if (getSelectedKey() !== key) return;
            contentCache[key + '|' + type] = data;
            renderCollRows(view, type, key, data, type === 'stream' && data.length >= STREAM_PAGE ? 'more' : '');
        })
        .catch((err) => {
            if (getSelectedKey() !== key) return;
            view.innerHTML = `<div class="coll-empty">加载失败：${escapeHtml(String(err))}</div>`;
        });
}

/** Stream「加载更多」：追加下一页到缓存并重渲染 */
function loadMoreStream(key) {
    const cached = contentCache[key + '|stream'] || [];
    if (!cached.length) return;
    invoke('get_stream', { connId: getCurrentConn().id, key, start: nextStreamId(cached[cached.length - 1].id), count: STREAM_PAGE })
        .then((more) => {
            if (getSelectedKey() !== key) return;
            const all = cached.concat(more);
            contentCache[key + '|stream'] = all;
            const view = document.getElementById('collView');
            if (view) renderCollRows(view, 'stream', key, all, more.length >= STREAM_PAGE ? 'more' : '');
        })
        .catch((err) => showToast('加载失败: ' + err, 'error'));
}

function renderCollRows(view, type, key, data, hasMore) {
    const conn = getCurrentConn();
    const readonly = !!(conn && conn.online && conn.readonly);
    const dis = readonly ? ' disabled' : '';
    const btn = (act, attrs, icon, title) =>
        `<button class="coll-btn${act.endsWith('del') ? ' danger' : ''}" data-act="${act}" ${attrs} title="${title}"${dis}><i class="fas fa-${icon}"></i></button>`;
    let rows = '';
    let emptyTip = '';
    if (type === 'hash') {
        const arr = data || [];
        rows = arr.map((it, i) => `
            <div class="coll-row">
                <span class="coll-key" title="${escapeHtml(it.field)}">${escapeHtml(it.field)}</span>
                <input class="coll-input grow" id="collVal${i}" value="${escapeHtml(it.value)}"${dis} />
                ${btn('hset', `data-field="${escapeHtml(it.field)}" data-input="collVal${i}"`, 'check', '保存字段')}
                ${btn('hdel', `data-field="${escapeHtml(it.field)}"`, 'trash-alt', '删除字段')}
            </div>`).join('');
        rows += `
            <div class="coll-row coll-add">
                <input class="coll-input" id="collAddKey" placeholder="字段名"${dis} />
                <input class="coll-input grow" id="collAddVal" placeholder="字段值"${dis} />
                ${btn('hadd', '', 'plus', '添加字段')}
            </div>`;
        if (!arr.length) emptyTip = '<div class="coll-empty">空 Hash</div>';
    } else if (type === 'list') {
        const arr = data || [];
        rows = arr.map((it) => `
            <div class="coll-row">
                <span class="idx">${it.index}</span>
                <input class="coll-input grow" id="collVal${it.index}" value="${escapeHtml(it.value)}"${dis} />
                ${btn('lset', `data-idx="${it.index}" data-input="collVal${it.index}"`, 'check', '保存元素')}
                ${btn('ldel', `data-val="${escapeHtml(it.value)}"`, 'trash-alt', '删除元素')}
            </div>`).join('');
        rows += `
            <div class="coll-row coll-add">
                <input class="coll-input grow" id="collAddVal" placeholder="新元素"${dis} />
                ${btn('lpush', '', 'angle-left', '左端插入')}
                ${btn('rpush', '', 'angle-right', '右端插入')}
            </div>`;
        if (!arr.length) emptyTip = '<div class="coll-empty">空 List</div>';
    } else if (type === 'set') {
        const arr = data || [];
        rows = arr.map((it, i) => `
            <div class="coll-row">
                <span class="idx">${i + 1}</span>
                <input class="coll-input grow" id="collVal${i}" value="${escapeHtml(it)}"${dis} />
                ${btn('sset', `data-old="${escapeHtml(it)}" data-input="collVal${i}"`, 'check', '保存成员（改名）')}
                ${btn('sdel', `data-val="${escapeHtml(it)}"`, 'trash-alt', '删除成员')}
            </div>`).join('');
        rows += `
            <div class="coll-row coll-add">
                <input class="coll-input grow" id="collAddVal" placeholder="新成员"${dis} />
                ${btn('sadd', '', 'plus', '添加成员')}
            </div>`;
        if (!arr.length) emptyTip = '<div class="coll-empty">空 Set</div>';
    } else if (type === 'stream') {
        const arr = data || [];
        rows = arr.map((it) => `
            <div class="coll-row coll-stream-row">
                <span class="idx" title="条目 id">${escapeHtml(it.id)}</span>
                <span class="coll-input grow coll-stream-fields">${it.fields.map(f => `<div><b>${escapeHtml(f.field)}</b>=${escapeHtml(f.value)}</div>`).join('')}</span>
                ${btn('xdel', `data-id="${escapeHtml(it.id)}"`, 'trash-alt', '删除条目')}
            </div>`).join('');
        if (hasMore === 'more') {
            rows += `
            <div class="coll-row coll-add">
                <button class="coll-btn" data-act="xmore" title="加载更多条目"><i class="fas fa-angle-down"></i> 加载更多</button>
            </div>`;
        }
        rows += `
            <div class="coll-row coll-add">
                <input class="coll-input" id="collAddStreamId" placeholder="条目 id（留空自动）"${dis} />
                <input class="coll-input" id="collAddKey" placeholder="字段名"${dis} />
                <input class="coll-input grow" id="collAddVal" placeholder="字段值"${dis} />
                ${btn('xadd', '', 'plus', '添加条目')}
            </div>`;
        if (!arr.length) emptyTip = '<div class="coll-empty">空 Stream</div>';
    } else if (type === 'zset') {
        const arr = data || [];
        rows = arr.map((it, i) => `
            <div class="coll-row">
                <span class="idx">${i + 1}</span>
                <input class="coll-input grow" id="collMember${i}" value="${escapeHtml(it.member)}"${dis} />
                <input class="coll-input coll-score" id="collScore${i}" value="${it.score}"${dis} />
                ${btn('zset', `data-old="${escapeHtml(it.member)}" data-i="${i}"`, 'check', '保存成员与分值')}
                ${btn('zdel', `data-val="${escapeHtml(it.member)}"`, 'trash-alt', '删除成员')}
            </div>`).join('');
        rows += `
            <div class="coll-row coll-add">
                <input class="coll-input grow" id="collAddKey" placeholder="成员"${dis} />
                <input class="coll-input coll-score" id="collAddScore" placeholder="分值" value="0"${dis} />
                ${btn('zadd', '', 'plus', '添加成员')}
            </div>`;
        if (!arr.length) emptyTip = '<div class="coll-empty">空 ZSet</div>';
    }
    view.innerHTML = emptyTip + rows;
    view.querySelectorAll('[data-act]').forEach(el => {
        el.addEventListener('click', () => runCollAction(el, type, key));
    });
}

// 执行字段级编辑操作，成功后刷新当前内容区
function runCollAction(el, type, key) {
    const conn = getCurrentConn();
    if (!conn || !conn.id) return;
    if (conn.readonly) {
        showToast('只读连接，禁止写操作', 'error');
        return;
    }
    const act = el.dataset.act;
    const connId = conn.id;
    const valOf = (id) => {
        const input = document.getElementById(id);
        return input ? input.value : '';
    };
    const require = (v, msg, focusId) => {
        if (v !== '') return true;
        showToast(msg, 'error');
        const f = document.getElementById(focusId);
        if (f) f.focus();
        return false;
    };
    const finish = (p, echoOk, echoErr) => {
        Promise.resolve(p)
            .then(() => {
                showToast('已保存', 'success');
                appendTerminal(echoOk, 'ok');
                wireCollView(type);
            })
            .catch((err) => {
                showToast('操作失败: ' + err, 'error');
                appendTerminal(`${echoErr}: ${err}`, 'error');
            });
    };

    if (act === 'hset') {
        const field = el.dataset.field;
        const value = valOf(el.dataset.input);
        if (!require(value, '请输入字段值', el.dataset.input)) return;
        finish(
            invoke('hash_set_field', { connId, key, field, value }),
            `HSET ${key} ${field} "${value}" → OK`,
            `HSET ${key} ${field}`
        );
    } else if (act === 'hdel') {
        const field = el.dataset.field;
        finish(
            invoke('hash_del_fields', { connId, key, fields: [field] }),
            `HDEL ${key} ${field} → OK`,
            `HDEL ${key} ${field}`
        );
    } else if (act === 'hadd') {
        const field = valOf('collAddKey');
        const value = valOf('collAddVal');
        if (!require(field, '请输入字段名', 'collAddKey')) return;
        if (!require(value, '请输入字段值', 'collAddVal')) return;
        finish(
            invoke('hash_set_field', { connId, key, field, value }),
            `HSET ${key} ${field} "${value}" → OK`,
            `HSET ${key} ${field}`
        );
    } else if (act === 'lset') {
        const index = parseInt(el.dataset.idx, 10);
        const value = valOf(el.dataset.input);
        if (!require(value, '请输入元素值', el.dataset.input)) return;
        finish(
            invoke('list_set_element', { connId, key, index, value }),
            `LSET ${key} ${index} "${value}" → OK`,
            `LSET ${key} ${index}`
        );
    } else if (act === 'ldel') {
        const value = el.dataset.val;
        finish(
            invoke('list_del_element', { connId, key, value }),
            `LREM ${key} 1 "${value}" → OK`,
            `LREM ${key} "${value}"`
        );
    } else if (act === 'lpush' || act === 'rpush') {
        const value = valOf('collAddVal');
        if (!require(value, '请输入新元素', 'collAddVal')) return;
        const left = act === 'lpush';
        finish(
            invoke('list_push_element', { connId, key, value, left }),
            `${left ? 'LPUSH' : 'RPUSH'} ${key} "${value}" → OK`,
            `${left ? 'LPUSH' : 'RPUSH'} ${key}`
        );
    } else if (act === 'sadd') {
        const member = valOf('collAddVal');
        if (!require(member, '请输入新成员', 'collAddVal')) return;
        finish(
            invoke('set_add_member', { connId, key, member }),
            `SADD ${key} "${member}" → OK`,
            `SADD ${key} "${member}"`
        );
    } else if (act === 'sset') {
        const old = el.dataset.old;
        const member = valOf(el.dataset.input);
        if (!require(member, '请输入成员', el.dataset.input)) return;
        // Set 成员不可原地修改：先加新值，再删旧值（同名时跳过删除）
        finish(
            invoke('set_add_member', { connId, key, member }).then(() => {
                if (member === old) return Promise.resolve();
                return invoke('set_del_member', { connId, key, member: old });
            }),
            `SREM ${key} "${old}" + SADD ${key} "${member}" → OK`,
            `修改 Set 成员`
        );
    } else if (act === 'sdel') {
        const member = el.dataset.val;
        finish(
            invoke('set_del_member', { connId, key, member }),
            `SREM ${key} "${member}" → OK`,
            `SREM ${key} "${member}"`
        );
    } else if (act === 'zadd') {
        const member = valOf('collAddKey');
        const score = parseFloat(valOf('collAddScore'));
        if (!require(member, '请输入成员', 'collAddKey')) return;
        if (!isFinite(score)) {
            showToast('分值必须是数字', 'error');
            return;
        }
        finish(
            invoke('zset_add_member', { connId, key, member, score }),
            `ZADD ${key} ${score} "${member}" → OK`,
            `ZADD ${key} ${score} "${member}"`
        );
    } else if (act === 'zset') {
        const i = el.dataset.i;
        const old = el.dataset.old;
        const member = valOf('collMember' + i);
        const score = parseFloat(valOf('collScore' + i));
        if (!require(member, '请输入成员', 'collMember' + i)) return;
        if (!isFinite(score)) {
            showToast('分值必须是数字', 'error');
            return;
        }
        finish(
            invoke('zset_add_member', { connId, key, member, score }).then(() => {
                if (member === old) return Promise.resolve();
                return invoke('zset_del_member', { connId, key, member: old });
            }),
            `ZADD ${key} ${score} "${member}" → OK`,
            `修改 ZSet 成员`
        );
    } else if (act === 'zdel') {
        const member = el.dataset.val;
        finish(
            invoke('zset_del_member', { connId, key, member }),
            `ZREM ${key} "${member}" → OK`,
            `ZREM ${key} "${member}"`
        );
    } else if (act === 'xadd') {
        const entryId = valOf('collAddStreamId').trim();
        const field = valOf('collAddKey');
        const value = valOf('collAddVal');
        if (!require(field, '请输入字段名', 'collAddKey')) return;
        if (!require(value, '请输入字段值', 'collAddVal')) return;
        finish(
            invoke('stream_add_entry', { connId, key, entryId: entryId || null, fields: [{ field, value }] }),
            `XADD ${key} ${entryId || '*'} ${field} "${value}" → OK`,
            `XADD ${key}`
        );
    } else if (act === 'xdel') {
        const entryId = el.dataset.id;
        finish(
            invoke('stream_del_entry', { connId, key, entryIds: [entryId] }),
            `XDEL ${key} ${entryId} → OK`,
            `XDEL ${key} ${entryId}`
        );
    } else if (act === 'xmore') {
        loadMoreStream(key);
    }
}
