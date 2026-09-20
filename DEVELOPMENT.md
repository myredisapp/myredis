# 开发进度与问题记录

> 本文档用于跟踪「麦地缓存」开发进度、已决策事项、已知缺口与待办事项。随开发持续更新。
> 最近更新：2026-09-15，状态：**v0.0.12 已发布；Hash/List/Set/ZSet 编辑、终端真实转发、Key 导入导出、
> 连接配置导入导出、Key 列表分页与虚拟滚动均已落地（后三项尚未进入任何发布 tag）；
> 新增自动更新（后台下载 + 重启生效，见 §1.7 / §8.9）。
> §2.1 的功能缺口已清空，剩余为代码质量与工程流程债务**。
>
> 相关文档分工：
> - **本文件** —— 开发视角的进度、缺口与待办（含内部实现细节）。
> - [`web/docs/index.html`](web/docs/index.html) —— 面向用户的使用文档，其中「功能现状」章节是用户侧的能力边界说明，与本文件「待办事项」保持一致。
> - [`PROJECT_PLAN.md`](PROJECT_PLAN.md) —— 立项方案书（v0.1 草案），仅作历史参考，里程碑状态未回填。
> - [`plan.md`](plan.md) —— 集群支持的实现方案（已完成，保留作技术记录）。

---

## 项目状态总览

| 版本 | 状态 | 说明 |
|:----:|:----:|------|
| v0.1.0 | 开发中 | 后端骨架（错误类型、连接池、连接 CRUD、持久化）+ PING |
| v0.2.x | 已发布 | 用户名鉴权、测试连接按钮、TTL 输入、侧栏折叠与拖拽调宽 |
| v0.0.x | 已发布 | 集群支持、MOVED 报错转可操作建议、UI 优化、应用图标、CI 三平台出包 |
| 当前 HEAD | 开发中 | 核心链路完整；§2 剩余为代码质量与工程流程债务 |

> ⚠️ **版本号的两个来源**：`src-tauri/Cargo.toml` 与 `src-tauri/tauri.conf.json` 里写的是 `0.1.0`（占位），
> 实际发布版本由 CI 从 git tag 反写（`.github/scripts/set-version.mjs`，见 §8.5）。
> 因此「源码里的版本号」不等于「用户手上的版本号」，改版本请打 tag，不要手改这两个文件。
> 最新 tag 为 `v0.0.12`。

技术栈：Tauri 2 + Rust 2021 + `redis` 0.25（`tokio-comp` / `connection-manager` / `cluster` / `cluster-async`）
+ tokio + sysinfo 0.39 + 单文件原生 JS 前端（`frontend/index.html`，无构建步骤，约 4000 行）。

---

## 1. 已实现能力

### 1.1 连接管理（✅ 完成）

- [x] 连接模型 `Connection`：`id / name / host / port / type / readonly / separator / db / username / password`
- [x] 连接类型 `ConnType::{Single, Cluster}`
- [x] 连接配置 JSON 持久化（`connections.json`，位于 Tauri `app_config_dir`，**临时文件 + rename 原子写入**）
- [x] 密码 / 用户名落盘（明文，见 §4 风险），支持 Redis 6.0+ ACL（`redis://user:pass@host:port/db`）
- [x] 用户名与密码按 RFC 3986 percent-encoding，避免 `@ / ? # :` 破坏 URL 结构
- [x] 命令：`connect` / `disconnect` / `list_connections` / `save_connection` / `delete_connection` / `test_connection`
- [x] `test_connection` 不写连接池，仅建立连接后 `PING` 再释放（前端「先测试，再保存」）
- [x] 只读连接：连接时发送 `READONLY`，写命令前经 `Pool::ensure_writable` 拦截
- [x] 自定义 Key 分隔符（默认 `:`），用于前端按分隔符折叠成目录树

### 1.2 集群支持（✅ 完成）

- [x] 用枚举 `Conn` 统一两种句柄（`ConnectionManager` / `ClusterConnection`），并实现 `redis::aio::ConnectionLike`，
      命令层通过 `pool.conn()` 取句柄，无需 match 分发
- [x] 集群连接句柄缓存复用（非每次重建）
- [x] 集群 URL 不带 `/db` 段，避免集群客户端误解析
- [x] `select_db` 在集群下返回明确错误「集群模式不支持切换数据库」
- [x] `get_server_info` 合并各节点 `INFO`（内存、连接数求和），`DBSIZE` 取聚合值
- [x] `list_keys` 用 `CLUSTER NODES` 枚举全部主节点（跳过 slave / fail 节点，未公布地址回退到配置 host），
      逐节点 `SCAN` 后合并去重
- [x] 单机模式误连集群节点时的 `MOVED` / `ASK` 报错转成可操作建议（提示勾选「集群模式」）

### 1.3 Key 操作（✅ 完成）

