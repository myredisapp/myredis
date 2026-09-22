//! # 统一错误类型
//!
//! 定义应用内所有自定义错误，并桥接底层库（`redis`、`std::io`、`serde_json` 等）的错误。
//!
//! 任何可能失败的后端函数都应返回 [`AppResult<T>`]。错误信息默认对用户友好，可直接展示在前端。

use serde::Serialize;

use crate::models::Connection;

/// 应用统一错误枚举。
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// Redis 底层错误（连接失败、命令执行失败、密码错误等）。
    #[error("Redis 错误: {0}")]
    Redis(#[from] redis::RedisError),

    /// 指定的连接不存在于连接池中。
    #[error("连接不存在: {0}")]
    ConnectionNotFound(String),

    /// 连接/命令执行超时。
    #[error("操作超时: {0}")]
    Timeout(String),

    /// IO 错误。
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    /// JSON 解析错误。
    #[error("配置解析错误: {0}")]
    Json(#[from] serde_json::Error),

    /// 通用业务错误（消息即用户提示）。
    #[error("{0}")]
    Business(String),
}

/// 便捷类型别名。
pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    /// 抛出一个业务错误。
    pub fn msg<S: Into<String>>(msg: S) -> Self {
        AppError::Business(msg.into())
    }
}

/// 将 Redis 命令执行错误转换为面向用户的中文提示。
///
/// 单机模式直连 Redis **集群**节点时，对不属于该节点哈希槽（slot）的 key，
/// 服务器会强制返回 `MOVED`/`ASK` 重定向错误（原始报错形如
/// `An error was signalled by the server - Moved: 7253 127.0.0.1:7102`，
/// 用户难以理解）。此时数据**不会**写入任何节点，并非写入失败或丢失；
/// 这里解析出负责该 key 的节点地址，并（若传入 `direct`）点明当前直连的节点，
/// 给出可操作建议。其他错误原样透传。
pub fn command_error_message(err: &redis::RedisError, direct: Option<&Connection>) -> String {
    use redis::ErrorKind;
    match err.kind() {
        ErrorKind::Moved | ErrorKind::Ask => {
            // redis-rs 的 Display 输出中带有服务器返回的重定向信息，
            // 实测形如 "... - Moved: 7253 127.0.0.1:7102"；不同版本 detail 字段
            // 内容不一致，不能依赖，直接从 to_string() 分词解析出 slot 与负责节点。
            let raw = err.to_string();
            let tokens: Vec<&str> = raw.split_whitespace().collect();
            let redirect_at = tokens.iter().position(|t| {
                let head = t.trim_end_matches(':').to_uppercase();
                head == "MOVED" || head == "ASK"
            });
            let (slot, owner) = match redirect_at {
                Some(i) => {
                    let mut slot = "?";
                    let mut owner = "集群中的其他节点";
                    for t in &tokens[i + 1..] {
                        if t.parse::<u32>().is_ok() {
                            if slot == "?" {
                                slot = t;
                            }
                        } else {
                            // 跳过 detail 中重复出现的关键字（如 "MOVED 7253 ..."）
                            let head = t.trim_end_matches(':').to_uppercase();
                            if head != "MOVED" && head != "ASK" {
                                owner = t;
                                break;
                            }
                        }
                    }
                    (slot, owner)
                }
                None => ("?", "集群中的其他节点"),
            };
            let direct_hint = match direct {
                Some(c) => format!("，当前以单机模式直连的是 {}:{}，它无权写入", c.host, c.port),
                None => "，当前直连节点无权写入".to_string(),
            };
            format!(
                "该 key 的哈希槽（slot {slot}）由集群节点 {owner} 负责{direct_hint}，数据未写入任何节点。若要让该 key 写入 {owner}，请单机模式直连该节点；如需操作集群中的任意 key，请使用「集群模式」建立连接"
            )
        }
        ErrorKind::CrossSlot => {
            // 集群里一条命令涉及多个 key、而这些 key 又不在同一个哈希槽时服务端回 CROSSSLOT。
            // 典型场景：重命名 / 复制的源与目标不同槽、批量删除跨槽的 key。
            "该命令里的多个 key 不在同一个哈希槽（slot），集群模式下无法跨 slot 操作，命令未执行。\
             可让这些 key 使用相同的 hash tag（如 user:{1001}:name 与 user:{1001}:email 会落在同一个槽），\
             或拆成逐条命令执行"
                .to_string()
        }
        _ => err.to_string(),
    }
}

/// 把命令层的错误转成面向用户的提示。
///
/// 与 [`command_error_message`] 的区别在于输入：命令层执行完 [`crate::connection_pool::PooledConn::query`]
/// 后拿到的是 [`AppError`]，其中既有底层 Redis 报错，也有超时这类客户端侧错误（没有对应的
/// `RedisError`）。只有 Redis 报错需要转写 MOVED / ASK 建议，其余直接取 [`AppError`] 自己的文案。
pub fn command_error_text(err: &AppError, direct: Option<&Connection>) -> String {
    match err {
        AppError::Redis(e) => command_error_message(e, direct),
        other => other.to_string(),
    }
}

