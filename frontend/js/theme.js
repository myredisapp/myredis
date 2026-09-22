/**
 * 主题切换：五款四季主题（春分 / 立夏 / 初秋 / 深秋 / 冬至）。
 * 只改 `html[data-theme]`（样式全在 styles.css 里），并把选择持久化到 localStorage；
 * 后端菜单栏的「主题」菜单也走同一套 applyTheme（见 main.js 的 menu:set-theme）。
 */

const THEME_KEY = 'mc_cache_theme';
const DEFAULT_THEME = 'autumn';

const themeBtn = document.getElementById('btnTheme');
const themeMenu = document.getElementById('themeMenu');

function applyTheme(name) {
    if (!name) name = DEFAULT_THEME;
    document.documentElement.setAttribute('data-theme', name);
}

function updateThemeMenu(name) {
    if (!name) name = DEFAULT_THEME;
    document.querySelectorAll('#themeMenu .theme-option').forEach(opt => {
        opt.classList.toggle('active', opt.getAttribute('data-theme-value') === name);
    });
}

/** 菜单栏切主题（后端事件）：换主题 + 持久化 + 同步菜单选中态 */
export function setThemeFromMenu(name) {
    applyTheme(name);
    try { localStorage.setItem(THEME_KEY, name); } catch (err) { /* 存不下就只影响下次启动 */ }
    updateThemeMenu(name);
}

export function initTheme() {
    themeBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        themeMenu.classList.toggle('open');
    });
    document.addEventListener('click', () => themeMenu.classList.remove('open'));

    themeMenu.addEventListener('click', (e) => {
        const opt = e.target.closest('.theme-option');
        if (!opt) return;
        const name = opt.getAttribute('data-theme-value') || DEFAULT_THEME;
        applyTheme(name);
        try { localStorage.setItem(THEME_KEY, name); } catch (err) { /* 同上 */ }
        updateThemeMenu(name);
        themeMenu.classList.remove('open');
    });

    // 恢复用户上次选择的主题
    try {
        const saved = localStorage.getItem(THEME_KEY) || DEFAULT_THEME;
        applyTheme(saved);
        updateThemeMenu(saved);
    } catch (e) {
        applyTheme(DEFAULT_THEME);
        updateThemeMenu(DEFAULT_THEME);
    }
}
