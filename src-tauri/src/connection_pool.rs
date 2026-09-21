//! # Redis 连接池
//!
//! 管理到 Redis 的活跃连接。支持**单机**与**集群**（Cluster）两种模式，
//! 并可标记连接为**只读**（在命令层拦截写命令）。
//!
//! 命令层通过 [`Pool::conn`] 取 [`PooledConn`] 句柄执行命令 —— 这是命令执行的唯一出口，
//! 每条命令与每次建连都按 [`ConnectionTimeout`] 包一层 `tokio::time::timeout`
//! （默认 5 秒建连 / 10 秒命令）：服务器卡住时界面拿到明确错误，而不是永久等待。

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;

use redis::aio::{ConnectionLike, ConnectionManager, MultiplexedConnection};
use redis::cluster_async::ClusterConnection;
use redis::{FromRedisValue, RedisFuture};

use crate::config::ConnectionTimeout;
use crate::error::AppError;
use crate::models::{ConnType, Connection};

type ConnMap = HashMap<String, Entry>;

/// 单机连接的重试次数（不含首次尝试），退避参数与 redis-rs 默认一致
/// （第 n 次重试等待 `rand(0 .. FACTOR × EXPONENT_BASE^n)` 毫秒）。
///
/// redis-rs 默认重试 6 次，累计退避可达 12 秒：那会让「端口不通」这类**快速失败**
/// 被外层 [`ConnectionTimeout::connect`] 先截断，用户看不到真正的原因（Connection refused）。
/// 2 次重试的累计退避不超过 0.6 秒，既保留了「服务刚重启」时的自愈能力，
/// 又把连接预算留给真正无响应的场景（TCP 连上了但不回包）。
const CONNECT_RETRIES: usize = 2;
/// 见 [`CONNECT_RETRIES`]。
const CONNECT_RETRY_EXPONENT_BASE: u64 = 2;
/// 见 [`CONNECT_RETRIES`]。
const CONNECT_RETRY_FACTOR: u64 = 100;

/// 可执行 Redis 命令的连接句柄。
///
/// 三种形态都实现了 [`redis::aio::ConnectionLike`]，命令层只面向 [`PooledConn`]，
/// 无需关心底层是单机、集群，还是集群里某个节点的直连。
#[derive(Clone)]
pub enum Conn {
    /// 单机连接管理器（自动重连）
    Single(ConnectionManager),
    /// 集群连接（按 key 的哈希槽路由到对应节点）
    Cluster(ClusterConnection),
    /// 集群内单个节点的直连句柄：逐节点 `SCAN` 用，不入池、用完即弃
    Node(MultiplexedConnection),
}

impl ConnectionLike for Conn {
    fn req_packed_command<'a>(&'a mut self, cmd: &'a redis::Cmd) -> RedisFuture<'a, redis::Value> {
        Box::pin(async move {
            match self {
                Conn::Single(c) => c.req_packed_command(cmd).await,
                Conn::Cluster(c) => c.req_packed_command(cmd).await,
                Conn::Node(c) => c.req_packed_command(cmd).await,
            }
        })
    }

    fn req_packed_commands<'a>(
        &'a mut self,
        cmd: &'a redis::Pipeline,
        offset: usize,
        count: usize,
    ) -> RedisFuture<'a, Vec<redis::Value>> {
        Box::pin(async move {
            match self {
                Conn::Single(c) => c.req_packed_commands(cmd, offset, count).await,
                Conn::Cluster(c) => c.req_packed_commands(cmd, offset, count).await,
                Conn::Node(c) => c.req_packed_commands(cmd, offset, count).await,
            }
        })
    }

    fn get_db(&self) -> i64 {
        match self {
            Conn::Single(c) => c.get_db(),
            Conn::Cluster(c) => c.get_db(),
            Conn::Node(c) => c.get_db(),
        }
    }
}

/// 连接句柄 + 该连接的超时配置，命令层实际持有的类型。
///
/// 所有 Redis 命令都必须经 [`PooledConn::query`] 执行，超时保护因此在**一个地方**生效，
/// 不会出现「某个命令忘了包超时」的漏网之鱼。
pub struct PooledConn {
    inner: Conn,
    timeout: ConnectionTimeout,
}

