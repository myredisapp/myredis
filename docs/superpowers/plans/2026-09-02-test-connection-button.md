# 连接对话框「测试连接」按钮 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在新建/编辑连接对话框中增加「测试连接」按钮，临时连接 Redis 执行 PING 并立即断开，在按钮旁显示结果。

**Architecture:** 后端在 `connection_pool.rs` 新增无缓存的 `Pool::test` 方法，暴露为 `test_connection` Tauri Command；前端在连接对话框增加按钮与状态文字，调用该命令并渲染结果。

**Tech Stack:** Tauri 2, Rust 2021, redis-rs 0.25, tokio, 原生 HTML/CSS/JS.

## Global Constraints

- 仅支持 `redis://` 明文，不支持 `rediss://`。
- 集群模式仅探测连通性，不实现完整集群路由。
- 所有网络 IO 必须加超时。
- 禁止 `unwrap()` / `panic!`，错误通过 `AppError` 透传。
- 提交前运行 `cargo clippy -- -D warnings` 和 `cargo fmt --check`。

---

### Task 1: 后端连接池增加 `Pool::test` 方法

**Files:**
- Modify: `src-tauri/src/connection_pool.rs`
- Create: `src-tauri/tests/test_connection.rs`

**Interfaces:**
- Consumes: `Connection::to_connection_url()`, `Connection.conn_type`, `crate::config::ConnectionTimeout`
- Produces: `pub async fn Pool::test(conn: &Connection) -> Result<String, AppError>`

