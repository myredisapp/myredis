# 麦地缓存 — 开发方案书

> 版本：v0.1（草案，待评审）
> 文档状态：**待确认**
> 技术栈：Tauri 2 + Rust + 原生 HTML/CSS/JS

---

## 1. 项目背景与目标

「麦地缓存」是一款 **Redis 图形化桌面管理客户端**，目标用户为需要连接、浏览、编辑、监控 Redis 实例的开发与运维人员。

### 1.1 当前现状

- 已基于 **Tauri 2** 完成桌面壳工程搭建（Rust 后端 + WebView 前端），可正常编译运行，窗口标题为「麦地缓存」。
- 前端提供了完整的界面，但**全部数据为前端的 mock 假数据**，具体包括：
  - 连接管理：`CONNECTIONS` 数组（内存态，含本地/生产等示例连接）。
  - Key 树：`KEY_DATA` + `flattenKeys()` 深度模拟的树形/扁平结构。
  - Key 详情：按 `string / hash / list / set / zset` 分类型渲染与编辑。
  - 终端：`executeCommand()` 内置了 `PING / INFO / DBSIZE / GET / SET / KEYS / FLUSHDB / HELP` 的**本地模拟实现**。
  - 服务器状态：`setInterval` 每 3 秒用`Math.random()`伪造的 CPU / 内存 / 连接数 / Key 总数。
- 连接、读取、写入、终端等**全部尚无真实 Redis 交互**。

### 1.2 项目目标

- **核心**：使用 Rust 作为后端，真实连接与操作 Redis，替换当前全部 mock 逻辑。
- **形态**：Rust 负责网络与 Redis 交互，前端仅负责展示与交互，二者通过 **Tauri Command** 通信。
- **范围**：本方案聚焦「后端能力」与「前后端对接设计」。前端 UI 已具备，本次以**最小改动接入后端**为主。

### 1.3 非目标（本期不做）

- 不引入重量级前端框架（保持原生 JS，或前端改动时再评估）。
- 不做多标签页 / 分片 / 集群可观测面板等高级运维能力（可后续迭代）。
- 不实现实时命令监控（Monitor）等高成本功能，保留为后续迭代。

---

## 2. 技术选型

| 层次 | 技术选型 | 说明 |
|------|----------|------|
| 桌面框架 | **Tauri 2** | 前端 WebView + Rust 后端 |
| 后端语言 | **Rust** (edition 2021) | 内存安全、性能高，适合做 Redis 客户端 |
| Redis 客户端 | **redis-rs**（`redis` crate with `tokio`） | Rust 生态最主流 Redis 客户端，支持连接池、异步 |
| 异步运行时 | **tokio** | Redis 异步驱动 |
| 序列化 | **serde / serde_json** | 前后端 JSON 数据交换 |
| 前端 | 原生 HTML/CSS/JS（已具备） | 通过 `@tauri-apps/api` 调 Rust 命令 |

> 依赖清单（Cargo）说明见下方第 5 节。

---

## 3. 系统架构

```text
┌──────────────────────────────────────────────────────────────┐
│                        WebView (前端)                       │
│   连接管理 UI | Key 树 | Key 编辑器 | 终端 | 状态栏          │
└─────────────────────────┬────────────────────────────────────┘
                        │  Tauri Invoke (Command, async)
┌───────────────────────▼────────────────────────────────────┐
│                      Tauri 事件/命令层                       │
│          (src/commands/*.rs — Redis 命令封装)                │
└───────────────────────┬────────────────────────────────────┘
                        │
┌───────────────────────▼────────────────────────────────────┐
│                     领域服务层 (Rust)                    │
│   connection.rs     连接管理 + 连接池                     │
│   service/key*.rs   Key 操作 (list/get/set/del/ttl...)   │
│   service/terminal 命令执行 (exec)                       │
│   service/server.rs 服务器信息 (info/ping)                │
└───────────────────────┬────────────────────────────────────┘
                        │ redis client pool
┌───────────────────────▼────────────────────────────────────┐
│              redis-rs (redis:// 连接池)                   │
└───────────────────────┬────────────────────────────────────┘
                        │ RESP
              ┌─────────▼─────────┐
              │   目标 Redis 服务器 │
              └───────────────────┘
```

**单向依赖**：前端始终通过 `invoke()` 调用 Tauri Command，不直接持有 Redis 连接。

---

## 4. 核心功能设计

### 4.1 连接管理

前端现有的连接保存逻辑将迁移到 Rust 侧持久化（存本地配置文件），并提供完整 CRUD。

#### 连接模型（Rust `struct`）

