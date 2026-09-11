# TTL 输入框设计文档

> 日期：2026-09-03  
> 状态：已评审  
> 范围：麦地缓存（MyRedis）创建/编辑 Key 时支持 TTL 设置

---

## 1. 背景与目标

当前前端「新增 Key」对话框仅包含 **Key 名称** 和 **值** 两个字段，后端 `set_key` 命令也仅接收 `conn_id`/`key`/`value`。用户在创建 Key 时无法设置过期时间，编辑 Key 时详情页也只展示 TTL 而不能修改。

本设计目标：
- 在「新增 Key」对话框增加 **TTL（秒）** 输入框，默认值为 `-1`（永不过期）。
- 在 Key 详情页把 TTL 展示改为可编辑输入框，保存时一并更新。
- 后端 `set_key` 原子化支持 TTL：`ttl > 0` 时使用 `SET key value EX ttl`。

---

## 2. 方案决策

### 2.1 方案 A（采用）：扩展 `set_key` 命令

- 后端 `set_key` 增加 `ttl: i64` 参数。
- `ttl > 0`：发送 `SET key value EX ttl`。
- `ttl == -1`：发送普通 `SET key value`。
- 校验：`ttl` 必须为 `-1` 或正整数，否则返回错误。

**优点：**
- 一次网络往返，原子操作。
- 语义与 Redis `SET ... EX` 一致。
- 对创建和编辑 Key 都适用。

**缺点：**
- 改动现有 `set_key` 命令签名；但前端是唯一的调用方，影响可控。

### 2.2 方案 B（未采用）：创建后单独调用 `EXPIRE`

- 保持 `set_key` 不变。
- 新增或复用 `expire_key` 命令，在 `set_key` 成功后根据条件调用。

**未采用原因：**
- 两次网络往返。
- 非原子：若 `SET` 成功但 `EXPIRE` 失败，Key 将永不过期，与预期不符。

---

## 3. 详细设计

### 3.1 后端改动

**文件：** `src-tauri/src/commands/key.rs`

- 修改 `set_key` 函数签名：
  ```rust
  pub async fn set_key(
      pool: tauri::State<'_, Pool>,
      conn_id: String,
      key: String,
      value: String,
      ttl: i64,
  ) -> Result<String, String>
  ```
- 校验逻辑：
  - `ttl == -1`：允许，表示不设置过期时间。
  - `ttl > 0`：允许，追加 `EX ttl` 参数。
  - `ttl == 0` 或 `ttl < -1`：返回错误 `"TTL 必须为 -1 或正整数"`。
- 命令构造：
  - `ttl > 0`：`redis::cmd("SET").arg(&key).arg(&value).arg("EX").arg(ttl)`。
  - `ttl == -1`：`redis::cmd("SET").arg(&key).arg(&value)`。

**兼容性：**
- `SET ... EX` 自 Redis 2.6.12 起支持，满足项目「兼容 Redis < 6.0」的要求。

### 3.2 前端改动

**文件：** `frontend/index.html`

#### 3.2.1 新增 Key 对话框

- 在「值」文本域下方新增表单组：
  - `<label>TTL（秒）</label>`
  - `<input type="number" id="addKeyTtl" value="-1" />`
  - 辅助说明：`-1 表示永不过期`
- `openAddKeyModal` 中重置 `addKeyTtl.value = '-1'`。
- `submitAddKey` 中：
  - 读取并解析 `addKeyTtl` 为整数。
  - 校验：仅允许 `-1` 或正整数；非法时提示并聚焦。
  - 调用 `set_key` 时新增 `ttl` 参数。
  - 终端日志根据 TTL 显示 `SET key value EX ttl` 或 `SET key value`。

#### 3.2.2 Key 详情页

- 将当前仅显示的 TTL 文本 `<span class="ttl">...</span>` 改为可编辑的 number 输入框：
  - 初始值为当前 Key 的 `item.ttl`。
  - 禁用样式与只读连接状态联动。
- 「保存」按钮事件：
  - 读取 `valueEditor.value` 和 TTL 输入框值。
  - 校验 TTL。
  - 调用 `set_key` 时传入 `ttl`。
  - 成功后刷新 `allKeys` 并更新本地缓存中的 TTL。

#### 3.2.3 输入校验规则

| 输入 | 行为 |
|------|------|
| `-1` | 允许，永不过期 |
| 正整数 | 允许，按秒过期 |
| `0` | 拒绝，避免误删除 Key |
| 负数（除 `-1`） | 拒绝 |
| 小数 / 非数字 | 拒绝 |

### 3.3 错误处理

- 前端即时校验：非法输入立即提示，不调用后端。
- 后端兜底校验：非法 TTL 返回中文错误。
- 只读连接：保存按钮与 TTL 输入框禁用，提示「只读连接，禁止写操作」。
- 网络/Redis 错误：沿用现有 `showToast` + `appendTerminal` 统一处理。

---

## 4. 测试计划

### 4.1 后端单元测试

针对 `set_key` 的 TTL 分支：
- `ttl = -1`：构造命令不含 `EX`。
- `ttl = 10`：构造命令包含 `EX 10`。
- `ttl = 0`：返回错误。
- `ttl = -2`：返回错误。

### 4.2 集成测试

- 依赖真实 Redis（标记 `#[ignore]`）。
- 验证：
  - 创建 `ttl = -1` 的 Key，`TTL key` 返回 `-1`。
  - 创建 `ttl = 5` 的 Key，`TTL key` 返回 `5` 或 `4`。
  - 编辑 Key 的 TTL 从 `-1` 改为 `10` 后生效。

### 4.3 前端验证

- 新增 Key 对话框默认 TTL 为 `-1`。
- 输入非法 TTL 时阻止提交并提示。
- 详情页 TTL 编辑后保存，列表与详情同步刷新。

---

## 5. 影响范围

| 文件 | 变更类型 |
|------|----------|
| `src-tauri/src/commands/key.rs` | 修改 `set_key` 签名与实现 |
| `frontend/index.html` | 新增 TTL 输入框、编辑逻辑、校验 |

无新增依赖。
