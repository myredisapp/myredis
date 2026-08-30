//! # 连接配置模型

use serde::{Deserialize, Serialize};

/// 连接类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConnType {
    /// 单机
    #[default]
    Single,
    /// 集群
    Cluster,
}

/// 一个 Redis 连接配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connection {
    /// 唯一 id
    pub id: String,
    /// 显示名称
    pub name: String,
    /// 主机
    pub host: String,
    /// 端口
    pub port: u16,
    /// 连接类型
    #[serde(rename = "type")]
    pub conn_type: ConnType,
    /// 是否只读连接（启用后前端禁用写操作，后端拦截写命令）
    #[serde(default)]
    pub readonly: bool,
    /// Key 分隔符，用于前端「文件夹折叠模式」下将 Key 按分隔符组织成目录树。
    /// 默认 `:`，如 `user:1001:name`。
    #[serde(default = "default_separator")]
    pub separator: String,
    /// 密码（可选）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

/// 默认 Key 分隔符。
fn default_separator() -> String {
    ":".to_string()
}

impl Connection {
    /// 构造 Redis 连接 URL。
    ///
    /// 当前仅支持 `redis://`（明文）协议，`rediss` 在解析前已拦截。
    ///
    /// 若配置了密码，则以 `redis://:password@host:port` 形式内嵌，用于认证。
    pub fn to_connection_url(&self) -> String {
        match &self.password {
            Some(pwd) if !pwd.is_empty() => {
                let escaped = pwd.replace('@', "%40").replace('/', "%2F");
                format!("redis://:{}@{}:{}", escaped, self.host, self.port)
            }
            _ => format!("redis://{}:{}", self.host, self.port),
        }
    }
/// 是否只读连接。
    ///
    /// 只读连接会前端禁用写操作、后端更发送 `READONLY` 并拦截写命令。
    pub fn is_readonly(&self) -> bool {
        self.readonly
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_connection_url() {
        let conn = Connection {
            id: "1".into(),
            name: "t".into(),
            host: "127.0.0.1".into(),
            port: 6379,
            conn_type: ConnType::Single,
            readonly: false,
            separator: ":".into(),
            password: None,
        };
        assert_eq!(conn.to_connection_url(), "redis://127.0.0.1:6379");
    }

    #[test]
    fn connection_url_with_password() {
        let conn = Connection {
            id: "1".into(),
            name: "t".into(),
            host: "127.0.0.1".into(),
            port: 6379,
            conn_type: ConnType::Single,
            readonly: false,
            separator: ":".into(),
            password: Some("secret@123".into()),
        };
        assert_eq!(
            conn.to_connection_url(),
            "redis://:secret%40123@127.0.0.1:6379"
        );
    }
}
