//! 应用菜单栏（macOS）。
//!
//! 菜单顺序：应用菜单 / Window / Settings / Help。
//!
//! 「为什么编辑项挂在应用菜单里」：macOS 上 `set_menu` 是整体替换菜单栏，而 ⌘C、⌘V、⌘A、⌘Z
//! 这类快捷键靠菜单项派发到响应链，菜单栏里没有对应菜单项时 WebView 的输入框和终端会失去
//! 复制粘贴能力。菜单栏不再单列 Edit，于是这些标准编辑项改挂在应用菜单内部：不展开菜单就
//! 看不到它们，快捷键照常生效。
//!
//! 菜单项只负责把点击转成前端事件（见 [`EVENT_SET_THEME`] / [`EVENT_CHECK_UPDATE`]），
//! 具体行为复用前端已有逻辑，避免主题、更新检查出现两套状态。

use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Wry};

/// 「设置 → 主题」的候选项。`id` 与前端 `data-theme` 取值一致，`label` 沿用主题名。
const THEMES: [(&str, &str); 5] = [
    ("spring", "春分"),
    ("summer", "立夏"),
    ("golden", "初秋"),
    ("autumn", "深秋"),
    ("winter", "冬至"),
];

/// 主题菜单项 id 前缀，完整形态为 `settings.theme.<name>`。
const THEME_ID_PREFIX: &str = "settings.theme.";
/// 「检查更新」菜单项 id。
const CHECK_UPDATE_ID: &str = "settings.check_update";

/// 前端事件：切换主题，payload 为主题名（与 `data-theme` 取值一致）。
const EVENT_SET_THEME: &str = "menu:set-theme";
/// 前端事件：触发一次更新检查。
const EVENT_CHECK_UPDATE: &str = "menu:check-update";

/// 构建菜单栏，交给 `tauri::Builder::menu` 使用。
///
/// 菜单项创建失败时向上抛出，让应用启动即报错，而不是静默退化成没有菜单栏。
pub fn build(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let app_menu = Submenu::with_items(
        app,
        "麦地缓存",
        true,
        &[
            &PredefinedMenuItem::about(app, None, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            // 编辑项：只为保留 ⌘Z / ⌘X / ⌘C / ⌘V / ⌘A 的快捷键派发，见模块注释
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;

    let window_menu = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;

    let settings_menu = Submenu::with_items(
        app,
        "Settings",
        true,
        &[
            &theme_menu(app)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(
                app,
                CHECK_UPDATE_ID,
                "Check for Updates…",
                true,
                None::<&str>,
            )?,
        ],
    )?;

    // Help 保持空菜单：条目留给后续的文档/反馈入口
    let help_items: [&dyn IsMenuItem<Wry>; 0] = [];
    let help_menu = Submenu::with_items(app, "Help", true, &help_items)?;

    Menu::with_items(app, &[&app_menu, &window_menu, &settings_menu, &help_menu])
}

/// 处理菜单点击，把菜单项 id 映射为前端事件。
///
/// 非本模块定义的菜单项（如预定义项）直接忽略，交给系统处理。
pub fn on_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    let id = event.id().as_ref();

    let emitted = match id.strip_prefix(THEME_ID_PREFIX) {
        Some(theme) => app.emit(EVENT_SET_THEME, theme),
        None if id == CHECK_UPDATE_ID => app.emit(EVENT_CHECK_UPDATE, ()),
        None => return,
    };

    if let Err(err) = emitted {
        eprintln!("派发菜单事件失败（{id}）：{err}");
    }
}

/// 构建「设置 → 主题」子菜单。
fn theme_menu(app: &AppHandle) -> tauri::Result<Submenu<Wry>> {
    let items = THEMES
        .iter()
        .map(|(name, label)| {
            MenuItem::with_id(
                app,
                format!("{THEME_ID_PREFIX}{name}"),
                *label,
                true,
                None::<&str>,
            )
        })
        .collect::<tauri::Result<Vec<_>>>()?;
    let item_refs: Vec<&dyn IsMenuItem<Wry>> = items
        .iter()
        .map(|item| item as &dyn IsMenuItem<Wry>)
        .collect();

    Submenu::with_items(app, "Theme", true, &item_refs)
}
