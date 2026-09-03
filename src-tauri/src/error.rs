//! # 统一错误类型
//!
//! 定义应用内所有自定义错误，并桥接底层库（`redis`、`std::io`、`serde_json` 等）的错误。
//!
//! 任何可能失败的后端函数都应返回 [`AppResult<T>`]。错误信息默认对用户友好，可直接展示在前端。

use serde::Serialize;

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

    /// 连接未就绪，需重新连接。
    #[error("连接未就绪: {0}")]
    NotConnected(String),

    /// TLS 加密连接尚未支持。
    #[error("暂不支持 TLS 加密连接 (rediss)，请使用明文 redis:// 连接")]
    TlsNotSupported,

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

// 让 Tauri 命令能返回 AppError（Tauri 2 要求错误实现 Serialize）
impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}
