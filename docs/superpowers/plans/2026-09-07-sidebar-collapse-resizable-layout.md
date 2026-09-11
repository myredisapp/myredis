# 侧栏折叠与模块拖拽调宽实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 `frontend/index.html` 增加标题栏折叠按钮（窄图标栏形态）和三条可拖拽的模块边界（连接列表宽度、Key 树宽度、终端高度），布局状态持久化到 localStorage。

**Architecture:** 布局尺寸由 CSS 变量 `--sidebar-width` / `--tree-width` / `--terminal-height` 控制（已存在）。在模块边界插入透明拖拽手柄，用 Pointer Events 更新对应 CSS 变量；`min-width`/`max-width`/`min-height`/`max-height` 在 CSS 层兜底约束。折叠通过 `.sidebar.collapsed` 类切换。状态读写集中在 `LAYOUT_KEY = 'myredis.layout'`。

**Tech Stack:** 原生 HTML/CSS/JS（单文件 `frontend/index.html`），无新依赖，不涉及 Rust 侧。

**设计文档:** `docs/superpowers/specs/2026-09-07-sidebar-collapse-resizable-layout-design.md`

## Global Constraints

- 只修改 `frontend/index.html` 一个文件；不改 Rust 代码、不改 `tauri.conf.json`。
- localStorage key 固定为 `myredis.layout`，值为 `{ sidebarCollapsed, sidebarWidth, treeWidth, terminalHeight }`。
- 横向边界约束：min 180px，max 480px；终端高度约束：min 100px，max 50% 视口高度。
- 折叠态侧栏宽度 52px。
- 现有代码风格：IIFE 包裹、`'use strict'`、`const $ = (sel) => document.querySelector(sel)`、4 空格缩进、CSS 注释分节（`/* ========== 标题 ========== */`）。

## 验证方式（所有任务通用）

页面在纯浏览器中即可渲染布局（`invoke` 失败只会弹 Toast，不影响布局功能）：

```bash
open frontend/index.html        # macOS 用默认浏览器打开
```

在 DevTools Console 中检查持久化值：

```js
JSON.parse(localStorage.getItem('myredis.layout'))
```

---

### Task 1: 布局状态模块 + 侧栏折叠按钮

**Files:**
- Modify: `frontend/index.html`（CSS 约 372-381 行的 `.sidebar` 规则；标题栏 HTML 约 1500-1505 行；侧栏 HTML 约 1533 行；JS 约 1824 行 `DB_COUNT` 之后插入新模块；初始化区约 2999-3005 行）

**Interfaces:**
- Consumes: 无（首个任务）。
- Produces（后续任务依赖）:
  - `layoutState`：对象 `{ sidebarCollapsed: boolean, sidebarWidth: number, treeWidth: number, terminalHeight: number }`
  - `saveLayout()`：防抖（300ms）写入 localStorage
  - `applyLayout()`：把 `layoutState` 应用到 CSS 变量与 `.collapsed` 类，并触发 `saveLayout()`
  - `LAYOUT_KEY`：字符串 `'myredis.layout'`
  - `updateSidebarToggleIcon()`：根据 `layoutState.sidebarCollapsed` 切换图标 class

- [ ] **Step 1: CSS — 改造 `.sidebar` 规则并新增折叠态样式**

在 `frontend/index.html` 中找到 `.sidebar` 规则（原内容）：

```css
        .sidebar {
            width: var(--sidebar-width);
            min-width: var(--sidebar-width);
            background: var(--bg-secondary);
            border-right: 1px solid var(--border-color);
            display: flex;
            flex-direction: column;
            overflow: hidden;
            flex-shrink: 0;
        }
```

替换为（min/max 改为固定像素约束，让 CSS 变量可被拖拽改写；宽度过渡用于折叠动画，拖拽期间由 `resizeTarget` 机制临时关闭）：

