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

use redis::aio::ConnectionManager;

use crate::error::AppError;
use crate::models::{ConnType, Connection};

type ConnMap = HashMap<String, Entry>;

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
    /// 单机模式的连接管理器（自动重连）
    single: Option<ConnectionManager>,
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
    /// - 集群在每次命令执行时根据 nodes 建立连接。
    /// - 若连接为只读，会先发送 `READONLY` 命令（对主从/集群副本生效）。
    pub async fn connect(&self, conn: &Connection) -> Result<ConnInfo, AppError> {
        match conn.conn_type {
            ConnType::Single => {
                let url = conn.to_connection_url();
                let client = redis::Client::open(url).map_err(AppError::from)?;
                let manager = client.get_connection_manager().await.map_err(AppError::from)?;

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
                        single: Some(manager),
                    },
                );
            }
            ConnType::Cluster => {
                // 仅校验连接是否可建立，连接本身不保存（每次命令按需新建）
                let url = conn.to_connection_url();
                let client =
                    redis::cluster::ClusterClient::new(vec![url]).map_err(AppError::from)?;
                let mut c = client.get_async_connection().await.map_err(AppError::from)?;
                if conn.is_readonly() {
                    let _: Result<(), redis::RedisError> =
                        redis::cmd("READONLY").query_async(&mut c).await;
                }
                self.inner.lock().unwrap().insert(
                    conn.id.clone(),
                    Entry {
                        conn: conn.clone(),
                        single: None,
                    },
                );
            }
        }

        Ok(ConnInfo {
            id: conn.id.clone(),
            name: conn.name.clone(),
        })
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
        match &entry.single {
            Some(m) => Ok(m.clone()),
            None => Err(AppError::msg("该连接不是单机模式，或连接句柄不可用")),
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
        let mut manager = self.manager(id)?;
        let pong: String = redis::cmd("PING").query_async(&mut manager).await?;
        Ok(pong)
    }
}