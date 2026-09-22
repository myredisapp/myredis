# 前端单文件拆分评估（2026-09-22）

> 对应 `DEVELOPMENT.md` §2.4「前端单文件拆分评估」与 §4 #4。
> 结论先行：**该拆，但按渐进式拆，且不引入打包器**。
>
> 进度：第 1 轮（2026-09-22）样式外置 `styles.css` + 固化冒烟脚本；第 2 轮（2026-09-22）
> 按第 4 节的四阶段把 3042 行内联 JS 拆成 19 个 ES 模块，`index.html` 只剩结构骨架。
> 每轮都用冒烟脚本（第 5 节）逐屏核对，没有一次性大搬家。

## 1. 现状量化

| 指标 | 数值 | 说明 |
|------|------|------|
| `frontend/index.html` | 3389 行 | 本轮之前 5429 行（样式外置后 -2040） |
| ├─ HTML 结构 | 341 行 | 5 个模态框 + 主布局骨架 |
| └─ 内联 `<script>` | 3042 行 | 单个 IIFE，全部逻辑与状态都在这一个闭包里 |
| `frontend/styles.css` | 2051 行 | 本轮从 `<style>` 原样搬出 |
| 顶层函数声明 | 113 个 | 全部挂在同一个闭包作用域 |
| 顶层可变状态 | 28 个 `let` | `selectedKey` / `currentConn` / `KEY_DATA` / `keyPaging` … |
| `invoke` 调用点 | 50 处（39 个命令） | 前端与后端的全部接触面 |
| 分区注释（`// ---------- xxx ----------`） | 20 个 | 已按功能切好了「逻辑分区」，只是没落成文件边界 |
| 跨分区共享函数的调用点 | 189 处 | `appendTerminal` / `showToast` / `renderKeyTree` / `loadKeys` … |

增长曲线（`git log` 逐提交统计 `frontend/index.html`）：

| 日期 | 行数 | 备注 |
|------|------|------|
| 09-11 | 3307 | |
| 09-14 | 3654 | 集合类型编辑、终端转发、导入导出 |
| 09-15 | 4520 | 自动更新 + 布局拖拽（单日 +866） |
| 09-20 | 4540 → 4797 | 工作区功能加入后又被移除 |
| 09-21 | 4563 | |
| 09-22 | 4715 | §2.4 四个 P1（TLS / Stream / 超时 / 密钥链） |
| 09-22（本轮） | 5429 → 3389 + 2051 | 实时监控 + 重命名/复制 + 样式外置 |

**11 天从 3307 涨到 5429（+64%，约 +190 行/天）**，「复杂度继续上升」这个触发条件
在数据上是成立的 —— 这也是本评估给出「该拆」结论的主要依据。

## 2. 拆分要解决什么、不解决什么

**要解决的**

1. **一次只读一小块**：加一个功能现在要在 3000 行 JS 里定位 + 在 2000 行样式里
   改类名；文件边界能让「这个功能改哪几个文件」一目了然。
2. **状态边界显式化**：现在 28 个 `let` 谁都能改（`selectedKey` 被 6 个分区写），
   拆模块时必须把读写收拢成显式接口，顺手治掉一批「谁改了我」的隐患。
3. **评审粒度**：3000 行文件里的改动在 PR 里是「一个大 diff」，拆开后是「一个模块」。

**不解决的**（别指望拆分带来这些）

1. **没有类型检查、没有单测**：拆文件不等于有测试；真正的兜底是第 5 节的冒烟脚本。
2. **性能**：单文件反而少几个请求；拆分对运行时性能没意义（也不是目标）。
3. **后端零改动**：确实零改动，但前端要么加构建步骤，要么用浏览器原生模块 —— 见下节。

## 3. 三个方案

