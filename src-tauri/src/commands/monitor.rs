//! # 实时命令监控（`MONITOR`）
//!
//! `MONITOR` 的输出是一条**持续流**：连接发出命令后即进入监控模式，服务器此后不断推送
//! 每一条被执行命令的文本，直到连接关闭。这与命令层「一次命令一次响应」的
//! [`crate::connection_pool::PooledConn::query`] 模型不兼容 —— 后者有命令超时、而且池里的
//! 句柄是**共享复用**的，被 MONITOR 占住就再也回不来了。因此这里单独开一条**专用连接**，
//! 由后台任务把收到的行**批量**推给前端：
//!
//! - 批量推送（攒够 [`FLUSH_MAX_LINES`] 行或距上次推送超过 [`FLUSH_INTERVAL`]，先到先推），
//!   避免忙碌实例上「一行一次 IPC」把通道打满；
//! - 缓冲超过 [`MAX_PENDING`] 行时丢弃最旧的行并如实上报条数（`dropped`），保证内存有界；
//! - 停止走 [`tokio::sync::Notify`]，任务自己收尾：冲刷剩余缓冲 + 推 `monitor:end` 事件；
//! - 连接断开 / 服务器报错时同样推 `monitor:end`，前端据此把按钮状态改回「开始监控」。
//!
//! 集群模式不支持：`MONITOR` 是**节点级**命令，集群里每个节点各有一条独立的流，
//! 「监控整个集群」需要同时开 N 条连接（与「集群下 SELECT 不支持」同类的边界）。

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Notify;

use crate::config::ConnectionTimeout;
use crate::connection_pool::Pool;
use crate::error::{command_error_text, AppError};
use crate::models::{ConnType, Connection};

/// 一批监控行推给前端的事件名。
pub const EVENT_LINES: &str = "monitor:lines";
/// 监控结束（用户停止 / 连接断开 / 出错）的事件名。
pub const EVENT_END: &str = "monitor:end";

/// 攒够这么多行就立刻推送，不等下一次心跳。
const FLUSH_MAX_LINES: usize = 500;
/// 心跳间隔：行数少时最多攒这么久再推，避免「一行一次 IPC」。
const FLUSH_INTERVAL: Duration = Duration::from_millis(120);
/// 未推送缓冲的上限：超过就丢最旧的行并计数（前端卡住 / 服务器洪水式刷命令时的兜底）。
const MAX_PENDING: usize = 2000;

/// 一行 `MONITOR` 输出，已按服务器格式拆成字段（前端按字段渲染，`raw` 供过滤与复制）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorLine {
    /// 原始行（去掉行尾空白）
    pub raw: String,
    /// 服务器时间戳（`秒.微秒`）
    pub time: String,
    /// 逻辑库编号
    pub db: i64,
    /// 发起命令的客户端：通常是 `host:port`；Lua 脚本是 `lua`，主从复制是 `repl`
    pub client: String,
    /// 命令名（服务器收到的原始大小写，通常是用户输入的那个）
    pub command: String,
    /// 命令参数（已还原 `sdscatrepr` 的转义）
    pub args: Vec<String>,
}

impl MonitorLine {
    /// 解析一行 `MONITOR` 输出（**尽力而为**：格式对不上时只保留 `raw`）。
    ///
    /// 服务器侧格式（`replicationFeedMonitors`）是：
    /// `<时间戳> [<db> <客户端>] "<命令>" "<参数>" …`，其中每个参数由 `sdscatrepr` 转义：
    /// `\n` / `\r` / `\t` / `\a` / `\b` / `\"` / `\\`，**非可打印字节**（含 UTF-8 的多字节）
    /// 逐个写成 `\xHH`。因此参数按**字节**还原后再做一次 UTF-8 解码，中文才不会变成乱码。
    pub fn parse(raw: &str) -> MonitorLine {
        let raw = raw.trim_end();
        let mut line = MonitorLine {
            raw: raw.to_string(),
            time: String::new(),
            db: 0,
            client: String::new(),
            command: String::new(),
            args: Vec::new(),
        };

        // 前缀 = 时间戳 + `[db 客户端]`，命令行从第一个引号开始
        let Some(quote_at) = raw.find('"') else {
            return line;
        };
        let (prefix, commands) = raw.split_at(quote_at);
        if let Some((time, bracket)) = prefix.split_once('[') {
            line.time = time.trim().to_string();
            // 前缀形如 `[0 127.0.0.1:60498] `：先去掉两侧空白再摘右括号
            let inner = bracket.trim().trim_end_matches(']');
            let mut parts = inner.splitn(2, ' ');
            line.db = parts
                .next()
                .and_then(|t| t.trim().parse().ok())
                .unwrap_or(0);
            line.client = parts.next().unwrap_or("").trim().to_string();
        }

        let mut args = parse_quoted_args(commands);
        if !args.is_empty() {
            line.command = args.remove(0);
            line.args = args;
        }
        line
    }
}

