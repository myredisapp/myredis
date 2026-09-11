# 开发进度与问题记录

> 本文档用于跟踪「麦地缓存」开发进度、已决策事项与已知问题。随开发持续更新。
> 最近更新：2024-08-30，开发者：Yan Shi，状态：开发中（后端骨架完成）。

---

## 项目状态总览

| 版本 | 状态 | 日期 | 说明 |
|:----:|:----:|:----:|------|
| v0.1.0 | 开发中 | 2024-08-30 | 后端核心骨架 + 连接 + PING 打通 |

---

## 1. 开发进度

> 说明：
> - ✅ 已完成
> - 🚧 进行中
> - ⬜ 未开始
> - ❌ 已取消 / 冻结

### 1.1 项目脚手架与基础（✅ 本轮完成）

- [x] ✅ Tauri 2 项目骨架（Rust 2021 edition）
- [x] ✅ `error.rs` 统一错误类型（`AppError` + `AppResult`）
- [x] ✅ 零编译警告（`cargo build` clean）
- [x] ✅ 单元测试（连接 URL 构造：无密码 / 带密码）
- [x] ✅ 集成测试（连真实 Redis `PING` → PONG）

### 1.2 连接管理

- [x] ✅ 源码模型 `Connection`（host/port/type/password）
- [x] ✅ 连接类型 `ConnType::{Single, Cluster}`（Cluster 预留）
- [x] ✅ 连接配置 JSON 持久化（`connections.json`，原子写入）
- [x] ✅ `connect` / `disconnect` 命令
- [x] ✅ `connect` / `disconnect` 命令
- [x] ✅ `list_connections` / `save_connection` / `delete_connection` 命令
- [x] ✅ 前端「连接 + PING」最小 Demo（后端连通性演示）

### 1.3 Redis 核心能力

- [x] ✅ 单机连接（`redis://`）
- [x] ✅ PING 连通性验证（真实 Redis 验证通过）
- [ ] ⬜ 命令执行（终端）
- [ ] ⬜ Key 操作（SCAN / GET/SET / 删除 / 过期）
- [x] ✅ 服务器信息（INFO 解析：版本/内存/连接数/Key 总数）

### 1.4 健壮性与异常处理

- [x] TLS 不支持的显式报错（`AppError::TlsNotSupported`）
- [x] 统一错误透传前端
- [x] 连接不存在 / 连接失败错误
- [ ] 命令超时（config 已预留，未接线）
- [ ] 交互式重连

### 1.9 前端对接

- [x] Tauri command 全部注册
- [x] 前端 Demo 对接连接管理与 PING
- [x] 点击连接 → 真实连 Redis → 显示服务器信息（`get_server_info`）

---

## 2. 不支持的功能（明确清单）

> 以下功能在 **当前版本明确不支持**。请勿误报为 bug。若需实现，先更新本文档与 PROJECT_PLAN.md 再开发。

### 网络层

- ❌ TLS/SSL（rediss）：仅支持 `redis://` 明文，连接会返回「暂不支持 TLS 加密连接 (rediss)」。
- ❌ SSH 隧道：不内置。
- ❌ ACL 用户名鉴权：仅 `AUTH password`，前端无用户名字段。

### 功能范围

- ❌ Redis Cluster 集群模式（`ConnType::Cluster` 已定义但连接时返回「开发中」错误）。
- ❌ 主从 / Sentinel 故障转移。

### 运维

- ✅ 连接断开可手动重连（`connect` 重入），不自动无限重试。

### 超时配置

- ❌ `config.rs` 中的 `ConnectionTimeout` / `AppConfig` 暂未接线（连接池用 redis-rs 默认超时），UI 配置项开发时接入。

---

## 3. 待确认 / 风险

| # | 事项 | 状态 | 备注 |
|---|------|:----:|------|
| 1 | 超时配置是否暴露给用户 | 待定 | 现为硬编码默认值 |
| 2 | 集群支持优先级 | 低 | 已留类型，未实现 |

---

## 4. 变更记录

| 日期 | 版本 | 说明 | 作者 |
|------|------|------|------|
| 2024-08-30 | v0.1.0 | 后端骨架 + 连接模型 + 持久化 + PING 命令 + 测试 | Claude |

---

## 5. 代码质量规范（强制）

1. **注释**：核心公有结构/方法必须有 `///` 文档注释；逻辑处写行内注释说明「为什么」。
2. **错误处理**：禁止 `unwrap()` / `panic!` 直接抛出（除配置加载等一次性场景可 `expect`）。
3. **超时**：所有网络 IO 包 `tokio::time::timeout`，防止卡死。
4. **提交**：遵循 Conventional Commits。
5. **检查**：提交前运行 `cargo clippy` 和 `cargo fmt --check`，保证无警告。
6. **测试**：核心逻辑（URL、解析）写单元测试；网络相关写带 `#[ignore]` 的集成测试。

---

## 6. 本地开发命令

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
```

## 7. 运行方式

```bash
cd src-tauri
cargo tauri dev
```

前端页面会加载 `frontend/index.html`（当前为「后端连通性演示」页面）：
- 输入连接名 / 主机 / 端口，点「连接」测试并保存连接。
- 在「已保存连接」列表可对连接执行 PING。

## 8. 打包约定

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
