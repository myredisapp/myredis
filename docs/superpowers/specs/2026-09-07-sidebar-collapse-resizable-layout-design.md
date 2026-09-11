# 侧栏折叠与模块拖拽调宽设计

日期：2026-09-07
状态：已确认

## 背景

`frontend/index.html` 的界面布局由三个 CSS 变量控制尺寸：`--sidebar-width`（左侧连接列表）、`--tree-width`（中间 Key 树）、`--terminal-height`（底部终端面板）。目前这些值固定，用户无法调整，连接列表也无法折叠。本设计为应用增加侧栏折叠按钮和模块边界拖拽调宽能力。

## 需求

1. 标题栏提供按钮，折叠/展开左侧连接列表；折叠后呈现为窄图标栏（约 52px），只保留连接图标列，可点击切换连接。
2. 三条边界支持拖拽调整尺寸：
   - 连接列表右缘（横向，控制 `--sidebar-width`）
   - Key 树右缘（横向，控制 `--tree-width`）
   - 终端面板上缘（纵向，控制 `--terminal-height`；终端折叠时手柄隐藏）
3. 折叠状态与三个尺寸持久化到 localStorage，重启后恢复。

## 方案

采用「CSS 变量 + 原生指针事件」实现：拖拽手柄用 `pointerdown / pointermove / pointerup` 更新对应 CSS 变量，不引入第三方库、不做额外组件抽象。

所有改动集中在 `frontend/index.html`（CSS + JS），不涉及 Rust 侧。

## 详细设计

### 折叠按钮与窄图标栏

- 标题栏左侧 logo 旁新增折叠/展开按钮，图标随状态切换：`fa-bars`（展开态）/ `fa-bars-staggered`（折叠态）。
- 折叠时给 `.sidebar` 添加 `.collapsed` 类，宽度变为约 52px，隐藏连接名、host、badge、`sidebar-footer`（db 选择器与统计信息）；`.connection-item` 内容居中对齐，仅保留图标。
- 「新建连接」按钮折叠态变为居中圆形 `+` 图标按钮。
- 再次点击标题栏按钮（或窄栏中的展开按钮）恢复全宽。

### 拖拽手柄

- 每条边界插入一个 5px 宽的透明热区元素（横向手柄 `cursor: col-resize`，纵向 `cursor: row-resize`），悬停时显示 accent 色高亮线。
- 拖拽过程中给 `body` 添加对应 resize 光标与 `user-select: none`，松开后移除。
- 终端手柄在终端面板折叠（`.terminal-panel.collapsed`）时隐藏。

### 约束

- 横向边界：min 180px，max 480px。
- 终端高度：min 100px，max 50% 视口高度。
- 窗口尺寸变化导致当前值越界时不强制收缩，仅保证 min 约束生效（CSS `min-width` / `min-height` 兜底）。

### 持久化

- localStorage key：`myredis.layout`，存储 `{ sidebarCollapsed, sidebarWidth, treeWidth, terminalHeight }`。
- 启动时读取并应用到 CSS 变量与折叠状态；读取失败或字段缺失时回退默认值（270 / 320 / 200 / 未折叠）。
- 拖拽与折叠的写入做约 300ms 防抖，避免频繁写盘。

## 测试

- 手动验证：折叠/展开切换、三条边界拖拽到 min/max 边界、终端折叠时手柄隐藏、刷新页面后布局恢复。
