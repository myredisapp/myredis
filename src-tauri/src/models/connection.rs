//! # 连接配置模型

use percent_encoding::{percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde::{Deserialize, Serialize};

/// URL userinfo 中不需要 percent-encode 的字符集合（RFC 3986 的 unreserved + sub-delims）。
const USERINFO_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'!')
    .remove(b'$')
    .remove(b'&')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')')
    .remove(b'*')
    .remove(b'+')
    .remove(b',')
    .remove(b';')
    .remove(b'=');

fn encode_userinfo(s: &str) -> String {
    percent_encode(s.as_bytes(), USERINFO_ENCODE_SET).to_string()
}

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
    /// 逻辑数据库编号（0-15）。连接 / 切换时通过 URL 的 `/db` 段选中。
    #[serde(default)]
    pub db: u64,
    /// 用户名（可选），用于 Redis 6.0+ ACL 认证。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
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
    /// 支持用户名 + 密码的 ACL 认证，生成形式如下：
    /// - 用户名 + 密码：`redis://user:password@host:port/db`
    /// - 仅用户名：`redis://user@host:port/db`
    /// - 仅密码：`redis://:password@host:port/db`
    /// - 都为空：`redis://host:port/db`
    ///
    /// 用户名与密码中的特殊字符会按 RFC 3986 进行 percent-encoding，
    /// 避免 `@`、`/`、`?`、`#`、`:` 等破坏 URL 结构。
    ///
    /// 集群模式只有 db0，URL 不带 `/db` 段，避免集群客户端误解析。
    pub fn to_connection_url(&self) -> String {
        // 集群忽略逻辑数据库编号（只有 db0）
        let db = if self.conn_type == ConnType::Cluster {
            String::new()
        } else {
            format!("/{}", self.db)
        };
        let username = self
            .username
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(encode_userinfo)
            .unwrap_or_default();
        let password = self
            .password
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(encode_userinfo)
            .unwrap_or_default();

        if !username.is_empty() && !password.is_empty() {
            format!(
                "redis://{}:{}@{}:{}{}",
                username, password, self.host, self.port, db
            )
        } else if !username.is_empty() {
            format!("redis://{}@{}:{}{}", username, self.host, self.port, db)
        } else if !password.is_empty() {
            format!("redis://:{}@{}:{}{}", password, self.host, self.port, db)
        } else {
            format!("redis://{}:{}{}", self.host, self.port, db)
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
            db: 0,
            username: None,
            password: None,
        };
        assert_eq!(conn.to_connection_url(), "redis://127.0.0.1:6379/0");
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
            db: 0,
            username: None,
            password: Some("secret@123".into()),
        };
        assert_eq!(
            conn.to_connection_url(),
            "redis://:secret%40123@127.0.0.1:6379/0"
        );
    }

    #[test]
    fn connection_url_with_db() {
        let conn = Connection {
            id: "1".into(),
            name: "t".into(),
            host: "127.0.0.1".into(),
            port: 6379,
            conn_type: ConnType::Single,
            readonly: false,
            separator: ":".into(),
            db: 3,
            username: None,
            password: None,
        };
        assert_eq!(conn.to_connection_url(), "redis://127.0.0.1:6379/3");
    }

    #[test]
    fn connection_url_with_username_only() {
        let conn = Connection {
            id: "1".into(),
            name: "t".into(),
            host: "127.0.0.1".into(),
            port: 6379,
            conn_type: ConnType::Single,
            readonly: false,
            separator: ":".into(),
            db: 0,
            username: Some("redis_user".into()),
            password: None,
        };
        assert_eq!(
            conn.to_connection_url(),
            "redis://redis_user@127.0.0.1:6379/0"
        );
    }

    #[test]
    fn connection_url_with_username_and_password() {
        let conn = Connection {
            id: "1".into(),
            name: "t".into(),
            host: "127.0.0.1".into(),
            port: 6379,
            conn_type: ConnType::Single,
            readonly: false,
            separator: ":".into(),
            db: 0,
            username: Some("user@domain".into()),
            password: Some("secret/123".into()),
        };
        assert_eq!(
            conn.to_connection_url(),
            "redis://user%40domain:secret%2F123@127.0.0.1:6379/0"
        );
    }

    #[test]
    fn connection_url_encodes_special_characters() {
        let conn = Connection {
            id: "1".into(),
            name: "t".into(),
            host: "127.0.0.1".into(),
            port: 6379,
            conn_type: ConnType::Single,
            readonly: false,
            separator: ":".into(),
            db: 0,
            username: Some("user:name".into()),
            password: Some("p@ss:w?rd#".into()),
        };
        assert_eq!(
            conn.to_connection_url(),
            "redis://user%3Aname:p%40ss%3Aw%3Frd%23@127.0.0.1:6379/0"
        );
    }

    #[test]
    fn cluster_connection_url_omits_db() {
        let conn = Connection {
            id: "1".into(),
            name: "t".into(),
            host: "127.0.0.1".into(),
            port: 7000,
            conn_type: ConnType::Cluster,
            readonly: false,
            separator: ":".into(),
            db: 3,
            username: Some("user".into()),
            password: Some("secret".into()),
        };
        assert_eq!(
            conn.to_connection_url(),
            "redis://user:secret@127.0.0.1:7000"
        );
    }
}