- [x] `list_keys`：**分页 + `SCAN` 游标遍历**（**不用 `KEYS *`**，避免阻塞生产实例），
      签名 `list_keys(conn_id, cursor, count, pattern) -> { keys, next_cursor }`，返回 `key / type / ttl`
      - 单页大小由调用方给定（默认 100，上限 1000），页内累积到 `count` 或游标归零为止；
        单页最多 32 轮 `SCAN`，命中不足时先返回已扫到的部分与继续用的游标，保证单次请求工作量有界
      - 集群下游标是 `节点地址:节点内游标` 的复合值，一页可跨多个主节点；用节点**地址**而非序号，
        避免 `CLUSTER NODES` 输出顺序变化导致翻页漏读或重复（节点已不在集群时报错提示刷新）
      - `pattern` 收的是用户原始搜索文本，后端按「不区分大小写的包含匹配」语义转成 `MATCH` 模式：
        glob 元字符（`* ? [ ] \`）转义后按字面量处理，ASCII 字母展开成字符类 `[aA]` 还原大小写不敏感
        （见 `to_match_pattern`，有单测）。搜索因此能命中尚未加载的 key，而不只是在已加载的那一页里找
- [x] `set_key`：`SET key value [EX ttl]`，TTL 仅接受 `-1`（永不过期）或正整数，其余报参数错误
- [x] `del_key`：支持批量 `DEL`，返回实际删除数量
- [x] `get_string`：读取 String 值
- [x] 前端 Key 树、关键字搜索、文件夹折叠、TTL 输入与保存、新增 String Key、删除 Key（带确认）
      - 列表按页加载：滚到底部自动预取下一页，状态条另有「加载更多」入口
        （文件夹折叠起来时列表撑不满视口、滚不动，需要有显式入口）
      - 渲染走虚拟滚动：只渲染视口内的行（上下各多渲染 8 行），几十万 key 不再一次性铺满 DOM
- [x] **Hash / List / Set / ZSet 的内容加载与字段级编辑**（`commands/key_content.rs`）
      - 读取：`get_hash`（HGETALL）/ `get_list`（LRANGE 0 -1，带下标）/ `get_set`（SMEMBERS）
        / `get_zset`（ZRANGE 0 -1 WITHSCORES）
      - 编辑：Hash `hash_set_field` / `hash_del_fields`；List `list_set_element`（LSET，按下标）/
        `list_del_element`（LREM 1，按值，避免并发下标漂移误删）/ `list_push_element`（LPUSH/RPUSH）；
        Set `set_add_member` / `set_del_member`（改名 = 加新删旧）；ZSet `zset_add_member` / `zset_del_member`
      - 任意类型的 TTL 修改：`set_key_ttl`（-1 → PERSIST，正数 → EXPIRE），String 仍走 `SET EX`
      - 前端详情区按类型渲染可编辑表格（行内保存 / 删除 + 底部虚线新增行），只读连接整体禁用
- [x] **终端真实命令转发**（`commands/terminal.rs`）：`execute_command` 解析整行命令
      （支持单双引号与 `\` 转义）后直接转发，`redis::Value` 渲染成 redis-cli 风格文本
      （OK / (nil) / (integer) N / (empty array)，多行数组逐行输出）；
      只读连接按 `WRITE_COMMANDS` 名单拦截写命令，命令执行包 10s `tokio::time::timeout`
- [x] **Key 导入 / 导出**（`commands/import_export.rs`）：导出为 JSON 文档
      （string/hash/list/set/zset 五种类型 + TTL，逐 key 独立命令读取，集群模式不触发跨 slot 错误；
      `keys` 传 `null` 时由后端全量枚举当前 keyspace —— 前端分页浏览时只持有已加载的那部分 key，
      拿它当导出范围会漏数据，所以全量枚举放在后端做）；
      导入支持覆盖 / 跳过两种冲突策略，逐 key 写入（SET / HSET / RPUSH / SADD / ZADD + EXPIRE），
      单条失败不影响其它 key，失败原因汇总返回；Stream 等类型跳过
- [x] **连接配置导入 / 导出**（`commands/connection.rs`）：导出全部连接配置为 JSON 文档
      （可选是否包含密码，含明文密码时前端弹确认框提示；格式带 `app` / `version` / `exportedAt` 元信息）；
      导入兼容 `{ connections: [...] }` 与裸数组两种形态，按 `id` 匹配已有连接（覆盖或跳过），
      单条字段缺失 / 类型错误只记入失败列表并带名称返回，不影响其它条目

### 1.4 服务器信息（✅ 完成）

- [x] `INFO` 解析：版本 / 操作系统 / 运行时长 / 已用内存 / 连接数
- [x] `DBSIZE` 作为 Key 总数
- [x] `CONFIG GET dir` 取持久化目录，再用 sysinfo 查该目录所在磁盘的总量 / 可用 / 使用率
      （按「挂载点是路径最长前缀」匹配磁盘；`CONFIG` 无权限时静默降级）
- [x] 前端每约 5 秒自动刷新指标

### 1.5 前端（✅ 完成）

- [x] 五款主题（春分 / 立夏 / 初秋 / 深秋 / 冬至），选择结果存 `localStorage`（key `mc_cache_theme`）
- [x] 面板拖拽与布局记忆，存 `localStorage`（key `myredis.layout`）
- [x] 只读连接下禁用「新增 Key」「保存」与 TTL 输入框
- [x] 快捷键：`Enter` 执行终端命令 / 保存连接弹窗 / 新增 Key 名称与 TTL；`Ctrl`(`⌘`)+`Enter` 提交新增 Key 的值；`Esc` 关闭弹窗
- [x] 终端为**真实命令转发**（见 §1.3），结果按 redis-cli 风格渲染，写命令在只读连接下被拦截
- [x] 导入（JSON 文件，覆盖前确认，报告成功/跳过/失败数量）/ 导出（当前库全部 Key 下载为 JSON）
- [x] **Key 列表分页 + 虚拟滚动**（见 §1.3）：滚到底部预取下一页，状态条常驻「已加载 N 个 Key / 加载更多」，
      只渲染视口内的行；搜索下推后端 `SCAN MATCH`（输入停顿 300 ms 后重扫），
      增 / 删 Key 只改本地列表，不再整表重扫
- [x] **连接配置导入 / 导出**：侧栏连接区两个入口，导出弹确认框（可选是否包含明文密码），
      导入前确认覆盖策略，报告新增/覆盖、跳过、失败数量
- [x] **macOS 菜单栏**（`src-tauri/src/menu.rs`，仅 macOS 生效）：Window / Settings / Help，
      Settings 下挂「Theme」子菜单（五款主题）与「Check for Updates…」，点击后由 Rust `emit` 事件、
      前端复用标题栏同一套逻辑（不会出现两套主题状态）；菜单栏不再单列 Edit —— macOS 的编辑快捷键靠
      菜单项派发到响应链，所以 Undo / Redo / Cut / Copy / Paste / Select All 这几个标准编辑项改挂在
      应用菜单内部（不展开就看不到），删掉它们才会让输入框和终端失去 ⌘Z / ⌘X / ⌘C / ⌘V / ⌘A

### 1.6 工程与发布（✅ 完成）

- [x] 单元测试：URL 构造（单机 / 密码 / 用户名 / ACL / 特殊字符 / 集群去 db）、`SET` 命令组装、
      `SET` TTL 校验、`CLUSTER NODES` 解析、`INFO` 解析、集群 INFO 提取与合并、磁盘挂载点匹配
- [x] 集成测试：连真实 Redis 验证 `SET` + TTL（均标 `#[ignore]`，见 §7）
- [x] 零编译警告目标 + `cargo clippy` 规范（见 §6）
- [x] CI（`.github/workflows/release.yml`）：打 `v*` tag 触发，前置 `check-deploy-env` 快速校验部署凭据，
      三平台矩阵出包（macOS universal / Linux x64 / Windows x64），规范化产物名，发 Release，推送 `web/` 静态页
