#!/usr/bin/env node
/**
 * 前端冒烟脚本（`docs/frontend-split-evaluation.md` 第 5 节的回归网）。
 *
 * 做什么：用 headless Chrome 加载 `frontend/index.html`，注入
 * `scripts/frontend-smoke-mock.js`（Tauri 后端替身）后按真实用户路径点击 ——
 * 连接、分页、搜索、四类集合编辑、重命名 / 复制、终端、监控、导入导出、更新 ——
 * 每一步断言 DOM 结果，并把截图写到 `target/frontend-smoke/`（`--out` 可改）。
 *
 * 为什么不用 puppeteer / playwright：前端是「无构建步骤」的原生 JS，这个脚本也遵守
 * 同一条约束 —— 只用 Node 内置能力（`node:http` + 内置 WebSocket 说 CDP），
 * 不引入 node_modules 与版本锁。Node 22+ 即可（内置 `fetch` / `WebSocket`）。
 *
 * 用法：
 *   node scripts/frontend-smoke.mjs                 # 全部场景 + 截图
 *   node scripts/frontend-smoke.mjs --no-shots      # 只跑断言，不落盘截图
 *   node scripts/frontend-smoke.mjs --only monitor  # 只跑名字含 monitor 的场景
 *   CHROME_PATH=/path/to/chrome node scripts/frontend-smoke.mjs
 *
 * 退出码：全部通过 0，有失败 1（可直接当 CI 门禁）。
 */
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { extname, join, resolve, sep } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(fileURLToPath(new URL('..', import.meta.url)));
const FRONTEND_DIR = join(ROOT, 'frontend');
const MOCK_FILE = join(ROOT, 'scripts', 'frontend-smoke-mock.js');

const argv = process.argv.slice(2);
const hasFlag = (f) => argv.includes(f);
const flagValue = (f, def) => {
  const i = argv.indexOf(f);
  return i >= 0 && argv[i + 1] ? argv[i + 1] : def;
};
const OUT_DIR = resolve(ROOT, flagValue('--out', 'target/frontend-smoke'));
const WRITE_SHOTS = !hasFlag('--no-shots');
const ONLY = flagValue('--only', '');
const KEEP = hasFlag('--keep');

// ------------------------------------------------------------------ 断言
function assertEq(actual, expected, msg) {
  if (actual !== expected) {
    throw new Error(`${msg}\n    实际: ${JSON.stringify(actual)}\n    期望: ${JSON.stringify(expected)}`);
  }
}
function assertOk(value, msg) {
  if (!value) throw new Error(`${msg}（实际: ${JSON.stringify(value)}）`);
}
function assertIncludes(haystack, needle, msg) {
  const text = haystack == null ? '' : String(haystack);
  if (!text.includes(needle)) {
    throw new Error(`${msg}\n    期望包含: ${JSON.stringify(needle)}\n    实际: ${JSON.stringify(text.slice(0, 300))}`);
  }
}
function assertNotIncludes(haystack, needle, msg) {
  const text = haystack == null ? '' : String(haystack);
  if (text.includes(needle)) throw new Error(`${msg}\n    不应包含: ${JSON.stringify(needle)}\n    实际: ${JSON.stringify(text.slice(0, 300))}`);
}
/** 轮询直到断言成立，用于「异步渲染完成后」的检查 */
async function assertEventually(fn, msg, timeout = 4000) {
  const deadline = Date.now() + timeout;
  let last;
  for (;;) {
    try {
      await fn();
      return;
    } catch (err) {
      last = err;
      if (Date.now() > deadline) throw new Error(`${msg}（等待 ${timeout}ms 仍未满足）: ${last.message}`);
      await new Promise((r) => setTimeout(r, 80));
    }
  }
}

// ------------------------------------------------------------------ 静态服务器
const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.woff2': 'font/woff2',
};

function startStaticServer() {
  const server = createServer((req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1');
    const rel = decodeURIComponent(url.pathname === '/' ? '/index.html' : url.pathname);
    const file = resolve(join(FRONTEND_DIR, rel));
    // 只服务 frontend/ 内的文件，挡掉 ../ 穿越
    if (!file.startsWith(FRONTEND_DIR + sep) && file !== FRONTEND_DIR) {
      res.writeHead(403).end('forbidden');
      return;
    }
    if (!existsSync(file) || !statSync(file).isFile()) {
      res.writeHead(404).end('not found');
      return;
    }
    res.writeHead(200, {
      'Content-Type': MIME[extname(file)] || 'application/octet-stream',
      'Cache-Control': 'no-store',
    });
    res.end(readFileSync(file));
  });
  return new Promise((resolvePromise) => {
    server.listen(0, '127.0.0.1', () => resolvePromise({ server, port: server.address().port }));
  });
}

// ------------------------------------------------------------------ Chrome
function findChrome() {
  const candidates = [
    process.env.CHROME_PATH,
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    '/Applications/Chromium.app/Contents/MacOS/Chromium',
    '/usr/bin/google-chrome',
    '/usr/bin/google-chrome-stable',
    '/usr/bin/chromium',
    '/usr/bin/chromium-browser',
    '/snap/bin/chromium',
  ].filter(Boolean);
  for (const c of candidates) if (existsSync(c)) return c;
  throw new Error('没找到 Chrome / Chromium，请用 CHROME_PATH 指定可执行文件');
}

async function launchChrome(exe, userDataDir) {
  const args = [
    '--headless=new',
    '--remote-debugging-port=0',
    `--user-data-dir=${userDataDir}`,
    '--no-first-run',
    '--no-default-browser-check',
    '--disable-extensions',
    '--disable-gpu',
    '--disable-dev-shm-usage',
    '--hide-scrollbars',
    '--force-device-scale-factor=1',
    '--window-size=1440,900',
    'about:blank',
  ];
  if (typeof process.getuid === 'function' && process.getuid() === 0) args.unshift('--no-sandbox');
  const child = spawn(exe, args, { stdio: ['ignore', 'pipe', 'pipe'] });
  const logs = [];
  child.stdout.on('data', (d) => logs.push(String(d)));
  child.stderr.on('data', (d) => logs.push(String(d)));

  const portFile = join(userDataDir, 'DevToolsActivePort');
  const deadline = Date.now() + 15000;
  while (!existsSync(portFile)) {
    if (child.exitCode !== null) throw new Error('Chrome 启动即退出：\n' + logs.join(''));
    if (Date.now() > deadline) throw new Error('等待 DevToolsActivePort 超时：\n' + logs.join(''));
    await new Promise((r) => setTimeout(r, 100));
  }
  const [port] = readFileSync(portFile, 'utf8').split('\n');
  return { child, port: Number(port) };
}