```rust
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connection {
    pub id: String,          // 唯一 ID（例如 "conn_1693300000000"）
    pub name: String,        // 显示名称
    pub host: String,       // 主机
    pub port: u16,         // 端口
    #[serde(default)]
    pub username: Option<String>, // 可选用户名（预留，本期固定为 None）
    #[serde(default)]
    pub password: Option<String>, // 可选密码（见 9.2 密码存储说明）
    #[serde(default)]
    pub is_cluster: bool,   // 是否集群连接（见 9.4）
    // 连接类型（单机 / 集群）
    pub conn_type: ConnType, // 见 4.1.1
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ConnType {
    Single,  // 单机
    Cluster, // 集群
}
```

#### 存储方案（二选一，见第 8 节确认）

- **方案 A（推荐）**：存储于单个 JSON 文件 `connections.json`，位于系统数据目录（Tauri `AppConfigDir`）。
- 方案 B：直接写入 Tauri 的 `settings.json`（适合极小配置，但不便扩展）。

> ⚠️ 出于安全考虑，无论是连接配置还是密码，均**不落库**，仅保存在前端内存变量中（即使如此也仅用于本次会话，重启后需重新输入密码）。

#### 连接池设计（`connection.rs`）

```rust
use std::collections::HashMap;
use std::sync::Mutex;

use tauri::State;

/// 全局连接池单例
pub struct AppState {
    /// 连接名 -> Redis 客户端池
    pub pools: Mutex<HashMap<String, ConnectionPool>>,
}

pub struct ConnectionPool {
    pub redis: redis::aio::ConnectionManager,
    pub config: Connection,
}
```

**要点**：
- 每个连接维护一个连接名 → 连接池的映射，支持并发命令。
- 连接选择（避免每个操作重建连接）。
- 提供统一 `get_pool(conn_id)` 查询连接方法。

---

### 4.2 服务器信息（原先 mock 的 CPU / 内存 / 连接数 / Key 数）

前端底栏显示 CPU、内存、连接数、Key 总数。这些需调用 Redis 的 `INFO` 命令解析：

| 前端显示 | Redis 来源 | 说明 |
|---------|-----------|------|
| Key 总数 | `DBSIZE` | 当前 DB 的 key 数 |
| 内存 | `INFO memory` → `used_memory_human` | 已用内存 |
| 连接数 | `INFO clients` → `connected_clients` | 已连接客户端数 |
| CPU | `INFO cpu` | 计算两帧差 |

---

### 4.2 连接管理

| 操作 | 前端 | Rust Command | redis 命令 |
|------|------|------------|-----------|
| 新增连接 | `saveConnection()` | `@tauri-apps/api` → `save_connection` | `PING`（测试连通性） |
| 删除连接 | `renderConnections()` | `delete_connection` | - |
| 测试连接 | — | `test_connection` | `PING` |
| 列出连接 | `renderConnections()` | `list_connections` | - |
| 连接池 | `connection_pool` | `get_connection_pool` | - |

#### 4.2.1 集群连接（Cluster）

通过连接模型中的 `conn_type: Cluster` 标识集群连接，使用 `redis::cluster::ClusterClient` 建立集群客户端。

| 能力 | 说明 |
|------|------|
| 连接方式 | 启动时为每个节点探测集群拓扑，维护节点 → 连接池映射 |
| 命令路由 | `CLUSTER KEYSLOT` 计算 key 的 hash slot，路由到对应主节点执行 |
| 故障转移 | 节点不可用时，从 `CLUSTER SLOTS` 自动发现新主节点（本期不做手动故障转移） |
| Key 统计 | `INFO cluster` / `CLUSTER INFO` 聚合各节点数据 |
| 批量操作 | `SCAN` / `DBSIZE` 对每个主节点遍历后合并结果 |

> ⚠️ 集群模式下**分页顺序不保证跨节点稳定**，纯 `SCAN` 可能与其他节点数据混合，需要按 key 的 slot 排序（默认已按 hash slot 排序）。

#### 4.2.2 连接模型扩展

- 新增 `ConnType` 枚举（`Single` / `Cluster`），在左侧连接列表中通过不同图标区分。
- `test_connection` 对 `Cluster` 调用 `CLUSTER INFO` 验证，对 `Single` 调用 `PING` 验证。

#### 4.2.3 不受支持能力的显式处理

启动 / 连接时若检测到以下情况，**统一返回语义化错误**，不静默降级：

```rust
// TLS / rediss 统一入口校验
if url.scheme() == "rediss" {
    return Err("当前版本不支持 TLS/SSL 加密连接 (rediss)，请在连接配置中关闭 SSL 或使用明文连接".into());
}
```