/// 解析 `"a" "b c" "d\"e"` 形式的参数序列（`sdscatrepr` 的转义规则，见 [`MonitorLine::parse`]）。
///
/// 引号外的空白与游离字符一律跳过：格式异常时宁可少解析几个参数，也不猜。
fn parse_quoted_args(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut bytes: Vec<u8> = Vec::new();
        let mut buf = [0u8; 4];
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' => match chars.next() {
                    Some('n') => bytes.push(b'\n'),
                    Some('r') => bytes.push(b'\r'),
                    Some('t') => bytes.push(b'\t'),
                    Some('a') => bytes.push(0x07),
                    Some('b') => bytes.push(0x08),
                    Some('x') => {
                        // \xHH：凑不齐两个十六进制字符就按字面量收下（不猜）
                        let hex: String = chars.by_ref().take(2).collect();
                        match u8::from_str_radix(&hex, 16) {
                            Ok(byte) => bytes.push(byte),
                            Err(_) => {
                                bytes.extend_from_slice(b"\\x");
                                bytes.extend_from_slice(hex.as_bytes());
                            }
                        }
                    }
                    Some(other) => bytes.extend_from_slice(other.encode_utf8(&mut buf).as_bytes()),
                    // 行尾的反斜杠：内容到此为止
                    None => break,
                },
                other => bytes.extend_from_slice(other.encode_utf8(&mut buf).as_bytes()),
            }
        }
        // 未闭合的引号也照收：至少把已经解析出的内容展示出来
        args.push(String::from_utf8_lossy(&bytes).into_owned());
    }
    args
}

/// `monitor:lines` 事件的载荷。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LinesPayload {
    conn_id: String,
    lines: Vec<MonitorLine>,
    /// 本次推送前因缓冲上限被丢弃的行数（0 表示没丢）
    dropped: usize,
}

/// `monitor:end` 事件的载荷。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EndPayload {
    conn_id: String,
    /// 面向用户的结束原因，直接展示
    reason: String,
    /// 是否由用户主动停止（主动停止不算错误，前端语气不同）
    stopped_by_user: bool,
}

/// 一次监控会话——后台任务在结束时把自己从表里摘掉，因此这里同时存会话序号。
struct Session {
    /// 会话序号（[`MonitorState::next_id`] 自增分配）
    id: u64,
    /// 停止信号：置位后任务冲刷缓冲并退出
    stop: Arc<Notify>,
    /// 本次停止是否由用户发起（决定结束文案是「已停止监控」还是「连接已断开」）
    user_stop: Arc<AtomicBool>,
}

/// 进程内的监控会话表（Tauri 状态）。
///
/// 同一连接同时只允许一个会话：重复点「开始监控」会拿到明确错误，而不是留下两条流。
/// 连接断开 / 删除连接时由命令层调用 [`MonitorState::stop_on_disconnect`] 收掉。
#[derive(Default)]
pub struct MonitorState {
    sessions: Mutex<HashMap<String, Session>>,
    /// 会话序号计数器：任务收尾时用「序号是否仍是自己的」判断该不该摘条目，
    /// 避免「停止后立刻重启」时旧任务把新会话摘掉
    next_id: Mutex<u64>,
}

impl MonitorState {
    /// 该连接当前是否正在监控。
    pub fn is_running(&self, conn_id: &str) -> bool {
        self.sessions.lock().unwrap().contains_key(conn_id)
    }

    /// 登记一个监控会话，返回（会话序号, 「是否用户停止」标志）。
    fn register(&self, conn_id: &str, stop: Arc<Notify>) -> (u64, Arc<AtomicBool>) {
        let mut next = self.next_id.lock().unwrap();
        *next += 1;
        let id = *next;
        let user_stop = Arc::new(AtomicBool::new(true));
        self.sessions.lock().unwrap().insert(
            conn_id.to_string(),
            Session {
                id,
                stop,
                user_stop: Arc::clone(&user_stop),
            },
        );
        (id, user_stop)
    }

    /// 任务收尾：仅当表里还是**自己这个序号**时摘掉条目。
    fn finish(&self, conn_id: &str, id: u64) {
        let mut sessions = self.sessions.lock().unwrap();
        if sessions.get(conn_id).map(|s| s.id) == Some(id) {
            sessions.remove(conn_id);
        }
    }