| | A 保持单文件 | **B ES 模块（推荐）** | C esbuild 打包 |
|---|---|---|---|
| 形态 | 现状 | `frontend/js/*.js` + `<script type="module">` | 源码多文件 → `dist/app.js` |
| 新增工具链 | 无 | **无**（浏览器原生 `import`） | node + esbuild + 构建/监听脚本 |
| `tauri.conf.json` | 不变 | 不变（`frontendDist: ../frontend` 照样整目录嵌入） | 需改指向 `dist/`，并加「构建后再打包」的 CI 步骤 |
| 开发流 | 改完刷新 | 同左（原生模块由 webview 加载） | 改完必须 build/watch，否则跑的是旧产物 |
| 体积 | 略大（无压缩） | 略大（无压缩） | 可压缩 + 可内联 |
| 主要风险 | 文件继续膨胀 | 首次改造要一次性把 `import/export` 打通 | 构建产物与源码不一致、截图/冒烟脚本都要跟着改 |
| 何时选它 | 不再加功能 | **现在** | 需要压缩体积、或将来要引 npm 依赖时 |

选 B 的关键依据：Tauri 的资源协议对 `.js` / `.mjs` 返回 `text/javascript`
（`tauri-utils` 的 `mime_type.rs`），**无需打包器**就能用标准 ES 模块；
`app.security.csp` 为 `null`，不会拦模块加载。C 方案省下的只有体积，代价是给一个
Rust 单仓引入 node 构建环节 —— 等真要引第三方依赖时再说（`PROJECT_PLAN.md` §9.5 也
是这么留的口子）。

## 4. 落地计划（B 方案，渐进式）

按「叶子先走、核心最后」的顺序，每一步都是一个独立可验证的小改动：

| 阶段 | 内容 | 预计影响 | 验证 |
|------|------|----------|------|
| 0（已完成） | 样式外置 `styles.css` | index.html -2040 行 | 冒烟脚本截图逐屏核对 |
| 1 | 建 `js/util.js`（`escapeHtml` / `formatAppBytes` / `parseTtl` / JSON 格式化）、`js/ui.js`（toast + 确认框）、`js/api.js`（`invoke` 封装 + `listenEvent`） | 这三个是**最纯**的叶子，只被调用、不持有状态 | 冒烟脚本 + 手工点「新建连接 / 删除确认 / 主题切换」 |
| 2（已完成） | 拆 `js/updater.js`（静默更新，350 行）、`js/terminal.js`（38 行）、`js/monitor.js`（280 行） | 三个分区自成一体的功能，只依赖 util/ui/api + `currentConn` | 冒烟脚本覆盖监控开停/过滤；手工点一次检查更新 |
| 3（已完成） | 拆 `js/theme.js` + `js/layout.js`（主题与布局持久化，176 行） | 依赖 DOM 与 localStorage，边界清楚 | 冒烟脚本切主题；手工拖拽尺寸后刷新 |
| 4（已完成） | 拆 `js/keys.js`（Key 列表 + 详情 + 集合编辑 + 重命名/复制，约 900 行）、`js/connections.js`（连接管理 350 行），`index.html` 只留骨架 + `js/main.js`（事件绑定 + 初始化） | 核心部分：`selectedKey` / `KEY_DATA` / `keyPaging` 要收进 `js/state.js` 的显式读写接口 | 全量手工回归一轮（连接 / 分页 / 搜索 / 四类集合编辑 / 导入导出 / 终端 / 监控） |

约束（避免拆成「换个地方的大文件」）：

- 每个模块只导出**明确要给别人用**的东西，不导出内部状态；
- `js/state.js` 里的共享状态只提供读写函数，不直接暴露变量；
- 单文件仍不超过 400 行（沿用现有分区粒度，20 个分区里最大的 350 行）。
  实际落地时 `keys` 与 `connections` 两个分区超了这一上限，因此各再拆细：
  「Key 列表 / 详情 / 集合编辑 / 重命名复制」拆成 4 个模块，
  「连接列表与建连 / 连接对话框 / 服务端信息与 Db 选择 / 导入导出」拆成 4 个模块
  —— 模块总数因此是 19 个（原计划 12 个），换来的是每个文件都真的能一眼读完。

## 5. 回归网：`scripts/frontend-smoke.mjs`（已落地）

