# 连接对话框「测试连接」按钮设计

> 日期：2026-09-02  
> 状态：已批准  
> 关联项目：麦地缓存（Maidi Cache）

## 背景

当前「麦地缓存」的连接对话框（新建/编辑连接）只有「取消」和「保存连接」两个按钮。用户填写主机、端口、密码后，必须保存才能知道连接参数是否正确。为了提升体验，需要在对话框中增加一个「测试连接」按钮，让用户在保存前先验证参数能否连上 Redis。

## 目标

- 在新建连接和编辑连接对话框中增加「测试连接」按钮。
- 点击后临时连接 Redis，执行 `PING`，验证成功后立即断开，不保留连接状态。
- 在按钮旁显示测试结果状态文字（成功/失败）。

## 方案

采用后端新增独立 `test_connection` 命令的方案，语义清晰且无副作用。

### 后端改动

1. **`src-tauri/src/connection_pool.rs`**
   - 在 `Pool` 上新增方法：
     ```rust
     pub async fn test(conn: &Connection) -> Result<String, AppError>
     ```
   - 实现逻辑：
     - 单机：通过 `conn.to_connection_url()` 打开 `redis::Client`，获取异步连接，执行 `PING`。
     - 集群：通过 `redis::cluster::ClusterClient` 打开集群连接，执行 `PING`。
     - 所有网络操作使用 `tokio::time::timeout` 包裹，默认 5 秒超时返回 `AppError::Timeout`。
   - 关键约束：**不写入 `Pool.inner` 连接池**，测试完成后连接自然释放，做到真正的临时测试。

2. **`src-tauri/src/commands/connection.rs`**
   - 新增 Tauri Command：
     ```rust
     #[tauri::command]
     pub async fn test_connection(conn: Connection) -> Result<String, String>
     ```
   - 直接调用 `Pool::test(&conn)`，错误通过 `.map_err(|e| e.to_string())` 透传前端。

3. **`src-tauri/src/lib.rs`**
   - 在 `invoke_handler` 中注册 `commands::connection::test_connection`。

### 前端改动

**`web/index.html`** 连接对话框（`connectionModal`）底部：

1. 在 `modal-actions` 区域左侧增加「测试连接」按钮，id 为 `modalTest`。
2. 在按钮右侧增加状态文字 `<span id="testStatus"></span>`。
3. 新增 `testConnection()` 函数：
   - 校验 `host`、`port` 已填写（连接名不是测试必填项）。
   - 构造与 `saveConnection()` 一致的 `conn` 对象。
   - 点击后：
     - 禁用测试按钮，按钮文字变为「测试中…」。
     - 状态文字显示「正在测试连接…」。
     - 调用 `invoke('test_connection', { conn })`。
     - 成功：状态文字显示 `✓ 连接成功（PONG）`，颜色为绿色。
     - 失败：状态文字显示 `✗ 失败原因`，颜色为红色。
     - 无论成败，恢复按钮文字与可用状态。
4. 打开/关闭对话框、修改表单内容时清空状态文字，避免显示过期结果。

### 错误处理

- **后端错误**：统一转换为字符串返回，包含但不限于：
  - 网络不可达 / 连接被拒绝
  - 密码错误（Redis 认证失败）
  - 集群模式连接异常
  - 连接超时
- **前端错误**：直接展示在 `#testStatus` 中，不再额外弹 Toast。
- **前端本地校验**：`host` 或 `port` 为空时直接提示，不发后端请求。

### 测试

- 新增集成测试 `src-tauri/tests/test_connection.rs`：
  - 成功 case：对本地 Redis（`redis://127.0.0.1:6379`）调用 `test_connection`，断言返回 `PONG`。
  - 失败 case：使用错误端口调用 `test_connection`，断言返回错误。
- 运行方式：
  ```bash
  cd src-tauri
  cargo test --test test_connection -- --nocapture
  ```

## 非目标

- 测试成功后不自动保存连接，仍需用户点击「保存连接」。
- 测试不保持长连接，验证完立即断开。
- 不改动现有 `connect` / `save_connection` / `ping` 命令的行为与签名。
- 不涉及 TLS/SSL、SSH 隧道、集群高级路由等未支持能力。