- [ ] **Step 1: Write the failing test**

  创建 `src-tauri/tests/test_connection.rs`：
  ```rust
  //! Redis 测试连接集成测试。
  //!
  //! 需先启动 Redis（默认连接 127.0.0.1:6379）。
  //! 由于涉及外部服务，默认通过 `#[ignore]` 忽略，需显式运行：
  //!
  //! ```bash
  //! cargo test --test test_connection -- --ignored --nocapture
  //! ```

  use maidi_cache_lib::connection_pool::Pool;
  use maidi_cache_lib::models::{ConnType, Connection};

  #[tokio::test]
  #[ignore]
  async fn test_connection_returns_pong() {
      let conn = Connection {
          id: "test_conn_1".into(),
          name: "integration".into(),
          host: "127.0.0.1".into(),
          port: 6379,
          conn_type: ConnType::Single,
          readonly: false,
          separator: ":".into(),
          db: 0,
          username: None,
          password: None,
      };
      let pong = Pool::test(&conn).await.expect("测试连接失败，请确认 Redis 已启动");
      assert_eq!(pong, "PONG");
      println!("测试连接返回: {}", pong);
  }

  #[tokio::test]
  #[ignore]
  async fn test_connection_fails_on_wrong_port() {
      let conn = Connection {
          id: "test_conn_2".into(),
          name: "integration".into(),
          host: "127.0.0.1".into(),
          port: 1,
          conn_type: ConnType::Single,
          readonly: false,
          separator: ":".into(),
          db: 0,
          username: None,
          password: None,
      };
      let result = Pool::test(&conn).await;
      assert!(result.is_err(), "错误端口应当返回连接失败");
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  ```bash
  cd src-tauri
  cargo test --test test_connection -- --ignored --nocapture
  ```
  Expected: 编译错误 `no function or associated item named test found for struct Pool`

- [ ] **Step 3: Write minimal implementation**

  在 `src-tauri/src/connection_pool.rs` 的 `impl Pool` 块中新增方法：
  ```rust
  /// 临时测试连接参数是否可用。
  ///
  /// 直接按 `conn` 建立 Redis 连接，执行一次 `PING` 后立即释放，
  /// 不写入连接池内部 map。
  pub async fn test(conn: &Connection) -> Result<String, AppError> {
      let timeout = crate::config::ConnectionTimeout::default().connect;
      match conn.conn_type {
          ConnType::Single => {
              let url = conn.to_connection_url();
              let client = redis::Client::open(url).map_err(AppError::from)?;
              let future = client.get_multiplexed_async_connection();
              let mut con = tokio::time::timeout(timeout, future)
                  .await
                  .map_err(|_| AppError::Timeout("连接超时".into()))?
                  .map_err(AppError::from)?;
              let pong: String = redis::cmd("PING")
                  .query_async(&mut con)
                  .await
                  .map_err(AppError::from)?;
              Ok(pong)
          }
          ConnType::Cluster => {
              let url = conn.to_connection_url();
              let client =
                  redis::cluster::ClusterClient::new(vec![url]).map_err(AppError::from)?;
              let future = client.get_async_connection();
              let mut con = tokio::time::timeout(timeout, future)
                  .await
                  .map_err(|_| AppError::Timeout("连接超时".into()))?
                  .map_err(AppError::from)?;
              let pong: String = redis::cmd("PING")
                  .query_async(&mut con)
                  .await
                  .map_err(AppError::from)?;
              Ok(pong)
          }
      }
  }
  ```

- [ ] **Step 4: Run test to verify it passes**

  先确保本地 Redis 已启动：
  ```bash
  docker run -d -p 6379:6379 redis
  ```
  然后运行：
  ```bash
  cd src-tauri
  cargo test --test test_connection -- --ignored --nocapture
  ```
  Expected: 两个测试均通过。

- [ ] **Step 5: Commit**

  ```bash
  git add src-tauri/src/connection_pool.rs src-tauri/tests/test_connection.rs
  git commit -m "feat: 添加 Pool::test 方法支持不缓存地测试 Redis 连接"
  ```

---

### Task 2: 暴露 `test_connection` Tauri Command

**Files:**
- Modify: `src-tauri/src/commands/connection.rs`
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: `Pool::test(&Connection) -> Result<String, AppError>`
- Produces: `#[tauri::command] pub async fn test_connection(conn: Connection) -> Result<String, String>`

- [ ] **Step 1: Add command function**

  在 `src-tauri/src/commands/connection.rs` 末尾追加：
  ```rust
  /// 测试连接参数是否可用（不保存、不缓存连接）。
  #[tauri::command]
  pub async fn test_connection(conn: Connection) -> Result<String, String> {
      Pool::test(&conn).await.map_err(|e| e.to_string())
  }
  ```

- [ ] **Step 2: Register the command**

  在 `src-tauri/src/lib.rs` 的 `invoke_handler` 中增加：
  ```rust
  .invoke_handler(tauri::generate_handler![
      commands::connection::connect,
      commands::connection::disconnect,
      commands::connection::list_connections,
      commands::connection::save_connection,
      commands::connection::delete_connection,
      commands::connection::test_connection,
      commands::server::ping,
      commands::server::get_server_info,
      commands::server::select_db,
      commands::key::list_keys,
      commands::key::set_key,
      commands::key::del_key,
      commands::key::get_string,
  ])
  ```

- [ ] **Step 3: Verify compilation**

  ```bash
  cd src-tauri
  cargo check
  ```
  Expected: 无编译错误。

- [ ] **Step 4: Commit**

  ```bash
  git add src-tauri/src/commands/connection.rs src-tauri/src/lib.rs
  git commit -m "feat: 注册 test_connection Tauri Command"
  ```

---

### Task 3: 前端连接对话框增加测试连接按钮与状态显示

**Files:**
- Modify: `frontend/index.html`

**Interfaces:**
- Consumes: Tauri `invoke('test_connection', { conn })`
- Produces: `#modalTest` 按钮点击事件、`#testStatus` 状态文字渲染

- [ ] **Step 1: Add button and status elements**

  在 `frontend/index.html` 中定位到连接对话框的 `modal-actions` 区域（约第 1605-1608 行），替换为：
  ```html
  <div class="modal-actions">
      <button class="secondary" id="modalTest">测试连接</button>
      <span id="testStatus"></span>
      <div class="spacer"></div>
      <button class="secondary" id="modalCancel">取消</button>
      <button class="primary" id="modalSave">保存连接</button>
  </div>
  ```

- [ ] **Step 2: Add status styles**

  在 `<style>` 中 `.modal .modal-actions` 相关规则附近追加：
  ```css
  #testStatus {
      font-size: 13px;
      margin-left: 8px;
      align-self: center;
      min-height: 20px;
  }
  #testStatus.success {
      color: var(--green);
  }
  #testStatus.error {
      color: #e06c75;
  }
  ```

- [ ] **Step 3: Implement testConnection and showStatus helpers**

  在 `frontend/index.html` 的 `<script>` 中，`saveConnection()` 函数之后添加：
  ```javascript
  function showTestStatus(msg, type) {
      const el = document.getElementById('testStatus');
      el.textContent = msg;
      el.className = type === 'success' ? 'success' : type === 'error' ? 'error' : '';
  }

  function testConnection() {
      const host = document.getElementById('connHost').value.trim();
      const port = parseInt(document.getElementById('connPort').value) || 0;
      if (!host || !port) {
          showTestStatus('请填写主机和端口', 'error');
          return;
      }

      const btn = document.getElementById('modalTest');
      const originalText = btn.textContent;
      btn.disabled = true;
      btn.textContent = '测试中…';
      showTestStatus('正在测试连接…', 'info');

      const conn = {
          id: editingConnId || ('test_' + Date.now()),
          name: document.getElementById('connName').value.trim() || '测试连接',
          host: host,
          port: port,
          type: document.getElementById('connCluster').checked ? 'cluster' : 'single',
          readonly: document.getElementById('connReadonly').checked,
          separator: document.getElementById('connSeparator').value || ':',
          username: document.getElementById('connUser').value || null,
          password: document.getElementById('connPass').value || null,
      };

      invoke('test_connection', { conn })
          .then(pong => {
              showTestStatus('✓ 连接成功（' + pong + '）', 'success');
          })
          .catch(err => {
              showTestStatus('✗ ' + err, 'error');
          })
          .finally(() => {
              btn.disabled = false;
              btn.textContent = originalText;
          });
  }
  ```

- [ ] **Step 4: Wire up events and clear status**

  在 `openModal()` 函数末尾、调用 `modal.classList.add('open')` 之前添加：
  ```javascript
  showTestStatus('', '');
  ```

  在 `closeModal()` 函数中添加：
  ```javascript
  function closeModal() {
      modal.classList.remove('open');
      showTestStatus('', '');
  }
  ```

  在事件绑定区域，为测试按钮添加监听：
  ```javascript
  document.getElementById('modalTest').addEventListener('click', testConnection);
  ```

  为所有连接表单输入框添加输入监听，避免旧状态残留：
  ```javascript
  ['connName', 'connHost', 'connPort', 'connUser', 'connPass', 'connSeparator'].forEach(id => {
      const el = document.getElementById(id);
      if (el) el.addEventListener('input', () => showTestStatus('', ''));
  });
  document.getElementById('connReadonly').addEventListener('change', () => showTestStatus('', ''));
  document.getElementById('connCluster').addEventListener('change', () => showTestStatus('', ''));
  ```

- [ ] **Step 5: Manual verification**

  ```bash
  cd src-tauri
  cargo tauri dev
  ```
  Expected:
  - 打开「新建连接」对话框，能看到「测试连接」按钮。
  - 填写正确 host/port，点击按钮，片刻后按钮左侧显示 `✓ 连接成功（PONG）`。
  - 填写错误端口，点击按钮，显示 `✗ Redis 错误: ...` 或超时信息。
  - 点击「保存连接」后对话框关闭，状态文字清空。

- [ ] **Step 6: Commit**

  ```bash
  git add frontend/index.html
  git commit -m "feat: 连接对话框增加测试连接按钮与状态显示"
  ```

---

### Task 4: 最终验证与代码清理

**Files:**
- All modified files above

- [ ] **Step 1: Run backend checks**

  ```bash
  cd src-tauri
  cargo fmt --check
  cargo clippy -- -D warnings
  cargo test
  ```
  Expected: `cargo fmt --check` 无差异，`cargo clippy` 无警告，单元测试全部通过。

- [ ] **Step 2: Run integration tests (requires Redis)**

  ```bash
  cd src-tauri
  cargo test --test test_connection -- --ignored --nocapture
  ```
  Expected: 两个测试均通过。

- [ ] **Step 3: Verify frontend behavior**

  重新启动应用：
  ```bash
  cd src-tauri
  cargo tauri dev
  ```
  确认：
  - 新建连接与编辑连接时都有「测试连接」按钮。
  - 成功/失败状态文字正确显示。
  - 无 JavaScript 控制台报错。

- [ ] **Step 4: Final commit (if any fixes were needed)**

  若步骤 1-3 有修改，提交：
  ```bash
  git add -A
  git commit -m "chore: 代码格式化与 clippy 修复"
  ```
  若无需修改，直接结束。

---

## Self-Review

**Spec coverage:**
- 新建/编辑连接对话框增加测试按钮 → Task 3
- 临时连接、PING、立即断开 → Task 1 (`Pool::test` 不写入连接池)
- 按钮旁显示成功/失败状态文字 → Task 3
- 后端新增独立 `test_connection` 命令 → Task 2
- 集成测试覆盖成功与失败场景 → Task 1

**Placeholder scan:**
- 无 TBD/TODO/"实现 later"/"适当处理" 等模糊描述。
- 每个步骤包含完整代码或命令。

**Type consistency:**
- `Pool::test` 返回 `Result<String, AppError>`，与 `test_connection` 命令中 `.map_err(|e| e.to_string())` 一致。
- 前端 `conn` 对象字段与 `saveConnection()` 一致，`db` 由 serde default 填充为 0。
