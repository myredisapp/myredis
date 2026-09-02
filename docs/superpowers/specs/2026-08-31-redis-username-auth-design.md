# Redis ACL 用户名认证支持设计

> 日期：2026-08-31  
> 状态：已批准  
> 关联项目：麦地缓存（Maidi Cache）

## 背景

当前「麦地缓存」Redis 客户端的连接模型仅支持密码（`password`）字段，前端连接对话框虽已提供「用户名 (可选)」输入框，但输入内容不会被后端识别，也无法通过 `redis://username:password@host:port/db` 的形式进行 Redis 6.0+ ACL 认证。

## 目标

支持用户在新建/编辑 Redis 连接时输入用户名，连接时通过 Redis ACL 机制完成 `AUTH username password` 认证；同时保持对无 ACL 的旧版 Redis 的兼容（用户名留空即可）。

## 方案

采用「URL 内嵌用户名」的最小改动方案。

### 后端改动

1. **`src-tauri/src/models/connection.rs`**
   - 在 `Connection` 结构体中新增字段：
     ```rust
     #[serde(default, skip_serializing_if = "Option::is_none")]
     pub username: Option<String>,
     ```
   - 修改 `to_connection_url()`，按以下规则生成连接 URL：
     - 用户名 + 密码同时存在：`redis://user:pass@host:port/db`
     - 仅用户名：`redis://user@host:port/db`
     - 仅密码：保持现有 `redis://:pass@host:port/db`
     - 都为空：`redis://host:port/db`
   - 用户名与密码使用 `percent-encoding` 按 RFC 3986 进行编码，避免 `@`、`/`、`:`、`?`、`#` 等特殊字符破坏 URL 结构。

2. **`src-tauri/tests/ping_integration.rs`**
   - 更新所有手动构造 `Connection` 的测试用例，补充 `username: None`。

### 前端改动

**`web/index.html`** 中已存在 `connUser` 输入框，按现有 `password` 字段的对称方式打通：

- `loadConnections()`：将后端返回的 `c.username` 映射到前端连接对象。
- `openModal(conn)`：编辑连接时回填 `conn.username`。
- 复制连接时：回填原连接的 `username`。
- `connectTo(conn)`：调用 `connect` 命令时传递 `username: conn.username || null`。
- `saveConnection()`：保存时读取 `connUser` 并作为 `username` 发送到后端。

### 兼容性

- 旧版 `connections.json` 中无 `username` 字段，因设置了 `serde(default)`，反序列化后自动为 `None`，不影响现有连接。
- 对 Redis < 6.0 的服务器，用户只需将用户名留空，即可继续使用原有 `AUTH password` 方式连接。

### 测试

- 单元测试覆盖 `to_connection_url()` 的四种组合（空/仅用户名/仅密码/用户名+密码）。
- 集成测试中的 `Connection` 构造补充 `username` 字段，保证编译通过。

## 非目标

- 不引入应用层独立登录体系。
- 不改动密码存储方式（仍按现有方案处理）。
- 不支持 TLS/SSL（`rediss://`）以及 SSH 隧道，保持与现有策略一致。
