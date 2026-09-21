//! # 全局配置
//!
//! 集中管理应用的各项可调参数（超时、路径等）。
//!
//! 超时值由连接池接线：建连走 [`ConnectionTimeout::connect`]，命令走
//! [`ConnectionTimeout::command`]，两者都以 `tokio::time::timeout` 兜住
//! （见 [`crate::connection_pool::PooledConn::query`] 与
//! [`crate::connection_pool::PooledConn::query_pipeline`]）。

use std::time::Duration;

use crate::error::AppError;

/// Redis 连接相关超时配置。
///
/// 默认值见 [`Default`] 实现，可由 [`AppConfig`] 注入连接池。
/// 目前尚未暴露成用户可配置项（见 DEVELOPMENT.md §4），要调默认值就改这里。
#[derive(Debug, Clone, Copy)]
pub struct ConnectionTimeout {
    /// 建立连接的超时时间。
    ///
    /// 这是一次**连接调用**的总预算，包含 TCP 连接、认证握手与 redis-rs 内部的重试退避
    /// （见 [`crate::connection_pool`] 中 `CONNECT_RETRIES` 的说明）；不含之后的命令往返。
    pub connect: Duration,
    /// 一批命令从发出到返回的最长等待时间。
    ///
    /// 防止 `BLPOP` 这类阻塞命令或网络异常导致界面永久卡死。单条命令与 pipeline
    /// （多条命令一次往返，如 Key 列表的 TYPE/TTL 批量查询）共用这个预算：
    /// pipeline 的等待**整批**只算一次，不按命令条数叠加。
    pub command: Duration,
}

impl Default for ConnectionTimeout {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(5),
            command: Duration::from_secs(10),
        }
    }
}

impl ConnectionTimeout {
    /// 建连超时的统一错误。
    ///
    /// 文案集中在这里，避免各调用点措辞不一；末尾给出排查方向，
    /// 因为「连不上」最常见的两个原因是端口写错与网络被限制。
    pub fn connect_timeout_error(&self) -> AppError {
        AppError::Timeout(format!(
            "连接 {}内未建立（请检查主机 / 端口是否可达、网络是否被限制）",
            humanize(self.connect)
        ))
    }

    /// 命令超时的统一错误。
    ///
    /// 阻塞类命令（`BLPOP` / `SUBSCRIBE` 等）走的就是这条路径，调用方若需要
    /// 可以在此文案后面追加更具体的提示（见 `commands::terminal`）。
    pub fn command_timeout_error(&self) -> AppError {
        AppError::Timeout(format!(
            "命令 {}内未返回（服务器繁忙、网络异常，或命令本身会阻塞）",
            humanize(self.command)
        ))
    }
}

/// 把超时时长写成用户可读的文案。
///
/// 不足 1 秒时按毫秒显示：测试与将来的「自定义超时」都可能配出亚秒级的值，
/// 一律按秒取整会显示成「0 秒」。
fn humanize(timeout: Duration) -> String {
    if timeout.as_secs() >= 1 {
        format!("{} 秒", timeout.as_secs())
    } else {
        format!("{} 毫秒", timeout.as_millis())
    }
}

/// 应用全局运行时配置。
#[derive(Default)]
pub struct AppConfig {
    /// Redis 连接相关超时
    pub conn_timeout: ConnectionTimeout,
}

#[cfg(test)]
mod tests {
    use super::humanize;
    use std::time::Duration;

    #[test]
    fn humanize_uses_seconds_when_at_least_one() {
        assert_eq!(humanize(Duration::from_secs(5)), "5 秒");
        assert_eq!(humanize(Duration::from_millis(1500)), "1 秒");
    }

    #[test]
    fn humanize_uses_millis_below_one_second() {
        assert_eq!(humanize(Duration::from_millis(300)), "300 毫秒");
        assert_eq!(humanize(Duration::ZERO), "0 毫秒");
    }

    #[test]
    fn timeout_errors_mention_the_configured_seconds() {
        let timeout = super::ConnectionTimeout::default();
        let msg = timeout.command_timeout_error().to_string();
        assert!(msg.contains("10 秒"), "命令超时提示应带上配置的秒数: {msg}");
        let msg = timeout.connect_timeout_error().to_string();
        assert!(msg.contains("5 秒"), "建连超时提示应带上配置的秒数: {msg}");
    }
}
