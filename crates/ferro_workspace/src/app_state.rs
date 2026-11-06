//! Process-wide state: settings, storage paths, and the app-level action
//! handlers (theme, language, keymap, zoom) that work in every window.

use std::time::Duration;

use ferro_i18n::Locale;
use ferro_keymap::KeymapPreset;
use ferro_settings::{
    DEFAULT_UI_FONT_SIZE, MAX_UI_FONT_SIZE, MIN_UI_FONT_SIZE, Paths, Settings, themes,
};
use gpui_kit::{App, Global};

use crate::{actions::*, menus};

/// Global application state.
pub struct AppState {
    pub settings: Settings,
    pub paths: Paths,
    /// Whether settings changes are written to disk. Tests turn it off.
    pub persist: bool,
    /// The duration of the most recent theme switch, for the status bar
    /// diagnostics and the Stage 1 budget (< 50 ms).
    pub last_theme_switch: Option<Duration>,
}

impl Global for AppState {}

impl AppState {
    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    pub fn settings(cx: &App) -> &Settings {
        &cx.global::<Self>().settings
    }

    /// Install the state and apply its settings: theme, fonts, language and
    /// keymap. Call after `gpui_kit::init` and `ferro_ui::init`.
    pub fn init(settings: Settings, paths: Paths, persist: bool, cx: &mut App) {
        ferro_i18n::set_locale(settings.locale);
        cx.set_global(AppState {
            settings,
            paths,
            persist,
            last_theme_switch: None,
        });
        apply_appearance(cx);
        let preset = AppState::settings(cx).keymap;
        install_keymap(preset, None, cx);
        register_app_actions(cx);
        menus::install(cx);
    }

    /// Edit the settings, persist them, and refresh every window.
    pub fn update_settings(cx: &mut App, edit: impl FnOnce(&mut Settings)) {
        let state = cx.global_mut::<Self>();
        edit(&mut state.settings);
        state.settings = state.settings.clone().sanitized();
        if state.persist
            && let Err(err) = state.paths.save_settings(&state.settings)
        {
            tracing::warn!("could not save settings: {err:#}");
        }
        cx.refresh_windows();
    }
}

/// Apply theme and font sizes from the current settings.
pub fn apply_appearance(cx: &mut App) {
    let appearance = AppState::settings(cx).appearance.clone();
    let editor_font = AppState::settings(cx).editor.font_size;
    ferro_ui::theme::set_font_sizes(appearance.ui_font_size, editor_font, cx);
    let took = ferro_ui::theme::apply_theme(&appearance.theme, cx);
    cx.global_mut::<AppState>().last_theme_switch = Some(took);
    tracing::info!(theme = %appearance.theme, ?took, "theme applied");
}

pub fn set_theme(name: &str, cx: &mut App) {
    let name = name.to_string();
    AppState::update_settings(cx, |s| s.appearance.theme = name);
    apply_appearance(cx);
}

pub fn set_locale(locale: Locale, cx: &mut App) {
    ferro_i18n::set_locale(locale);
    AppState::update_settings(cx, |s| s.locale = locale);
    // Menus hold translated strings, so they are rebuilt.
    menus::install(cx);
}

pub fn set_keymap(preset: KeymapPreset, cx: &mut App) {
    let previous = AppState::settings(cx).keymap;
    install_keymap(preset, Some(previous), cx);
    AppState::update_settings(cx, |s| s.keymap = preset);
    menus::install(cx);
}

pub fn zoom_ui(delta: Option<f32>, cx: &mut App) {
    AppState::update_settings(cx, |s| {
        s.appearance.ui_font_size = match delta {
            Some(delta) => {
                (s.appearance.ui_font_size + delta).clamp(MIN_UI_FONT_SIZE, MAX_UI_FONT_SIZE)
            }
            None => DEFAULT_UI_FONT_SIZE,
        };
    });
    apply_appearance(cx);
}

fn register_app_actions(cx: &mut App) {
    cx.on_action(|_: &ThemeFerroDark, cx| set_theme(themes::FERRO_DARK, cx));
    cx.on_action(|_: &ThemeFerroLight, cx| set_theme(themes::FERRO_LIGHT, cx));
    cx.on_action(|_: &ThemeHighContrast, cx| set_theme(themes::HIGH_CONTRAST, cx));
    cx.on_action(|_: &LanguageEnglish, cx| set_locale(Locale::En, cx));
    cx.on_action(|_: &LanguagePolish, cx| set_locale(Locale::Pl, cx));
    cx.on_action(|_: &KeymapIntelliJ, cx| set_keymap(KeymapPreset::IntelliJ, cx));
    cx.on_action(|_: &KeymapVsCode, cx| set_keymap(KeymapPreset::VsCode, cx));
    cx.on_action(|_: &ZoomInUi, cx| zoom_ui(Some(1.0), cx));
    cx.on_action(|_: &ZoomOutUi, cx| zoom_ui(Some(-1.0), cx));
    cx.on_action(|_: &ResetUiZoom, cx| zoom_ui(None, cx));
    cx.on_action(|_: &ToggleCompactMode, cx| {
        AppState::update_settings(cx, |s| s.appearance.compact = !s.appearance.compact);
    });
    cx.on_action(|_: &Quit, cx| cx.quit());
}