// 让 Tauri 命令能返回 AppError（Tauri 2 要求错误实现 Serialize）
impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{command_error_message, command_error_text};
    use crate::models::{ConnType, Connection};

    fn direct_conn() -> Connection {
        Connection {
            id: "1".into(),
            name: "t".into(),
            host: "127.0.0.1".into(),
            port: 7002,
            conn_type: ConnType::Single,
            readonly: false,
            separator: ":".into(),
            db: 0,
            username: None,
            password: None,
            tls: false,
            tls_insecure: false,
            connect_timeout_secs: None,
            command_timeout_secs: None,
        }
    }

    /// 模拟真实服务器 MOVED 错误：kind + 固定 message + 重定向 detail。
    fn moved_error(detail: &str) -> redis::RedisError {
        redis::RedisError::from((
            redis::ErrorKind::Moved,
            "An error was signalled by the server",
            detail.to_string(),
        ))
    }

    #[test]
    fn moved_error_becomes_friendly_message() {
        // 实测 redis-rs Display 输出："An error was signalled by the server - Moved: 7253 127.0.0.1:7102"
        let err = moved_error("7253 127.0.0.1:7102");
        let msg = command_error_message(&err, None);
        assert!(msg.contains("127.0.0.1:7102"), "应提示负责节点: {msg}");
        assert!(msg.contains("7253"), "应提示哈希槽: {msg}");
        assert!(msg.contains("集群模式"), "应建议改用集群模式: {msg}");
        assert!(msg.contains("未写入"), "应说明数据未写入: {msg}");
    }

    #[test]
    fn moved_error_with_direct_connection_mentions_it() {
        let err = moved_error("7253 127.0.0.1:7102");
        let msg = command_error_message(&err, Some(&direct_conn()));
        assert!(msg.contains("127.0.0.1:7002"), "应点明当前直连节点: {msg}");
    }

    #[test]
    fn moved_error_with_full_detail_prefix() {
        // 兼容 detail 里带 "MOVED " 前缀的形态
        let err = moved_error("MOVED 7253 127.0.0.1:7102");
        let msg = command_error_message(&err, None);
        assert!(msg.contains("127.0.0.1:7102"), "应提示负责节点: {msg}");
        assert!(msg.contains("7253"), "应提示哈希槽: {msg}");
    }

    #[test]
    fn ask_error_becomes_friendly_message() {
        let err = redis::RedisError::from((
            redis::ErrorKind::Ask,
            "An error was signalled by the server",
            "100 127.0.0.1:7103".to_string(),
        ));
        let msg = command_error_message(&err, None);
        assert!(msg.contains("127.0.0.1:7103"), "应提示负责节点: {msg}");
        assert!(msg.contains("100"), "应提示哈希槽: {msg}");
    }

    #[test]
    fn redirect_error_without_detail_falls_back_gracefully() {
        let err = redis::RedisError::from((redis::ErrorKind::Moved, "unknown error"));
        let msg = command_error_message(&err, None);
        assert!(
            msg.contains("集群模式"),
            "无重定向信息时也应给出建议: {msg}"
        );
    }

    #[test]
    fn other_errors_pass_through() {
        let err = redis::RedisError::from((redis::ErrorKind::TypeError, "WRONGTYPE message"));
        assert_eq!(command_error_message(&err, None), err.to_string());
    }

    /// 跨 slot 的多 key 命令：给出「hash tag / 逐条执行」这样能照做的建议。
    #[test]
    fn cross_slot_error_explains_hash_tags() {
        let err = redis::RedisError::from((
            redis::ErrorKind::CrossSlot,
            "An error was signalled by the server",
            "CROSSSLOT Keys in request don't hash to the same slot".to_string(),
        ));
        let msg = command_error_message(&err, None);
        assert!(msg.contains("哈希槽"), "应说明是槽不一致: {msg}");
        assert!(msg.contains("hash tag"), "应给出 hash tag 方案: {msg}");
        assert!(msg.contains("未执行"), "应说明命令没有执行: {msg}");
    }

    /// 命令层现在拿到的是 [`AppError`]：Redis 报错仍要转写，客户端侧错误直接取文案。
    #[test]
    fn command_error_text_rewrites_redis_redirect_errors() {
        let err = super::AppError::Redis(moved_error("7253 127.0.0.1:7102"));
        let msg = command_error_text(&err, Some(&direct_conn()));
        assert!(msg.contains("集群模式"), "MOVED 应转写成可操作建议: {msg}");
        assert!(msg.contains("127.0.0.1:7102"), "应提示负责节点: {msg}");
    }

    #[test]
    fn command_error_text_keeps_client_side_errors() {
        let err = crate::config::ConnectionTimeout::default().command_timeout_error();
        let msg = command_error_text(&err, None);
        assert!(msg.contains("10 秒"), "超时文案应带上配置的秒数: {msg}");
        assert_eq!(msg, err.to_string(), "客户端侧错误应原样透出");
    }
}