- [x] 三平台打包踩坑记录见 §8（**这一节是实测经验，勿删**）

### 1.7 自动更新（✅ 完成）

- [x] 标题栏更新入口**默认不显示**：启动后静默查一次，只有查到新版本才把按钮放出来（带红点，不弹窗挡操作）；
      没新版就完全不占位，标题栏只留刷新 / 清库 / 新建连接。入口藏起来后靠 **30 分钟一次静默复查**兜底
      （下载中 / 已就绪时后端直接返回缓存结果，不会重复打网络请求），否则挂机久了发新版也看不到入口
- [x] **后台下载**：点「立即更新」后在 Rust 侧的独立任务里下载，前端每 500 ms 轮询 `get_update_progress`
      画窗口底部进度条；下载期间应用照常可用，甚至刷新页面也不中断（进度与安装包都存在后端状态里）
- [x] 下载完成弹「更新已就绪」提示，点「立即重启」才真正落盘安装并重启；底部进度条上常驻重启入口，
      关掉弹窗也不会找不到入口
- [x] 安装包验签：`Update::download` 内部先验签再返回字节，所以「下到一半的坏包」不会被装上；
      公钥在 `tauri.conf.json`，私钥只在 CI（见 §8.9）
- [x] 失败兜底：下载失败在进度条上说明原因；安装失败（如应用目录不可写）会把安装包留在内存里，可再点一次重启。
      失败提示不长期占位：进度条上的失败原因与失败 toast 都是 **8 秒后自动消失**（也可以点 × 立刻收起），
      重试再点标题栏的更新入口即可 —— 失败时新版本依然存在，所以那个入口不会跟着藏起来
- [x] 安装包已下好时点更新入口，直接弹「更新已就绪」给重启入口，不再走一遍「发现新版本 → 立即更新」

---

## 2. 待办事项

> 状态标记：✅ 已完成 / 🚧 进行中 / ⬜ 未开始 / ❌ 不做（见 §3）

### 2.1 功能缺口（P1，用户可见）

> **当前无遗留项** —— 本节 4 项已全部完成，`web/docs/index.html` 的「功能现状 → 仍在开发中」也已清空，
> 两处请保持同步（改动用户可见能力时按 §6 第 7 条一起更新）。

- [x] ✅ **Hash / List / Set / ZSet 的内容加载与字段级编辑**（2026-09-14 完成）
  - 实现：`commands/key_content.rs` + 前端详情区可编辑表格（行内保存/删除 + 底部新增行），详见 §1.3。

- [x] ✅ **内置终端的真实命令转发**（2026-09-14 完成）
  - 实现：`commands/terminal.rs` 的 `execute_command`（解析 → 转发 → `redis::Value` 渲染），详见 §1.3。

- [x] ✅ **Key 导入 / 导出**（2026-09-14 完成）
  - 实现：`commands/import_export.rs`（JSON 格式，覆盖/跳过两种冲突策略），详见 §1.3。

- [x] ✅ **连接配置的导入 / 导出**（2026-09-14 完成）
  - 实现：`commands/connection.rs` 的 `export_connections` / `import_connections`
    （侧栏两个入口，导出可选是否带明文密码，导入按 `id` 覆盖/跳过），详见 §1.3 与 §1.5。

### 2.2 代码质量与健壮性（P1–P2）

- [ ] ⬜ **P1｜超时配置接线**（风险最高、改动最小，建议先做）
  - 现状：`src-tauri/src/config.rs` 整个文件挂着 `#![allow(dead_code)]`，`ConnectionTimeout`
    目前只被 `Pool::test()` 用到；`connect()` 与全部命令层（`list_keys` / `set_key` / `del_key` /
    `get_string` / `get_server_info`）都是裸的 `query_async(...).await`。
  - 后果：命令没有 `tokio::time::timeout` 保护，遇到卡住的 Redis 或 `BLPOP` 类阻塞命令，UI 会永久等待。
  - 这直接违反 §6 第 3 条强制规范。接线后请移除 `config.rs` 的 `#![allow(dead_code)]`。

- [ ] ⬜ **P1｜`rediss://` 缺乏友好提示**
  - 现状：`AppError::TlsNotSupported` 已定义但**无任何代码路径构造它**；`to_connection_url()` 无条件拼 `redis://`，
    用户在主机名里填 `rediss://x` 只会得到一条通用 URL 解析错误。
  - `PROJECT_PLAN.md` §9.4.1 / §4.2.3 要求拦截并返回「暂不支持 TLS 加密连接 (rediss)」，需补上入口校验。