```css
        .sidebar {
            width: var(--sidebar-width);
            min-width: 180px;
            max-width: 480px;
            background: var(--bg-secondary);
            border-right: 1px solid var(--border-color);
            display: flex;
            flex-direction: column;
            overflow: hidden;
            flex-shrink: 0;
            transition: width 0.2s ease, min-width 0.2s ease;
        }
        .sidebar.collapsed {
            width: 52px;
            min-width: 52px;
            max-width: 52px;
        }
        .sidebar.collapsed .section-title,
        .sidebar.collapsed .connection-item .info,
        .sidebar.collapsed .connection-item .badge,
        .sidebar.collapsed .connection-item .dropdown-container,
        .sidebar.collapsed .sidebar-footer {
            display: none;
        }
        .sidebar.collapsed .connection-item {
            justify-content: center;
            padding: 8px 6px;
        }
        .sidebar.collapsed .connection-item.active {
            border-left-width: 3px;
        }
        .sidebar.collapsed .add-connection {
            margin: 0 8px 12px;
            font-size: 0;
            gap: 0;
        }
        .sidebar.collapsed .add-connection i {
            font-size: 16px;
        }
```

说明：`.add-connection` 的文本「添加连接」是裸文本节点，`font-size: 0` 可隐藏之，再恢复图标字号。

- [ ] **Step 2: CSS — 标题栏折叠按钮样式**

在 `.titlebar .logo` 规则（约 175 行）之前插入：

```css
        .titlebar .sidebar-toggle {
            background: transparent;
            border: none;
            color: var(--text-secondary);
            width: 32px;
            height: 32px;
            border-radius: 6px;
            cursor: pointer;
            font-size: 15px;
            display: flex;
            align-items: center;
            justify-content: center;
            transition: all 0.2s;
            flex-shrink: 0;
        }
        .titlebar .sidebar-toggle:hover {
            background: var(--bg-hover);
            color: var(--text-primary);
        }
```

- [ ] **Step 3: HTML — 标题栏按钮与侧栏 id**

找到（约 1500-1505 行）：

```html
        <header class="titlebar">
            <div class="logo">
```

替换为：

```html
        <header class="titlebar">
            <button title="折叠/展开连接列表" id="btnSidebarToggle" class="sidebar-toggle"><i class="fas fa-bars" id="sidebarToggleIcon"></i></button>
            <div class="logo">
```

找到（约 1533 行）`<aside class="sidebar">`，替换为 `<aside class="sidebar" id="sidebar">`。

- [ ] **Step 4: JS — 布局状态模块**

在 JS 中找到 `const DB_COUNT = 16;`（约 1824 行），在其后插入：

```js
            // ---------- 布局状态（折叠 + 拖拽尺寸持久化） ----------
            const LAYOUT_KEY = 'myredis.layout';
            const LAYOUT_DEFAULTS = {
                sidebarCollapsed: false,
                sidebarWidth: 270,
                treeWidth: 320,
                terminalHeight: 200,
            };
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
                document.getElementById('sidebarToggleIcon').className =
                    layoutState.sidebarCollapsed ? 'fas fa-bars-staggered' : 'fas fa-bars';
            }

            function applyLayout() {
                const rootStyle = document.documentElement.style;
                rootStyle.setProperty('--sidebar-width', layoutState.sidebarWidth + 'px');
                rootStyle.setProperty('--tree-width', layoutState.treeWidth + 'px');
                rootStyle.setProperty('--terminal-height', layoutState.terminalHeight + 'px');
                document.getElementById('sidebar').classList.toggle('collapsed', layoutState.sidebarCollapsed);
                updateSidebarToggleIcon();
                saveLayout();
            }
```

- [ ] **Step 5: JS — 按钮事件与初始化**

在 `// ---------- 事件绑定 ----------` 一节的起始处（`let terminalCollapsed = false;` 约 2813 行之前）插入：

```js
            document.getElementById('btnSidebarToggle').addEventListener('click', () => {
                layoutState.sidebarCollapsed = !layoutState.sidebarCollapsed;
                applyLayout();
            });
```

在初始化区（约 2999-3005 行，`refreshDbSelector();` 之前）插入：

```js
            applyLayout();
```

- [ ] **Step 6: 手动验证**

```bash
open frontend/index.html
```

预期：
1. 标题栏 logo 左侧出现汉堡按钮，图标为 `fa-bars`。
2. 点击按钮 → 侧栏收起为 52px 窄栏：只见连接图标列与底部圆形 `+` 按钮，图标变为 `fa-bars-staggered`；再点击恢复全宽，图标切回。
3. DevTools Console 执行 `JSON.parse(localStorage.getItem('myredis.layout'))` 可见 `sidebarCollapsed: true/false` 随点击变化。
4. 刷新页面，折叠状态保持。

