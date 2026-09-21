//! # 内置终端命令
//!
//! 把前端终端输入的整行命令解析成 (命令, 参数)，直接转发到 Redis 服务器执行，
//! 并把 `redis::Value` 渲染成 redis-cli 风格的可读文本。
//!
//! - 只读连接会按 [`WRITE_COMMANDS`] 名单拦截写命令（与连接池的 `ensure_writable`
//!   策略一致：已知写命令在命令层拦截，其余透传由服务器 ACL 兜底）。
//! - 命令执行走连接池的 [`crate::connection_pool::PooledConn::query`]，
//!   由它统一包 `tokio::time::timeout`：`BLPOP` / `SUBSCRIBE` 这类阻塞命令
//!   不会把 UI 永久卡死（超时返回错误提示）。

use crate::connection_pool::Pool;
use crate::error::{command_error_text, AppError};

/// 已知会修改数据、或会阻塞 / 改变连接状态的命令。
///
/// 只读连接下这些命令直接在前端提示前被拦截；名单外的命令透传，
/// 由 Redis 服务端 ACL 决定是否允许。
const WRITE_COMMANDS: &[&str] = &[
    // string 写入
    "SET",
    "SETNX",
    "SETEX",
    "PSETEX",
    "GETSET",
    "GETDEL",
    "APPEND",
    "SETRANGE",
    "MSET",
    "MSETNX",
    // key 生命周期 / 删除 / 迁移
    "DEL",
    "UNLINK",
    "EXPIRE",
    "PEXPIRE",
    "EXPIREAT",
    "PEXPIREAT",
    "PERSIST",
    "RENAME",
    "RENAMENX",
    "COPY",
    "MOVE",
    "RESTORE",
    "MIGRATE",
    "FLUSHDB",
    "FLUSHALL",
    "SWAPDB",
    // hash
    "HSET",
    "HMSET",
    "HSETNX",
    "HDEL",
    "HINCRBY",
    "HINCRBYFLOAT",
    "HRANDFIELD",
    // list
    "LPUSH",
    "RPUSH",
    "LPUSHX",
    "RPUSHX",
    "LPOP",
    "RPOP",
    "LSET",
    "LREM",
    "LINSERT",
    "LTRIM",
    "RPOPLPUSH",
    "LMOVE",
    "BLMOVE",
    // set
    "SADD",
    "SREM",
    "SPOP",
    "SMOVE",
    "SINTERSTORE",
    "SUNIONSTORE",
    "SDIFFSTORE",
    // zset
    "ZADD",
    "ZREM",
    "ZINCRBY",
    "ZPOPMIN",
    "ZPOPMAX",
    "ZREMRANGEBYRANK",
    "ZREMRANGEBYSCORE",
    "ZREMRANGEBYLEX",
    "ZRANGESTORE",
    "ZUNIONSTORE",
    "ZINTERSTORE",
    // 计数器
    "INCR",
    "DECR",
    "INCRBY",
    "DECRBY",
    "INCRBYFLOAT",
    // 阻塞 / 订阅 / 事务 / 脚本（执行后连接状态改变或长时间阻塞）
    "BLPOP",
    "BRPOP",
    "BLMPOP",
    "SUBSCRIBE",
    "PSUBSCRIBE",
    "SSUBSCRIBE",
    "UNSUBSCRIBE",
    "PUNSUBSCRIBE",
    "MULTI",
    "EXEC",
    "DISCARD",
    "WATCH",
    "EVAL",
    "EVALSHA",
    "SCRIPT",
    "MONITOR",
    // 其它
    "GEOADD",
    "XADD",
    "XDEL",
    "XTRIM",
    "XSETID",
    "XACK",
    "XCLAIM",
    "PFADD",
    "PFMERGE",
    "SHUTDOWN",
];

/// 执行一行 Redis 命令，返回渲染后的文本（多行用 `\n` 连接）。
///
/// 命令解析失败、连接只读拦截、执行错误都返回 `Err(String)`，
/// 由前端以错误样式展示。
#[tauri::command]
pub async fn execute_command(
    pool: tauri::State<'_, Pool>,
    conn_id: String,
    line: String,
) -> Result<String, String> {
    let (cmd_name, args) = parse_command_line(&line).ok_or("命令解析失败：引号未闭合")?;

    let conn_cfg = pool.get(&conn_id).map_err(|e| e.to_string())?;
    let mut con = pool.conn(&conn_id).map_err(|e| e.to_string())?;

    if conn_cfg.is_readonly() && WRITE_COMMANDS.contains(&cmd_name.to_uppercase().as_str()) {
        return Err(format!(
            "只读连接，禁止写操作: {cmd_name}。如需执行写命令，请编辑连接取消「只读」后重新连接"
        ));
    }

    let mut cmd = redis::cmd(&cmd_name);
    for arg in &args {
        cmd.arg(arg);
    }

    // 终端允许任意命令（含用户手输的阻塞命令）；超时由连接池统一兜住（见 `PooledConn::query`），
    // 这里只在超时文案后面补一句阻塞类命令的说明。
    let result: redis::Value = match con.query(&cmd).await {
        Ok(value) => value,
        Err(AppError::Timeout(msg)) => {
            return Err(format!(
                "{msg}。阻塞类命令（BLPOP / SUBSCRIBE 等）不会返回结果，请在其它工具中执行"
            ))
        }
        Err(e) => return Err(command_error_text(&e, Some(&conn_cfg))),
    };

    Ok(value_to_text(&result))
}

