//! # 全局配置
//!
//! 集中管理应用的各项可调参数（超时、路径等）。
//!
//! > ⚠️ 说明：以下超时配置当前尚未与连接池接线（连接池使用 redis-rs 默认超时），
//! > 待连接配置 UI 接入后统一从 UI 读取；届时移除下方 `#[allow(dead_code)]`。

#![allow(dead_code)]

use std::time::Duration;

/// Redis 连接相关超时配置。
///
/// 默认值见 [`Default`] 实现，可在实例化时覆盖。
#[derive(Debug, Clone)]
pub struct ConnectionTimeout {
    /// 建立 TCP 连接的超时时间。
    ///
    /// 该时间是建立 socket 连接的最长等待，不包含认证与命令往返时间。
    pub connect: Duration,
    /// 单条 Redis 命令从发出到返回的最长等待时间。
    ///
    /// 防止 `BLPOP` 这类阻塞命令或网络异常导致界面永久卡死。
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
    /// 返回连接超时毫秒数（供 `TcpStream::connect_timeout` 等使用）。
    pub fn connect_millis(&self) -> u64 {
        self.connect.as_millis() as u64
    }

    /// 返回命令超时毫秒数。
    pub fn command_millis(&self) -> u64 {
        self.command.as_millis() as u64
    }
}

/// 应用全局运行时配置。
#[derive(Default)]
pub struct AppConfig {
    /// Redis 连接相关超时
    pub conn_timeout: ConnectionTimeout,
}