- [x] ✅ **P2｜Key 列表全量加载，分页与虚拟滚动未落地**（2026-09-14 完成）
  - 实现：`list_keys` 改为游标分页（见 §1.3），前端改成虚拟滚动 + 滚动预取 + 状态条「加载更多」，
    搜索下推后端 `SCAN MATCH`。与 `PROJECT_PLAN.md` §4.3 的设计对齐。

- [ ] ⬜ **P2｜`list_keys` 的 N+1 命令**
  - 现状：对拿到的**每个 key** 各发一次 `TYPE` 和一次 `TTL`（`commands/key.rs`）。
    分页后单次请求的往返数已被页大小封顶（默认 100 个 key → 200 次往返），
    但仍是 O(页大小) 次往返，可用 pipeline 收拢成 1 次。

- [ ] ⬜ **P2｜同步阻塞 IO 跑在 async 上下文**
  - 现状：`storage.rs` 的模块注释写明「调用方应放在 `spawn_blocking` 中执行」，
    但 `commands/connection.rs` 等处直接在 async fn 里调 `repo.load()` / `save_all()`。
    文件小的时候无感，属潜在卡顿点。

- [ ] ⬜ **P2｜清理死代码**
  - `Pool::manager()`、`Pool::ensure_conn()`（`connection_pool.rs`）已无调用方
    —— 集群改造后命令层统一切到了 `pool.conn()`。
  - `AppError::NotConnected`（`error.rs`）同样无使用者。

### 2.3 文档与工程流程（P2）

- [ ] ⬜ **CI 缺少质量门禁**：`release.yml` 只做「构建 → 改名 → 发 Release → 推网站」，
      全流程没有 `cargo test` / `cargo clippy -- -D warnings` / `cargo fmt --check`。
      §6 把这些列为提交前强制项，目前完全靠人工自觉，标了 `#[ignore]` 的集成测试也永远不会被执行。
- [ ] ⬜ **根目录缺 `README.md`**：仓库首页目前是空的（`design/README.md` 只是设计素材说明）。
- [ ] ⬜ **`PROJECT_PLAN.md` 需要回填或标注**：它是 v0.1「草案，待评审」，里程碑 M1–M5 状态未更新；
      §4.4 列的命令名（`get_key` / `exists_key` / `expire_key` / 各类型专属命令）与实际实现
      （`get_string` / `set_key` / `del_key`）不一致。
- [ ] ⬜ **清理陈旧本地分支**：`feat/logo`、`feat/redis-cluster-support`、`feat/redis-username-auth`、
      `feat/test-connection-button`、`feature/ttl-input`、`fix/single-mode-cluster-moved-hint`
      均已合入 `main`，可删除。

---

## 3. 明确不支持的功能（非待办）

> 以下为**已决策不做**。请勿误报为 bug，也不要顺手实现——如需变更，先更新本文档与 `PROJECT_PLAN.md` 再开发。

| 能力 | 决策 | 说明 |
|------|:----:|------|
| TLS / SSL（`rediss`） | ❌ | 本期不支持，仅 `redis://` 明文；应返回友好错误（该提示尚未接线，见 §2.2） |
| SSH 隧道 | ❌ | 不内置 |
| 主从 / Sentinel 故障转移 | ❌ | 不实现故障转移，仅透传命令 |
| 集群的故障转移 / 扩缩容 | ❌ | 由 Redis 集群自身负责 |
| 跨 slot 的多 key 操作 | ❌ | `MGET` / `DEL` 跨 slot 属 Redis 集群固有限制，报错透传 |
| 集群下 `SELECT` 切库 | ❌ | 集群只有 db0，前端隐藏选择器、后端返回明确错误 |
| 实时命令监控（Monitor） | ❌ | 保留为后续迭代 |
| 引入前端框架 / 构建工具 | ❌ | 坚持原生 JS 单文件（见 `PROJECT_PLAN.md` §9.5） |

---

## 4. 待确认 / 风险

| # | 事项 | 状态 | 备注 |
|---|------|:----:|------|
| 1 | 密码明文存 `connections.json` | 已知风险 | 任何有本机读权限的进程都能取到；后续可改系统密钥链（macOS Keychain / Windows Credential Manager / Secret Service） |
| 2 | 超时值是否暴露给用户配置 | 待定 | 现为硬编码默认值（5s 连接 / 10s 命令），且尚未接线，见 §2.2 |
| 3 | 兼容 Redis 6.0 以下 | 需持续注意 | 目标为 Redis 2.8+；避免使用仅新版本才有的参数，`CLIENT SETINFO` 等需容错或降级 |
| 4 | 前端 `frontend/index.html` 单文件已约 4000 行 | 观察中 | 复杂度继续上升时再评估拆分为多文件 + esbuild，与桌面客户端解耦，不影响后端 |
| 5 | 更新签名私钥丢失 | 已知风险 | 私钥只在 CI secret（`TAURI_SIGNING_PRIVATE_KEY`）与本地 `~/.tauri/myredis-updater.key`。**丢失或轮换后，已装旧版本的应用将永远收不到自动更新**（客户端只认配置里那份公钥），只能让用户手动重装。务必备份私钥文件 |
| 6 | macOS 构建未做代码签名 / 公证 | 已知风险 | 替换 `.app` 由 Tauri 自己完成并只认 minisign 验签，不依赖 Apple 签名；但首次安装仍会被 Gatekeeper 拦（需右键打开）。后续要公证需另配 `APPLE_*` 凭据 |

---

## 5. 变更记录