- [ ] **Step 7: Commit**

```bash
git add frontend/index.html
git commit -m "feat: 标题栏按钮折叠/展开连接列表（窄图标栏）"
```

---

### Task 2: 拖拽手柄基础设施 + 连接列表与 Key 树宽度拖拽

**Files:**
- Modify: `frontend/index.html`（CSS：`.key-tree` 规则约 757-765 行后新增手柄样式块；HTML：`</aside>` 与 key-tree 之后各插入手柄，约 1550、1580 行；JS：`makeSplitter` 及调用，插在 Task 1 的布局模块之后）

**Interfaces:**
- Consumes: Task 1 的 `layoutState`、`saveLayout`、`applyLayout`。
- Produces:
  - `makeSplitter(handleId, options)`：`options = { axis: 'x'|'y', min: number, max?: number, maxSize?: () => number, invert?: boolean, get: () => number, set: (v: number) => void, resizeTarget?: () => HTMLElement | null }`。拖拽期间给 `resizeTarget` 指向的元素加 `.no-transition`（关闭 width/height 过渡，避免拖拽滞后），Task 3 复用此函数调纵向终端手柄。

- [ ] **Step 1: CSS — 改造 `.key-tree` 约束**

找到（约 757-765 行）：

```css
        .key-tree {
            width: var(--tree-width);
            min-width: var(--tree-width);
            background: var(--bg-secondary);
            border-right: 1px solid var(--border-color);
            overflow-y: auto;
            padding: 6px 0;
            flex-shrink: 0;
        }
```

替换为：

```css
        .key-tree {
            width: var(--tree-width);
            min-width: 180px;
            max-width: 480px;
            background: var(--bg-secondary);
            border-right: 1px solid var(--border-color);
            overflow-y: auto;
            padding: 6px 0;
            flex-shrink: 0;
        }
```

- [ ] **Step 2: CSS — 拖拽手柄样式**

在 `.key-tree` 规则之后插入：

```css
        /* ---- 模块边界拖拽手柄 ---- */
        .splitter {
            flex-shrink: 0;
            position: relative;
            z-index: 5;
            background: transparent;
            transition: background 0.15s;
        }
        .splitter::before {
            content: '';
            position: absolute;
            background: var(--border-color);
            transition: background 0.15s;
        }
        .splitter-v {
            width: 5px;
            cursor: col-resize;
        }
        .splitter-v::before {
            left: 2px;
            top: 0;
            bottom: 0;
            width: 1px;
        }
        .splitter:hover::before,
        .splitter.dragging::before {
            background: var(--accent);
        }
        .splitter:hover,
        .splitter.dragging {
            background: var(--accent-dim);
        }
        .splitter.hidden {
            display: none;
        }
        .no-transition {
            transition: none !important;
        }
        body.resizing-h,
        body.resizing-h * {
            cursor: col-resize !important;
            user-select: none !important;
        }
        body.resizing-v,
        body.resizing-v * {
            cursor: row-resize !important;
            user-select: none !important;
        }
```

- [ ] **Step 3: HTML — 插入两个横向手柄**

找到（约 1550 行）：

```html
            </aside>

            <!-- ===== 主内容 ===== -->
```

替换为：

```html
            </aside>
            <div class="splitter splitter-v" id="splitterSidebar"></div>

            <!-- ===== 主内容 ===== -->
```

找到（约 1580 行）：

```html
                    <div class="key-tree" id="keyTree">
                        <!-- 由 JS 动态渲染 -->
                    </div>

                    <!-- Key 详情 -->
```

替换为：

```html
                    <div class="key-tree" id="keyTree">
                        <!-- 由 JS 动态渲染 -->
                    </div>
                    <div class="splitter splitter-v" id="splitterTree"></div>

                    <!-- Key 详情 -->
```

- [ ] **Step 4: JS — makeSplitter 与两个手柄接线**

在 Task 1 插入的布局模块（`applyLayout` 函数之后）追加：

