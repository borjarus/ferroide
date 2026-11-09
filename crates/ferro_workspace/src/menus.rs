//! The application menu, built from the action catalog.
//!
//! On macOS it becomes the native global menu bar; on Windows and Linux the
//! same menus open from the hamburger button in the title bar (as in Zed). Labels are localized, so the menus
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
        ferro_keymap::KeymapPreset::ALL
            .iter()
            .map(|preset| checked(preset.action_id(), keymap.base == *preset, cx))
            .chain([
                sep(),
                checked("ferro::ToggleVimMode", keymap.vim_mode, cx),
                item("ferro::OpenKeymapSettings", cx),
            ])
            .collect(),
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
                item("ferro::NewFile", cx),
                item("ferro::NewProject", cx),
                item("ferro::OpenProject", cx),
                item("ferro::GetFromVcs", cx),
                sep(),
                item("ferro::NewWindow", cx),
                item("ferro::CloseProject", cx),
                sep(),
                item("ferro::OpenSettings", cx),
                item("ferro::OpenKeymapSettings", cx),
                item("ferro::OpenSettingsFile", cx),
                sep(),
                item("ferro::Quit", cx),
            ],
        ),
        menu(
            "menu-navigate",
            vec![
                item("ferro::QuickOpen", cx),
                item("ferro::CommandPalette", cx),
                item("ferro::GoToSymbolInFile", cx),
                item("ferro::GoToSymbolInWorkspace", cx),
                item("ferro::GoToLine", cx),
                sep(),
                item("ferro::GoBack", cx),
                item("ferro::GoForward", cx),
                sep(),
                item("ferro::FindInFiles", cx),
                item("ferro::ReplaceInFiles", cx),
                sep(),
                item("ferro::SelectOpenedFile", cx),
                item("ferro::NextProblem", cx),
                item("ferro::PreviousProblem", cx),
            ],
        ),
        menu(
            "menu-code",
            vec![
                item("ferro::TriggerCompletion", cx),
                item("ferro::QuickFix", cx),
                item("ferro::QuickDocumentation", cx),
                item("ferro::ParameterInfo", cx),
                sep(),
                item("ferro::RenameSymbol", cx),
                item("ferro::ToggleBreakpoint", cx),
            ],
        ),
        Menu::new(tr("menu-view")).items(
            [
                Some(MenuItem::submenu(tool_windows)),
                item("ferro::ToggleLeftDock", cx),
                item("ferro::ToggleBottomDock", cx),
                item("ferro::ToggleRightDock", cx),
                item("ferro::HideAllToolWindows", cx),
                item("ferro::MaximizeToolWindow", cx),
                item("ferro::ResetLayout", cx),
                sep(),
                Some(MenuItem::submenu(appearance)),
                Some(MenuItem::submenu(language)),
                Some(MenuItem::submenu(keymaps)),
                sep(),
                item("ferro::CommandPalette", cx),
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
                item("ferro::ReopenClosedTab", cx),
            ],
        ),
        menu(
            "menu-run",
            vec![
                item("ferro::Run", cx),
                item("ferro::Debug", cx),
                item("ferro::Stop", cx),
                sep(),
                item("ferro::EditRunConfigurations", cx),
            ],
        ),
        menu(
            "menu-vcs",
            vec![
                item("ferro::ToggleCommit", cx),
                item("ferro::ToggleGit", cx),
                sep(),
                item("ferro::ShowDiff", cx),
                item("ferro::ResolveConflicts", cx),
            ],
        ),
        menu(
            "menu-help",
            vec![
                item("ferro::CommandPalette", cx),
                item("ferro::ShowWelcome", cx),
                sep(),
                item("ferro::About", cx),
            ],
        ),
    ]
}

/// Install the menus: the native menu bar on macOS, and the copy GPUI Kit's
/// menu components read everywhere.
pub fn install(cx: &mut App) {
    let menus = build(cx);
    let owned = build(cx).into_iter().map(Menu::owned).collect();
    GlobalState::global_mut(cx).set_app_menus(owned);
    cx.set_menus(menus);
    cx.refresh_windows();
}