| 日期 | 版本 | 说明 |
|------|------|------|
| 2026-09-16 | — | 菜单栏去掉 **Edit**（§1.5）：`menu.rs` 不再单列 Edit 子菜单，Undo / Redo / Cut / Copy / Paste / Select All 六个标准编辑项移入应用菜单，保证 ⌘Z / ⌘X / ⌘C / ⌘V / ⌘A 仍能派发到响应链 |
| 2026-09-16 | — | 新增 **macOS 菜单栏**（§1.5）：`src-tauri/src/menu.rs` 自建菜单，顺序 Window / Settings / Help（File、View 去掉，Edit 保留以支撑编辑快捷键）；Settings 下挂主题子菜单与「检查更新」，菜单项走 `emit` + 前端监听（`plugin:event|listen`）复用标题栏逻辑，主题切换与标题栏共享同一份 `localStorage` 记录 |
| 2026-09-15 | — | 标题栏更新入口改为**默认不显示**：只有查到新版本才出现（`#btnCheckUpdate.show`），没新版完全不占位；配套加 30 分钟一次静默复查（`UPDATE_RECHECK_MS`），避免挂机期间发新版看不到入口；安装包已下好时点入口直接弹「更新已就绪」（§1.7） |
| 2026-09-15 | — | 新增 `scripts/rotate-signing-key.sh`：交互式输入密码 → 重新生成更新签名密钥对 → 旧密钥自动备份 → 新公钥写回 `tauri.conf.json` → 覆盖两个 CI secret → 真签自检；`update-dryrun.sh` 支持带密码的私钥（终端里问一次）；修掉几处「`$var` 紧邻中文」的写法（macOS bash 3.2 会把中文并进变量名，CI 的 bash 5 不会，本地跑 `check-deploy-env.sh` 会静默吞标点或直接报错） |
| 2026-09-15 | — | 发布流水线加**签名私钥闸门** `check-signing-key.mjs`（流水线第一步 + 构建 job 签名前各跑一次）：真签一次判定私钥能否解开、与公钥是否配对（比对 key id），并检查 `endpoints`/`createUpdaterArtifacts`；发布 job 加**清单回读校验**（按客户端用的两个地址匿名取回逐字节比对，`latest` 别名带重试）；`check-deploy-env.sh` 的私钥检查降为「存在性」，深度校验归新脚本 |
| 2026-09-15 | — | 自动更新收尾：失败提示（进度条上的原因 + 失败 toast）改为 **8 秒后自动消失**；「检查更新」的错误文案不再重复前缀，并把插件那句英文 `Could not fetch a valid release JSON` 翻成「更新服务没有返回版本清单」（`ReleaseNotFound` 分支，附单测）；新增 `scripts/update-dryrun.sh` 本地演练更新源（§8.9），搞清「发版前检查更新必然失败」的原因 |
| 2026-09-15 | — | 新增**自动更新**（§1.7）：`tauri-plugin-updater` + 后台下载 + 前端轮询进度条 + 重启生效，四个命令见 `src-tauri/src/commands/update.rs`；发布流程加 `createUpdaterArtifacts` 签名与 `latest.json` 清单生成（§8.9），签名私钥存 CI secret；标题栏版本号改为读真实版本（原先硬编码 `v2.0`） |
| 2026-09-14 | — | §1.5 两项收尾：**连接配置导入/导出**（`export_connections` / `import_connections` + 侧栏入口 + 带复选框的确认框）与 **Key 列表分页 + 虚拟滚动**（`list_keys` 改游标分页、集群复合游标、前端虚拟渲染与滚动预取、搜索下推 `SCAN MATCH`）；`export_keys` 改为后端全量枚举（`keys` 可传 `null`）；§2.1 功能缺口清空 |
| 2026-09-14 | — | §1.3 三项缺口落地：Hash/List/Set/ZSet 内容加载与字段级编辑（`key_content.rs`）、终端真实命令转发（`terminal.rs`）、Key 导入导出（`import_export.rs`）；任意类型 TTL 修改（`set_key_ttl`，PERSIST/EXPIRE） |
| 2026-09-14 | — | 全面重写：按实际代码回填进度；新增「待办事项」章节（4 项功能缺口 + 8 项代码/工程债务）；明确非待办清单 |
| 2026-09（历史） | v0.2.x | 用户名鉴权、测试连接按钮、TTL 输入、侧栏折叠与模块拖拽调宽 |
| 2026-09（历史） | v0.0.x | 集群支持、MOVED 报错转建议、UI 优化、应用图标、CI 三平台出包与产物改名 |
| 2024-08-30 | v0.1.0 | 后端骨架 + 连接模型 + 持久化 + PING 命令 + 测试（原文档，本次重写前的基线） |

---

## 6. 代码质量规范（强制）

1. **注释**：核心公有结构 / 方法必须有 `///` 文档注释；逻辑处写行内注释说明「为什么」。
2. **错误处理**：禁止 `unwrap()` / `panic!` 直接抛出（除配置加载等一次性场景可 `expect`）。
3. **超时**：所有网络 IO 包 `tokio::time::timeout`，防止卡死。
   > ⚠️ 当前命令层尚未满足此条，见 §2.2「超时配置接线」。
4. **提交**：遵循 Conventional Commits。
5. **检查**：提交前运行 `cargo clippy` 和 `cargo fmt --check`，保证无警告。
   > ⚠️ CI 未强制这两步，见 §2.3，需靠人工执行。
6. **测试**：核心逻辑（URL、解析）写单元测试；网络相关写带 `#[ignore]` 的集成测试。
7. **文档同步**：改动用户可见的能力边界时，同步更新 `web/docs/index.html` 的「功能现状」与本文档 §2。

---

## 7. 本地开发命令

```bash
cd src-tauri

# 运行（开发）
cargo tauri dev

# 单元测试（不依赖 Redis）
cargo test

# 集成测试（需先启动 Redis，如 docker run -p 6379:6379 redis）
# 所有带 #[ignore] 的 Redis 依赖测试：
cargo test -- --ignored

# 代码检查
cargo clippy -- -D warnings
cargo fmt --check
```