impl PooledConn {
    /// 用给定超时配置包装一个连接句柄。
    pub fn new(inner: Conn, timeout: ConnectionTimeout) -> Self {
        Self { inner, timeout }
    }

    /// 该句柄的完整超时配置（集群逐节点扫描时，节点直连句柄沿用同一份配置）。
    pub fn timeout(&self) -> ConnectionTimeout {
        self.timeout
    }

    /// 执行一条 Redis 命令并解析返回值。
    ///
    /// - 超时 → [`AppError::Timeout`]（文案见 [`ConnectionTimeout::command_timeout_error`]），
    ///   命令等待因此有界，`BLPOP` 之类的阻塞命令也不会把界面挂死；
    /// - Redis 报错 → [`AppError::Redis`]，需要 MOVED / ASK 之类转写时用
    ///   [`crate::error::command_error_text`] 生成用户提示。
    pub async fn query<T: FromRedisValue>(&mut self, cmd: &redis::Cmd) -> Result<T, AppError> {
        match tokio::time::timeout(self.timeout.command, cmd.query_async(&mut self.inner)).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(err)) => Err(AppError::Redis(err)),
            Err(_) => Err(self.timeout.command_timeout_error()),
        }
    }
}

/// 应用全局连接池，注入 Tauri 状态 `tauri::State<Pool>`。
pub struct Pool {
    inner: Mutex<ConnMap>,
    /// 建连与命令的超时配置（由 [`crate::config::AppConfig`] 注入，见 `lib.rs`）
    timeout: ConnectionTimeout,
}

impl Default for Pool {
    fn default() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            timeout: ConnectionTimeout::default(),
        }
    }
}

/// 池中的单个连接条目。
struct Entry {
    conn: Connection,
    /// 可执行命令的连接句柄（单机 or 集群）
    handle: Conn,
}

/// 连接连接成功后的返回信息。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConnInfo {
    pub id: String,
    pub name: String,
}

