/**
 * 前端冒烟脚本注入的 Tauri 后端替身（`scripts/frontend-smoke.mjs` 用）。
 *
 * 这不是一个通用 mock：它只覆盖 `frontend/` 实际用到的 39 个命令，返回的结构与
 * `src-tauri/src/commands/*` 的 serde 输出逐字段对齐（camelCase），目的只有一个 ——
 * 让界面在无 Rust、无 Redis 的环境里跑到「已连接 + 有数据」的状态，好让
 * `docs/frontend-split-evaluation.md` 第 5 节的回归网能核对渲染结果。
 *
 * 事件按生产路径投递：真实 Tauri 由 Rust 侧 eval
 * `window.__TAURI_INTERNALS__.runCallback(handlerId, eventData)`
 * （见 tauri 2.x 的 `src/event/mod.rs`），这里同样走 runCallback，
 * 因此不依赖任何「简化版事件系统」，事件相关的 bug 也能被这个替身暴露。
 *
 * 页面可用 `window.__mock` 控制场景（见文件末尾），runner 通过 CDP evaluate 调用。
 */
(function () {
  'use strict';

  // ---------------------------------------------------------------- 假数据
  // 每个 key 都带默认分隔符（:），用来覆盖「文件夹折叠 + 展开」的渲染路径
  const DEMO_KEYS = [
    { key: 'app:config', type: 'string', ttl: -1 },
    { key: 'cache:user:1', type: 'hash', ttl: 120 },
    { key: 'cache:user:2', type: 'hash', ttl: -1 },
    { key: 'events', type: 'stream', ttl: -1 },
    { key: 'hello', type: 'string', ttl: -1 },
    { key: 'queue:jobs', type: 'list', ttl: -1 },
    { key: 'rank', type: 'zset', ttl: -1 },
    { key: 'session:token', type: 'string', ttl: 60 },
    { key: 'tags', type: 'set', ttl: -1 },
  ];

  const DEMO_STRINGS = {
    'hello': '{"name":"麦地缓存","nested":{"depth":2},"items":[1,2,3]}',
    'app:config': '{"port":6379,"tls":true}',
    'session:token': 'tok-abc123',
  };
  const DEMO_HASH = {
    'cache:user:1': [
      { field: 'name', value: 'Alice' },
      { field: 'level', value: 'gold' },
      { field: '备注', value: '中文值 with "quote"' },
    ],
    'cache:user:2': [{ field: 'name', value: 'Bob' }],
  };
  const DEMO_LIST = {
    'queue:jobs': [
      { index: 0, value: 'job-1' },
      { index: 1, value: 'job-2' },
      { index: 2, value: 'job-3' },
    ],
  };
  const DEMO_SET = { tags: ['redis', 'tauri', '中文标签'] };
  const DEMO_ZSET = {
    rank: [
      { member: 'alice', score: 9.5 },
      { member: 'bob', score: 3 },
    ],
  };
  const DEMO_STREAM = {
    events: [
      { id: '1726884000000-0', fields: [{ field: 'type', value: 'login' }, { field: 'user', value: 'alice' }] },
      { id: '1726884000001-0', fields: [{ field: 'type', value: 'logout' }, { field: 'user', value: 'bob' }] },
    ],
  };

  const DEFAULT_CONNECTIONS = [
    {
      id: 'conn_local',
      name: '本地开发',
      host: '127.0.0.1',
      port: 6379,
      type: 'single',
      readonly: false,
      separator: ':',
      username: '',
      password: '',
      tls: false,
      tls_insecure: false,
      connect_timeout_secs: null,
      command_timeout_secs: null,
    },
    {
      id: 'conn_readonly',
      name: '只读连接',
      host: '127.0.0.1',
      port: 6380,
      type: 'single',
      readonly: true,
      separator: ':',
      username: '',
      password: '',
      tls: false,
      tls_insecure: false,
      connect_timeout_secs: null,
      command_timeout_secs: null,
    },
    {
      id: 'conn_cluster',
      name: '集群测试',
      host: '127.0.0.1',
      port: 7001,
      type: 'cluster',
      readonly: false,
      separator: ':',
      username: '',
      password: '',
      tls: false,
      tls_insecure: false,
      connect_timeout_secs: null,
      command_timeout_secs: null,
    },
  ];

  // ---------------------------------------------------------------- 状态
  const state = {
    connections: DEFAULT_CONNECTIONS.map((c) => ({ ...c })),
    online: new Set(),
    version: '7.4.0',
    update: { stage: 'idle', downloaded: 0, total: 0, version: null, error: null },
    monitor: { running: false, lastConnId: null },
    latencyMs: 5,
    calls: [],
    failOnce: new Map(),
    // 分页：模拟 SCAN 游标，每页 4 条（真机 COUNT 只是提示，实际页大小可变）
    pageSize: 4,
  };

  const listeners = []; // { event, handlerId }

  function record(cmd, args) {
    state.calls.push({ cmd, args, at: Date.now() });
  }

  function delay(ms) {
    return new Promise((resolve) => setTimeout(resolve, ms == null ? state.latencyMs : ms));
  }

  function failIfForced(cmd) {
    if (!state.failOnce.has(cmd)) return null;
    const msg = state.failOnce.get(cmd);
    state.failOnce.delete(cmd);
    return msg;
  }

  function serverInfo() {
    return {
      redisVersion: state.version,
      os: 'Darwin arm64',
      uptimeSeconds: 3 * 86400 + 5 * 3600,
      usedMemory: 12 * 1024 * 1024,
      connectedClients: 7,
      dbKeys: DEMO_KEYS.length,
      db: 0,
      disk: { path: '/data', total: 500 * 1024 * 1024 * 1024, available: 200 * 1024 * 1024 * 1024, usedPercent: 60.2 },
    };
  }

  /** 按 SCAN 语义做「包含匹配 + 游标分页」，与后端 list_keys 的行为一致 */
  function listKeys(args) {
    const cursor = parseInt(args.cursor || '0', 10) || 0;
    const pattern = (args.pattern || '').toString().toLowerCase();
    const matched = DEMO_KEYS.filter((k) => !pattern || k.key.toLowerCase().includes(pattern));
    const slice = matched.slice(cursor, cursor + state.pageSize);
    const next = cursor + state.pageSize < matched.length ? String(cursor + state.pageSize) : '0';
    return { keys: slice.map((k) => ({ ...k })), nextCursor: next };
  }

  // ---------------------------------------------------------------- 命令实现
  const handlers = {
    'list_connections': () =>
      new URLSearchParams(location.search).get('empty') === '1' ? [] : state.connections.map((c) => ({ ...c })),
    'save_connection': (args) => {
      const conn = args.conn;
      const idx = state.connections.findIndex((c) => c.id === conn.id);
      if (idx >= 0) state.connections[idx] = { ...conn };
      else state.connections.push({ ...conn });
      return null;
    },
    'delete_connection': (args) => {
      const before = state.connections.length;
      state.connections = state.connections.filter((c) => c.id !== args.connId);
      state.online.delete(args.connId);
      return state.connections.length !== before;
    },
    'connect': (args) => {
      state.online.add(args.conn.id);
      return null;
    },
    'disconnect': (args) => {
      state.online.delete(args.connId);
      state.monitor.running = false;
      return null;
    },
    'test_connection': () => 'PONG',
    'export_connections': (args) => ({
      count: state.connections.length,
      content: JSON.stringify({ version: 1, connections: state.connections.map((c) => (args && args.includePasswords ? c : { ...c, password: null })) }, null, 2),
    }),
    'import_connections': (args) => {
      let doc = null;
      try {
        doc = JSON.parse(args.content || '{}');
      } catch (e) {
        return Promise.reject('连接配置不是合法 JSON: ' + e.message);
      }
      const list = (doc && doc.connections) || [];
      return { imported: list.length, skipped: 0, failed: [] };
    },
    'get_server_info': () => serverInfo(),
    'select_db': (args) => args.db,
    'list_keys': (args) => listKeys(args),
    'get_string': (args) => (args.key in DEMO_STRINGS ? DEMO_STRINGS[args.key] : null),
    'get_hash': (args) => DEMO_HASH[args.key] || [],
    'get_list': (args) => DEMO_LIST[args.key] || [],
    'get_set': (args) => DEMO_SET[args.key] || [],
    'get_zset': (args) => DEMO_ZSET[args.key] || [],
    'get_stream': (args) => (DEMO_STREAM[args.key] || []).slice(0, args.count || 200),
    'set_key': (args) => {
      const existing = DEMO_KEYS.find((k) => k.key === args.key);
      if (!existing) DEMO_KEYS.push({ key: args.key, type: 'string', ttl: args.ttl });
      DEMO_STRINGS[args.key] = args.value;
      return 'OK';
    },
    'set_key_ttl': () => 1,
    'del_key': (args) => {
      const before = DEMO_KEYS.length;
      for (const key of args.keys) {
        const i = DEMO_KEYS.findIndex((k) => k.key === key);
        if (i >= 0) DEMO_KEYS.splice(i, 1);
      }
      return before - DEMO_KEYS.length;
    },
    'rename_key': (args) => {
      const entry = DEMO_KEYS.find((k) => k.key === args.key);
      if (!entry) return Promise.reject('ERR no such key');
      if (DEMO_KEYS.some((k) => k.key === args.newKey) && !args.overwrite) {
        return Promise.reject('ERR target key name already exists');
      }
      entry.key = args.newKey;
      return null;
    },
    'copy_key': (args) => {
      const entry = DEMO_KEYS.find((k) => k.key === args.key);
      if (!entry) return Promise.reject('ERR no such key');
      if (DEMO_KEYS.some((k) => k.key === args.newKey) && !args.overwrite) {
        return Promise.reject('ERR target key already exists');
      }
      DEMO_KEYS.push({ ...entry, key: args.newKey });
      return null;
    },
    'hash_set_field': () => 1,
    'hash_del_fields': () => 1,
    'list_set_element': () => 'OK',
    'list_del_element': () => 1,
    'list_push_element': () => 1,
    'set_add_member': () => 1,
    'set_del_member': () => 1,
    'zset_add_member': () => 1,
    'zset_del_member': () => 1,
    'stream_add_entry': (args) => args.entryId || '1726884009999-0',
    'stream_del_entry': () => 1,
    'execute_command': (args) => {
      const line = (args.line || '').trim().toUpperCase();
      if (line === 'PING') return 'PONG';
      if (line.startsWith('DBSIZE')) return '(integer) ' + DEMO_KEYS.length;
      if (line.startsWith('GET')) return '"' + (DEMO_STRINGS['hello'] || '') + '"';
      if (line.startsWith('SET') && state.version.startsWith('6.0')) return 'OK';
      return '(integer) 1';
    },
    'export_keys': () => ({ count: DEMO_KEYS.length, content: JSON.stringify({ version: 1, keys: DEMO_KEYS }, null, 2) }),
    'import_keys': (args) => {
      let doc = null;
      try {
        doc = JSON.parse(args.content || '{}');
      } catch (e) {
        return Promise.reject('导入内容不是合法 JSON: ' + e.message);
      }
      const list = (doc && doc.keys) || [];
      return { imported: list.length, skipped: 0, failed: [] };
    },
    'start_monitor': (args) => {
      state.monitor.running = true;
      state.monitor.lastConnId = args.connId;
      return null;
    },
    'stop_monitor': () => {
      state.monitor.running = false;
      return true;
    },
    'monitor_status': () => state.monitor.running,
    // 更新：plugin:app|version 与四个 update 命令
    'plugin:app|version': () => '0.1.0',
    'check_update': () =>
      new URLSearchParams(location.search).get('update') === 'available'
        ? { available: true, version: '0.2.0', currentVersion: '0.1.0', notes: '· 新增实时监控\n· 修复分页', date: '2026-09-20T08:00:00Z' }
        : { available: false, version: null, currentVersion: '0.1.0', notes: null, date: null },
    'start_update_download': () => {
      state.update = { stage: 'downloading', downloaded: 0, total: 8 * 1024 * 1024, version: '0.2.0', error: null };
      return null;
    },
    'get_update_progress': () => {
      if (state.update.stage === 'downloading') {
        state.update.downloaded = Math.min(state.update.total, state.update.downloaded + state.update.total / 4);
      }
      return { ...state.update };
    },
    'install_update_and_restart': () => null,
    'plugin:event|listen': (args) => {
      listeners.push({ event: args.event, handlerId: args.handler });
      return null;
    },
  };

  // ---------------------------------------------------------------- 事件投递
  const callbacks = new Map();
  let nextCallbackId = 1;

  function transformCallback(cb, once) {
    const id = nextCallbackId++;
    callbacks.set(id, (data) => {
      if (once) callbacks.delete(id);
      return cb && cb(data);
    });
    return id;
  }

  function runCallback(id, data) {
    const cb = callbacks.get(id);
    if (cb) cb(data);
    else console.warn('[TAURI] Couldn\'t find callback id ' + id);
  }

  function invoke(cmd, args) {
    record(cmd, args);
    const forced = failIfForced(cmd);
    const handler = handlers[cmd];
    if (!handler) return Promise.reject('command ' + cmd + ' not found');
    return delay().then(() => {
      if (forced) throw forced;
      return handler(args || {});
    });
  }

  Object.defineProperty(window, '__TAURI_INTERNALS__', {
    configurable: true,
    value: {
      invoke,
      transformCallback,
      unregisterCallback: (id) => callbacks.delete(id),
      runCallback,
      callbacks,
      metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
      plugins: {},
    },
  });

  // ---------------------------------------------------------------- 场景控制
  window.__mock = {
    /** 收集到的 invoke 调用（断言用） */
    get calls() {
      return state.calls;
    },
    count(cmd) {
      return state.calls.filter((c) => c.cmd === cmd).length;
    },
    lastCall(cmd) {
      const hits = state.calls.filter((c) => c.cmd === cmd);
      return hits.length ? hits[hits.length - 1] : null;
    },
    resetCalls() {
      state.calls = [];
    },
    setConnections(list) {
      state.connections = list.map((c) => ({ ...c }));
    },
    setRedisVersion(v) {
      state.version = v;
    },
    setLatency(ms) {
      state.latencyMs = ms;
    },
    setPageSize(n) {
      state.pageSize = n;
    },
    /** 让下一次该命令失败一次（覆盖错误提示 / 版本降级路径） */
    failNext(cmd, message) {
      state.failOnce.set(cmd, message);
    },
    setUpdate(p) {
      state.update = { stage: 'idle', downloaded: 0, total: 0, version: null, error: null, ...p };
    },
    /** 按真实路径推一批监控行（后端 monitor.rs 的批量语义：lines + dropped） */
    emitMonitorLines(lines, dropped) {
      const payload = { connId: state.monitor.lastConnId || 'conn_local', lines: lines || [], dropped: dropped || 0 };
      for (const l of listeners.filter((x) => x.event === 'monitor:lines')) {
        runCallback(l.handlerId, { event: 'monitor:lines', id: l.handlerId, payload });
      }
    },
    emitMonitorEnd(payload) {
      for (const l of listeners.filter((x) => x.event === 'monitor:end')) {
        runCallback(l.handlerId, { event: 'monitor:end', id: l.handlerId, payload });
      }
    },
    /** 后端菜单事件（主题 / 检查更新） */
    emitMenu(event, payload) {
      for (const l of listeners.filter((x) => x.event === event)) {
        runCallback(l.handlerId, { event, id: l.handlerId, payload });
      }
    },
    /** 造一行监控输出，与 MonitorLine 的 JSON 字段一致（commands/monitor.rs） */
    monitorLine(overrides) {
      const time = (overrides && overrides.time) || '1726884000.123456';
      const db = (overrides && overrides.db) !== undefined ? overrides.db : 0;
      const client = (overrides && overrides.client) || '127.0.0.1:51234';
      const command = (overrides && overrides.command) || 'GET';
      const args = (overrides && overrides.args) || ['hello'];
      const raw = (overrides && overrides.raw) || time + ' [' + db + ' ' + client + '] "' + command + '" ' + args.map((a) => '"' + a + '"').join(' ');
      return { time, db, client, command, args, raw };
    },
    listeners,
  };
})();