前端无构建步骤，改 `frontend/index.html` 后由 WebView 直接加载（`cargo tauri dev` 会热重载）。

---

## 8. 打包与发布约定

### 8.1 Linux 包名必须是 ASCII（`src-tauri/tauri.linux.conf.json`）

Linux 打包会把 `productName` 直接当成软件包名（deb 的 `Package` 字段、rpm 的 `Name`、AppImage 文件名）。
tauri 自己用 `ar` 拼 `.deb`（不调用 `dpkg-deb`），所以中文名不会让构建失败，但会产出装不上的包：

```
dpkg: error: parsing file ... invalid package name ... must start with an alphanumeric character
```

dpkg 只接受 `[a-z0-9][a-z0-9+.-]*` 形式的包名，因此该文件把 Linux 的包名单独覆盖成 `maidi-cache`。
macOS / Windows 仍沿用 `tauri.conf.json` 里的中文名，窗口标题也依然是「麦地缓存」。

### 8.2 `tauri.<平台>.conf.json` 里不能写注释

`tauri.conf.json` 解析失败时会回退到 JSON5（容忍注释），但**平台覆盖文件不会**：tauri-build 默认只启用
`config-json` feature，平台文件一律走 serde_json 严格解析。写入 `//` 注释会让 Linux 构建直接失败：

```
unable to parse JSON Tauri config file at src-tauri/tauri.linux.conf.json
because key must be a string at line 3 column 3
```

同样原因，把它改名成 `tauri.linux.conf.json5` 也不行 —— 该 feature 未启用时文件会被**静默忽略**，
包名退回中文，产出一个装不上的包（比构建失败更难发现）。原因说明写在本文件，不要写回 JSON。

`tauri.conf.json` 虽然能被 tauri 回退成 JSON5 解析（容忍注释），但 CI 的 `.github/scripts/set-version.mjs`
是用 `JSON.parse` 读它的，加了注释会让「同步版本号」这步直接崩掉，所以主配置同样只能写严格 JSON。

### 8.3 Windows 上 `--bundles` 的值必须加引号

Windows runner 的默认 shell 是 PowerShell，未加引号的 `nsis,msi` 会被它当成数组、展开成单个参数
`nsis msi` 传给 tauri，报 `invalid value 'nsis msi'`。CI 里已写成 `--bundles "${{ matrix.bundles }}"`。

### 8.4 MSI 的码页必须放得下中文名（`bundle.windows.wix.language`）

Windows 上 WiX 默认语言是 `en-US`，对应码页 1252（Latin-1），放不下「麦地缓存」这类中日韩字符，
`light.exe` 会失败。tauri 只抛出这一行，看不到 WiX 的具体原因：

```
Error failed to bundle project: `failed to run ...\WixTools314\light.exe`
```

真正的错误要用 `tauri build --verbose` 才会显示：

```
error LGHT0311 : A string was provided with characters that are not available in the specified
database code page '1252'. Either change these characters to ones that exist in the database's
code page, or update the database's code page by modifying one of the following attributes:
Product/@Codepage, Module/@Codepage, Patch/@Codepage, PatchCreation/@Codepage, or
WixLocalization/@Codepage.
```

tauri 生成的 `main.wxs` 里写的是 `<Package SummaryCodepage="!(loc.TauriCodepage)">`，这个值取自
tauri-bundler 自带的 `languages.json`（`en-US → 1252`，`zh-CN → 936`）。所以在 `tauri.conf.json` 里声明
`bundle.windows.wix.language = "zh-CN"` 即可把码页换成 936（GBK），中文名正常写进 MSI，安装包名也变成
`麦地缓存_<版本>_x64_zh-CN.msi`。可选语言受那份 `languages.json` 限制，写错会 panic 并列出全部可用值。

NSIS 不受影响（走 UTF-16），macOS / Linux 忽略 `bundle.windows`。

### 8.5 Release 上的安装包名（`.github/scripts/rename-artifacts.mjs`）

本地 `tauri build` 的产物名由 `productName` 决定，于是 macOS / Windows 出来是 `麦地缓存_<版本>_<架构>.dmg`、
`麦地缓存_<版本>_x64-setup.exe`，Linux 是 `maidi-cache_<版本>_amd64.deb`。这套名字发布到下载页上既不好认
也不统一，所以 CI 在**打包之后、上传 artifact 之前**多跑一步改名，统一成：

```
myredis-<tag>-<platform>.<ext>
```

| 平台 label | 产物 |
| --- | --- |
| `macos-universal` | `myredis-v0.2.0-macos-universal.dmg` |
| `linux-x64` | `myredis-v0.2.0-linux-x64.deb`、`myredis-v0.2.0-linux-x64.AppImage` |
| `windows-x64` | `myredis-v0.2.0-windows-x64.exe`、`myredis-v0.2.0-windows-x64.msi` |

几个容易踩的点：

- **只改发布产物，不改 `productName`**：应用名、窗口标题、安装目录仍是「麦地缓存」，本地打包的输出名也不变。
  上面 §8.1 / §8.4 描述的命名行为依然成立，改名只发生在 CI 上传前。
- **扩展名原样保留**：`.AppImage` 不能写成 `.appimage`，否则下载后无法识别。
- **按 `matrix.bundles` 限定要认的扩展名**：脚本只会处理本 job 声明要打的 kind，不会去动别的扩展名。
  某个 kind 没产出文件就报错退出 —— 宁可让流水线失败，也不要发出一个少包的 Release。
- **`bundle/` 不递归扫描**：下一层（`dmg` / `deb` / `appimage` / `nsis` / `msi`）才是最终安装包，
  更深层是 tauri 的临时产物；`.AppImage.tar.gz` 这类附属文件不在白名单里，会被原样留下、不参与上传。