拆分前固化下来的做法是「headless Chrome + mock `invoke` + 脚本驱动点击 + 截图」，
现在是仓库里的一个脚本，**不引 node_modules**（只用 Node 22+ 内置的 `fetch` / `WebSocket`
说 CDP，Chrome 用系统已装的）：

```bash
node scripts/frontend-smoke.mjs                # 17 个场景 + 截图到 target/frontend-smoke/
node scripts/frontend-smoke.mjs --only monitor # 只跑名字含 monitor 的场景
node scripts/frontend-smoke.mjs --no-shots     # 只跑断言
```

- `scripts/frontend-smoke-mock.js`：Tauri 后端替身，覆盖前端用到的全部命令，返回结构与
  `src-tauri/src/commands/*` 的 serde 输出逐字段对齐；事件按生产路径
  （`__TAURI_INTERNALS__.runCallback`）投递，不另造一套简化版事件系统。
- 17 个场景覆盖：空态 / 连接对话框（含 rediss:// 拦截与超时传参）/ 连接与分页加载 /
  文件夹嵌套展开 / 字符串详情与 JSON 格式化 / Hash 字段级保存 / 重命名与复制（含覆盖语义）/
  COPY 版本门 / 实时监控（开停、过滤、省略行数、集群禁用）/ 终端 / 搜索下推与切库 /
  删除确认 / 新增 Key / 连接配置导出 / 主题与布局 / 更新全流程（发现 → 下载 → 就绪 → 失败）/
  错误路径（列表加载失败后重试）/ 只读连接（写入口禁用、监控仍可用）。
- 断言口径：每一步核对 DOM 结果与后端调用参数；每个场景额外要求「没有未捕获异常、
  没有 `console.error`」（错误路径场景显式豁免并断言确实产生了报错）。
- 退出码非 0 即失败，已接进 `ci.yml` 的 `frontend-smoke` job（失败时上传截图与
  `summary.json` 留档）。

这一层网的价值在拆分过程中立刻体现了：它抓出了重构时引入的一个同名遮蔽 bug
（`const isOnline = isOnline(conn.id)` 造成 TDZ，界面直接白屏），这类错误在
3000 行内联脚本里靠肉眼是看不出来的。

## 6. 已落地

### 第 1 轮（样式外置 + 冒烟脚本）

- `frontend/styles.css`（2051 行）：从 `index.html` 的 `<style>` 原样搬出，
  `index.html` 改为 `<link rel="stylesheet" href="./styles.css" />`。
  样式内容一字未改（缩进、顺序、主题变量块全部保持原样），因此渲染结果与之前逐像素一致
  （已用 headless 截图核对主界面 / 监控面板 / 详情区 / 对话框四个状态）。
- `index.html`：5429 → 3389 行，只剩结构 + 逻辑。
- `scripts/frontend-smoke.mjs` + `scripts/frontend-smoke-mock.js`（第 5 节）。

### 第 2 轮（JS 模块化，阶段 1–4 完成）

`index.html`：3389 → 345 行（只剩 HTML 结构 + `<script type="module" src="./js/main.js">`），
3042 行内联 JS 拆成 19 个模块（合计 3510 行）：