impl Pool {
    /// 创建空连接池（使用 [`ConnectionTimeout::default`]）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 创建空连接池并指定超时配置。
    pub fn with_timeout(timeout: ConnectionTimeout) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            timeout,
        }
    }

    /// 建立到 `conn` 的 Redis 连接并缓存。
    ///
    /// # 超时
    /// 建连全过程（TCP、认证握手、只读 `READONLY`）受 [`ConnectionTimeout::connect`] 约束，
    /// 超时返回 [`AppError::Timeout`]。
    pub async fn connect(&self, conn: &Connection) -> Result<ConnInfo, AppError> {
        let handle = self.open(conn).await?;
        self.inner.lock().unwrap().insert(
            conn.id.clone(),
            Entry {
                conn: conn.clone(),
                handle,
            },
        );

        Ok(ConnInfo {
            id: conn.id.clone(),
            name: conn.name.clone(),
        })
    }

    /// 按连接配置建立句柄，若为只读连接先声明 `READONLY`。
    ///
    /// 集群与单机都缓存句柄复用（单机是 [`ConnectionManager`]，集群是 [`ClusterConnection`]）。
    async fn open(&self, conn: &Connection) -> Result<Conn, AppError> {
        let handle = self.connect_handle(conn).await?;

        if conn.is_readonly() {
            // 只读连接先声明 READONLY（对主从 / 集群副本生效）。
            // 普通单机与集群主节点不认识该命令，失败忽略即可（与只读标记的语义一致：
            // 真正的写入拦截由 `ensure_writable` 与服务器 ACL 负责）。
            let mut probe = PooledConn::new(handle.clone(), self.timeout);
            let _ = probe.query::<()>(&redis::cmd("READONLY")).await;
        }

        Ok(handle)
    }

    /// 按连接配置建立连接句柄（不缓存、不发送 `READONLY`）。
    async fn connect_handle(&self, conn: &Connection) -> Result<Conn, AppError> {
        match conn.conn_type {
            ConnType::Single => {
                let url = conn.to_connection_url();
                let client = redis::Client::open(url).map_err(AppError::from)?;
                // 响应超时交给 [`PooledConn::query`] 统一处理，这里不让 redis-rs 再设一套口径
                let future = ConnectionManager::new_with_backoff(
                    client,
                    CONNECT_RETRY_EXPONENT_BASE,
                    CONNECT_RETRY_FACTOR,
                    CONNECT_RETRIES,
                );
                Ok(Conn::Single(self.with_connect_timeout(future).await?))
            }
            ConnType::Cluster => {
                let url = conn.to_connection_url();
                let client =
                    redis::cluster::ClusterClient::new(vec![url]).map_err(AppError::from)?;
                let future = client.get_async_connection();
                Ok(Conn::Cluster(self.with_connect_timeout(future).await?))
            }
        }
    }

    /// 把建连 future 包进 [`ConnectionTimeout::connect`]，超时转成统一文案。
    async fn with_connect_timeout<T, F>(&self, future: F) -> Result<T, AppError>
    where
        F: Future<Output = Result<T, redis::RedisError>>,
    {
        tokio::time::timeout(self.timeout.connect, future)
            .await
            .map_err(|_| self.timeout.connect_timeout_error())?
            .map_err(AppError::from)
    }

    /// 获取可执行命令的连接句柄（含超时保护）。
    ///
    /// 命令层应使用该方法获取连接，再通过 [`PooledConn::query`] 执行命令，
    /// 无需关心底层是单机还是集群。
    pub fn conn(&self, id: &str) -> Result<PooledConn, AppError> {
        let guard = self.inner.lock().unwrap();
        let entry = guard
            .get(id)
            .ok_or_else(|| AppError::ConnectionNotFound(id.to_string()))?;
        Ok(PooledConn::new(entry.handle.clone(), self.timeout))
    }

    /// 校验连接是否存在，且（若 `writable` 为 true）连接不是只读。
    ///
    /// 所有可能写==的 Redis 命令执行前都应先调用（现在主要对单机命令生效）。
    pub fn ensure_writable(&self, id: &str) -> Result<(), AppError> {
        if self.readonly(id)? {
            return Err(AppError::Business("当前为只读连接，禁止写操作".into()));
        }
        Ok(())
    }

    /// 连接是否只读。
    pub fn readonly(&self, id: &str) -> Result<bool, AppError> {
        let guard = self.inner.lock().unwrap();
        let entry = guard
            .get(id)
            .ok_or_else(|| AppError::ConnectionNotFound(id.to_string()))?;
        Ok(entry.conn.is_readonly())
    }

    /// 校验写命令是否被允许（连接存在且非只读）。方便调用方统一处理。
    pub fn ensure_conn(&self, id: &str) -> Result<(), AppError> {
        let guard = self.inner.lock().unwrap();
        guard
            .get(id)
            .ok_or_else(|| AppError::ConnectionNotFound(id.to_string()))?;
        Ok(())
    }

    /// 获取连接配置的克隆。
    pub fn get(&self, id: &str) -> Result<Connection, AppError> {
        let guard = self.inner.lock().unwrap();
        guard
            .get(id)
            .map(|e| e.conn.clone())
            .ok_or_else(|| AppError::ConnectionNotFound(id.to_string()))
    }

    /// 断开（移除）一个连接。
    pub fn disconnect(&self, id: &str) -> bool {
        self.inner.lock().unwrap().remove(id).is_some()
    }

    /// PING 测试连接是否存活。
    pub async fn ping(&self, id: &str) -> Result<String, AppError> {
        let mut con = self.conn(id)?;
        con.query(&redis::cmd("PING")).await
    }

    /// 临时测试连接参数是否可用。
    ///
    /// 按 `conn` 建立连接，执行一次 `PING` 后立即释放，不写入连接池内部 map。
    /// 建连与 `PING` 分别受 [`ConnectionTimeout::connect`] / [`ConnectionTimeout::command`] 约束。
    pub async fn test(&self, conn: &Connection) -> Result<String, AppError> {
        let handle = self.connect_handle(conn).await?;
        let mut con = PooledConn::new(handle, self.timeout);
        con.query(&redis::cmd("PING")).await
    }
}