- **`shell: bash` 不能省**：Windows runner 默认 shell 是 PowerShell，续行符与引号规则都和 bash 不同。
- 脚本是 Node 而不是内联 shell，和 `set-version.mjs` 一致，也能在本地直接跑：

```bash
node .github/scripts/rename-artifacts.mjs <bundleDir> <tag> <platform> <bundles>
node .github/scripts/rename-artifacts.mjs \
  src-tauri/target/universal-apple-darwin/release/bundle v0.2.0 macos-universal app,dmg
```

### 8.6 macOS 出 universal 单包

matrix 里 macOS 只有一项（`universal-apple-darwin`），不再分别出 arm64 / x64 两个 dmg：
用户不用先分辨自己的机器是 Apple Silicon 还是 Intel，下载页上也少一个包。

代价是 rust 侧要装 `aarch64-apple-darwin` 与 `x86_64-apple-darwin` 两个 target（matrix 的
`rust-targets` 字段），代码编译两遍，由 tauri 调 `lipo` 合成。需要同时装多个 target 的平台就填逗号分隔的列表，
只装一个的平台照常填单个值 —— `dtolnay/rust-toolchain` 的 `targets` 直接吃这个字段。

Linux 仍刻意留在 `ubuntu-22.04`：产物会继承构建机的 glibc 版本，在 22.04 上打包才能兼容更老的发行版。

### 8.7 网站静态页随 Release 一起发布

`release.yml` 的 `发布网站静态页面` job 在 Release 成功后执行：把当前 tag 注入 `web/index.html`
（替换 `__RELEASE_TAG__` 占位符，页面据此展示版本号并推导直链下载地址），再通过 `scp` 推到服务器。
替换后脚本会复查占位符是否残留，残留即报错 —— 避免发出一个版本号错误的下载页。

### 8.8 触发方式与前置闸门

打 tag 即触发（`git tag v0.2.0 && git push origin v0.2.0`），只匹配 `v*`。
构建矩阵前先跑 `check-deploy-env` job，它按顺序做三件事，任何一步失败就整体停下：

1. **校验更新签名私钥**（`check-signing-key.mjs`，本 job 的第一个检查步骤）：真签一次来判定
   私钥能不能用给的密码解开、是不是与 `tauri.conf.json` 里的公钥配对，并顺带检查
   `plugins.updater.endpoints` 与 `bundle.createUpdaterArtifacts` 是否就位。详见 §8.9
   （它需要 `tauri signer sign`，所以之前先装 Node 与 CLI，见 job 内的步骤注释）；
2. 校验必需环境变量齐全且非空、SSH 认证方式有且只有一种；
3. 校验 SSH 可登录（试连）并能 `scp` 试传。

这样缺失/配错的凭据在几秒内失败，而不是等几十分钟的构建跑完才暴露。
构建 job 在签名前会再跑一次同一个私钥校验（单独重跑构建 job 时前置闸门不会跑）。
同一 tag 重复触发会取消上一次未跑完的运行（`concurrency.cancel-in-progress`）。

### 8.9 自动更新的签名与清单（`latest.json`）

发布流程为自动更新多做了两件事：构建阶段给更新包签名，发布阶段生成更新清单。

**公钥 / 私钥**：`tauri.conf.json` 的 `plugins.updater.pubkey` 是公钥（明文，可进仓库）；
私钥在 GitHub secret `TAURI_SIGNING_PRIVATE_KEY`，其密码在 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`，
本地副本 `~/.tauri/myredis-updater.key`（`.pub` 在同目录）。

**轮换密钥**用 `bash scripts/rotate-signing-key.sh`：交互式输入密码（不进命令历史）→ 重新生成密钥对
→ 把新公钥写回 `tauri.conf.json`（打印新旧 key id）→ 用 stdin 覆盖两个 CI secret → 跑一次签名自检。
它同时支持 `KEY_PATH` / `CONFIG_PATH` / `SKIP_GH=1` / `SKIP_GENERATE=1`（自己 `tauri signer generate`
之后接着换公钥与 secret）。

> ⚠️ 轮换的前提是**还没有任何客户端带着旧公钥发出去**。公钥是编译进安装包的，旧公钥一旦随包发布，
> 换私钥就等于那些用户再也收不到自动更新（只能手动重装）。v0.0.13 发布之后不要再换密钥 ——
> 除非接受「老用户手动重装一次」。

另一个必须记住的细节：tauri 要求 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 这个变量**存在**。
空值 = 无密码；完全缺失则会去交互式索要密码（CI 里会卡住）。所以工作流里显式传了这个 secret，
`check-signing-key.mjs` 也会兜底给子进程补一个值。
私钥**内容**与**密码**都要备份：密码忘了 = 私钥作废 = 之后再也发不出自动更新。

`check-deploy-env` 会检查这个私钥「是否存在」（见该脚本第 2 步）。但「存在」不等于「可用」：
私钥/密码配错时 `tauri build` 依然成功，只是产出的 `.sig` 客户端验不过（或干脆没有 `.sig`），
症状是「已安装的应用永远收不到更新」这种静默失败 —— 等几十分钟构建跑完、Release 也发出去才发现，
而发出去的版本收不回。所以第一道检查是 `check-signing-key.mjs`：它不停留在「变量存在」，
而是真跑一次 `tauri signer sign` 并比对 key id，判定私钥能不能用给的密码解开、是否与配置里的
公钥配对，顺带检查 `endpoints` 非空、`createUpdaterArtifacts: true`。判定依据与失败样例：

| 情况 | 表现 |
|------|------|
| 私钥缺失 | `缺少 TAURI_SIGNING_PRIVATE_KEY…` |
| 密码不对 / 私钥损坏 | 打印签名命令原文（如 `Wrong password for that key`）并给出两条常见原因 |
| 私钥与公钥不是一对 | 同时打印两边的 key id，指出「客户端一律验签失败」 |
| 私钥是别项目的、或公钥被换过 | 同上（key id 不同即可判定，不需要引入验签依赖） |
| `endpoints` 为空 / `createUpdaterArtifacts` 不为 true | 直接失败，指出客户端会取不到清单 / 不会产出 `.sig` |

key id 的取法：minisign 的公钥与签名载荷都是 `算法[2] | key id[8] | …`，key id 是明文，
正好是注释行里那串十六进制的倒序写法。注意 tauri 的 `.sig` 文件与配置里的 `pubkey` 存的都是
minisign 原文的 base64（`check-signing-key.mjs` 里 `minisignText()` 负责两种形式都认）。
npm 版 `@tauri-apps/cli`（CI 装的就是它）自带 `signer` 子命令，所以这一步不需要额外装 cargo 版 CLI。

**更新源**：`plugins.updater.endpoints` 指向
`https://github.com/myredisapp/myredis/releases/latest/download/latest.json`，
也就是「最新一个正式 Release 的附件」。预发布（tag 带 `-`，如 `v0.2.0-beta.1`）不会顶掉 `latest`，
所以预发布用户收不到自动更新、也不会拿到半成品，这符合预期。