| 场景 | 错误提示 |
|------|----------|
| `rediss://` 协议 | `暂不支持 TLS 加密连接 (rediss)` |
| 输入了 ACL 用户名 | 界面无用户名输入框，忽略该值 |
| 集群模式下未开启 `cluster-enabled` | 返回 Redis 自身错误，透传展示 |

#### Rust Command 示例

```rust
#[tauri::command]
pub async fn test_connection(
    conn: State<'_, AppState>,
    conn_id: String,
) -> Result<(), String> {
    let pool = conn.get_connection_pool(&conn_id).await?;
    pool.redis.get("__ping__")
        .map_err(|e| e.to_string())
        .map(|_| ())
}
```

---

### 4.3 Key 树与虚拟滚动

Key 量大时，前端一次性渲染会导致卡顿。本方案采用**后端分页 + 前端渲染**：

| 机制 | 实现 |
|------|------|
| 后端扫描 | `SCAN cursor MATCH pattern COUNT n`，避免 `KEYS *` 阻塞 |
| 分页加载 | 维护游标，`next_page` / `previous_page` |
| 正则搜索 | 前端搜索直接传 `pattern` 给后端 SCAN |
| 虚拟化 | 先渲染可视区域的 Key，避免一次渲染过多 DOM |

**为何不用 `KEYS *`**：生产环境 `KEYS *` 是阻塞命令，会导致 Redis 卡顿。用 `SCAN` 游标迭代更安全。

---

### 4.4 Key 操作接口

以下为 Rust 侧暴露给前端的全部 Command 清单。前端通过 `import { invoke } from '@tauri-apps/api/core'` 调用。

#### 泛型 Key 操作

| Command | 请求参数 | 响应 | redis 命令 |
|---------|---------|------|-----------|
| `list_keys` | `conn_id, pattern, cursor, count` | `{ keys: Vec<KeyMeta>, next_cursor }` | `SCAN` |
| `get_key` | `conn_id, key` | `KeyData` | `DUMP`, `TYPE`, `TTL` |
| `set_key` | `Connection, key, value` | `()` | `RESTORE` / `SET` |
| `delete_key` | `conn_id, key` | `u64` (影响行数) | `DEL` |
| `exists_key` | `conn_id, key` | `bool` | `EXISTS` |
| `expire_key` | `conn_id, key, ttl` | `bool` | `EXPIRE` |

#### 各类型专属操作

| Command | 说明 | redis 命令 |
|---------|------|-----------|
| `get_hash` / `set_hash` / `hget_all` | Hash 操作 | `HGETALL`, `HSET`, ... |
| `push_list` / `range_list` / `pop_list` | List 操作 | `LRANGE`, `LPUSH`, `LPOP` |
| `get_set` / `set_member` | Set 操作 | `SMEMBERS`, `SADD` |
| `get_zset` / `zadd_zset` | Sorted Set 操作 | `ZADD`, `ZRANGE` |
---

### 4.4 终端（命令执行）

将现有 `executeCommand()` 的 mock 实现，改为连接后把用户输入原样转发给 Redis 执行：

```rust
#[tauri::command]
async fn execute_command(params: CommandParams) -> Result<Value, String> {
    let cmd = parse_command(&params.raw)?;  // 解析 "GET foo" -> ("GET", ["foo"])
    session.execute(cmd).await
}
```

> ⚠️ 前端 JS **不再自行实现** `PING/GET/SET/KEYS` 等命令，统一由 Rust 后端调用 Redis，保证与服务器状态一致。

---

### 4.5 异常与容错设计（重要）

Redis 客户端面向网络环境，**连接超时、网络中断、命令超时**等异常必须被妥善处理，避免界面假死或状态错乱。

#### 4.5.1 超时控制策略

| 超时类型 | 默认值 | 实现 |
|---------|:------:|------|
| 连接超时 `connect_timeout` | 5s | 建立 TCP 连接的最长等待 |
| 命令超时 `timeout` | 30s | 单条命令从发出到返回的最长等待 |
| 空闲超时 `connection_timeout` | 60s | 连接池中空闲连接的最大存活时间 |

**配置常量（`src/config.rs`）**

```rust
use std::time::Duration;

/// 连接相关超时配置
pub struct ConnectionTimeout {
    /// 建立 TCP 连接超时
    pub connect: Duration,
    /// 单条命令执行超时
    pub command: Duration,
    /// 空闲连接回收时间（用于断线重连）

    pub idle: Duration,
}

impl Default for ConnectionTimeout {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(5),   // 5 秒连不上直接失败
            command: Duration::from_secs(10),  // 单命令 10 秒没返回视为超时
            idle: Duration::from_secs(60),
        }
    }
}
```

