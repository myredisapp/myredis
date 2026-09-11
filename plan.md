# 支持连接 Redis 集群（Cluster）实现方案

## 背景与现状

项目「麦地缓存」是一个 Tauri 2 + Rust + 原生 JS 的 Redis 图形化管理客户端。
当前**只支持单机连接**，集群（Cluster）仅定义了类型与占位逻辑，尚未真正打通。

### 现状梳理

**后端（Rust / redis-rs 0.25.5，已启用 `cluster` + `cluster-async` feature）**

- `models/connection.rs`：`ConnType::{Single, Cluster}` 已定义；`Connection` 含 `host/port/type/username/password/db` 等字段；`to_connection_url()` 只生成单机 URL。
- `connection_pool.rs`：核心连接池。
  - `Entry { conn, single: Option<ConnectionManager> }` —— **只缓存单机连接管理器**。
  - `connect()`：`Single` 分支用 `ConnectionManager` 缓存；`Cluster` 分支**只校验连接、不缓存任何句柄**（`single: None`）。
  - `manager(&id) -> Result<ConnectionManager, _>`：**命令层唯一取连接入口，只支持单机**，集群调用会报「该连接不是单机模式」。
  - `test()`：`Single`/`Cluster` 都已实现 PING 校验（集群用 `ClusterClient::get_async_connection()`）。
- `commands/key.rs`、`commands/server.rs`：所有命令（`list_keys`/`set_key`/`del_key`/`get_string`/`get_server_info`/`select_db`/`ping`）**全部通过 `pool.manager(&conn_id)` 拿单机 `ConnectionManager` 执行**。
- `lib.rs`：注册了全部命令。

**前端（frontend/index.html，单文件）**

- 连接表单已有「集群模式」复选框（`#connCluster`），保存/测试时已传 `type: 'cluster'`。
- `connectTo()` 已传 `type` 给后端。
- 有数据库选择器 `#dbSelect`（集群不支持 `SELECT`，需处理）。

### 核心差距

> **命令执行层（`pool.manager()`）只支持单机 `ConnectionManager`，缺少对 `ClusterConnection` 的分发能力。** 这是打通集群的关键。

---

## 目标

让应用能够**连接、浏览、增删改查 Redis 集群（Cluster）中的数据**，与单机体验一致。

## 非目标（明确不做）

- 集群的故障转移、扩缩容等运维能力（由 Redis 集群自身负责）。
- 跨 slot 的多 key 操作（`MGET`/`DEL` 多个 key 时若跨 slot 会失败，属 Redis 集群固有限制，报错透传即可）。
- 集群的 `SELECT` 切换数据库（集群只有 db0，前端隐藏选择器）。

---

## 技术方案

### 核心思路：抽象「可执行命令的连接」

redis-rs 中，单机 `ConnectionManager` 和集群 `ClusterConnection` 都实现了 `ConnectionLike` trait，
且都支持 `cmd.query_async(&mut conn)`。因此可以**用枚举封装两种连接句柄**，命令层通过它统一执行。

### 1. 连接池抽象（`connection_pool.rs`）

新增一个枚举，统一两种连接句柄：

```rust
/// 可执行 Redis 命令的连接句柄（单机 or 集群）
pub enum Conn {
    Single(ConnectionManager),
    Cluster(redis::cluster_async::ClusterConnection),
}
```

- 为 `Conn` 实现 `ConnectionLike`（转发到内部变体），或提供 `async fn cmd(&mut self, cmd: &Cmd) -> Result<Value, RedisError>` 便捷方法。
- `Entry` 改为持有 `Conn`，替代现在的 `single: Option<ConnectionManager>`。
- 新增 `pool.conn(&id) -> Result<Conn, AppError>`，命令层用它替代 `pool.manager()`。
- `manager(&id)` 保留（或内部改调 `conn()` 后取单机变体），避免大改命令层；但推荐命令层直接改用 `conn()`。

### 2. 连接建立（`connect` / `test` 的 Cluster 分支）

- `connect()` 的 `Cluster` 分支：用 `ClusterClient::new(vec![url])` + `get_async_connection()` 建立连接，**缓存到 `Entry.conn`**（当前是丢弃句柄）。
- `test()` 的 `Cluster` 分支：已实现，保持即可（`ClusterClient::new` + `get_async_connection` + PING）。
- 集群 URL 构造：`to_connection_url()` 目前生成 `redis://host:port/db`。集群应忽略 `db` 段（或保留但无副作用），需确认 redis-rs 集群客户端能解析。**建议**：为集群单独构造 URL（去掉 `/db` 后缀），避免误解析。

### 3. 命令层改造（`commands/key.rs`、`commands/server.rs`）

将 `let mut con = pool.manager(&conn_id)?;` 统一改为 `let mut con = pool.conn(&conn_id)?;`。
由于 `Conn` 实现了 `ConnectionLike`，`query_async` 调用方式不变，改动集中在取句柄处。

涉及命令：`list_keys`、`set_key_inner`、`del_key`、`get_string`、`get_server_info`、`ping`。

### 4. 集群专属处理

- **`select_db`**：集群不支持 `SELECT`，若连接为集群类型，前端隐藏选择器、后端返回错误或 no-op。建议后端在集群模式下返回明确错误「集群模式不支持切换数据库」。
- **`get_server_info`**：`CONFIG GET dir` 在集群下可能因权限/节点返回空，已有 `.ok()` 兜底，保持健壮即可。
- **只读 `READONLY`**：集群连接在 `connect` 时若只读，可发送 `READONLY` 命令（现有单机逻辑已做，集群分支补上）。

### 5. 前端（`frontend/index.html`）

- 连接表单已有「集群模式」复选框，**无需新增 UI**。
- 连接建立后，若 `conn.type === 'cluster'`，隐藏数据库选择器 `#dbSelectorWrap`（集群只有 db0）。
- 其余 key 浏览、增删改查逻辑复用现有代码。

---

## 涉及文件

| 文件 | 改动 |
|------|------|
| `src-tauri/src/connection_pool.rs` | 核心：新增 `Conn` 枚举、`Entry` 改造、`connect`/`conn` 支持集群 |
| `src-tauri/src/commands/key.rs` | `pool.manager()` → `pool.conn()` |
| `src-tauri/src/commands/server.rs` | 同上；`select_db` 集群报错 |
| `src-tauri/src/models/connection.rs` | （可选）集群 URL 构造 |
| `frontend/index.html` | 集群时隐藏数据库选择器 |

---

## 验证方式

1. **单元测试**：`cargo test`（现有 URL 构造测试保持通过）。
2. **本地起一个 Redis 集群**（`redis-cli --cluster create ...` 或 docker 起 6 个节点），用应用连接：
   - 能建立连接、PING 成功、显示服务器信息。
   - 能列出 key、查看 string 值、新建/删除 key。
3. **回归**：单机连接功能不受影响。
4. **边界**：集群连接时数据库选择器隐藏；`select_db` 返回明确错误。

---

## 风险与注意事项

- **redis-rs 集群 URL 解析**：`ClusterClient::new` 对 URL 格式敏感，需验证 `redis://host:port` 形式（不带 db）可正常解析。
- **连接复用**：集群连接每次命令是否复用同一 `ClusterConnection` 需确认（`ClusterConnection` 内部是 `mpsc` 发送者，可 clone 共享）。
- **`ConnectionLike` 实现**：若手动实现较繁琐，可改用「命令层 match 分发」的朴素方案（`match &mut conn { Single(c) => ..., Cluster(c) => ... }`），但推荐用 trait 对象/枚举封装，避免命令层散落 match。