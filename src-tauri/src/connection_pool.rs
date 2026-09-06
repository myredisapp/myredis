//! # Redis 连接池
//!
//! 管理到 Redis 的活跃连接。支持**单机**与**集群**（Cluster）两种模式，
//! 并可标记连接为**只读**（在命令层拦截写命令）。
//!
//! 内部为每个连接保存一个 [`Connection`]，命令层通过 [`Pool::conn`] 获取，
//! 并在需要时用该配置重新建立连接执行命令。单机连接缓存一个
//! [`redis::aio::ConnectionManager`] 以便复用、免重建。

use std::collections::HashMap;
use std::sync::Mutex;

use redis::aio::{ConnectionLike, ConnectionManager};
use redis::cluster_async::ClusterConnection;
use redis::RedisFuture;

use crate::error::AppError;
use crate::models::{ConnType, Connection};

type ConnMap = HashMap<String, Entry>;

/// 可执行 Redis 命令的连接句柄（单机 or 集群）。
///
/// 单机与集群都实现了 [`redis::aio::ConnectionLike`]，命令层通过该 trait
/// 统一执行 `query_async`，无需关心底层是单机还是集群。
#[derive(Clone)]
pub enum Conn {
    /// 单机连接管理器（自动重连）
    Single(ConnectionManager),
    /// 集群连接
    Cluster(ClusterConnection),
}

impl ConnectionLike for Conn {
    fn req_packed_command<'a>(&'a mut self, cmd: &'a redis::Cmd) -> RedisFuture<'a, redis::Value> {
        Box::pin(async move {
            match self {
                Conn::Single(c) => c.req_packed_command(cmd).await,
                Conn::Cluster(c) => c.req_packed_command(cmd).await,
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
            }
        })
    }

    fn get_db(&self) -> i64 {
        match self {
            Conn::Single(c) => c.get_db(),
            Conn::Cluster(c) => c.get_db(),
        }
    }
}

/// 应用全局连接池，注入 Tauri 状态 `tauri::State<Pool>`。
pub struct Pool {
    inner: Mutex<ConnMap>,
}

impl Default for Pool {
    fn default() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
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
    /// 创建空连接池。
    pub fn new() -> Self {
        Self::default()
    }

    /// 建立到 `conn` 的 Redis 连接并缓存。
    ///
    /// # 注意
    /// - 单机使用 [`ConnectionManager`]（自动重连）。
    /// - 集群使用 [`ClusterConnection`]，句柄同样缓存复用。
    /// - 若连接为只读，会先发送 `READONLY` 命令（对主从/集群副本生效）。
    pub async fn connect(&self, conn: &Connection) -> Result<ConnInfo, AppError> {
        match conn.conn_type {
            ConnType::Single => {
                let url = conn.to_connection_url();
                let client = redis::Client::open(url).map_err(AppError::from)?;
                let manager = client
                    .get_connection_manager()
                    .await
                    .map_err(AppError::from)?;

                // 若只读，发送 READONLY（单机一般无副作用，但保留以便代理/哨兵环境也可用）
                if conn.is_readonly() {
                    let mut c = manager.clone();
                    let _: Result<(), redis::RedisError> =
                        redis::cmd("READONLY").query_async(&mut c).await;
                }

                self.inner.lock().unwrap().insert(
                    conn.id.clone(),
                    Entry {
                        conn: conn.clone(),
                        handle: Conn::Single(manager),
                    },
                );
            }
            ConnType::Cluster => {
                let url = conn.to_connection_url();
                let client =
                    redis::cluster::ClusterClient::new(vec![url]).map_err(AppError::from)?;
                let mut c = client
                    .get_async_connection()
                    .await
                    .map_err(AppError::from)?;
                if conn.is_readonly() {
                    let _: Result<(), redis::RedisError> =
                        redis::cmd("READONLY").query_async(&mut c).await;
                }
                self.inner.lock().unwrap().insert(
                    conn.id.clone(),
                    Entry {
                        conn: conn.clone(),
                        handle: Conn::Cluster(c),
                    },
                );
            }
        }

        Ok(ConnInfo {
            id: conn.id.clone(),
            name: conn.name.clone(),
        })
    }

    /// 获取可执行命令的连接句柄（单机 or 集群）。
    ///
    /// 命令层应使用该方法获取连接，再通过 [`redis::aio::ConnectionLike`] 的
    /// `query_async` 执行命令，无需关心底层是单机还是集群。
    pub fn conn(&self, id: &str) -> Result<Conn, AppError> {
        let guard = self.inner.lock().unwrap();
        let entry = guard
            .get(id)
            .ok_or_else(|| AppError::ConnectionNotFound(id.to_string()))?;
        Ok(entry.handle.clone())
    }

    /// 获取单机连接管理器句柄（仅单机且该连接以单机模式建立时才可用）。
    ///
    /// # Panics
    /// 当连接不存在时 panic —— 调用方应先用 [`Pool::manager`] 判空。
    pub fn manager(&self, id: &str) -> Result<ConnectionManager, AppError> {
        let guard = self.inner.lock().unwrap();
        let entry = guard
            .get(id)
            .ok_or_else(|| AppError::ConnectionNotFound(id.to_string()))?;
        match &entry.handle {
            Conn::Single(m) => Ok(m.clone()),
            Conn::Cluster(_) => Err(AppError::msg("该连接不是单机模式，或连接句柄不可用")),
        }
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
        let pong: String = redis::cmd("PING").query_async(&mut con).await?;
        Ok(pong)
    }

    /// 临时测试连接参数是否可用。
    ///
    /// 直接按 `conn` 建立 Redis 连接，执行一次 `PING` 后立即释放，
    /// 不写入连接池内部 map。
    pub async fn test(conn: &Connection) -> Result<String, AppError> {
        let timeout_cfg = crate::config::ConnectionTimeout::default();
        let connect_timeout = timeout_cfg.connect;
        let command_timeout = timeout_cfg.command;
        match conn.conn_type {
            ConnType::Single => {
                let url = conn.to_connection_url();
                let client = redis::Client::open(url).map_err(AppError::from)?;
                let future = client.get_multiplexed_async_connection();
                let mut con = tokio::time::timeout(connect_timeout, future)
                    .await
                    .map_err(|_| AppError::Timeout("连接超时".into()))?
                    .map_err(AppError::from)?;
                let pong: String =
                    tokio::time::timeout(command_timeout, redis::cmd("PING").query_async(&mut con))
                        .await
                        .map_err(|_| AppError::Timeout("PING 命令超时".into()))?
                        .map_err(AppError::from)?;
                Ok(pong)
            }
            ConnType::Cluster => {
                let url = conn.to_connection_url();
                let client =
                    redis::cluster::ClusterClient::new(vec![url]).map_err(AppError::from)?;
                let future = client.get_async_connection();
                let mut con = tokio::time::timeout(connect_timeout, future)
                    .await
                    .map_err(|_| AppError::Timeout("连接超时".into()))?
                    .map_err(AppError::from)?;
                let pong: String =
                    tokio::time::timeout(command_timeout, redis::cmd("PING").query_async(&mut con))
                        .await
                        .map_err(|_| AppError::Timeout("PING 命令超时".into()))?
                        .map_err(AppError::from)?;
                Ok(pong)
            }
        }
    }
}