**产物与清单**：

- `bundle.createUpdaterArtifacts: true` 后，tauri 在安装包旁边额外产出 `.sig`，
  以及 macOS 的 `.app.tar.gz`（更新用 tar.gz；`.dmg` 只用于首次安装，没有 `.sig`）；
- `rename-artifacts.mjs` 现在也认 `app` 类型（`.app.tar.gz`），并把 `.sig` 跟着安装包一起改名成
  `myredis-<tag>-<platform>.<ext>.sig`；
- `gen-latest-json.mjs` 在 Release 建好后执行，读各平台的 `.sig` 内容生成 `latest.json` 并上传为附件
  （`notes` 取自 Release 正文，即 `--generate-notes` 的结果）。**上传不等于客户端取得到**，
  所以紧接着还有一个「回读校验」步骤：按客户端真正使用的两个地址（`releases/download/<tag>/latest.json`
  与 `releases/latest/download/latest.json`）匿名 `curl` 回来与本地逐字节 `diff`。
  `latest` 别名在 Release 刚建好时可能延迟几秒，脚本重试 10 次 × 3 秒；预发布 tag 不占用 `latest`，
  只校验 tag 地址。缺附件/挂错名字/内容不一致都会在这里失败，而不是等用户点「检查更新」才发现。

**v0.0.13 的清单已按上述流程演练过**（用真实私钥对占位产物签名，跑 `rename-artifacts.mjs` 三个平台 +
`gen-latest-json.mjs`）：产出 `version 0.0.13`、4 个平台键（`darwin-aarch64` / `darwin-x86_64` /
`linux-x86_64` / `windows-x86_64`），下载地址指向
`releases/download/v0.0.13/myredis-v0.0.13-<platform>.<ext>`，且三份签名都能被 `tauri.conf.json`
里的公钥验过（`minisign -V`，等价于客户端下载后 `Update::download` 的校验）。
注意发布时清单里的 `version` 必须等于安装包版本，而安装包版本由构建 job 的
`set-version.mjs "<tag>"` 从 tag 写入；两边都来自同一个 tag，所以是自洽的。

平台键是 tauri 的 `OS-ARCH` 形式，必须与运行端算出来的 target 一致：macOS 出的是 universal 单包，
所以 `darwin-aarch64` 与 `darwin-x86_64` 两个键指向同一个 `.app.tar.gz`；Windows **只挂 NSIS**（`.exe`），
因为 msi 与 nsis 各有一套独立的卸载信息，用 msi 去更新 nsis 装的机器会留下两份安装。

命令链路（检查 / 后台下载 / 轮询进度 / 安装重启）见 §1.7 与 `src-tauri/src/commands/update.rs`。

**发版之前「检查更新」一定是失败的**：清单是发布阶段的产物，而 `releases/latest/download/latest.json`
读的是**最新那个 Release 的附件**。所以只要最新 Release 还是在启用自动更新（v0.0.12 及更早）之前打的，
这个地址就返回 404，界面上会提示「更新服务没有返回版本清单（当前最新发布可能还没附带 latest.json）」——
这不是客户端 bug，打一个带清单的新 tag 即可（见 §8.8）。同理，装了 v0.0.12 及更早版本的用户没有更新模块，
无法自动升上来，得手动装一次带自动更新的版本。

**本地演练（不发版也能看完整流程）**：`bash scripts/update-dryrun.sh` 会本地造一份已签名的占位安装包
与同构的 `latest.json`（版本号默认取当前版本 +1），起一个本地 HTTP 服务当更新源，并打印让开发版指向它的命令：

```bash
cargo tauri dev --config '{"plugins":{"updater":{"endpoints":["http://127.0.0.1:<port>/latest.json"]}}}'
```

`--config` 是**深合并**：只覆盖 `endpoints`，`pubkey` 等其余配置照旧（可用
`TAURI_CONFIG='<同上 JSON>' cargo build` 加 `grep` 二进制里的端点和公钥字符串复现这个结论）。
脚本里的 `minisign -V` 自检与客户端下载后的校验等价，用的是 `tauri.conf.json` 里那把公钥，
所以它同时验证了「私钥可用 + 公私钥配对 + 清单格式」。
注意演练包是占位文件（内容随意、只用来验签），**不要点「立即重启」**：开发版不是 `.app` 包，
插件会把 `current_exe` 的父目录（`target/debug`）当成安装目标。