#### 4.5.2 连接中断处理

| 场景 | 前端表现 | 后端处理 |
|------|----------|----------|
| 连接超时 | 弹窗提示「连接超时，请检查网络或地址」 | `tokio::time::timeout` 包裹连接逻辑，超时返回 `ConnectionTimeout` 错误 |
| 连接被拒 | 同上 | 补 `IO Error: Connection refused` |
| 连接中断（保持连接后对端关闭） | 标记该连接为 `disconnected`，状态栏变红 | 捕获 IO 错误，标记连接失效，下次操作自动重连 |
| TLS 错误 | 提示「不支持的协议」 | 拦截 `rediss://` |
| Redis 密码错误 | 弹窗「认证失败」 | 传回 Redis 原始错误 |

**统一错误类型（`src/error.rs`）**

```rust
/// 应用统一错误类型，前端通过 `e.message` 展示给用户
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Redis 连接失败: {0}")]
    Connection(#[from] redis::RedisError),
    #[error("连接不存在: {0}")]
    ConnectionNotFound(String),
    #[error("连接超时: {}", .0)]
    Timeout(String),
    #[error("TLS 尚未支持: {0}")]
    TlsNotSupported(String),
    #[error("ACL 用户名尚未支持")]
    AclNotSupported,
    #[error("{0}")]
    Other(String),
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // 只序列化给用户看的 message 字段
        serializer.serialize_str(&self.to_string())
    }
}
```

#### 4.5.3 命令执行超时兜底

使用 `tokio::time::timeout` 包裹所有 Redis 命令，从根本上避免命令卡死：

```rust
async fn execute_with_timeout<F, T>(&self, future: F) -> Result<T, AppError>
where
    F: Future<Output = Result<T, AppError>>,
{
    match tokio::time::timeout(self.timeout.command, future).await {
        Ok(v) => v,
        Err(_) => Err(AppError::Timeout(self.timeout.command)),
    }
}
```

#### 4.5.4 断线自动重连

- 单机模式：每次命令发现连接失效（`ConnectionRefused` / `Timeout`）时，**移除旧连接**，下次操作前按配置重新 `connect`。
- 使用 `redis` crate 的连接池特性，连接不可用时自动重建队列。
- **注意**：Redis 无操作时可能被服务端或中间层（如 NAT）静默断开，因此需要：
  - 回收空闲连接：`get_connection` 时校验。
  - 提供前端「重连 (Reconnect)」按钮，主动强制重建连接并返回最新状态。

#### 4.5.5 前端错误展示（统一入口）

```js
// web/src/main.js 中统一错误处理
async function callCommand(cmd, args) {
    try {
        return await invoke(cmd, args);
    } catch (err) {
        showToast(`操作失败: ${err}`, 'error');
        throw err; // 或吞掉，视场景而定
    }
}
```

所有 Redis 操作错误（类型转换、命令参数错、连接错误）统一从后端 `AppError` 序列化为字符串返回，前端展示。

---

## 5. 依赖清单（Cargo.toml）

```toml
[dependencies]
tauri = { version = "2", features = [] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
# ===== Redis 相关 =====
redis = { version = "0.25", features = ["tokio-comp"] }   # tokio 异步客户端
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
# 连接信息持久化
directories = "5"            # 跨平台数据目录
```

> 如需连接池可额外加 `r2d2-redis` 或 `deadpool-redis`，首批暂用简单连接 + Mutex 即可。

---

## 6. 目录结构（目标状态）

```text
myredis/
├── Cargo.toml
├── index.html -> web/index.html   # 前端入口（已存在，迁移）
├── src-tauri/
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── src/
│   │   ├── lib.rs          # 注册 commands
│   │   ├── main.rs
│   │   ├── commands/        # Tauri Command 层
│   │   │   ├── mod.rs
│   │   │   ├── connection.rs
│   │   │   ├── key.rs
│   │   │   └── server.rs
│   │   └── service/        # 业务逻辑层
│   │       └── mod.rs
│   └── ...
└── web/                   # 前端静态资源
    └── index.html
```

---

## 7. 前后端通信协议

前端通过 `invoke` 调用后端命令：

```js
import { invoke } from '@tauri-apps/api/core';

// 示例：加载 Key 树
const data = await invoke('list_keys', { connId: 'conn1', pattern: '*', cursor: 0 });
```

---

## 8. 迭代计划（Milestone）

