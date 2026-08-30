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

# 单元测试
cargo test

# 集成测试（需先启动 Redis，如 docker run -p 6379:6379 redis）
cargo test --test ping_integration -- --nocapture

# 代码检查
cargo clippy -- -D warnings
```

## 7. 运行方式

```bash
cd src-tauri
cargo tauri dev
```

前端页面会加载 `web/index.html`（当前为「后端连通性演示」页面）：
- 输入连接名 / 主机 / 端口，点「连接」测试并保存连接。
- 在「已保存连接」列表可对连接执行 PING。