| 模块 | 行数 | 职责 | 依赖方向 |
|------|------|------|----------|
| `js/api.js` | 30 | `invoke` / `listenEvent`：与 Tauri 的唯一接触面 | 叶子 |
| `js/util.js` | 142 | 纯函数：转义 / JSON / 字节与百分比格式化 / TTL 解析 / Stream id / 下载 / 剪贴板降级 | 叶子 |
| `js/ui.js` | 115 | Toast、确认对话框、只读闸门 | 叶子 |
| `js/state.js` | 143 | 跨模块共享状态（连接 / 选中 Key / KEY_DATA）+ 变更通知 | 叶子 |
| `js/theme.js` | 58 | 五款四季主题与持久化 | 叶子 |
| `js/layout.js` | 120 | 侧栏折叠 + 三处尺寸拖拽持久化 | 叶子 |
| `js/terminal.js` | 90 | 终端输出 / 命令转发 / 折叠 | util, api, state |
| `js/monitor.js` | 355 | MONITOR 视图 + 底部双标签 | + ui, terminal |
| `js/collections.js` | 336 | Hash / List / Set / ZSet / Stream 字段级编辑 | + ui, terminal |
| `js/keyops.js` | 129 | 重命名 / 复制对话框（含覆盖语义与版本门） | + ui, terminal, state 通知 |
| `js/detail.js` | 282 | 详情区：值编辑器、TTL、保存 / 刷新 / 删除 | + collections, keyops |
| `js/keys.js` | 350 | Key 树：分页 / 虚拟滚动 / 文件夹 / 搜索 | + detail |
| `js/addkey.js` | 88 | 新增 Key 对话框 | + ui, terminal |
| `js/conn-form.js` | 241 | 连接对话框（新建 / 编辑 / 复制 / 测试） | + ui |
| `js/server-status.js` | 143 | 服务端信息、状态栏、Db 选择 | + keys |
| `js/connections.js` | 298 | 连接列表、建连 / 切换 / 断开 / 删除 | + keys, monitor, conn-form, server-status |
| `js/import-export.js` | 179 | 连接配置与 Key 的导入导出 | + connections, keys |
| `js/updater.js` | 316 | 静默更新全流程 | util, api, ui |
| `js/main.js` | 95 | 装配各模块、启动顺序、后端事件 | 全部 |

拆分带来的三处结构性改进（不是为了行数而拆）：

1. **共享状态显式化**：原来 28 个闭包内 `let` 谁都能改（`selectedKey` 被 6 个分区写），
   现在跨模块的那部分收进 `state.js`，只暴露读写函数（`getSelectedKey` / `patchKeyEntry` …），
   改动在 diff 里看得见；Key 树的分页游标、监控会话、更新进度这些「只属于一个功能」的
   状态则刻意留在各自模块里，没有为了统一而集中。
2. **依赖单向**：`keys → detail → collections/keyops` 是一条链，反向需要（详情区改完数据
   要重绘 Key 树）走 `state.js` 的 `onKeysChanged` 订阅，由 `main.js` 把
   「重绘 Key 树 + 重绘详情区」订阅进去 —— 没有为绕开循环依赖而互相 `import`。
3. **顺手清掉死代码**：拆分时确认无引用的 `flattenKeys`、`expandedFolders`、`$$`
   与 `renderServerInfo` 里未使用的 `diskEl` 一并删除；`frontend-split-evaluation`
   第 5 节的冒烟脚本替代了「靠肉眼扫一眼」。

### 验证

- `node scripts/frontend-smoke.mjs`：17/17 场景通过，零未捕获异常、零 `console.error`。
- **真实 WebView（macOS WKWebView + 自定义协议）**：用 `WKURLSchemeHandler` 复刻
  Tauri 的 `tauri://localhost` 资源协议（同样的 `Content-Type` / `Access-Control-Allow-Origin`
  响应头）加载 `frontend/index.html`，19 个模块全部以 `text/javascript` 正常加载，
  `window.__app` 就绪、空态文案与连接列表渲染正确、页面零 JS 错误；
  另外重建 debug 包并实际启动应用，其 WebKit 数据目录的 LocalStorage 在启动时被写入
  （`myredis.layout` / `mc_cache_theme`），确认应用内的模块图确实执行。
- 保真度核对：把拆分前的内联 JS 与 19 个模块做逐行（归一化缩进后）比对，
  400 处「原代码有、新代码没有」的行全部是三类有意改动 —— 改用 `state.js` 读写函数、
  走 `notifyKeysChanged` 通知、删除死代码；没有功能逻辑被顺手改写。

## 7. 与既有决策的关系

- `DEVELOPMENT.md` §3「引入前端框架 / 构建工具 ❌」：本评估**不引入框架**，
  也不引入打包器（B 方案用浏览器原生模块），因此该决策行无需修改。
- `PROJECT_PLAN.md` §9.5：「现状前端文件保持单文件 …… 若后续复杂度上升，
  再在 `frontend/` 内引入 vitest 测试 / esbuild 打包」—— 复杂度已上升（见第 1 节），
  本评估是那个「再评估」的落地；打包器仍留到确有需要时。
