//! # 连接管理命令
//!
//! 提供连接配置的增删查改、导入导出，以及建立/断开 Redis 连接的能力。

use serde::Serialize;
use tauri::State;

use crate::connection_pool::{ConnInfo, Pool};
use crate::models::Connection;
use crate::AppState;

/// 建立到 Redis 的连接。
#[tauri::command]
pub async fn connect(pool: State<'_, Pool>, conn: Connection) -> Result<ConnInfo, String> {
    pool.connect(&conn).await.map_err(|e| e.to_string())
}

/// 断开一个 Redis 连接。
#[tauri::command]
pub async fn disconnect(pool: State<'_, Pool>, conn_id: String) -> Result<bool, String> {
    Ok(pool.disconnect(&conn_id))
}

/// 列出所有已保存的连接配置。
///
/// 文件读写走阻塞线程池（[`crate::storage::ConnectionRepo::load_async`]），不卡 async 工作线程。
#[tauri::command]
pub async fn list_connections(state: State<'_, AppState>) -> Result<Vec<Connection>, String> {
    let repo = state.repo();
    repo.load_async().await.map_err(|e| e.to_string())
}

/// 保存（新增或更新）一个连接配置，持久化到本地。
///
/// 不受支持的协议前缀（如未开 TLS 的 `rediss://`）在这里就拦下；已开 TLS 的
/// `rediss://host` 剥掉前缀后保存，避免存下一条带重复协议的 host。
#[tauri::command]
pub async fn save_connection(
    state: State<'_, AppState>,
    mut conn: Connection,
) -> Result<(), String> {
    conn.host = conn.check_supported_scheme().map_err(|e| e.to_string())?;
    let repo = state.repo();
    let mut all = repo.load_async().await.map_err(|e| e.to_string())?;
    if let Some(existing) = all.iter_mut().find(|c| c.id == conn.id) {
        *existing = conn;
    } else {
        all.push(conn);
    }
    repo.save_all_async(&all).await.map_err(|e| e.to_string())
}

/// 删除一个连接配置，返回是否删除成功。
///
/// 密钥链里对应的密码条目一并删除（尽力而为：密钥链不可用时无条目可删）。
#[tauri::command]
pub async fn delete_connection(
    state: State<'_, AppState>,
    conn_id: String,
) -> Result<bool, String> {
    let repo = state.repo();
    let mut all = repo.load_async().await.map_err(|e| e.to_string())?;
    let before = all.len();
    all.retain(|c| c.id != conn_id);
    repo.save_all_async(&all).await.map_err(|e| e.to_string())?;
    if all.len() != before {
        repo.delete_password(&conn_id);
    }
    Ok(all.len() != before)
}

/// 测试连接参数是否可用（不保存、不缓存连接）。
///
/// 建连与 `PING` 分别受连接池的超时配置约束（见 [`crate::config::ConnectionTimeout`]）。
#[tauri::command]
pub async fn test_connection(pool: State<'_, Pool>, conn: Connection) -> Result<String, String> {
    pool.test(&conn).await.map_err(|e| e.to_string())
}

// ---------- 连接配置的导入 / 导出 ----------

/// 连接配置导出文档的格式版本（供以后兼容旧文件用）。
const EXPORT_VERSION: u32 = 1;

/// 导出结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionExport {
    /// 导出文档的 JSON 文本
    pub content: String,
    /// 导出的连接数
    pub count: usize,
}

/// 单条连接导入失败的原因。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionImportFailure {
    /// 连接名（解析失败时回退到 id 或条目序号）
    pub name: String,
    /// 失败原因
    pub error: String,
}

/// 连接配置导入结果。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionImportResult {
    /// 成功写入的连接数（新增 + 覆盖）
    pub imported: u64,
    /// 因同 id 已存在且不覆盖而跳过的连接数
    pub skipped: u64,
    /// 解析或校验失败的条目
    pub failed: Vec<ConnectionImportFailure>,
}