```js
            function makeSplitter(handleId, options) {
                const handle = document.getElementById(handleId);
                handle.addEventListener('pointerdown', (e) => {
                    if (e.button !== 0) return;
                    e.preventDefault();
                    const startPos = options.axis === 'x' ? e.clientX : e.clientY;
                    const startSize = options.get();
                    const target = options.resizeTarget ? options.resizeTarget() : null;
                    handle.classList.add('dragging');
                    document.body.classList.add(options.axis === 'x' ? 'resizing-h' : 'resizing-v');
                    if (target) target.classList.add('no-transition');

                    function onMove(ev) {
                        const pos = options.axis === 'x' ? ev.clientX : ev.clientY;
                        let size = startSize + (pos - startPos) * (options.invert ? -1 : 1);
                        if (options.maxSize) {
                            size = Math.min(size, options.maxSize());
                        } else if (typeof options.max === 'number') {
                            size = Math.min(size, options.max);
                        }
                        size = Math.max(options.min, size);
                        options.set(size);
                    }
                    function onUp() {
                        handle.classList.remove('dragging');
                        document.body.classList.remove('resizing-h', 'resizing-v');
                        handle.removeEventListener('pointermove', onMove);
                        handle.removeEventListener('pointerup', onUp);
                        saveLayout();
                        if (target) {
                            // 松手后稍等再恢复过渡，避免高度跳变产生动画闪烁
                            setTimeout(() => target.classList.remove('no-transition'), 50);
                        }
                    }
                    handle.addEventListener('pointermove', onMove);
                    handle.addEventListener('pointerup', onUp);
                });
            }

            makeSplitter('splitterSidebar', {
                axis: 'x', min: 180, max: 480,
                get: () => layoutState.sidebarWidth,
                set: (v) => { layoutState.sidebarWidth = v; applyLayout(); },
                resizeTarget: () => document.getElementById('sidebar'),
            });
            makeSplitter('splitterTree', {
                axis: 'x', min: 180, max: 480,
                get: () => layoutState.treeWidth,
                set: (v) => { layoutState.treeWidth = v; applyLayout(); },
            });
```

- [ ] **Step 5: 手动验证**

```bash
open frontend/index.html
```

预期：
1. 连接列表右缘有一条 1px 竖线，悬停时变宽变亮（accent 色背景热区）。
2. 按住左键左右拖动 → 连接列表实时变宽/变窄，拖到 180px/480px 停住；整个窗口光标为 `col-resize`，拖动中页面文字不可选中。
3. Key 树右缘手柄行为相同。
4. 拖动后 Console 查 `JSON.parse(localStorage.getItem('myredis.layout'))`，`sidebarWidth`/`treeWidth` 已更新；刷新页面后尺寸保持。

- [ ] **Step 6: Commit**

```bash
git add frontend/index.html
git commit -m "feat: 连接列表与 Key 树边界支持拖拽调宽并持久化"
```

---

### Task 3: 终端高度拖拽 + 整体回归

**Files:**
- Modify: `frontend/index.html`（CSS：`.terminal-panel` 规则约 1118-1128 行；HTML：`app-body` 结束与终端面板之间约 1590 行；JS：终端折叠/展开两个既有事件处理约 2813-2824 行）

**Interfaces:**
- Consumes: Task 1 的 `layoutState`、`saveLayout`、`applyLayout`、`updateSidebarToggleIcon`；Task 2 的 `makeSplitter(handleId, options)`（本任务以 `axis: 'y'` + `invert: true` 调用）。
- Produces: 无（收尾任务）。

- [ ] **Step 1: CSS — 终端面板约束与拖拽时禁用过渡**

找到（约 1118-1128 行）：

```css
        .terminal-panel {
            height: var(--terminal-height);
            min-height: var(--terminal-height);
            background: var(--bg-secondary);
            border-top: 1px solid var(--border-color);
            display: flex;
            flex-direction: column;
            flex-shrink: 0;
            transition: all 0.25s ease;
            overflow: hidden;
        }
```

替换为：

