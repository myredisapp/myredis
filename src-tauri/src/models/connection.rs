//! # 连接配置模型

use percent_encoding::{percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// 本期不支持的加密协议（判定标准见 `PROJECT_PLAN.md` §9.4.1）：
/// 只提供明文 `redis://`，不加载任何 CA 证书。
const TLS_SCHEMES: [&str; 3] = ["rediss", "tls", "ssl"];

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

/// 从主机字段里取出 URL 协议名（统一转小写），没有协议前缀时返回 `None`。
///
/// 只有形如 `scheme://` 且 `scheme` 符合 RFC 3986 协议名规则
/// （字母开头，后接字母 / 数字 / `+` / `-` / `.`）时才认作协议前缀，因此：
/// - `127.0.0.1`、`redis.example.com`、IPv6 字面量 `::1` / `[::1]` 都不含 `://`，返回 `None`；
/// - 主机名里出现「rediss」字样（如 `rediss.example.com`）也不会被误判成协议；
/// - `foo bar://x` 这类不像协议的输入一律放行，交给 redis-rs 自己报错。
fn host_scheme(host: &str) -> Option<String> {
    let (scheme, _) = host.trim().split_once("://")?;
    let is_valid_scheme = scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    is_valid_scheme.then(|| scheme.to_ascii_lowercase())
}

impl Connection {
    /// 校验主机字段里的协议前缀是否受支持。
    ///
    /// 用户在「主机」字段里粘贴整条 URL（`rediss://host:6379` 之类）是很常见的操作。
    /// [`Connection::to_connection_url`] 无条件拼 `redis://`，若直接放行，拼出的地址会变成
    /// `redis://rediss://host:6379` —— 用户只能看到一条难以理解的 URL 解析错误。
    /// 因此在保存、导入、建连三个入口都先调用本方法（`PROJECT_PLAN.md` §4.2.3 / §9.4.1）。
    ///
    /// 提示分工：
    /// - TLS 系协议（`rediss` / `tls` / `ssl`）→ [`AppError::TlsNotSupported`]；
    /// - 其它合法协议（如粘贴了 `redis://host:6379`）→ 说明该字段只需填主机名；
    /// - 没有协议前缀（常规主机名、IPv4 / IPv6）→ 直接放行。
    pub fn check_supported_scheme(&self) -> AppResult<()> {
        let Some(scheme) = host_scheme(&self.host) else {
            return Ok(());
        };
        if TLS_SCHEMES.contains(&scheme.as_str()) {
            return Err(AppError::TlsNotSupported);
        }
        Err(AppError::msg(format!(
            "主机字段只需填主机名（如 127.0.0.1），不要带 {scheme}:// 前缀"
        )))
    }

    /// 构造 Redis 连接 URL。
    ///
    /// 当前仅支持 `redis://`（明文）协议；`rediss://` / `tls://` 等前缀由
    /// [`Connection::check_supported_scheme`] 在保存 / 导入 / 建连入口拦截。
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

    /// 只关心主机字段的用例共用的构造器。
    fn with_host(host: &str) -> Connection {
        Connection {
            id: "1".into(),
            name: "t".into(),
            host: host.into(),
            port: 6379,
            conn_type: ConnType::Single,
            readonly: false,
            separator: ":".into(),
            db: 0,
            username: None,
            password: None,
        }
    }

    /// TLS 系协议前缀必须返回 [`AppError::TlsNotSupported`]，文案即用户看到的提示。
    #[test]
    fn tls_scheme_hosts_are_rejected() {
        for host in [
            "rediss://redis.example.com",
            "REDISS://redis.example.com",
            "tls://redis.example.com",
            "ssl://redis.example.com",
            "rediss://user:pass@redis.example.com:6379/0",
            "  rediss://redis.example.com  ", // 导入的文件里可能带空白
        ] {
            let err = with_host(host)
                .check_supported_scheme()
                .expect_err("TLS 协议应被拦下");
            assert!(
                matches!(err, super::AppError::TlsNotSupported),
                "{host} 应返回 TlsNotSupported，实际: {err:?}"
            );
            let text = err.to_string();
            assert!(text.contains("暂不支持 TLS 加密连接"), "{host}: {text}");
            assert!(text.contains("rediss"), "{host}: {text}");
        }
    }

    /// 常规主机名不能被误判：主机名里含「rediss」字样、IPv4、IPv6 都应放行。
    #[test]
    fn plain_hosts_pass_validation() {
        for host in [
            "127.0.0.1",
            "redis.example.com",
            "rediss.example.com",
            "tls-cluster.internal",
            "::1",
            "[::1]",
            "my-host.local",
        ] {
            assert!(
                with_host(host).check_supported_scheme().is_ok(),
                "{host} 是合法主机名，不应被拦下"
            );
        }
    }

    /// 粘贴了明文的整条 URL：提示「只需填主机名」，而不是让 redis-rs 报 URL 解析错误。
    #[test]
    fn non_tls_scheme_hint_asks_for_host_only() {
        let err = with_host("redis://127.0.0.1:6379")
            .check_supported_scheme()
            .expect_err("带协议前缀的输入应被拦下");
        let text = err.to_string();
        assert!(text.contains("只需填主机名"), "{text}");
        assert!(text.contains("redis://"), "{text}");
        assert!(
            !matches!(err, super::AppError::TlsNotSupported),
            "明文协议不应报 TLS 错误: {text}"
        );
    }
}