/// 导出全部连接配置为 JSON 文档。
///
/// `include_passwords` 为 `false` 时导出文档不含密码字段；为 `true` 时密码以**明文**
/// 写入（与 `connections.json` 的落盘方式一致，见 DEVELOPMENT.md §4 风险），
/// 因此前端在导出前会就此提示用户。
#[tauri::command]
pub async fn export_connections(
    state: State<'_, AppState>,
    include_passwords: bool,
) -> Result<ConnectionExport, String> {
    let repo = state.repo();
    let conns = repo.load_async().await.map_err(|e| e.to_string())?;
    let doc = build_export_doc(&conns, include_passwords);
    let content = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    Ok(ConnectionExport {
        content,
        count: conns.len(),
    })
}

/// 从 JSON 文档导入连接配置。
///
/// 按 `id` 匹配已有连接：`overwrite` 为 `true` 时覆盖，否则跳过。单条解析失败或字段
/// 不合法只记入失败列表，不影响其它条目。
#[tauri::command]
pub async fn import_connections(
    state: State<'_, AppState>,
    content: String,
    overwrite: bool,
) -> Result<ConnectionImportResult, String> {
    let (incoming, failed) = parse_import_doc(&content)?;
    let repo = state.repo();
    let existing = repo.load_async().await.map_err(|e| e.to_string())?;
    let (merged, imported, skipped) = merge_connections(&existing, incoming, overwrite);
    repo.save_all_async(&merged)
        .await
        .map_err(|e| e.to_string())?;
    Ok(ConnectionImportResult {
        imported,
        skipped,
        failed,
    })
}

/// 把连接列表组装成导出文档。
///
/// `include_passwords` 为 `false` 时把密码置空 —— `Connection::password` 带
/// `skip_serializing_if = "Option::is_none"`，因此导出文档里不会出现该字段。
fn build_export_doc(conns: &[Connection], include_passwords: bool) -> serde_json::Value {
    let items: Vec<Connection> = conns
        .iter()
        .map(|c| {
            let mut copy = c.clone();
            if !include_passwords {
                copy.password = None;
            }
            copy
        })
        .collect();
    serde_json::json!({
        "app": "maidi-cache",
        "version": EXPORT_VERSION,
        "exportedAt": crate::commands::import_export::chrono_like_now(),
        "connections": items,
    })
}

/// 解析导入文档中的连接列表。
///
/// 兼容 `{ "connections": [...] }` 与裸数组两种形态；单条解析失败或字段不合法的条目
/// 收集到失败列表里，不影响其它条目。
fn parse_import_doc(
    content: &str,
) -> Result<(Vec<Connection>, Vec<ConnectionImportFailure>), String> {
    let doc: serde_json::Value =
        serde_json::from_str(content).map_err(|e| format!("导入文件不是有效的 JSON: {e}"))?;
    let raw = if doc.is_array() {
        doc.as_array().cloned().unwrap_or_default()
    } else {
        doc.get("connections")
            .and_then(|v| v.as_array().cloned())
            .ok_or("导入文件格式不正确：缺少 connections 数组")?
    };

    let mut conns = Vec::with_capacity(raw.len());
    let mut failed = Vec::new();
    for (idx, item) in raw.into_iter().enumerate() {
        // 失败信息要能定位到条目：优先用名称，其次 id，最后用序号
        let label = item
            .get("name")
            .and_then(|v| v.as_str())
            .or_else(|| item.get("id").and_then(|v| v.as_str()))
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("#{idx}"));
        match serde_json::from_value::<Connection>(item) {
            Ok(mut conn) => match validate_connection(&conn) {
                Ok(host) => {
                    conn.host = host;
                    conns.push(conn);
                }
                Err(msg) => failed.push(ConnectionImportFailure {
                    name: label,
                    error: msg,
                }),
            },
            Err(e) => failed.push(ConnectionImportFailure {
                name: label,
                error: format!("字段缺失或类型错误: {e}"),
            }),
        }
    }
    Ok((conns, failed))
}

/// 校验一条导入的连接配置是否可用，返回剥掉协议前缀后的主机名。
///
/// 协议前缀同样要校验：导入文件里可能带着 `rediss://` 之类地址 —— 未开 TLS 的
/// 记入失败列表比存下来、连接时再报错更早也更清楚；已开 TLS 的剥掉前缀入库。
fn validate_connection(conn: &Connection) -> Result<String, String> {
    if conn.id.trim().is_empty() {
        return Err("缺少 id".into());
    }
    if conn.name.trim().is_empty() {
        return Err("缺少名称".into());
    }
    if conn.host.trim().is_empty() {
        return Err("缺少主机地址".into());
    }
    if conn.port == 0 {
        return Err("端口不合法".into());
    }
    conn.check_supported_scheme().map_err(|e| e.to_string())
}