| 里程碑 | 内容 | 交付 |
|--------|------|------|
| **M1 前端接入后端** | 前端调用 Rust 后端 `list_keys / get_key` | 能从真实 Redis 读出 key，替代 mock |
| **M2 连接管理** | 连接 CRUD + 配置持久化到本地文件 | 可新增/编辑/删除连接 |
| **M3 Key 完整 CRUD** | string/hash/list/set/zset 全部类型可视化读写 | 基础管理能力闭环 |
| **M4 终端与搜索** | 替换 `executeCommand` mock 为真实命令转发 | 终端可执行 Redis 命令 |
| **M5 服务器监控** | 解析 `INFO` 显示真实指标 | 状态栏显示真实服务器状态 |

---

## 9. 需求决策记录（ADR）

> 以下为需求确认后的最终决定，作为开发基线，双方确认后即生效。

### 9.1 连接配置持久化

**决策：** 使用**本地独立 JSON 文件**持久化连接配置。

- 文件名：`connections.json`，存放于系统应用数据目录（Tauri `AppConfigDir`）。
- 每次保存连接时原子写入（先写临时文件再 rename），避免写坏导致配置丢失。
- 应用启动时加载，进程内维护 `Vec<Connection>` 缓存。

### 9.2 密码存储

**决策：需要保存密码**，保存于上述 JSON 配置文件中。

> ⚠️ **安全说明（务必知晓）**：本方案采用本地明文（或可逆 base64）存储密码，任何有本机文件读取权限的进程 / 用户均可获取。请在 UI 中向用户明确提示风险。若后续对安全有更高要求，可改为系统密钥链（如 macOS Keychain / Windows Credential Manager / Secret Service）存储。

具体字段：`Connection` 结构体新增 `password: Option<String>` 字段，仅当输入非空时才更新该字段（编辑连接时留空表示不修改密码）。为后续支持密钥链 / 加密做扩展。

### 9.3 目标 Redis 版本

**决策：需兼容 Redis 6.0 以下版本（即 Redis 2.8+ / 3.x / 4.x / 5.x / 6.x / 7.x 均需兼容）。**

- 由于 6.0 以下没有 ACL，命令封装时需**避免使用仅新版本才有的参数**。
- 交互时注意：`CLIENT SETINFO` 等命令在旧版本可能报错，需用 `|| true` 容错或降级处理。
- 与服务端无关的前端渲染逻辑保持一致，运行期针对低版本 Redis 做降级或提示。

### 9.4 身份认证与网络能力

| 能力 | 支持 | 说明 |
|------|:----:|------|
| 单节点 Redis | ✅ | 必做 |
| 集群（Cluster） | ✅ | 见下 |
| 主从 / Sentinel | ⚠️ 部分 | 本期仅透传命令，不实现故障转移 |
| 密码认证（`AUTH password`） | ✅ | 明文密码，见 9.2 |
| **TLS / SSL（rediss）** | ❌ | **本期不支持**，详见下方说明 |
| **ACL 用户名鉴权** | ❌ | **本期不支持**，仅支持默认 `default` 用户 |
| SSH 隧道 | ❌ | 本期不支持 |

#### 9.4.1 TLS / rediss 不支持说明

- 连接 URL 固定为 `redis://`，**不提供** `rediss://` 或 `tls://` 协议，也不加载任何 CA 证书。
- 若用户输入 `rediss://` 开头的地址，程序将返回错误提示「暂不支持 TLS 加密连接 (rediss)」。
- 依赖 `redis` crate 时关闭其 `tls` / `tokio-native-tls` 等 feature，避免误启用。

#### 9.4.2 ACL 不支持说明

- 连接仅支持 `AUTH password` 密码认证，登录框**不出现**「用户名」字段。
- 连接参数中 `username` 固定为 `None`，不使用 `AUTH username password` 双参数形式。
- 如后续需要，可平滑支持 `AUTH username password` 而不破坏现有结构（字段已预留 `Option`），但本期不开放。

### 9.5 前端技术栈

**决策：坚持原生 JS**，不引入 React / Vue / 构建工具。

- 现状前端文件保持单文件 `web/index.html`，通过 `@tauri-apps/api` 调后端命令。
- 若后续复杂度上升，再在 `web/` 内引入 vitest 测试 / esbuild 打包，但与桌面客户端解耦,不影响本轮。

### 9.6 已确认的需求核对

| 序号 | 结论 | 涉及章节 |
|:----:|------|:--------:|
| 1 | 连接配置存本地独立 JSON | 4.1 |
| 2 | 存储密码（明文/可逆加密） | 4.1 / 9.2 |
| 3 | 兼容 Redis < 6.0 | 9.3 |
| 4 | 支持集群；不支持 TLS/ACL 用户名并写明 | 9.4 |
| 5 | 前端用原生 JS | 2 |