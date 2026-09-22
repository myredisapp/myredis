/**
 * 导入 / 导出：连接配置与 Key（都是「选文件 / 存文件 + 走一次后端命令」的流程）。
 *
 * - 连接配置：导出走 `export_connections`（可选是否带密码），导入走 `import_connections`（同 id 覆盖）。
 * - Key：导出走 `export_keys`（不传 keys，由后端全量枚举当前 keyspace），
 *   导入走 `import_keys`。
 * 文件选择用隐藏的 `<input type="file">`，只在首次点击时创建一次。
 */
import { invoke } from './api.js';
import { downloadJson, todayStamp } from './util.js';
import { getConfirmOption, showConfirm, showToast } from './ui.js';
import { getConnections, getCurrentConn } from './state.js';
import { appendTerminal } from './terminal.js';
import { loadConnections } from './connections.js';
import { loadKeys } from './keys.js';

// ---------- 连接配置 ----------

// 导出全部连接配置为 JSON 文件（可选是否带密码）
function exportConnections() {
    const CONNECTIONS = getConnections();
    if (!CONNECTIONS.length) {
        showToast('暂无连接配置可导出', 'info');
        return;
    }
    showConfirm(`将把 ${CONNECTIONS.length} 个连接配置导出为 JSON 文件，可在另一台机器导入。`, {
        title: '导出连接配置',
        okText: '导出',
        danger: false,
        checkbox: { label: '包含密码（明文保存，请妥善保管）', checked: true },
    }).then((ok) => {
        if (!ok) return;
        const includePasswords = getConfirmOption();
        invoke('export_connections', { includePasswords })
            .then((res) => {
                if (!res || !res.count) {
                    showToast('没有可导出的连接配置', 'info');
                    return;
                }
                downloadJson(`maidi-connections-${todayStamp()}.json`, res.content);
                showToast(`已导出 ${res.count} 个连接配置`, 'success');
                appendTerminal(`EXPORT ${res.count} connections → OK${includePasswords ? '（含密码）' : ''}`, 'ok');
            })
            .catch(err => {
                showToast('导出连接配置失败: ' + err, 'error');
                appendTerminal(`导出连接配置失败: ${err}`, 'error');
            });
    });
}

// 从 JSON 文件导入连接配置（按 id 匹配，同 id 覆盖）
let connImportFileInput = null;
function importConnections() {
    if (!connImportFileInput) {
        connImportFileInput = document.createElement('input');
        connImportFileInput.type = 'file';
        connImportFileInput.accept = '.json,application/json';
        connImportFileInput.style.display = 'none';
        document.body.appendChild(connImportFileInput);
        connImportFileInput.addEventListener('change', () => {
            const file = connImportFileInput.files && connImportFileInput.files[0];
            connImportFileInput.value = '';
            if (!file) return;
            showConfirm(`将从「${file.name}」导入连接配置。已存在同 id 的连接会被覆盖。继续吗？`, {
                title: '导入连接配置',
                okText: '导入',
            }).then((ok) => {
                if (!ok) return;
                const reader = new FileReader();
                reader.onload = () => {
                    invoke('import_connections', { content: String(reader.result), overwrite: true })
                        .then((res) => {
                            const failed = res.failed ? res.failed.length : 0;
                            const msg = `导入完成：新增/覆盖 ${res.imported}，跳过 ${res.skipped}，失败 ${failed}`;
                            showToast(msg, failed > 0 ? 'error' : 'success');
                            appendTerminal(`IMPORT → ${msg}`, failed > 0 ? 'error' : 'ok');
                            if (failed > 0) {
                                res.failed.slice(0, 5).forEach(f => appendTerminal(`  失败 ${f.name}: ${f.error}`, 'error'));
                            }
                            return loadConnections();
                        })
                        .catch(err => {
                            showToast('导入连接配置失败: ' + err, 'error');
                            appendTerminal(`导入连接配置失败: ${err}`, 'error');
                        });
                };
                reader.readAsText(file);
            });
        });
    }
    connImportFileInput.click();
}

// ---------- Key ----------

let importFileInput = null;
function importKeys() {
    const conn = getCurrentConn();
    if (!conn || !conn.online) {
        showToast('请先连接 Redis 服务器', 'error');
        return;
    }
    if (conn.readonly) {
        showToast('只读连接，禁止写操作', 'error');
        return;
    }
    if (!importFileInput) {
        importFileInput = document.createElement('input');
        importFileInput.type = 'file';
        importFileInput.accept = '.json,application/json';
        importFileInput.style.display = 'none';
        document.body.appendChild(importFileInput);
        importFileInput.addEventListener('change', () => {
            const file = importFileInput.files && importFileInput.files[0];
            importFileInput.value = '';
            if (!file) return;
            showConfirm(`将从「${file.name}」导入 Key，已存在的同名 Key 将被覆盖。继续吗？`, { title: '导入 Key', okText: '导入' }).then((ok) => {
                if (!ok) return;
                const reader = new FileReader();
                reader.onload = () => {
                    invoke('import_keys', { connId: getCurrentConn().id, content: String(reader.result), overwrite: true })
                        .then((res) => {
                            const failed = res.failed ? res.failed.length : 0;
                            const msg = `导入完成：成功 ${res.imported}，跳过 ${res.skipped}，失败 ${failed}`;
                            showToast(msg, failed > 0 ? 'error' : 'success');
                            appendTerminal(`IMPORT → ${msg}`, failed > 0 ? 'error' : 'ok');
                            if (failed > 0) {
                                res.failed.slice(0, 5).forEach(f => appendTerminal(`  失败 ${f.key}: ${f.error}`, 'error'));
                            }
                            loadKeys();
                        })
                        .catch((err) => {
                            showToast('导入失败: ' + err, 'error');
                            appendTerminal(`IMPORT 失败: ${err}`, 'error');
                        });
                };
                reader.readAsText(file);
            });
        });
    }
    importFileInput.click();
}

function exportKeys() {
    const conn = getCurrentConn();
    if (!conn || !conn.online) {
        showToast('请先连接 Redis 服务器', 'error');
        return;
    }
    const btn = document.getElementById('btnExport');
    btn.disabled = true;
    // 不传 keys：由后端全量枚举当前 keyspace。分页浏览时前端只持有已加载的
    // 那部分 key，拿它当导出范围会漏数据。
    invoke('export_keys', { connId: conn.id, keys: null })
        .then((res) => {
            const count = (res && res.count) || 0;
            if (!count) {
                showToast('当前数据库没有可导出的 Key', 'info');
                return;
            }
            downloadJson(`maidi-export-${todayStamp()}.json`, res.content);
            showToast(`已导出 ${count} 个 Key`, 'success');
            appendTerminal(`EXPORT ${count} keys → OK`, 'ok');
        })
        .catch((err) => {
            showToast('导出失败: ' + err, 'error');
            appendTerminal(`EXPORT 失败: ${err}`, 'error');
        })
        .finally(() => {
            btn.disabled = false;
        });
}

export function initImportExport() {
    document.getElementById('btnImportConnections').addEventListener('click', importConnections);
    document.getElementById('btnExportConnections').addEventListener('click', exportConnections);
    document.getElementById('btnImport').addEventListener('click', importKeys);
    document.getElementById('btnExport').addEventListener('click', exportKeys);
}