/// 把导入的连接合并进现有列表。
///
/// 按 `id` 匹配：命中时 `overwrite` 决定覆盖还是跳过，未命中则追加。
/// 返回（合并后的列表, 写入数, 跳过数）。
fn merge_connections(
    existing: &[Connection],
    incoming: Vec<Connection>,
    overwrite: bool,
) -> (Vec<Connection>, u64, u64) {
    let mut merged = existing.to_vec();
    let mut imported = 0u64;
    let mut skipped = 0u64;
    for conn in incoming {
        match merged.iter_mut().find(|c| c.id == conn.id) {
            Some(occupied) => {
                if overwrite {
                    *occupied = conn;
                    imported += 1;
                } else {
                    skipped += 1;
                }
            }
            None => {
                merged.push(conn);
                imported += 1;
            }
        }
    }
    (merged, imported, skipped)
}

#[cfg(test)]
mod tests {
    use super::{build_export_doc, merge_connections, parse_import_doc};
    use crate::models::{ConnType, Connection};

    fn sample(id: &str, name: &str, password: Option<&str>) -> Connection {
        Connection {
            id: id.into(),
            name: name.into(),
            host: "127.0.0.1".into(),
            port: 6379,
            conn_type: ConnType::Single,
            readonly: false,
            separator: ":".into(),
            db: 0,
            username: Some("default".into()),
            password: password.map(|p| p.to_string()),
            tls: false,
            tls_insecure: false,
            connect_timeout_secs: None,
            command_timeout_secs: None,
        }
    }

    #[test]
    fn export_doc_writes_metadata_and_connections() {
        let doc = build_export_doc(&[sample("c1", "本地", Some("secret"))], true);
        assert_eq!(doc["app"], "maidi-cache");
        assert_eq!(doc["version"], 1);
        assert!(doc["exportedAt"].as_str().unwrap_or("").ends_with('Z'));
        assert_eq!(doc["connections"].as_array().map(|a| a.len()), Some(1));
        assert_eq!(doc["connections"][0]["password"], "secret");
        assert_eq!(doc["connections"][0]["host"], "127.0.0.1");
        assert_eq!(doc["connections"][0]["type"], "single");
    }

    #[test]
    fn export_doc_can_drop_passwords() {
        let doc = build_export_doc(&[sample("c1", "本地", Some("secret"))], false);
        // 密码字段整体消失，而不是变成空串
        assert!(doc["connections"][0].get("password").is_none());
        assert_eq!(doc["connections"][0]["username"], "default");
    }