    /// 用户点「停止监控」：停掉该连接的监控，返回是否确有会话被停掉。
    pub fn stop(&self, conn_id: &str) -> bool {
        self.stop_with(conn_id, true)
    }

    /// 连接断开 / 删除连接：停掉该连接的监控（结束文案按「连接已断开」走）。
    pub fn stop_on_disconnect(&self, conn_id: &str) -> bool {
        self.stop_with(conn_id, false)
    }

    /// 只置停止信号、不等任务结束：任务会自己冲刷缓冲并推 `monitor:end`。
    /// 条目立即摘除，因此随后的「开始监控」不会被上一个正在收尾的任务挡住。
    fn stop_with(&self, conn_id: &str, user_stop: bool) -> bool {
        match self.sessions.lock().unwrap().remove(conn_id) {
            Some(session) => {
                session.user_stop.store(user_stop, Ordering::SeqCst);
                session.stop.notify_one();
                true
            }
            None => false,
        }
    }
}

/// 打开专用连接执行 `MONITOR`，持续读取并把每一批行交给 `on_batch`。
///
/// 返回 `Ok(())` 表示收到停止信号正常退出（`on_batch` 已拿到最后一批）；`Err` 表示连接
/// 建立失败或流中断（错误文案面向用户，见 [`crate::error::command_error_text`]）。
///
/// 把「取行」与「交付」分开（`on_batch` 回调）是为了让集成测试能直接跑这条真实链路，
/// 不必构造 Tauri 的 `AppHandle`。
pub async fn run_monitor<F>(
    cfg: &Connection,
    timeout: ConnectionTimeout,
    stop: Arc<Notify>,
    mut on_batch: F,
) -> Result<(), AppError>
where
    F: FnMut(Vec<MonitorLine>, usize),
{
    let client = redis::Client::open(cfg.to_connection_url())?;
    let mut monitor = tokio::time::timeout(timeout.connect, client.get_async_monitor())
        .await
        .map_err(|_| timeout.connect_timeout_error())??;
    // MONITOR 命令本身是有响应的（+OK），按命令超时等待；进入流模式后不再设超时 ——
    // 长时间没有命令是空闲实例的正常状态，不是「卡住」
    tokio::time::timeout(timeout.command, monitor.monitor())
        .await
        .map_err(|_| timeout.command_timeout_error())??;

    let mut stream = monitor.into_on_message::<String>();
    // 停止信号只等一次：在循环外创建并 pin 住，避免每轮 select 重新注册
    let stopped = stop.notified();
    tokio::pin!(stopped);
    let mut ticker = tokio::time::interval(FLUSH_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut pending: VecDeque<MonitorLine> = VecDeque::new();
    let mut dropped = 0usize;

    loop {
        tokio::select! {
            _ = &mut stopped => {
                flush(&mut pending, &mut dropped, &mut on_batch);
                return Ok(());
            }
            _ = ticker.tick() => {
                flush(&mut pending, &mut dropped, &mut on_batch);
            }
            line = stream.next() => {
                match line {
                    Some(text) => pending.push_back(MonitorLine::parse(&text)),
                    // 流结束：服务器关了连接（被 KILL、实例重启、网络中断）
                    None => {
                        flush(&mut pending, &mut dropped, &mut on_batch);
                        return Err(AppError::msg(
                            "监控连接已断开（服务器关闭了连接或网络中断），请重新开始监控",
                        ));
                    }
                }
                if pending.len() >= MAX_PENDING {
                    // 消费不过来：丢掉最旧的一批（用 VecDeque 让丢弃是 O(1)），
                    // 丢了多少如实上报，前端会提示「已省略 N 行」
                    let excess = pending.len() - MAX_PENDING / 2;
                    pending.drain(..excess);
                    dropped += excess;
                }
                if pending.len() >= FLUSH_MAX_LINES || dropped > 0 {
                    flush(&mut pending, &mut dropped, &mut on_batch);
                }
            }
        }
    }
}

/// 把缓冲里的行交给回调（空缓冲且没有丢弃计数时什么也不做）。
fn flush<F>(pending: &mut VecDeque<MonitorLine>, dropped: &mut usize, on_batch: &mut F)
where
    F: FnMut(Vec<MonitorLine>, usize),
{
    if pending.is_empty() && *dropped == 0 {
        return;
    }
    let lines: Vec<MonitorLine> = pending.drain(..).collect();
    let dropped_now = std::mem::take(dropped);
    on_batch(lines, dropped_now);
}

/// 开始监控某连接上执行的命令。
///
/// 用**专用连接**（不是池里的共享句柄）：MONITOR 会把连接切进监控模式，
/// 共享句柄一旦被占住，同一连接上的 Key 列表 / 详情 / 终端就全部不可用了。
///
/// 只读连接也可以监控：MONITOR 不修改数据，观察线上实例正是它的主要用途
/// （终端里的 MONITOR 被拦是因为它会把那条共享连接占死，和这里的实现不是一回事）。
#[tauri::command]
pub async fn start_monitor(
    app: AppHandle,
    pool: State<'_, Pool>,
    state: State<'_, MonitorState>,
    conn_id: String,
) -> Result<(), String> {
    let cfg = pool.get(&conn_id).map_err(|e| e.to_string())?;
    if cfg.conn_type != ConnType::Single {
        return Err(
            "集群模式不支持实时监控：MONITOR 是节点级命令，集群里每个节点各有一条独立的流。\
             请用「单机」模式直连要观察的节点"
                .into(),
        );
    }
    if state.is_running(&conn_id) {
        return Err("该连接已在监控中".into());
    }
    // 生效超时跟连接配置走（`Pool::conn` 已把连接级覆盖算好）
    let timeout = pool.conn(&conn_id).map_err(|e| e.to_string())?.timeout();

    let stop = Arc::new(Notify::new());
    let (session_id, user_stop) = state.register(&conn_id, stop.clone());

    let app_for_lines = app.clone();
    let app_for_end = app.clone();
    let conn_for_task = cfg.clone();
    let conn_id_for_task = conn_id.clone();
    tauri::async_runtime::spawn(async move {
        let result = run_monitor(&conn_for_task, timeout, stop, move |lines, dropped| {
            let payload = LinesPayload {
                conn_id: conn_id_for_task.clone(),
                lines,
                dropped,
            };
            let _ = app_for_lines.emit(EVENT_LINES, payload);
        })
        .await;

        // 先摘条目再推结束事件：前端收到 end 后立刻「开始监控」也不会被旧条目挡住
        app_for_end
            .state::<MonitorState>()
            .finish(&conn_id, session_id);
        let payload = match result {
            Ok(()) => {
                let by_user = user_stop.load(Ordering::SeqCst);
                EndPayload {
                    conn_id: conn_id.clone(),
                    reason: if by_user {
                        "已停止监控".into()
                    } else {
                        "连接已断开，监控随之停止".into()
                    },
                    stopped_by_user: by_user,
                }
            }
            Err(err) => EndPayload {
                conn_id: conn_id.clone(),
                reason: command_error_text(&err, Some(&cfg)),
                stopped_by_user: false,
            },
        };
        let _ = app_for_end.emit(EVENT_END, payload);
    });

    Ok(())
}

/// 停止监控（用户点「停止」）。
///
/// 返回是否确有会话被停掉；被停掉的那个会话随后会推一条 `monitor:end`。
#[tauri::command]
pub fn stop_monitor(state: State<'_, MonitorState>, conn_id: String) -> bool {
    state.stop(&conn_id)
}

/// 查询某连接是否正在监控（切换连接后前端据此恢复按钮状态）。
#[tauri::command]
pub fn monitor_status(state: State<'_, MonitorState>, conn_id: String) -> bool {
    state.is_running(&conn_id)
}

#[cfg(test)]
mod tests {
    use super::{parse_quoted_args, MonitorLine, MonitorState};
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Notify;

    #[test]
    fn parses_a_real_monitor_line() {
        // 实测（Redis 8.x）：`redis-cli set 中文键:1 'a"b\c<换行>d'` 的输出
        let raw = "1790046834.904313 [0 127.0.0.1:60498] \"set\" \"\\xe4\\xb8\\xad\\xe6\\x96\\x87\\xe9\\x94\\xae:1\" \"a\\\"b\\\\c\\nd\" ";
        let line = MonitorLine::parse(raw);

        assert_eq!(line.time, "1790046834.904313");
        assert_eq!(line.db, 0);
        assert_eq!(line.client, "127.0.0.1:60498");
        assert_eq!(line.command, "set");
        // \xHH 是**字节**：三个字节一组还原成 UTF-8 的「中」
        assert_eq!(
            line.args,
            vec!["中文键:1".to_string(), "a\"b\\c\nd".to_string()]
        );
        assert_eq!(line.raw, raw.trim_end(), "raw 去掉行尾空白后原样保留");
    }

    #[test]
    fn parses_line_without_args() {
        let line = MonitorLine::parse("1790000000.000001 [3 10.0.0.9:52000] \"ping\" ");
        assert_eq!(line.db, 3);
        assert_eq!(line.client, "10.0.0.9:52000");
        assert_eq!(line.command, "ping");
        assert!(line.args.is_empty());
    }

    #[test]
    fn parses_client_identifiers_other_than_host_port() {
        // Lua 脚本与主从复制发出的命令：客户端标识不是 host:port
        let lua = MonitorLine::parse("1790000000.0 [0 lua] \"set\" \"k\" \"v\" ");
        assert_eq!(lua.client, "lua");
        assert_eq!(lua.args, vec!["k".to_string(), "v".to_string()]);

        let repl = MonitorLine::parse("1790000000.0 [0 repl] \"ping\" ");
        assert_eq!(repl.client, "repl");
        assert_eq!(repl.command, "ping");
    }

    #[test]
    fn parses_control_character_escapes() {
        let line = MonitorLine::parse("1.0 [0 lua] \"set\" \"a\\tb\\rc\\x07d\\x08e\" ");
        assert_eq!(line.args, vec!["a\tb\rc\u{7}d\u{8}e".to_string()]);
    }

    #[test]
    fn malformed_lines_keep_the_raw_text() {
        // 没有引号（格式意外变化）：至少 raw 还在，前端仍能展示
        let line = MonitorLine::parse("garbage without quotes");
        assert_eq!(line.raw, "garbage without quotes");
        assert!(line.command.is_empty());
        assert!(line.args.is_empty());
        assert_eq!(line.db, 0);

        // 引号未闭合：已解析的部分照收
        let line = MonitorLine::parse("1.0 [0 lua] \"get\" \"unclosed ");
        assert_eq!(line.command, "get");
        assert_eq!(line.args, vec!["unclosed".to_string()]);

        // \x 后面不是十六进制：按字面量收下，不猜
        assert_eq!(
            parse_quoted_args("\"a\\xZZb\""),
            vec!["a\\xZZb".to_string()]
        );
    }

    #[test]
    fn skips_text_outside_quotes() {
        assert_eq!(
            parse_quoted_args("  \"a\"   junk  \"b\"  "),
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(parse_quoted_args("").is_empty());
        assert!(parse_quoted_args("no quotes here").is_empty());
    }

    /// 会话表：同一连接同时只有一个会话，停止只作用于该连接。
    #[test]
    fn stop_only_affects_the_target_connection() {
        let state = MonitorState::default();
        assert!(!state.is_running("c1"));

        let (id, _) = state.register("c1", Arc::new(Notify::new()));
        assert!(state.is_running("c1"));
        assert!(!state.stop("c2"), "其它连接没有会话，停止应当返回 false");
        assert!(state.is_running("c1"), "停别的连接不该影响 c1");

        assert!(state.stop("c1"));
        assert!(!state.is_running("c1"), "停止后条目立即摘除");

        // 任务收尾时条目已经不在了：finish 是幂等的
        state.finish("c1", id);
        assert!(!state.is_running("c1"));
    }

    /// 「停止后立刻重新开始」时，旧任务的收尾不能把新会话摘掉。
    #[test]
    fn finishing_a_stale_session_keeps_the_new_one() {
        let state = MonitorState::default();
        let (old_id, _) = state.register("c1", Arc::new(Notify::new()));
        state.stop("c1");
        let (new_id, _) = state.register("c1", Arc::new(Notify::new()));
        assert_ne!(old_id, new_id, "会话序号必须严格递增");

        state.finish("c1", old_id);
        assert!(state.is_running("c1"), "旧任务收尾不该摘掉新会话");

        state.finish("c1", new_id);
        assert!(!state.is_running("c1"));
    }

    /// 停止信号必须立刻唤醒读取任务（否则停止按钮要等到下一条命令才生效）。
    #[tokio::test]
    async fn stop_wakes_the_runner_immediately() {
        let state = MonitorState::default();
        let stop = Arc::new(Notify::new());
        let (_id, user_stop) = state.register("c1", Arc::clone(&stop));
        assert!(
            user_stop.load(Ordering::SeqCst),
            "默认按「用户停止」处理，连接断开时由 stop_on_disconnect 改掉"
        );

        let waiter = stop.notified();
        tokio::pin!(waiter);
        assert!(state.stop("c1"));
        assert!(user_stop.load(Ordering::SeqCst));

        tokio::time::timeout(Duration::from_secs(1), &mut waiter)
            .await
            .expect("停止信号应当立即唤醒等待中的任务");
    }
}
