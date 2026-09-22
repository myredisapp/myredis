/**
 * 与 Tauri 后端的唯一接触面。
 *
 * 前端不引 `@tauri-apps/api`（无构建步骤、无 node_modules），直接走 Tauri 注入的
 * `window.__TAURI_INTERNALS__`：命令用 `invoke`，后端推来的事件用 `plugin:event|listen`
 * 注册回调（回调 id 来自 `transformCallback`，与 tauri 2.x 的 core.js 一致）。
 *
 * 只有本文件知道这两个 API 的存在 —— 其余模块一律 `import { invoke } from './api.js'`，
 * 这样「哪个命令、在哪些地方被调用」始终是一处可查的。
 */

/** 调用后端命令；Tauri 环境未就绪时给出明确拒绝（而不是静默挂起） */
export function invoke(cmd, args) {
    if (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke) {
        return window.__TAURI_INTERNALS__.invoke(cmd, args);
    }
    return Promise.reject(new Error('Tauri 环境未就绪'));
}

/**
 * 注册后端事件监听（菜单项、实时监控等事件都由后端 emit）。
 * 拿不到 `transformCallback` 说明不在 Tauri 环境里（例如浏览器里直接打开），静默跳过。
 */
export function listenEvent(event, handler) {
    const internals = window.__TAURI_INTERNALS__;
    if (!internals || !internals.transformCallback) return;
    const id = internals.transformCallback(handler);
    invoke('plugin:event|listen', { event, target: { kind: 'Any' }, handler: id })
        .catch(err => console.warn('事件监听注册失败:', event, err));
}