/// 把一行终端输入解析为 `(命令名, 参数列表)`。
///
/// 规则与 redis-cli 的简化版一致：
/// - 空白字符分隔参数；
/// - 单引号 / 双引号包裹的片段可包含空格，引号本身被去掉；
/// - `\` 可转义下一个字符（如 `\"`、`\'`、`\\`、`\ `）；
/// - 引号未闭合返回 `None`。
pub fn parse_command_line(input: &str) -> Option<(String, Vec<String>)> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut quote: Option<char> = None;

    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else if c == '\\' {
                    // 引号内只转义引号自身与反斜杠，其余保留原样
                    match chars.next() {
                        Some(next) if next == q || next == '\\' => current.push(next),
                        Some(next) => {
                            current.push('\\');
                            current.push(next);
                        }
                        None => return None,
                    }
                } else {
                    current.push(c);
                }
            }
            None => {
                if c.is_whitespace() {
                    if in_token {
                        tokens.push(std::mem::take(&mut current));
                        in_token = false;
                    }
                } else if c == '\'' || c == '"' {
                    quote = Some(c);
                    in_token = true;
                } else if c == '\\' {
                    match chars.next() {
                        Some(next) => {
                            current.push(next);
                            in_token = true;
                        }
                        None => return None,
                    }
                } else {
                    current.push(c);
                    in_token = true;
                }
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if in_token {
        tokens.push(current);
    }
    if tokens.is_empty() {
        return None;
    }

    let mut iter = tokens.into_iter();
    let cmd_name = iter.next()?;
    Some((cmd_name, iter.collect()))
}

/// 把 `redis::Value` 渲染成 redis-cli 风格文本。
///
/// - `Okay` → `OK`，`Nil` → `(nil)`，整数 → `(integer) N`；
/// - 字符串原样输出（二进制做有损的 UTF-8 转换）；
/// - 数组逐行输出，空数组输出 `(empty array)`。
pub fn value_to_text(value: &redis::Value) -> String {
    let mut lines = Vec::new();
    render_value(value, &mut lines);
    lines.join("\n")
}

fn render_value(value: &redis::Value, lines: &mut Vec<String>) {
    match value {
        redis::Value::Okay => lines.push("OK".to_string()),
        redis::Value::Nil => lines.push("(nil)".to_string()),
        redis::Value::Int(n) => lines.push(format!("(integer) {n}")),
        redis::Value::Status(s) => lines.push(s.clone()),
        redis::Value::Data(bytes) => lines.push(String::from_utf8_lossy(bytes).into_owned()),
        redis::Value::Bulk(items) => {
            if items.is_empty() {
                lines.push("(empty array)".to_string());
            } else {
                for item in items {
                    render_value(item, lines);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_command_line, value_to_text};

    #[test]
    fn parse_simple_command() {
        let (cmd, args) = parse_command_line("GET foo").unwrap();
        assert_eq!(cmd, "GET");
        assert_eq!(args, vec!["foo"]);
    }

    #[test]
    fn parse_extra_whitespace_is_ignored() {
        let (cmd, args) = parse_command_line("  SET   key   value  ").unwrap();
        assert_eq!(cmd, "SET");
        assert_eq!(args, vec!["key", "value"]);
    }

    #[test]
    fn parse_double_quoted_arg_with_spaces() {
        let (_, args) = parse_command_line("SET greeting \"hello world\"").unwrap();
        assert_eq!(args, vec!["greeting", "hello world"]);
    }

    #[test]
    fn parse_single_quoted_arg_preserves_double_quote() {
        let (_, args) = parse_command_line("SET k 'say \"hi\"'").unwrap();
        assert_eq!(args, vec!["k", "say \"hi\""]);
    }

    #[test]
    fn parse_escaped_quote_inside_double_quotes() {
        let (_, args) = parse_command_line("SET k \"a\\\"b\"").unwrap();
        assert_eq!(args, vec!["k", "a\"b"]);
    }

    #[test]
    fn parse_backslash_space_outside_quotes() {
        let (_, args) = parse_command_line("SET key\\ name value").unwrap();
        assert_eq!(args, vec!["key name", "value"]);
    }

    #[test]
    fn parse_empty_input_returns_none() {
        assert!(parse_command_line("").is_none());
        assert!(parse_command_line("   ").is_none());
    }

    #[test]
    fn parse_unclosed_quote_returns_none() {
        assert!(parse_command_line("SET k \"unclosed").is_none());
        assert!(parse_command_line("SET k 'unclosed").is_none());
    }

    #[test]
    fn render_okay() {
        assert_eq!(value_to_text(&redis::Value::Okay), "OK");
    }

    #[test]
    fn render_nil() {
        assert_eq!(value_to_text(&redis::Value::Nil), "(nil)");
    }

    #[test]
    fn render_integer() {
        assert_eq!(value_to_text(&redis::Value::Int(42)), "(integer) 42");
    }

    #[test]
    fn render_binary_data_as_lossy_string() {
        let v = redis::Value::Data(b"hello".to_vec());
        assert_eq!(value_to_text(&v), "hello");
    }

    #[test]
    fn render_empty_bulk() {
        assert_eq!(value_to_text(&redis::Value::Bulk(vec![])), "(empty array)");
    }

    #[test]
    fn render_nested_bulk() {
        let v = redis::Value::Bulk(vec![
            redis::Value::Data(b"user:1".to_vec()),
            redis::Value::Data(b"user:2".to_vec()),
        ]);
        assert_eq!(value_to_text(&v), "user:1\nuser:2");
    }
}