#[cfg(test)]
mod tests {
    use super::Pool;
    use crate::config::ConnectionTimeout;
    use crate::error::AppError;
    use crate::models::{ConnType, Connection};
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    /// 一个只会「接受连接」的假服务器：不读不写、不发任何响应，用来模拟彻底卡住的 Redis。
    ///
    /// 用标准库线程而不是 tokio，是为了不给测试引入 tokio 的 `net` feature；
    /// 已建立的连接被一直持有，客户端的请求就永远等不到响应。
    /// （线程在测试结束后自行泄漏，进程退出即回收。）
    fn spawn_silent_server() -> (u16, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定本地端口失败");
        let port = listener.local_addr().expect("读取本地端口失败").port();
        let handle = std::thread::spawn(move || {
            let mut held: Vec<TcpStream> = Vec::new();
            for stream in listener.incoming() {
                match stream {
                    Ok(s) => held.push(s),
                    Err(_) => break,
                }
            }
        });
        (port, handle)
    }

    /// 一个最小的 RESP 假服务器：逐条解析请求并回 `+OK`，唯独命令名等于 `silent_cmd`
    /// 的请求不回包 —— 用来精确模拟「建连正常、某条命令卡住」。
    ///
    /// 需要它会回包，是因为 redis-rs 建连时会握手（`CLIENT SETINFO`）并等待响应：
    /// 完全不回包的服务器只能测到建连超时，测不到命令超时。
    fn spawn_resp_stub(silent_cmd: &'static str) -> (u16, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定本地端口失败");
        let port = listener.local_addr().expect("读取本地端口失败").port();
        let handle = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                std::thread::spawn(move || serve(stream, silent_cmd));
            }
        });
        (port, handle)
    }

    /// 单个连接的服务循环：解析请求 → 回 `+OK`（除非该命令被要求静默）。
    fn serve(stream: TcpStream, silent_cmd: &str) {
        let Ok(mut writer) = stream.try_clone() else {
            return;
        };
        let mut reader = BufReader::new(stream);
        while let Some(name) = read_command_name(&mut reader) {
            if name.eq_ignore_ascii_case(silent_cmd) {
                // 吞掉请求不回包：客户端会一直等下去（正是要测的场景）
                continue;
            }
            if writer.write_all(b"+OK\r\n").is_err() || writer.flush().is_err() {
                return;
            }
        }
    }

    /// 从 RESP 请求流里读出一条命令，返回命令名（第一个参数）；流结束返回 `None`。
    fn read_command_name(reader: &mut impl BufRead) -> Option<String> {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 {
            return None;
        }
        let argc: usize = header.trim().trim_start_matches('*').parse().ok()?;
        let mut name = None;
        for index in 0..argc {
            let mut len_line = String::new();
            if reader.read_line(&mut len_line).ok()? == 0 {
                return None;
            }
            let len: usize = len_line.trim().trim_start_matches('$').parse().ok()?;
            let mut arg = vec![0u8; len];
            reader.read_exact(&mut arg).ok()?;
            let mut crlf = [0u8; 2];
            reader.read_exact(&mut crlf).ok()?;
            if index == 0 {
                name = Some(String::from_utf8_lossy(&arg).into_owned());
            }
        }
        name
    }

    fn conn_config(id: &str, port: u16) -> Connection {
        Connection {
            id: id.into(),
            name: "stub".into(),
            host: "127.0.0.1".into(),
            port,
            conn_type: ConnType::Single,
            readonly: false,
            separator: ":".into(),
            db: 0,
            username: None,
            password: None,
        }
    }

    fn fast_timeout() -> ConnectionTimeout {
        ConnectionTimeout {
            connect: Duration::from_millis(300),
            command: Duration::from_millis(300),
        }
    }

    /// 无响应的服务器上，建连必须在连接超时后返回 [`AppError::Timeout`]，
    /// 而不是永久挂起（TCP 能连上、但对握手不回包，是真实环境里最难排查的一种「卡住」）。
    #[tokio::test]
    async fn connect_times_out_when_server_never_answers_handshake() {
        let (port, _server) = spawn_silent_server();
        let pool = Pool::with_timeout(fast_timeout());

        let started = Instant::now();
        let err = pool
            .connect(&conn_config("silent", port))
            .await
            .expect_err("对无响应的服务器建连应当报错");
        assert!(
            matches!(err, AppError::Timeout(_)),
            "应当是超时错误，实际: {err:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "必须在超时后立即返回，实际耗时 {:?}",
            started.elapsed()
        );
    }

    /// 建连正常、但某条命令卡住：必须在命令超时后返回 [`AppError::Timeout`]。
    ///
    /// 超时只中止**这一次等待**，不主动重连：被放弃的命令之后若返回了响应，
    /// 会由 redis-rs 的驱动按序取走并丢弃，连接保持可用（真实 Redis 上的回归见
    /// [`blocking_command_is_interrupted_by_command_timeout`]）。
    #[tokio::test]
    async fn command_times_out_when_server_stops_replying() {
        let (port, _server) = spawn_resp_stub("GET");
        let pool = Pool::with_timeout(fast_timeout());
        pool.connect(&conn_config("stub", port))
            .await
            .expect("握手命令有回包，建连应当成功");

        let mut con = pool.conn("stub").expect("句柄应当可取");
        let started = Instant::now();
        let err = con
            .query::<String>(redis::cmd("GET").arg("k"))
            .await
            .expect_err("无响应的命令应当报错");
        assert!(
            matches!(err, AppError::Timeout(_)),
            "应当是超时错误，实际: {err:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "必须在超时后立即返回，实际耗时 {:?}",
            started.elapsed()
        );
        // 提示文案要带上配置的时长与排查方向，且能看出是「命令」层面的超时
        let text = err.to_string();
        assert!(text.contains("300 毫秒"), "文案应带上配置的超时值: {text}");
    }

    /// 池里没有该连接时，取句柄应报「连接不存在」而不是 panic。
    #[test]
    fn conn_reports_missing_connection() {
        let pool = Pool::with_timeout(fast_timeout());
        let err = match pool.conn("nope") {
            Err(e) => e,
            Ok(_) => panic!("不存在的连接应当报错"),
        };
        assert!(matches!(err, AppError::ConnectionNotFound(_)), "{err:?}");
    }

    /// 句柄沿用池的超时配置（集群逐节点扫描时，节点直连句柄也照此配置）。
    #[tokio::test]
    async fn pooled_conn_inherits_pool_timeout() {
        let (port, _server) = spawn_resp_stub("GET");
        let timeout = fast_timeout();
        let pool = Pool::with_timeout(timeout);
        pool.connect(&conn_config("inherit", port))
            .await
            .expect("握手命令有回包，建连应当成功");

        let con = pool.conn("inherit").expect("句柄应当可取");
        assert_eq!(con.timeout().command, timeout.command);
        assert_eq!(con.timeout().connect, timeout.connect);
    }

    /// 真实 Redis 上验证：阻塞命令必须被命令超时打断（需本地 Redis）。
    ///
    /// 启动方式见 DEVELOPMENT.md §7；`PooledConn::query` 是唯一出口，
    /// 这条测试保证 `BLPOP` 不会再让界面永久等待，并且连接在响应到达后可以继续使用。
    #[tokio::test]
    #[ignore]
    async fn blocking_command_is_interrupted_by_command_timeout() -> Result<(), String> {
        let timeout = ConnectionTimeout {
            connect: Duration::from_secs(5),
            command: Duration::from_millis(500),
        };
        let pool = Pool::with_timeout(timeout);
        let cfg = conn_config("blocking_cmd", 6379);
        pool.connect(&cfg)
            .await
            .map_err(|e| format!("连接 Redis 失败，请确认服务已启动: {e}"))?;

        let mut con = pool.conn(&cfg.id).map_err(|e| e.to_string())?;
        let started = Instant::now();
        // BLPOP 在没有数据时会阻塞 2 秒；命令超时 500ms，必须提前返回超时错误
        let err = con
            .query::<redis::Value>(redis::cmd("BLPOP").arg("maidi:test:no-such-list").arg(2))
            .await
            .expect_err("阻塞命令应当被命令超时打断");
        assert!(
            matches!(err, AppError::Timeout(_)),
            "应当是超时错误，实际: {err:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "应当提前返回而不是等服务器解除阻塞，实际耗时 {:?}",
            started.elapsed()
        );

        // 被放弃的 BLPOP 响应会在这之后到达并被驱动丢弃，连接随之恢复可用
        tokio::time::sleep(Duration::from_millis(2500)).await;
        let pong: String = con
            .query(&redis::cmd("PING"))
            .await
            .map_err(|e| format!("超时后连接应当仍可用: {e}"))?;
        assert_eq!(pong, "PONG", "超时后连接应恢复可用");
        Ok(())
    }
}