    #[test]
    fn export_doc_roundtrips_through_parse() {
        let original = vec![
            sample("c1", "本地", Some("p@ss:w/rd")),
            sample("c2", "线上", None),
        ];
        let doc = build_export_doc(&original, true);
        let (parsed, failed) = parse_import_doc(&doc.to_string()).expect("导出文档应能被导入解析");
        assert!(failed.is_empty(), "不应有失败条目: {failed:?}");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].id, "c1");
        assert_eq!(parsed[0].password.as_deref(), Some("p@ss:w/rd"));
        assert_eq!(parsed[0].host, "127.0.0.1");
        assert_eq!(parsed[1].id, "c2");
        assert!(parsed[1].password.is_none());
    }

    #[test]
    fn parse_accepts_bare_array_and_rejects_unknown_shape() {
        let bare = r#"[{"id":"c1","name":"n","host":"h","port":6379,"type":"single"}]"#;
        let (conns, failed) = parse_import_doc(bare).expect("裸数组应被接受");
        assert_eq!(conns.len(), 1);
        assert!(failed.is_empty());

        assert!(parse_import_doc(r#"{"foo":[]}"#).is_err());
        assert!(parse_import_doc("not json").is_err());
    }

    #[test]
    fn parse_collects_bad_entries_without_aborting() {
        let content = r#"{"connections":[
            {"id":"ok","name":"好的","host":"127.0.0.1","port":6379,"type":"single"},
            {"id":"no-host","name":"缺主机","port":6379,"type":"single"},
            {"id":"bad-port","name":"坏端口","host":"127.0.0.1","port":"6379x","type":"single"},
            {"id":"","name":"空 id","host":"127.0.0.1","port":6379,"type":"single"}
        ]}"#;
        let (conns, failed) = parse_import_doc(content).expect("文档整体可解析");
        assert_eq!(conns.len(), 1, "只有第一条是合法的: {conns:?}");
        assert_eq!(conns[0].id, "ok");
        assert_eq!(failed.len(), 3, "三条坏数据都应记账: {failed:?}");
        // 失败信息带条目名称，方便定位
        assert!(failed.iter().any(|f| f.name == "缺主机"));
        assert!(failed.iter().any(|f| f.name == "坏端口"));
        assert!(failed.iter().any(|f| f.name == "空 id"));
    }

    /// 导入文件里的 TLS 地址（`rediss://`）在未勾选 TLS 时记入失败列表并给出友好提示 ——
    /// 而不是存下来、等用户连接时才报一条看不懂的错误；勾选 TLS 的条目正常导入。
    #[test]
    fn parse_rejects_tls_hosts_without_tls_flag() {
        let content = r#"{"connections":[
            {"id":"tls","name":"加密连接","host":"rediss://redis.example.com","port":6379,"type":"single"},
            {"id":"tls-on","name":"加密连接2","host":"rediss://redis.example.com","port":6379,"type":"single","tls":true},
            {"id":"plain","name":"明文连接","host":"redis.example.com","port":6379,"type":"single"}
        ]}"#;
        let (conns, failed) = parse_import_doc(content).expect("文档整体可解析");
        assert_eq!(conns.len(), 2, "勾选 TLS 与明文两条可以导入: {conns:?}");
        assert_eq!(conns[0].id, "tls-on");
        assert_eq!(conns[0].host, "redis.example.com", "前缀应在导入时被剥掉");
        assert_eq!(conns[1].id, "plain");
        assert_eq!(failed.len(), 1, "未开 TLS 的条目应记入失败列表: {failed:?}");
        assert_eq!(failed[0].name, "加密连接");
        assert!(
            failed[0].error.contains("TLS 加密"),
            "提示应引导打开 TLS 开关: {}",
            failed[0].error
        );
    }

    /// 校验函数对协议前缀的判定：未开 TLS 拦下并提示，粘贴的 `redis://` 给出填法提示，
    /// 勾选 TLS 的 `rediss://` 放行并剥掉前缀，普通主机名放行。
    #[test]
    fn validate_connection_checks_scheme() {
        assert!(super::validate_connection(&sample("c1", "本地", None)).is_ok());

        let mut tls = sample("c2", "加密", None);
        tls.host = "rediss://redis.example.com".into();
        let err = super::validate_connection(&tls).expect_err("未开 TLS 不应通过校验");
        assert!(err.contains("TLS 加密"), "{err}");

        tls.tls = true;
        assert!(super::validate_connection(&tls).is_ok());

        let mut pasted = sample("c3", "粘贴的 URL", None);
        pasted.host = "redis://127.0.0.1".into();
        let err = super::validate_connection(&pasted).expect_err("带协议前缀不应通过校验");
        assert!(err.contains("只需填主机名"), "{err}");
    }

    #[test]
    fn merge_appends_new_and_overwrites_or_skips_same_id() {
        let existing = vec![sample("c1", "旧名", None), sample("c2", "保留", None)];

        // 覆盖模式：同 id 被替换，新 id 追加
        let (merged, imported, skipped) = merge_connections(
            &existing,
            vec![sample("c1", "新名", None), sample("c3", "新增", None)],
            true,
        );
        assert_eq!((imported, skipped), (2, 0));
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].name, "新名");
        assert_eq!(merged[1].name, "保留");
        assert_eq!(merged[2].id, "c3");
        // 原列表不被就地修改
        assert_eq!(existing[0].name, "旧名");

        // 跳过模式：同 id 原样保留
        let (merged, imported, skipped) = merge_connections(
            &existing,
            vec![sample("c1", "新名", None), sample("c3", "新增", None)],
            false,
        );
        assert_eq!((imported, skipped), (1, 1));
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].name, "旧名");
    }
}
