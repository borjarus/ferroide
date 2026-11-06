//! The application menu, built from the action catalog.
//!
//! On macOS it becomes the native global menu bar; on Windows and Linux the
//! same menus are drawn in the window by GPUI Kit's `AppMenuBar` (or behind
//! the hamburger button in compact mode). Labels are localized, so the menus
//! are rebuilt whenever the language or keymap changes.

use ferro_ui::tr;
use gpui_kit::{App, Menu, MenuItem, base::GlobalState};

use crate::{actions, app_state::AppState};

fn item(id: &str, cx: &App) -> Option<MenuItem> {
    let descriptor = ferro_keymap::ACTIONS.iter().find(|a| a.id == id)?;
    let action = actions::build(id, cx)?;
    Some(MenuItem::Action {
        name: tr(descriptor.title_key),
        action,
        os_action: None,
        checked: false,
        disabled: false,
    })
}

fn checked(id: &str, is_checked: bool, cx: &App) -> Option<MenuItem> {
    item(id, cx).map(|item| match item {
        MenuItem::Action {
            name,
            action,
            os_action,
            disabled,
            ..
        } => MenuItem::Action {
            name,
            action,
            os_action,
            checked: is_checked,
            disabled,
        },
        other => other,
    })
}

fn menu(name_key: &str, items: Vec<Option<MenuItem>>) -> Menu {
    Menu::new(tr(name_key)).items(items.into_iter().flatten())
}

fn sep() -> Option<MenuItem> {
    Some(MenuItem::separator())
}

/// Build the menus for the current language and settings.
pub fn build(cx: &App) -> Vec<Menu> {
    let settings = AppState::settings(cx);
    let theme = settings.appearance.theme.as_str();
    let locale = settings.locale;
    let keymap = settings.keymap;
    use ferro_settings::themes;

    let appearance = menu(
        "menu-appearance",
        vec![
            checked("ferro::ThemeFerroDark", theme == themes::FERRO_DARK, cx),
            checked("ferro::ThemeFerroLight", theme == themes::FERRO_LIGHT, cx),
            checked(
                "ferro::ThemeHighContrast",
                theme == themes::HIGH_CONTRAST,
                cx,
            ),
            sep(),
            checked("ferro::ToggleCompactMode", settings.appearance.compact, cx),
            item("ferro::ZoomInUi", cx),
            item("ferro::ZoomOutUi", cx),
            item("ferro::ResetUiZoom", cx),
        ],
    );
    let language = menu(
        "menu-language",
        vec![
            checked(
                "ferro::LanguageEnglish",
                locale == ferro_i18n::Locale::En,
                cx,
            ),
            checked(
                "ferro::LanguagePolish",
                locale == ferro_i18n::Locale::Pl,
                cx,
            ),
        ],
    );
    let keymaps = menu(
        "menu-keymap",
        vec![
            checked(
                "ferro::KeymapIntelliJ",
                keymap == ferro_keymap::KeymapPreset::IntelliJ,
                cx,
            ),
            checked(
                "ferro::KeymapVsCode",
                keymap == ferro_keymap::KeymapPreset::VsCode,
                cx,
            ),
        ],
    );
    let tool_windows = menu(
        "menu-tool-windows",
        crate::tool_window::ToolWindowKind::ALL
            .iter()
            .map(|kind| item(kind.action_id(), cx))
            .collect(),
    );

    vec![
        menu(
            "menu-file",
            vec![
                item("ferro::NewProject", cx),
                item("ferro::OpenProject", cx),
                item("ferro::GetFromVcs", cx),
                sep(),
                item("ferro::NewWindow", cx),
                item("ferro::CloseProject", cx),
                sep(),
                item("ferro::OpenSettings", cx),
                sep(),
                item("ferro::Quit", cx),
            ],
        ),
        Menu::new(tr("menu-view")).items(
            [
                Some(MenuItem::submenu(tool_windows)),
                item("ferro::HideAllToolWindows", cx),
                item("ferro::MaximizeToolWindow", cx),
                item("ferro::ResetLayout", cx),
                sep(),
                Some(MenuItem::submenu(appearance)),
                Some(MenuItem::submenu(language)),
                Some(MenuItem::submenu(keymaps)),
                sep(),
                item("ferro::FindAction", cx),
            ]
            .into_iter()
            .flatten(),
        ),
        menu(
            "menu-window",
            vec![
                item("ferro::SplitRight", cx),
                item("ferro::SplitDown", cx),
                sep(),
                item("ferro::PinTab", cx),
                item("ferro::NextTab", cx),
                item("ferro::PreviousTab", cx),
                sep(),
                item("ferro::CloseTab", cx),
                item("ferro::CloseOtherTabs", cx),
                item("ferro::CloseAllTabs", cx),
            ],
        ),
        menu(
            "menu-run",
            vec![
                item("ferro::Run", cx),
                item("ferro::Debug", cx),
                item("ferro::Stop", cx),
            ],
        ),
        menu(
            "menu-help",
            vec![
                item("ferro::FindAction", cx),
                item("ferro::ShowWelcome", cx),
                sep(),
                item("ferro::About", cx),
            ],
        ),
    ]
}

/// Install the menus: the native menu bar on macOS, and the copy GPUI Kit's
/// in-window `AppMenuBar` reads everywhere.
pub fn install(cx: &mut App) {
    let menus = build(cx);
    let owned = build(cx).into_iter().map(Menu::owned).collect();
    GlobalState::global_mut(cx).set_app_menus(owned);
    cx.set_menus(menus);
    cx.refresh_windows();
}