```css
        .terminal-panel {
            height: var(--terminal-height);
            min-height: 100px;
            max-height: 50vh;
            background: var(--bg-secondary);
            border-top: 1px solid var(--border-color);
            display: flex;
            flex-direction: column;
            flex-shrink: 0;
            transition: all 0.25s ease;
            overflow: hidden;
        }
        .splitter-h {
            height: 5px;
            cursor: row-resize;
        }
        .splitter-h::before {
            top: 2px;
            left: 0;
            right: 0;
            height: 1px;
        }
```

说明：`.terminal-panel` 原有 `transition: all 0.25s` 会让高度拖拽滞后半拍，拖拽期间由 `makeSplitter` 的 `resizeTarget` 机制加 `.no-transition`（Task 2 定义的通用工具类）关闭过渡。

- [ ] **Step 2: HTML — 插入纵向手柄**

找到（约 1590-1593 行）：

```html
        </div>

        <!-- ===== 终端面板 ===== -->
```

替换为：

```html
        </div>
        <div class="splitter splitter-h" id="splitterTerminal"></div>

        <!-- ===== 终端面板 ===== -->
```

- [ ] **Step 3: JS — 终端手柄接线 + 折叠联动**

在 Task 2 的两个 `makeSplitter(...)` 调用之后追加：

```js
            makeSplitter('splitterTerminal', {
                axis: 'y', min: 100, invert: true,
                maxSize: () => Math.round(window.innerHeight * 0.5),
                get: () => layoutState.terminalHeight,
                set: (v) => { layoutState.terminalHeight = v; applyLayout(); },
                resizeTarget: () => terminalPanel,
            });
```

（`terminalPanel` 是既有 DOM 引用常量，定义位置在本模块之前，闭包内可直接使用。）

修改终端折叠/展开的两个既有处理。找到（约 2813-2824 行）：

```js
            let terminalCollapsed = false;
            document.getElementById('terminalToggle').addEventListener('click', () => {
                terminalCollapsed = !terminalCollapsed;
                terminalPanel.classList.toggle('collapsed', terminalCollapsed);
                termChevron.className = terminalCollapsed ? 'fas fa-chevron-up' : 'fas fa-chevron-down';
            });
            termToggleBtn.addEventListener('click', (e) => {
                e.stopPropagation();
                terminalCollapsed = !terminalCollapsed;
                terminalPanel.classList.toggle('collapsed', terminalCollapsed);
                termChevron.className = terminalCollapsed ? 'fas fa-chevron-up' : 'fas fa-chevron-down';
            });
```

替换为（两处均增加 splitter 显隐同步）：

```js
            let terminalCollapsed = false;
            document.getElementById('terminalToggle').addEventListener('click', () => {
                terminalCollapsed = !terminalCollapsed;
                terminalPanel.classList.toggle('collapsed', terminalCollapsed);
                termChevron.className = terminalCollapsed ? 'fas fa-chevron-up' : 'fas fa-chevron-down';
                document.getElementById('splitterTerminal').classList.toggle('hidden', terminalCollapsed);
            });
            termToggleBtn.addEventListener('click', (e) => {
                e.stopPropagation();
                terminalCollapsed = !terminalCollapsed;
                terminalPanel.classList.toggle('collapsed', terminalCollapsed);
                termChevron.className = terminalCollapsed ? 'fas fa-chevron-up' : 'fas fa-chevron-down';
                document.getElementById('splitterTerminal').classList.toggle('hidden', terminalCollapsed);
            });
```

- [ ] **Step 4: 手动验证（整体回归清单）**

```bash
open frontend/index.html
```

逐项确认：
1. 终端上缘手柄悬停显示横线，上下拖拽实时改变终端高度；拖到 100px 或窗口高度 50% 停住；拖拽过程无 0.25s 过渡滞后。
2. 点击终端标题栏折叠终端 → 手柄消失；展开 → 手柄恢复。
3. 三条手柄 + 折叠按钮全部回归 Task 1/2 的验证项。
4. 改变尺寸并折叠侧栏后，刷新页面 → 所有状态（折叠、三个尺寸）完整恢复。
5. DevTools Application → Local Storage 确认 `myredis.layout` 值为合法 JSON，含四个字段。

- [ ] **Step 5: Commit**

```bash
git add frontend/index.html
git commit -m "feat: 终端面板高度支持拖拽调整并持久化"
```
