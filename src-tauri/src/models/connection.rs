//! # 连接配置模型

use std::time::Duration;

use percent_encoding::{percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde::{Deserialize, Serialize};

use crate::config::ConnectionTimeout;
use crate::error::{AppError, AppResult};

/// TLS 系协议前缀（用户在主机字段里粘贴整条 `rediss://` URL 时命中，
/// 判定标准见 `PROJECT_PLAN.md` §9.4.1）。
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
    /// 密码（可选）。
    ///
    /// 传输与内存中均为明文；落盘时由 [`crate::storage::ConnectionRepo`] 转存系统
    /// 密钥链（keychain），`connections.json` 里不保留（见 DEVELOPMENT.md §2.4）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// 是否走 TLS 加密连接（`rediss://`）。
    ///
    /// 自签名证书等校验不过的场景可配合 [`Self::tls_insecure`] 跳过校验。
    #[serde(default)]
    pub tls: bool,
    /// TLS 模式下跳过证书校验（`#insecure`，redis-rs 的 URL fragment 语法）。
    ///
    /// 只影响 TLS 连接；明文连接忽略该字段。
    #[serde(default)]
    pub tls_insecure: bool,
    /// 自定义建连超时（秒）。`None` 用全局默认值（[`ConnectionTimeout::default`]）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_timeout_secs: Option<u64>,
    /// 自定义命令超时（秒）。`None` 用全局默认值。
    ///
    /// 大 key 的整表读取（`LRANGE 0 -1` / `HGETALL` / `XRANGE` 全量等）可能超过
    /// 默认 10 秒，慢链路或大数据量时可单独放宽（见 DEVELOPMENT.md §2.4）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_timeout_secs: Option<u64>,
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
    /// 校验主机字段里的协议前缀是否受支持，并返回**去掉协议前缀后的主机名**。
    ///
    /// 用户在「主机」字段里粘贴整条 URL（`rediss://host:6379` 之类）是很常见的操作。
    /// [`Connection::to_connection_url`] 会自行拼协议，若直接放行，明文会拼出
    /// `redis://rediss://host:6379` —— 用户只能看到一条难以理解的 URL 解析错误。
    /// 因此在保存、导入、建连三个入口都先调用本方法（`PROJECT_PLAN.md` §4.2.3 / §9.4.1）。
    ///
    /// 提示分工：
    /// - TLS 系协议（`rediss` / `tls` / `ssl`）且**已勾选 TLS**：剥掉前缀，余下的部分
    ///   必须是纯主机名（不带 userinfo / 路径 / 端口，端口请填在端口字段）；
    /// - TLS 系协议且**未勾选 TLS**：提示去连接设置里打开 TLS 开关；
    /// - 其它合法协议（如粘贴了 `redis://host:6379`）→ 说明该字段只需填主机名；
    /// - 没有协议前缀（常规主机名、IPv4 / IPv6）→ 直接放行。
    pub fn check_supported_scheme(&self) -> AppResult<String> {
        let Some(scheme) = host_scheme(&self.host) else {
            return Ok(self.host.trim().to_string());
        };
        if TLS_SCHEMES.contains(&scheme.as_str()) {
            if !self.tls {
                return Err(AppError::msg(
                    "主机字段检测到 rediss:// 前缀：如需 TLS 加密连接，请在连接设置中勾选「TLS 加密」，主机字段只需填主机名",
                ));
            }
            let host = self.host.trim();
            let rest = host.split_once("://").map(|(_, r)| r).unwrap_or(host);
            if rest.contains('@') || rest.contains('/') || rest.contains(':') {
                return Err(AppError::msg(
                    "主机字段只需填主机名（如 redis.example.com），账号、密码填在专门字段，端口填在端口字段",
                ));
            }
            return Ok(rest.to_string());
        }
        Err(AppError::msg(format!(
            "主机字段只需填主机名（如 127.0.0.1），不要带 {scheme}:// 前缀"
        )))
    }

    /// 构造 Redis 连接 URL。
    ///
    /// 协议由 [`Self::tls`] 决定：`redis://`（明文）或 `rediss://`（TLS，
    /// 自签名证书时按 [`Self::tls_insecure`] 追加 `#insecure` 跳过校验）。
    /// 主机字段若带 `rediss://` 前缀，需先经 [`Connection::check_supported_scheme`]
    /// 剥掉（保存 / 导入 / 建连入口都会调）。
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
        let scheme = if self.tls { "rediss" } else { "redis" };
        let insecure = if self.tls && self.tls_insecure {
            "#insecure"
        } else {
            ""
        };

        if !username.is_empty() && !password.is_empty() {
            format!(
                "{scheme}://{}:{}@{}:{}{}{}",
                username, password, self.host, self.port, db, insecure
            )
        } else if !username.is_empty() {
            format!(
                "{scheme}://{}@{}:{}{}{}",
                username, self.host, self.port, db, insecure
            )
        } else if !password.is_empty() {
            format!(
                "{scheme}://:{}@{}:{}{}{}",
                password, self.host, self.port, db, insecure
            )
        } else {
            format!("{scheme}://{}:{}{}{}", self.host, self.port, db, insecure)
        }
    }

    /// 该连接生效的超时配置：连接上自定义的字段优先，未配置回落到 `default`。
    ///
    /// 连接池的全局超时只是默认值；按连接差异化后缓存键无需变化 ——
    /// `Pool::connect` 每次都用当时配置重新建连并覆盖条目（见 DEVELOPMENT.md §2.4）。
    pub fn effective_timeout(&self, default: &ConnectionTimeout) -> ConnectionTimeout {
        ConnectionTimeout {
            connect: self
                .connect_timeout_secs
                .map(Duration::from_secs)
                .unwrap_or(default.connect),
            command: self
                .command_timeout_secs
                .map(Duration::from_secs)
                .unwrap_or(default.command),
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
            tls: false,
            tls_insecure: false,
            connect_timeout_secs: None,
            command_timeout_secs: None,
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
            tls: false,
            tls_insecure: false,
            connect_timeout_secs: None,
            command_timeout_secs: None,
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
            tls: false,
            tls_insecure: false,
            connect_timeout_secs: None,
            command_timeout_secs: None,
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
            tls: false,
            tls_insecure: false,
            connect_timeout_secs: None,
            command_timeout_secs: None,
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
            tls: false,
            tls_insecure: false,
            connect_timeout_secs: None,
            command_timeout_secs: None,
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
            tls: false,
            tls_insecure: false,
            connect_timeout_secs: None,
            command_timeout_secs: None,
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
            tls: false,
            tls_insecure: false,
            connect_timeout_secs: None,
            command_timeout_secs: None,
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
            tls: false,
            tls_insecure: false,
            connect_timeout_secs: None,
            command_timeout_secs: None,
        }
    }

    /// TLS 系协议前缀：未勾选 TLS 时给出「打开 TLS 开关」的提示；勾选后剥掉前缀放行。
    #[test]
    fn tls_scheme_hosts_require_tls_enabled() {
        for host in [
            "rediss://redis.example.com",
            "REDISS://redis.example.com",
            "tls://redis.example.com",
            "ssl://redis.example.com",
            "  rediss://redis.example.com  ", // 导入的文件里可能带空白
        ] {
            let err = with_host(host)
                .check_supported_scheme()
                .expect_err("未勾选 TLS 时 rediss:// 应被拦下");
            let text = err.to_string();
            assert!(text.contains("TLS 加密"), "{host}: {text}");
            assert!(
                !text.contains("暂不支持"),
                "{host}: 文案不应再说「不支持」: {text}"
            );
        }

        // 勾选 TLS 后：剥掉前缀，返回纯主机名
        let mut conn = with_host("rediss://redis.example.com");
        conn.tls = true;
        assert_eq!(
            conn.check_supported_scheme().expect("勾选 TLS 应放行"),
            "redis.example.com"
        );
    }

    /// 勾选 TLS 但 URL 里带账号 / 端口 / 路径：要求拆到专门字段，而不是混进主机名。
    #[test]
    fn tls_host_with_userinfo_or_port_is_rejected() {
        for host in [
            "rediss://user:pass@redis.example.com",
            "rediss://redis.example.com:6379",
            "rediss://redis.example.com/0",
        ] {
            let mut conn = with_host(host);
            conn.tls = true;
            let err = conn.check_supported_scheme().expect_err(host);
            assert!(err.to_string().contains("只需填主机名"), "{host}: {err}");
        }
    }

    /// TLS 开关反映到连接 URL：明文 / 加密 / 跳过校验三种形态。
    #[test]
    fn connection_url_schemes_follow_tls_flags() {
        let mut conn = with_host("127.0.0.1");
        assert_eq!(conn.to_connection_url(), "redis://127.0.0.1:6379/0");

        conn.tls = true;
        assert_eq!(conn.to_connection_url(), "rediss://127.0.0.1:6379/0");

        conn.tls_insecure = true;
        assert_eq!(
            conn.to_connection_url(),
            "rediss://127.0.0.1:6379/0#insecure"
        );
    }

    /// 连接级超时时长：配置了用配置的，没配置回落默认值。
    #[test]
    fn effective_timeout_prefers_per_connection_values() {
        let default = ConnectionTimeout::default();
        let conn = with_host("127.0.0.1");
        assert_eq!(conn.effective_timeout(&default).connect, default.connect);
        assert_eq!(conn.effective_timeout(&default).command, default.command);

        let mut custom = with_host("127.0.0.1");
        custom.connect_timeout_secs = Some(1);
        custom.command_timeout_secs = Some(60);
        let eff = custom.effective_timeout(&default);
        assert_eq!(eff.connect, Duration::from_secs(1));
        assert_eq!(eff.command, Duration::from_secs(60));
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
    }
}