// ------------------------------------------------------------------ CDP 客户端
class Cdp {
  constructor(ws) {
    this.ws = ws;
    this.seq = 0;
    this.pending = new Map();
    this.onEvent = () => {};
    ws.addEventListener('message', (ev) => {
      const msg = JSON.parse(ev.data);
      if (msg.id && this.pending.has(msg.id)) {
        const { resolve: res, reject } = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        if (msg.error) reject(new Error(`${msg.error.message}（${JSON.stringify(msg.error.data ?? '')}）`));
        else res(msg.result);
        return;
      }
      if (msg.method) this.onEvent(msg);
    });
  }
  send(method, params = {}) {
    const id = ++this.seq;
    return new Promise((res, reject) => {
      this.pending.set(id, { resolve: res, reject });
      this.ws.send(JSON.stringify({ id, method, params }));
      setTimeout(() => {
        if (this.pending.has(id)) {
          this.pending.delete(id);
          reject(new Error(`CDP 超时: ${method}`));
        }
      }, 20000);
    });
  }
}

async function connectCdp(port) {
  const deadline = Date.now() + 10000;
  let targets = [];
  while (Date.now() < deadline) {
    try {
      targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      if (targets.some((t) => t.type === 'page')) break;
    } catch (e) {
      /* Chrome 还没起来，继续等 */
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  const page = targets.find((t) => t.type === 'page');
  if (!page) throw new Error('没有可用的 page target');
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((res, rej) => {
    ws.addEventListener('open', res, { once: true });
    ws.addEventListener('error', () => rej(new Error('连接 CDP WebSocket 失败')), { once: true });
  });
  return new Cdp(ws);
}

// ------------------------------------------------------------------ 页面操作
class Page {
  constructor(cdp) {
    this.cdp = cdp;
    this.baseUrl = '';
    this.consoleErrors = [];
    this.consoleWarns = [];
    this.exceptions = [];
    this.allowConsoleErrors = false;
  }

  async init(mockSource) {
    this.cdp.onEvent = (msg) => {
      if (msg.method === 'Runtime.consoleAPICalled') {
        const text = (msg.params.args || []).map((a) => a.value ?? a.description ?? a.type).join(' ');
        if (msg.params.type === 'error') this.consoleErrors.push(text);
        else if (msg.params.type === 'warning') this.consoleWarns.push(text);
      } else if (msg.method === 'Runtime.exceptionThrown') {
        const d = msg.params.exceptionDetails;
        this.exceptions.push(d.exception?.description || d.text);
      } else if (msg.method === 'Log.entryAdded') {
        // 只统计 JS 侧的错误；CDN 字体（离线跑）等网络噪声不算前端缺陷
        const e = msg.params.entry;
        if (e.level === 'error' && e.source !== 'network') this.consoleErrors.push(`[${e.source}] ${e.text}`);
      }
    };
    await this.cdp.send('Page.enable');
    await this.cdp.send('Runtime.enable');
    await this.cdp.send('Log.enable');
    await this.cdp.send('Network.enable');
    // 离线也能跑：把 CDN 字体请求挡掉（界面用系统字体渲染，不影响断言）
    await this.cdp.send('Network.setBlockedURLs', { urls: ['*cdnjs.cloudflare.com*'] });
    await this.cdp.send('Emulation.setDeviceMetricsOverride', {
      width: 1440, height: 900, deviceScaleFactor: 1, mobile: false,
    });
    await this.cdp.send('Page.addScriptToEvaluateOnNewDocument', { source: mockSource });
  }

  resetLogs() {
    this.consoleErrors = [];
    this.consoleWarns = [];
    this.exceptions = [];
  }

  async goto(baseUrl, query) {
    this.resetLogs();
    const url = `${baseUrl}/index.html?smoke=1${query ? '&' + query : ''}`;
    await this.cdp.send('Page.navigate', { url });
    await this.waitFor('!!window.__app && !!window.__TAURI_INTERNALS__ && document.readyState === "complete"', '页面加载完成');
  }

  async ev(expression, { awaitPromise = true } = {}) {
    const res = await this.cdp.send('Runtime.evaluate', {
      expression, awaitPromise, returnByValue: true, userGesture: true,
    });
    if (res.exceptionDetails) {
      throw new Error('页面执行异常: ' + (res.exceptionDetails.exception?.description || res.exceptionDetails.text));
    }
    return res.result.value;
  }

  async waitMs(ms) {
    await new Promise((r) => setTimeout(r, ms));
  }

  async waitFor(expression, label, timeout = 6000) {
    const deadline = Date.now() + timeout;
    for (;;) {
      if (await this.ev(expression).catch(() => false)) return;
      if (Date.now() > deadline) throw new Error(`等待超时（${label}）`);
      await this.waitMs(80);
    }
  }

  async click(sel) {
    const ok = await this.ev(
      `(() => { const el = document.querySelector(${JSON.stringify(sel)}); if (!el) return false; el.click(); return true; })()`
    );
    if (!ok) throw new Error(`点击失败，元素不存在: ${sel}`);
    await this.waitMs(20);
  }

  /** 模拟用户输入：改值 + 派发 input / change（原生 setter 之外的事件才是应用监听的） */
  async setValue(sel, value, { event = 'input' } = {}) {
    const ok = await this.ev(`(() => {
      const el = document.querySelector(${JSON.stringify(sel)});
      if (!el) return false;
      el.value = ${JSON.stringify(value)};
      el.dispatchEvent(new Event(${JSON.stringify(event)}, { bubbles: true }));
      return true;
    })()`);
    if (!ok) throw new Error(`输入失败，元素不存在: ${sel}`);
    await this.waitMs(20);
  }

  async check(sel, checked = true) {
    const ok = await this.ev(`(() => {
      const el = document.querySelector(${JSON.stringify(sel)});
      if (!el) return false;
      if (el.checked !== ${checked}) { el.click(); }
      return el.checked === ${checked};
    })()`);
    if (!ok) throw new Error(`勾选失败: ${sel}`);
    await this.waitMs(20);
  }

  async text(sel) {
    return this.ev(`(() => { const el = document.querySelector(${JSON.stringify(sel)}); return el ? el.textContent : null; })()`);
  }

  async prop(sel, name) {
    return this.ev(`(() => {
      const el = document.querySelector(${JSON.stringify(sel)});
      if (!el) return null;
      return ${JSON.stringify(name)}.split('.').reduce((o, k) => (o == null ? null : o[k]), el);
    })()`);
  }

  async attr(sel, name) {
    return this.ev(`(() => { const el = document.querySelector(${JSON.stringify(sel)}); return el ? el.getAttribute(${JSON.stringify(name)}) : null; })()`);
  }

  async count(sel) {
    return this.ev(`document.querySelectorAll(${JSON.stringify(sel)}).length`);
  }

  async shot(name) {
    if (!WRITE_SHOTS) return null;
    const res = await this.cdp.send('Page.captureScreenshot', { format: 'png' });
    const file = join(OUT_DIR, `${name}.png`);
    writeFileSync(file, Buffer.from(res.data, 'base64'));
    return file;
  }
}

// ------------------------------------------------------------------ 场景
const KEY_ROW = (key) => `#keyTree .node-key-select[data-key="${key}"]`;
const FOLDER_ROW = (path) => `#keyTree .node-folder-select[data-folder="${path}"]`;

/** 只点连接并等状态栏变成已连接 —— 不假设 key 列表能加载出来（错误路径场景用） */
async function connectRaw(page) {
  await page.waitFor('!!document.querySelector(\'#connectionList .connection-item[data-id="conn_local"]\')', '连接列表渲染');
  await page.click('#connectionList .connection-item[data-id="conn_local"] .refresh-conn');
  await page.waitFor('document.querySelector("#statusLabel").textContent.includes("已连接")', '连接建立');
}

/** 连接单机连接并等第一页 key 渲染出来 —— 分页相关的场景用它 */
async function connectOnly(page) {
  await connectRaw(page);
  await page.waitFor('document.querySelectorAll("#keyTree .vt-row").length > 0', 'Key 树出现行');
}

/** 把分页拉完（key 树按「加载更多」逐页追加） */
async function loadAllPages(page) {
  for (let i = 0; i < 20; i++) {
    const done = await page.ev('document.getElementById("keyTreeStatus").textContent.includes("已全部加载")');
    if (done) return;
    const clicked = await page.ev(
      '(() => { const b = document.querySelector("#keyTreeStatus .vt-more"); if (!b) return false; b.click(); return true; })()'
    );
    if (!clicked) return;
    await page.waitMs(120);
  }
  throw new Error('分页没有在 20 次「加载更多」内结束');
}

/** 连接并把分页全部拉完 —— 需要「点某个 key」的场景用它（虚拟滚动只渲染视口内的行） */
async function connectLocal(page) {
  await connectOnly(page);
  await loadAllPages(page);
}

/**
 * 逐层展开文件夹直到能看见目标 key。
 * 分隔符是单字符，`cache:user:1` 会先落进 `cache` 再落进 `cache:user`（嵌套文件夹），
 * 因此要按前缀逐级展开 —— 顺便把嵌套折叠这条路径也测到。
 */
async function revealKey(page, key, sep = ':') {
  const parts = key.split(sep);
  let prefix = '';
  for (let i = 0; i < parts.length - 1; i++) {
    prefix = prefix ? prefix + sep + parts[i] : parts[i];
    await page.click(FOLDER_ROW(prefix));
    await page.waitMs(80);
  }
  await assertEventually(async () => assertEq(await page.count(KEY_ROW(key)), 1, ''), `展开后应看到 ${key}`);
}

const scenarios = [
  {
    name: '01-empty-state',
    query: 'empty=1',
    steps: [
      ['连接列表为空态', async (p) => assertIncludes(await p.text('#connectionList'), '暂无连接')],
      ['Key 树提示先连接', async (p) => assertIncludes(await p.text('#keyTree'), '请先连接 Redis 服务器')],
      ['详情区提示选择 Key', async (p) => assertIncludes(await p.text('#keyDetail'), '选择一个 Key')],
      ['监控按钮禁用', async (p) => {
        assertEq(await p.prop('#monitorToggle', 'disabled'), true, '未连接时监控按钮应禁用');
        assertIncludes(await p.attr('#monitorToggle', 'title'), '请先连接', '禁用原因应写在 title 里');
      }],
      ['数据库选择器禁用', async (p) => assertEq(await p.prop('#dbSelect', 'disabled'), true, '未连接时 Db 选择器应禁用')],
      ['终端就绪', async (p) => assertIncludes(await p.text('#terminalBody'), 'Redis 终端已就绪')],
      ['版本号已回填', async (p) => assertEventually(async () => assertEq(await p.text('#appVersion'), 'v0.1.0', '标题栏版本号应来自安装包'), '版本号')],
      ['截图', async (p) => p.shot('01-empty-state')],
    ],
  },
  {
    name: '02-connect-modal',
    query: 'empty=1',
    steps: [
      ['打开新建连接对话框', async (p) => {
        await p.click('#btnNewConnection');
        assertEq(await p.attr('#connectionModal', 'class'), 'modal-overlay open', '对话框应打开');
        assertEq(await p.text('#modalTitle'), '新建连接', '标题');
      }],
      ['跳过证书校验默认隐藏', async (p) => assertEq(await p.prop('#connTlsInsecureWrap', 'style.visibility'), 'hidden', '未勾 TLS 时隐藏')],
      ['勾选 TLS 后出现「跳过证书校验」', async (p) => {
        await p.check('#connTls', true);
        assertEq(await p.prop('#connTlsInsecureWrap', 'style.visibility'), 'visible', '勾 TLS 后应显示');
      }],
      ['rediss:// 前缀在未勾 TLS 时被拦', async (p) => {
        await p.check('#connTls', false);
        await p.setValue('#connHost', 'rediss://127.0.0.1');
        await p.click('#modalTest');
        await assertEventually(async () => assertIncludes(await p.text('#testStatus'), '请勾选「TLS 加密」'), '应提示去勾 TLS');
      }],
      ['勾上 TLS 后测试连接成功', async (p) => {
        await p.check('#connTls', true);
        await p.click('#modalTest');
        await assertEventually(async () => assertIncludes(await p.text('#testStatus'), '连接成功（PONG）'), '测试连接应成功');
      }],
      ['带端口的 rediss:// 仍被拦', async (p) => {
        await p.setValue('#connHost', 'rediss://127.0.0.1:6379');
        await p.click('#modalTest');
        await assertEventually(async () => assertIncludes(await p.text('#testStatus'), '只需填主机名'), '带端口应被拦下');
      }],
      ['超时输入参与建连参数', async (p) => {
        await p.setValue('#connHost', '127.0.0.1');
        await p.setValue('#connConnectTimeout', '1');
        await p.setValue('#connCommandTimeout', '30');
        await p.click('#modalTest');
        await assertEventually(async () => assertIncludes(await p.text('#testStatus'), '连接成功'), '测试连接应成功');
        const call = await p.ev('window.__mock.lastCall("test_connection").args.conn');
        assertEq(call.connect_timeout_secs, 1, '建连超时应传入');
        assertEq(call.command_timeout_secs, 30, '命令超时应传入');
        assertEq(call.tls, true, 'TLS 开关应传入');
      }],
      ['截图', async (p) => p.shot('02-connect-modal')],
      ['取消关闭对话框', async (p) => {
        await p.click('#modalCancel');
        assertEq(await p.attr('#connectionModal', 'class'), 'modal-overlay', '对话框应关闭');
      }],
    ],
  },
  {
    name: '03-connect-and-list',
    steps: [
      ['连接本地实例', async (p) => {
        await connectOnly(p);
        assertIncludes(await p.text('#statusLabel'), '已连接 · 127.0.0.1:6379', '状态栏');
      }],
      ['服务端信息回填侧栏', async (p) => {
        assertEq(await p.text('#statKeys'), '9', 'Key 总数');
        assertEq(await p.text('#statMem'), '12.0MB', '内存');
        assertEq(await p.text('#statConn'), '7', '连接数');
        assertIncludes(await p.text('#statDisk'), '60.2%', '磁盘使用率');
      }],
      ['第一页只加载 4 个 key，并给「加载更多」入口', async (p) => {
        assertIncludes(await p.text('#keyTreeStatus'), '已加载 4 个 Key', '分页状态条');
        assertIncludes(await p.text('#keyTreeStatus'), '加载更多', '未扫完时应给显式入口');
        assertOk((await p.count('#keyTree .node-folder-select')) >= 1, '第一页就应出现文件夹行');
        assertEq(await p.count(KEY_ROW('hello')), 0, '第二页的 key 此时还不该出现');
      }],
      ['「加载更多」逐页追加直到扫完', async (p) => {
        await p.click('#keyTreeStatus .vt-more');
        await assertEventually(async () => assertIncludes(await p.text('#keyTreeStatus'), '已加载 8 个 Key'), '第二页');
        await p.click('#keyTreeStatus .vt-more');
        await assertEventually(async () => assertIncludes(await p.text('#keyTreeStatus'), '已全部加载'), '第三页应扫完');
        assertEq(await p.ev('window.__app.allKeys.length'), 9, '全部 9 个 key 已进入本地列表');
      }],
      ['Key 树按分隔符折叠成文件夹，顶层 key 直接可见', async (p) => {
        assertOk((await p.count('#keyTree .node-folder-select')) >= 4, '应有文件夹行（app / cache / queue / session）');
        assertEq(await p.count(FOLDER_ROW('cache')), 1, 'cache 文件夹');
        assertEq(await p.count(KEY_ROW('hello')), 1, '顶层 key 应直接可见');
      }],
      ['展开文件夹显示子 key（嵌套两层）', async (p) => {
        await p.click(FOLDER_ROW('cache'));
        await assertEventually(async () => assertEq(await p.count(FOLDER_ROW('cache:user')), 1, ''), 'cache 下应出现 cache:user 子文件夹');
        await p.click(FOLDER_ROW('cache:user'));
        await assertEventually(async () => assertEq(await p.count(KEY_ROW('cache:user:1')), 1, ''), '再展开一层才看到叶子 key');
      }],
      ['截图', async (p) => p.shot('03-connected-list')],
    ],
  },
  {
    name: '04-string-detail',
    steps: [
      ['连接并选中 string key', async (p) => {
        await connectLocal(p);
        await p.click(KEY_ROW('hello'));
        await assertEventually(async () => assertIncludes(await p.text('#keyDetail'), 'STRING'), '详情区类型徽章');
      }],
      ['值从后端拉取并回填编辑器', async (p) => {
        await assertEventually(async () => {
          const v = await p.prop('#valueEditor', 'value');
          assertIncludes(v, '"name":"麦地缓存"', '应显示原始 JSON 文本');
        }, '字符串值加载');
        assertIncludes(await p.text('#keyDetail'), '永不过期', 'TTL 展示');
      }],
      ['格式化按钮美化 JSON', async (p) => {
        await p.click('#editorFormat');
        const v = await p.prop('#valueEditor', 'value');
        assertIncludes(v, '\n', '应换行美化');
        assertIncludes(v, '  "name"', '应缩进');
      }],
      ['复制到剪贴板不报错', async (p) => {
        await p.click('#editorCopy');
        await assertEventually(async () => assertIncludes(await p.text('.toast-notification'), '已复制到剪贴板'), '复制提示');
      }],
      ['截图', async (p) => p.shot('04-string-detail')],
    ],
  },
  {
    name: '05-hash-detail',
    steps: [
      ['连接并展开 cache 文件夹', async (p) => {
        await connectLocal(p);
        await revealKey(p, 'cache:user:1');
      }],
      ['选中 hash key 渲染字段表格', async (p) => {
        await p.click(KEY_ROW('cache:user:1'));
        await assertEventually(async () => assertEq(await p.count('#collView .coll-row'), 4, ''), '3 个字段 + 1 行新增');
        // 字段名在 span 里，值在 input.value 里（textContent 看不到 input 的值）
        assertIncludes(await p.text('#collView'), '备注', '字段名');
        assertEq(await p.prop('#collVal0', 'value'), 'Alice', '第一个字段的值');
        assertEq(await p.prop('#collVal2', 'value'), '中文值 with "quote"', '中文与引号应原样渲染');
        assertEq(await p.count('#collView [data-act="hset"]'), 3, '每行一个保存按钮');
      }],
      ['字段级保存调用后端的 hash_set_field', async (p) => {
        await p.ev('window.__mock.resetCalls()');
        await p.setValue('#collVal1', 'platinum');
        await p.click('#collView [data-act="hset"][data-field="level"]');
        await assertEventually(async () => assertOk(await p.ev('window.__mock.count("hash_set_field") === 1'), '应调用 hash_set_field'), '字段保存');
        const args = await p.ev('window.__mock.lastCall("hash_set_field").args');
        assertEq(args.field, 'level', '字段名');
        assertEq(args.value, 'platinum', '新值');
        // 保存成功后重新拉一次内容（以服务器为准），不是只改本地 DOM
        await assertEventually(async () => assertOk(await p.ev('window.__mock.count("get_hash") >= 1'), '保存后应重新读取内容'), '重新读取');
      }],
      ['截图', async (p) => p.shot('05-hash-detail')],
    ],
  },
  {
    name: '06-rename-copy',
    steps: [
      ['连接并选中 key', async (p) => {
        await connectLocal(p);
        await p.click(KEY_ROW('hello'));
        await assertEventually(async () => assertIncludes(await p.text('#keyDetail'), 'hello'), '详情区');
      }],
      ['重命名对话框带默认目标名与说明', async (p) => {
        await p.click('#detailRename');
        assertEq(await p.attr('#keyOpModal', 'class'), 'modal-overlay open', '对话框应打开');
        assertEq(await p.text('#keyOpTitle'), '重命名 Key', '标题');
        assertEq(await p.prop('#keyOpSource', 'value'), 'hello', '源 key');
        assertEq(await p.prop('#keyOpDest', 'value'), 'hello', '重命名默认原名（原地改名）');
        assertIncludes(await p.text('#keyOpHint'), 'TTL 一起带走', '说明文案');
        assertIncludes(await p.text('#keyOpOverwriteHint'), 'RENAMENX', '不覆盖语义');
        assertEq(await p.prop('#keyOpOverwrite', 'checked'), false, '默认不覆盖');
      }],
      ['截图', async (p) => p.shot('06-rename-dialog')],
      ['提交重命名：本地列表跟着搬', async (p) => {
        await p.setValue('#keyOpDest', 'hello:renamed');
        await p.click('#keyOpConfirm');
        await assertEventually(async () => assertEq(await p.ev('window.__mock.count("rename_key")'), 1, ''), '应调用 rename_key');
        const args = await p.ev('window.__mock.lastCall("rename_key").args');
        assertEq(args.key, 'hello', '源 key');
        assertEq(args.newKey, 'hello:renamed', '目标 key');
        assertEq(args.overwrite, false, '默认不覆盖');
        await assertEventually(async () => assertOk(await p.ev('window.__app.KEY_DATA["hello:renamed"] !== undefined'), '本地列表应出现新名'), '重命名后本地同步');
        assertEq(await p.ev('window.__app.KEY_DATA["hello"] === undefined'), true, '旧名应被摘掉');
      }],
      ['目标已存在时报错并保留对话框', async (p) => {
        await p.ev('window.__mock.failNext("copy_key", "ERR target key already exists")');
        await p.click('#detailCopyKey');
        await p.setValue('#keyOpDest', 'rank');
        await p.click('#keyOpConfirm');
        await assertEventually(async () => assertIncludes(await p.text('.toast-notification'), '复制失败'), '复制失败提示');
        assertEq(await p.attr('#keyOpModal', 'class'), 'modal-overlay open', '失败后对话框应留着让用户改');
      }],
      ['勾选覆盖后带 REPLACE 语义重试', async (p) => {
        await p.check('#keyOpOverwrite', true);
        await p.click('#keyOpConfirm');
        await assertEventually(async () => assertEq(await p.ev('window.__mock.count("copy_key")'), 2, ''), '第二次调用');
        const args = await p.ev('window.__mock.lastCall("copy_key").args');
        assertEq(args.overwrite, true, '勾选后覆盖');
        await assertEventually(async () => assertEq(await p.attr('#keyOpModal', 'class'), 'modal-overlay', ''), '成功后关闭');
      }],
    ],
  },
  {
    name: '07-copy-version-gate',
    steps: [
      ['Redis 6.0 上连接', async (p) => {
        await p.ev('window.__mock.setRedisVersion("6.0.16")');
        await connectLocal(p);
      }],
      ['COPY 按钮按版本置灰并说明原因', async (p) => {
        await p.click(KEY_ROW('hello'));
        await assertEventually(async () => assertIncludes(await p.text('#keyDetail'), 'STRING'), '详情区');
        assertEq(await p.prop('#detailCopyKey', 'disabled'), true, '6.0 下复制应禁用');
        assertIncludes(await p.attr('#detailCopyKey', 'title'), 'COPY 需要 Redis 6.2', '应说明版本要求');
        assertEq(await p.prop('#detailRename', 'disabled'), false, '重命名不受版本限制');
      }],
      ['截图', async (p) => p.shot('07-copy-version-gate')],
      ['Redis 7.x 上按钮恢复可用', async (p) => {
        await p.ev('window.__mock.setRedisVersion("7.4.0")');
        await p.click('#connectionList .connection-item[data-id="conn_local"] .refresh-conn');
        // 重连会复位选中项（切连接时清 key 列表），重新选中第一页里的某个 key 再看按钮
        await assertEventually(async () => assertEq(await p.count(KEY_ROW('events')), 1, ''), '重连后 key 列表回来');
        await p.click(KEY_ROW('events'));
        await assertEventually(async () => assertEq(await p.prop('#detailCopyKey', 'disabled'), false, ''), '重连后按新版本放行');
      }],
    ],
  },
  {
    name: '08-monitor',
    steps: [
      ['连接后切到监控标签', async (p) => {
        await connectLocal(p);
        await p.click('#tabMonitor');
        assertEq(await p.ev('document.getElementById("terminalPanel").classList.contains("monitor-active")'), true, '面板应切到监控视图');
        assertEq(await p.ev('document.getElementById("tabMonitor").classList.contains("active")'), true, '标签应高亮');
        assertIncludes(await p.text('#monitorState'), '未开始', '初始状态');
      }],
      ['开始监控', async (p) => {
        await p.click('#monitorToggle');
        await assertEventually(async () => assertEq(await p.ev('window.__mock.count("start_monitor")'), 1, ''), '应调用 start_monitor');
        await assertEventually(async () => assertIncludes(await p.text('#monitorState'), '监控中'), '状态文案');
        assertIncludes(await p.text('#monitorToggle'), '停止监控', '按钮应变为停止');
      }],
      ['收到批量监控行后渲染表格', async (p) => {
        await p.ev(`window.__mock.emitMonitorLines([
          window.__mock.monitorLine({ command: 'GET', args: ['hello'] }),
          window.__mock.monitorLine({ command: 'HGETALL', args: ['cache:user:1'] }),
          window.__mock.monitorLine({ command: 'SET', args: ['中文key', '带"引号"的值'] }),
        ])`);
        await assertEventually(async () => assertEq(await p.count('#monitorBody .mon-row'), 3, ''), '三行应渲染出来');
        assertIncludes(await p.text('#monitorCount'), '已显示 3 / 3 行', '计数');
        assertIncludes(await p.text('#monitorBody'), '中文key', '参数逐字还原');
      }],
      ['省略行数如实上报', async (p) => {
        await p.ev('window.__mock.emitMonitorLines([window.__mock.monitorLine({command:"PING"})], 12)');
        await assertEventually(async () => assertIncludes(await p.text('#monitorCount'), '已省略 12 行'), '省略提示');
      }],
      ['过滤按原文包含匹配', async (p) => {
        await p.setValue('#monitorFilter', 'hgetall');
        await assertEventually(async () => assertEq(await p.count('#monitorBody .mon-row'), 1, ''), '只留 HGETALL 行');
        assertIncludes(await p.text('#monitorCount'), '已显示 1 / 16 行', '过滤后的计数（4 行 + 12 行被后端省略）');
        await p.setValue('#monitorFilter', '不存在的命令');
        await assertEventually(async () => assertIncludes(await p.text('#monitorBody'), '没有匹配的命令'), '空态提示');
      }],
      ['截图', async (p) => {
        await p.setValue('#monitorFilter', '');
        await p.waitMs(100);
        await p.shot('08-monitor');
      }],
      ['停止监控后状态复位', async (p) => {
        await p.click('#monitorToggle');
        await assertEventually(async () => assertEq(await p.ev('window.__mock.count("stop_monitor")'), 1, ''), '应调用 stop_monitor');
        await assertEventually(async () => assertIncludes(await p.text('#monitorState'), '已停止监控'), '状态文案');
      }],
      ['监控结束事件（连接断开）触发错误提示', async (p) => {
        await p.ev('window.__mock.emitMonitorEnd({ connId: "conn_local", stoppedByUser: false, reason: "连接已断开" })');
        await assertEventually(async () => assertIncludes(await p.text('.toast-notification'), '监控已结束'), '应提示');
      }],
      ['集群连接禁用监控并说明原因', async (p) => {
        await p.waitFor('!!document.querySelector(\'#connectionList .connection-item[data-id="conn_cluster"]\')', '集群连接出现在列表');
        await p.click('#connectionList .connection-item[data-id="conn_cluster"] .refresh-conn');
        await assertEventually(async () => assertEq(await p.prop('#monitorToggle', 'disabled'), true, ''), '集群下应禁用');
        assertIncludes(await p.attr('#monitorToggle', 'title'), '集群模式不支持', '禁用原因');
        assertIncludes(await p.text('#monitorState'), '集群模式不支持', '状态文案');
        assertEq(await p.prop('#dbSelectorWrap', 'style.display'), 'none', '集群隐藏 Db 选择器');
      }],
    ],
  },
  {
    name: '09-terminal',
    steps: [
      ['连接后执行 PING', async (p) => {
        await connectLocal(p);
        await p.setValue('#terminalInput', 'PING');
        await p.click('#terminalSend');
        await assertEventually(async () => assertIncludes(await p.text('#terminalBody'), 'PONG'), '应显示 PONG');
        const call = await p.ev('window.__mock.lastCall("execute_command").args');
        assertEq(call.line, 'PING', '命令原样转发');
        assertEq(await p.prop('#terminalInput', 'value'), '', '发送后清空输入框');
      }],
      ['help 走本地提示不转发', async (p) => {
        await p.ev('window.__mock.resetCalls()');
        await p.setValue('#terminalInput', 'help');
        await p.click('#terminalSend');
        await assertEventually(async () => assertIncludes(await p.text('#terminalBody'), '命令直接转发到已连接的 Redis 服务器执行'), '帮助文案');
        assertEq(await p.ev('window.__mock.count("execute_command")'), 0, 'help 不应发给后端');
      }],
      ['清空终端', async (p) => {
        await p.click('#termClear');
        assertIncludes(await p.text('#terminalBody'), '终端已清空', '清空提示');
        assertNotIncludes(await p.text('#terminalBody'), 'PONG', '旧输出应清掉');
      }],
      ['命令报错时显示错误行', async (p) => {
        await p.ev('window.__mock.failNext("execute_command", "ERR unknown command")');
        await p.setValue('#terminalInput', 'BOGUS');
        await p.click('#terminalSend');
        await assertEventually(async () => assertIncludes(await p.text('#terminalBody'), 'ERR unknown command'), '错误输出');
      }],
      ['截图', async (p) => p.shot('09-terminal')],
    ],
  },
  {
    name: '10-search-and-db',
    steps: [
      ['连接', async (p) => connectLocal(p)],
      ['搜索下推后端 SCAN MATCH（防抖 300ms）', async (p) => {
        await p.ev('window.__mock.resetCalls()');
        await p.setValue('#keySearch', 'cache');
        await assertEventually(async () => assertEq(await p.ev('window.__mock.count("list_keys")'), 1, ''), '应只发一次请求', 3000);
        const args = await p.ev('window.__mock.lastCall("list_keys").args');
        assertEq(args.pattern, 'cache', '搜索词应下推');
        await assertEventually(async () => assertIncludes(await p.text('#keyTreeStatus'), '匹配 2 个 Key'), '命中 2 个');
        assertEq(await p.count('#keyTree .node-folder-select'), 0, '搜索时不折叠文件夹');
      }],
      ['清空搜索恢复折叠视图', async (p) => {
        await p.setValue('#keySearch', '');
        await assertEventually(async () => assertEq(await p.count(FOLDER_ROW('cache')), 1, ''), '首页的文件夹应回来');
        await loadAllPages(p);
      }],
      ['切换数据库清空选中与列表', async (p) => {
        await p.click(KEY_ROW('hello'));
        await assertEventually(async () => assertIncludes(await p.text('#keyDetail'), 'hello'), '先选中');
        await p.setValue('#dbSelect', '3', { event: 'change' });
        await assertEventually(async () => assertEq(await p.ev('window.__mock.lastCall("select_db").args.db'), 3, ''), '应调用 select_db');
        await assertEventually(async () => assertIncludes(await p.text('#keyDetail'), '选择一个 Key'), '切库后详情区复位');
        assertIncludes(await p.text('#terminalBody'), '已切换到数据库 Db3', '终端应有回显');
      }],
    ],
  },
  {
    name: '11-delete-confirm',
    steps: [
      ['连接并选中 key', async (p) => {
        await connectLocal(p);
        await p.click(KEY_ROW('tags'));
        await assertEventually(async () => assertIncludes(await p.text('#keyDetail'), 'SET'), '详情区');
      }],
      ['删除走确认框（不是原生 confirm）', async (p) => {
        await p.click('#detailDelete');
        assertEq(await p.attr('#confirmModal', 'class'), 'modal-overlay open', '确认框应打开');
        assertIncludes(await p.text('#confirmMessage'), '确定要删除 Key「tags」吗', '确认文案');
        assertEq(await p.text('#confirmOk'), '删除', '确认按钮文案');
        await p.click('#confirmOk');
        await assertEventually(async () => assertEq(await p.ev('window.__mock.count("del_key")'), 1, ''), '应调用 del_key');
        await assertEventually(async () => assertIncludes(await p.text('.toast-notification'), 'Key 已删除'), '成功提示');
        assertEq(await p.ev('window.__app.KEY_DATA["tags"] === undefined'), true, '本地应摘掉该 key');
        assertIncludes(await p.text('#keyDetail'), '选择一个 Key', '详情区复位');
      }],
      ['取消不删除', async (p) => {
        await p.ev('window.__mock.resetCalls()');
        await p.click(KEY_ROW('rank'));
        await assertEventually(async () => assertIncludes(await p.text('#keyDetail'), 'ZSET'), '选中 zset');
        await p.click('#detailDelete');
        await p.click('#confirmCancel');
        assertEq(await p.ev('window.__mock.count("del_key")'), 0, '取消后不应删除');
        assertEq(await p.attr('#confirmModal', 'class'), 'modal-overlay', '确认框应关闭');
      }],
      ['截图', async (p) => p.shot('11-zset-detail')],
    ],
  },
  {
    name: '12-add-key',
    steps: [
      ['连接后打开新增 Key 对话框', async (p) => {
        await connectLocal(p);
        await p.click('#btnAddKey');
        assertEq(await p.attr('#addKeyModal', 'class'), 'modal-overlay open', '对话框应打开');
      }],
      ['TTL 非法时拦截', async (p) => {
        await p.setValue('#addKeyName', 'brand:new');
        await p.setValue('#addKeyTtl', '-5');
        await p.click('#addKeySave');
        await assertEventually(async () => assertIncludes(await p.text('.toast-notification'), 'TTL 必须为 -1 或正整数'), 'TTL 校验');
        assertEq(await p.ev('window.__mock.count("set_key")'), 0, '校验失败不应发请求');
      }],
      ['创建成功：本地插入并选中', async (p) => {
        await p.setValue('#addKeyTtl', '30');
        await p.setValue('#addKeyValue', 'fresh-value');
        await p.click('#addKeySave');
        await assertEventually(async () => assertEq(await p.ev('window.__mock.count("set_key")'), 1, ''), '应调用 set_key');
        const args = await p.ev('window.__mock.lastCall("set_key").args');
        assertEq(args.ttl, 30, 'TTL 应传 -1 或正整数');
        assertEq(args.value, 'fresh-value', '值');
        assertEq(await p.ev('window.__app.KEY_DATA["brand:new"].ttl'), 30, '本地应插入新 key');
        assertEq(await p.attr('#addKeyModal', 'class'), 'modal-overlay', '成功后关闭');
      }],
      ['重名被本地拦下', async (p) => {
        await p.click('#btnAddKey');
        await p.setValue('#addKeyName', 'brand:new');
        await p.click('#addKeySave');
        await assertEventually(async () => assertIncludes(await p.text('.toast-notification'), 'Key 已存在'), '重名提示');
      }],
    ],
  },
  {
    name: '13-conn-export',
    steps: [
      ['导出连接配置走确认框（可勾选含密码）', async (p) => {
        await p.click('#btnExportConnections');
        assertEq(await p.attr('#confirmModal', 'class'), 'modal-overlay open', '确认框');
        assertIncludes(await p.text('#confirmMessage'), '将把 3 个连接配置导出为 JSON 文件', '确认文案');
        assertEq(await p.prop('#confirmOption', 'style.display'), 'flex', '应显示「包含密码」复选框');
        assertIncludes(await p.text('#confirmOptionLabel'), '包含密码', '复选框说明');
        await p.click('#confirmOk');
        await assertEventually(async () => assertEq(await p.ev('window.__mock.count("export_connections")'), 1, ''), '应调用 export_connections');
        assertEq(await p.ev('window.__mock.lastCall("export_connections").args.includePasswords'), true, '默认含密码');
        await assertEventually(async () => assertIncludes(await p.text('.toast-notification'), '已导出 3 个连接配置'), '成功提示');
      }],
      ['Key 导出为整库枚举（不传 keys）', async (p) => {
        await connectLocal(p);
        await p.click('#btnExport');
        await assertEventually(async () => assertEq(await p.ev('window.__mock.count("export_keys")'), 1, ''), '应调用 export_keys');
        assertEq(await p.ev('window.__mock.lastCall("export_keys").args.keys'), null, '不传 keys，由后端全量枚举');
        await assertEventually(async () => assertIncludes(await p.text('.toast-notification'), '已导出 9 个 Key'), '成功提示');
      }],
    ],
  },
  {
    name: '14-theme-layout',
    steps: [
      ['默认主题（深秋）', async (p) => {
        assertEq(await p.attr('html', 'data-theme'), 'autumn', '默认主题');
        assertEq(await p.count('#themeMenu .theme-option.active'), 1, '菜单里应有一个选中项');
      }],
      ['切主题并持久化', async (p) => {
        await p.click('#btnTheme');
        assertEq(await p.attr('#themeMenu', 'class'), 'theme-menu open', '菜单应打开');
        await p.click('#themeMenu .theme-option[data-theme-value="spring"]');
        assertEq(await p.attr('html', 'data-theme'), 'spring', '主题应切换');
        assertEq(await p.ev('localStorage.getItem("mc_cache_theme")'), 'spring', '应写入 localStorage');
      }],
      ['后端菜单事件也能切主题', async (p) => {
        await p.ev('window.__mock.emitMenu("menu:set-theme", "winter")');
        await assertEventually(async () => assertEq(await p.attr('html', 'data-theme'), 'winter', ''), '菜单事件切主题');
      }],
      ['侧栏折叠 + 终端折叠', async (p) => {
        await p.click('#btnSidebarToggle');
        assertEq(await p.ev('document.getElementById("sidebar").classList.contains("collapsed")'), true, '侧栏应折叠');
        assertEq(await p.ev('document.getElementById("splitterSidebar").classList.contains("hidden")'), true, '折叠时隐藏拖拽手柄');
        await p.click('#terminalToggle');
        assertEq(await p.ev('document.getElementById("terminalPanel").classList.contains("collapsed")'), true, '终端应折叠');
        // 布局落盘有 300ms 防抖
        await p.waitMs(400);
        assertEq(await p.ev('JSON.parse(localStorage.getItem("myredis.layout")).sidebarCollapsed'), true, '布局应持久化');
      }],
      ['截图', async (p) => p.shot('14-theme-spring')],
    ],
  },
  {
    name: '15-update-flow',
    query: 'update=available',
    steps: [
      ['启动后静默检查发现新版本', async (p) => {
        await assertEventually(async () => assertEq(await p.ev('document.getElementById("btnCheckUpdate").classList.contains("show")'), true, ''), '更新入口应出现', 6000);
        assertEq(await p.ev('document.getElementById("updateDot").classList.contains("show")'), true, '小红点应出现');
        assertIncludes(await p.attr('#btnCheckUpdate', 'title'), '发现新版本 v0.2.0', 'title 提示');
        await assertEventually(async () => assertIncludes(await p.text('.toast-notification'), '发现新版本 v0.2.0'), '轻提示一次');
      }],
      ['点更新入口弹出「发现新版本」', async (p) => {
        await p.click('#btnCheckUpdate');
        await assertEventually(async () => assertEq(await p.attr('#updateModal', 'class'), 'modal-overlay open', ''), '弹窗应打开');
        assertEq(await p.text('#updateModalTitle'), '发现新版本', '标题');
        assertIncludes(await p.text('#updateModalBody'), 'v0.2.0', '版本号');
        assertIncludes(await p.text('#updateModalBody'), '新增实时监控', '更新说明');
      }],
      ['截图', async (p) => p.shot('15-update-available')],
      ['确认后后台下载并画进度条', async (p) => {
        await p.click('#updateModalOk');
        await assertEventually(async () => assertEq(await p.ev('window.__mock.count("start_update_download")'), 1, ''), '应开始下载');
        await assertEventually(async () => assertIncludes(await p.text('#updateBarText'), '正在后台下载更新 v0.2.0'), '进度条文案');
        assertEq(await p.ev('document.getElementById("updateBar").classList.contains("open")'), true, '进度条应显示');
        await assertEventually(async () => assertOk(await p.ev('document.querySelector("#updateBarPct").textContent.includes("%")'), '百分比应出现'), '下载百分比');
      }],
      ['下载完成弹「更新已就绪」并给重启入口', async (p) => {
        await p.ev('window.__mock.setUpdate({stage:"ready", downloaded: 8388608, total: 8388608, version:"0.2.0"})');
        await assertEventually(async () => assertEq(await p.text('#updateModalTitle'), '更新已就绪', ''), '就绪弹窗', 4000);
        assertEq(await p.prop('#updateBarRestart', 'style.display'), '', '重启按钮应出现');
        assertIncludes(await p.text('#updateBarText'), '重启后生效', '进度条文案');
        await p.click('#updateModalCancel');
      }],
      ['截图', async (p) => p.shot('15-update-ready')],
      ['失败阶段给出原因（点重启后安装失败的真实路径）', async (p) => {
        await p.ev('window.__mock.setUpdate({stage:"failed", downloaded: 1024, total: 8388608, version:"0.2.0", error:"签名校验失败"})');
        await p.ev('window.__mock.failNext("install_update_and_restart", "安装失败")');
        await p.click('#updateBarRestart');
        await assertEventually(async () => assertIncludes(await p.text('#updateBarText'), '签名校验失败'), '失败原因');
        assertEq(await p.ev('document.getElementById("updateBar").classList.contains("failed")'), true, '失败样式');
        assertIncludes(await p.text('.toast-notification'), '更新安装失败', '失败提示');
      }],
    ],
  },
  {
    name: '16-error-paths',
    allowConsoleErrors: true,
    steps: [
      ['连接后让 list_keys 失败：列表给错误提示且不崩', async (p) => {
        await p.ev('window.__mock.failNext("list_keys", "ERR 模拟的扫描失败")');
        await connectRaw(p);
        await assertEventually(async () => assertIncludes(await p.text('#keyTree'), '加载失败'), '空列表时应显示失败原因');
        assertIncludes(await p.text('.toast-notification'), '加载 Key 列表失败', 'toast 提示');
        assertOk(await p.ev('window.__app.allKeys.length === 0'), '失败时本地列表应为空');
      }],
      ['失败后重试恢复正常', async (p) => {
        await p.click('#btnRefresh');
        await assertEventually(async () => assertOk(await p.ev('window.__app.allKeys.length > 0'), '重试后应加载到数据'), '重试恢复');
        assertNotIncludes(await p.text('#keyTree'), '加载失败', '错误提示应消失');
      }],
      ['console.error 只来自这一次预期失败', async (p) => {
        await assertOk(p.consoleErrors.length >= 1, '失败路径应留下 console.error（供排查）');
      }],
    ],
  },
  {
    name: '17-readonly-connection',
    steps: [
      ['切到只读连接', async (p) => {
        await p.waitFor('!!document.querySelector(\'#connectionList .connection-item[data-id="conn_readonly"]\')', '只读连接出现在列表');
        await p.click('#connectionList .connection-item[data-id="conn_readonly"] .refresh-conn');
        await assertEventually(async () => assertIncludes(await p.text('#statusLabel'), '(只读)'), '状态栏标注只读');
        await assertEventually(async () => assertIncludes(await p.text('#keyTreeStatus'), '已加载'), 'Key 列表照常加载');
        await loadAllPages(p);
      }],
      ['写入口禁用并把原因写在 title 里', async (p) => {
        assertEq(await p.prop('#btnAddKey', 'disabled'), true, '「新增 Key」应禁用');
        assertIncludes(await p.attr('#btnAddKey', 'title'), '只读连接', '禁用原因');
        assertEq(await p.prop('#btnFlush', 'disabled'), true, '「清空当前数据库」应禁用');
      }],
      ['选中 key 后保存 / 重命名 / 复制 / TTL 全部禁用', async (p) => {
        await p.click(KEY_ROW('hello'));
        await assertEventually(async () => assertIncludes(await p.text('#keyDetail'), 'STRING'), '详情区');
        assertEq(await p.prop('#detailSave', 'disabled'), true, '保存应禁用');
        assertEq(await p.prop('#detailRename', 'disabled'), true, '重命名应禁用');
        assertEq(await p.prop('#detailCopyKey', 'disabled'), true, '复制应禁用');
        assertEq(await p.prop('#detailTtl', 'disabled'), true, 'TTL 输入应禁用');
        assertIncludes(await p.attr('#detailRename', 'title'), '只读连接', '禁用原因');
      }],
      ['只读连接仍可监控（MONITOR 不改数据）', async (p) => {
        assertEq(await p.prop('#monitorToggle', 'disabled'), false, '监控按钮应可用');
        await p.click('#tabMonitor');
        await p.click('#monitorToggle');
        await assertEventually(async () => assertIncludes(await p.text('#monitorState'), '监控中'), '监控应能启动');
      }],
      ['截图', async (p) => p.shot('17-readonly')],
    ],
  },
];


// ------------------------------------------------------------------ 主流程
async function main() {
  const chromePath = findChrome();
  if (WRITE_SHOTS) {
    // 清掉上一轮的截图：否则旧的失败截图会一直留着，看不出「这次到底过没过」
    rmSync(OUT_DIR, { recursive: true, force: true });
    mkdirSync(OUT_DIR, { recursive: true });
  }
  const { server, port: httpPort } = await startStaticServer();
  const userDataDir = mkdtempSync(join(tmpdir(), 'myredis-smoke-'));
  const { child, port: cdpPort } = await launchChrome(chromePath, userDataDir);
  const mockSource = readFileSync(MOCK_FILE, 'utf8');

  const results = [];
  let page = null;
  try {
    const cdp = await connectCdp(cdpPort);
    page = new Page(cdp);
    await page.init(mockSource);
    const baseUrl = `http://127.0.0.1:${httpPort}`;

    const selected = scenarios.filter((s) => !ONLY || s.name.includes(ONLY));
    if (selected.length === 0) throw new Error(`没有匹配 --only ${ONLY} 的场景`);

    for (const scenario of selected) {
      page.allowConsoleErrors = !!scenario.allowConsoleErrors;
      const steps = [];
      let failed = false;
      try {
        await page.goto(baseUrl, scenario.query || '');
      } catch (err) {
        results.push({ scenario: scenario.name, steps, failure: `页面加载失败: ${err.message}` });
        console.error(`✗ ${scenario.name} — 页面加载失败: ${err.message}`);
        continue;
      }
      for (const [label, run] of scenario.steps) {
        const started = Date.now();
        try {
          await run(page);
          steps.push({ label, ok: true, ms: Date.now() - started });
          process.stdout.write(`  ✓ ${label}\n`);
        } catch (err) {
          steps.push({ label, ok: false, ms: Date.now() - started, error: err.message });
          failed = true;
          console.error(`  ✗ ${label}\n      ${err.message.split('\n').join('\n      ')}`);
          if (WRITE_SHOTS) await page.shot(`fail-${scenario.name}`).catch(() => {});
          break;
        }
      }
      // 页面异常（未捕获错误）任何场景都不允许
      if (page.exceptions.length) {
        failed = true;
        console.error(`  ✗ 页面抛出未捕获异常: ${page.exceptions.join(' | ')}`);
      }
      if (!page.allowConsoleErrors && page.consoleErrors.length) {
        failed = true;
        console.error(`  ✗ console.error 非空: ${page.consoleErrors.join(' | ')}`);
      }
      results.push({
        scenario: scenario.name,
        ok: !failed,
        steps,
        consoleWarns: page.consoleWarns,
        consoleErrors: page.consoleErrors,
      });
      console.log(`${failed ? '✗' : '✓'} ${scenario.name}`);
    }
  } finally {
    if (page) await page.cdp.send('Browser.close').catch(() => {});
    child.kill('SIGKILL');
    server.close();
    await new Promise((r) => setTimeout(r, 200));
    if (!KEEP) {
      // Chrome 退出时可能还在写用户目录，删不掉就算了（临时目录里的垃圾而已）
      try {
        rmSync(userDataDir, { recursive: true, force: true });
      } catch (err) {
        console.warn(`临时用户目录没删干净（可忽略）: ${userDataDir}`);
      }
    } else {
      console.log(`Chrome 用户目录保留在 ${userDataDir}`);
    }
  }

  const failedCount = results.filter((r) => r.ok === false).length;
  const summary = {
    scenarios: results.length,
    failed: failedCount,
    shotsDir: WRITE_SHOTS ? OUT_DIR : null,
    results,
  };
  if (WRITE_SHOTS) writeFileSync(join(OUT_DIR, 'summary.json'), JSON.stringify(summary, null, 2));
  console.log(`\n${results.length - failedCount}/${results.length} 个场景通过${WRITE_SHOTS ? `（截图: ${OUT_DIR}）` : ''}`);
  process.exit(failedCount === 0 ? 0 : 1);
}

main().catch((err) => {
  console.error('冒烟脚本自身出错: ' + (err && err.stack ? err.stack : err));
  process.exit(1);
});
