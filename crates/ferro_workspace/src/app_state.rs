//! Process-wide state: settings, the effective keymap, storage paths, and
//! the app-level action handlers (theme, language, keymap, zoom) that work in
//! every window.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use ferro_db::StateDb;
use ferro_i18n::Locale;
use ferro_keymap::{KeymapPreset, Platform, ResolvedBinding, UserKeymap};
use ferro_settings::{
    DEFAULT_UI_FONT_SIZE, EditorSettings, MAX_UI_FONT_SIZE, MIN_UI_FONT_SIZE, Paths, Settings,
    themes,
};
use gpui_kit::{App, Global};

use crate::{actions::*, menus};

/// Global application state.
pub struct AppState {
    pub settings: Settings,
    /// The user's `keymap.toml` overrides.
    pub user_keymap: UserKeymap,
    /// The scheme with the overrides applied: what is installed in GPUI.
    pub keymap: Vec<ResolvedBinding>,
    pub paths: Paths,
    /// Whether settings changes are written to disk. Tests turn it off.
    pub persist: bool,
    /// The duration of the most recent theme switch, for the status bar
    /// diagnostics and the Stage 1 budget (< 50 ms).
    pub last_theme_switch: Option<Duration>,
    /// Workspace state: layouts, history, recent projects, trust. In memory
    /// when `persist` is off.
    pub db: Arc<StateDb>,
    /// Settings with a project's `.ferroide/settings.toml` applied, per
    /// project root.
    pub project_settings: Vec<(PathBuf, Settings)>,
    /// Watches `settings.toml` and `keymap.toml` for edits made outside
    /// the Settings window.
    _config_watcher: Option<ferro_project::FileWatcher>,
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
        let user_keymap = if persist {
            paths.load_user_keymap()
        } else {
            UserKeymap::default()
        };
        let keymap = ferro_keymap::resolve(settings.keymap.base, Platform::current(), &user_keymap);
        let db = if persist {
            StateDb::open(&paths.state_db_file()).unwrap_or_else(|err| {
                tracing::error!("workspace state will not be kept: {err:#}");
                StateDb::open_in_memory().expect("an in-memory database")
            })
        } else {
            StateDb::open_in_memory().expect("an in-memory database")
        };
        if persist && let Err(err) = paths.write_schema() {
            tracing::warn!("could not write the settings schema: {err:#}");
        }
        let config_watcher = if persist {
            watch_config(&paths, cx)
        } else {
            None
        };
        cx.set_global(AppState {
            settings,
            user_keymap,
            keymap,
            paths,
            persist,
            last_theme_switch: None,
            db: Arc::new(db),
            project_settings: Vec::new(),
            _config_watcher: config_watcher,
        });
        apply_appearance(cx);
        let keymap = AppState::global(cx).keymap.clone();
        install_keymap(&keymap, None, cx);
        register_app_actions(cx);
        menus::install(cx);
    }

    /// The editor settings for a file: the global ones, with the
    /// `[editor]` overrides of the project the file belongs to.
    pub fn editor_settings<'a>(path: &Path, cx: &'a App) -> &'a EditorSettings {
        let state = Self::global(cx);
        state
            .project_settings
            .iter()
            .find(|(root, _)| path.starts_with(root))
            .map_or(&state.settings.editor, |(_, s)| &s.editor)
    }

    /// (Re)read the project settings of `root`.
    pub fn load_project_settings(root: &Path, cx: &mut App) {
        let state = cx.global_mut::<Self>();
        let merged = Paths::with_project_settings(&state.settings, root);
        state.project_settings.retain(|(r, _)| r != root);
        if merged != state.settings {
            state.project_settings.push((root.to_path_buf(), merged));
        }
        cx.refresh_windows();
    }

    pub fn forget_project_settings(root: &Path, cx: &mut App) {
        cx.global_mut::<Self>()
            .project_settings
            .retain(|(r, _)| r != root);
    }

    fn reapply_project_settings(&mut self) {
        let roots: Vec<PathBuf> = self
            .project_settings
            .iter()
            .map(|(r, _)| r.clone())
            .collect();
        self.project_settings = roots
            .into_iter()
            .map(|root| {
                let merged = Paths::with_project_settings(&self.settings, &root);
                (root, merged)
            })
            .collect();
    }

    /// Edit the settings, persist them, and refresh every window.
    pub fn update_settings(cx: &mut App, edit: impl FnOnce(&mut Settings)) {
        let state = cx.global_mut::<Self>();
        edit(&mut state.settings);
        state.settings = state.settings.clone().sanitized();
        state.reapply_project_settings();
        if state.persist
            && let Err(err) = state.paths.save_settings(&state.settings)
        {
            tracing::warn!("could not save settings: {err:#}");
        }
        cx.refresh_windows();
    }

    /// The first keystroke bound to `action` in the effective keymap.
    pub fn shortcut(action: &str, cx: &App) -> Option<String> {
        ferro_keymap::shortcuts_in(action, &AppState::global(cx).keymap)
            .first()
            .map(|s| s.to_string())
    }

    /// Edit the user's keymap overrides, persist them and reinstall the
    /// effective keymap.
    pub fn update_user_keymap(cx: &mut App, edit: impl FnOnce(&mut UserKeymap)) {
        let state = cx.global_mut::<Self>();
        edit(&mut state.user_keymap);
        if state.persist
            && let Err(err) = state.paths.save_user_keymap(&state.user_keymap)
        {
            tracing::warn!("could not save keymap: {err:#}");
        }
        reinstall_keymap(cx);
    }
}

/// Resolve the keymap from the settings and overrides, and install it in
/// place of the previous one.
pub fn reinstall_keymap(cx: &mut App) {
    let state = AppState::global(cx);
    let previous = state.keymap.clone();
    let keymap = ferro_keymap::resolve(
        state.settings.keymap.base,
        Platform::current(),
        &state.user_keymap,
    );
    install_keymap(&keymap, Some(&previous), cx);
    cx.global_mut::<AppState>().keymap = keymap;
    menus::install(cx);
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

/// Switch the keymap scheme. User overrides are kept.
pub fn set_keymap(preset: KeymapPreset, cx: &mut App) {
    AppState::update_settings(cx, |s| s.keymap.base = preset);
    reinstall_keymap(cx);
}

pub fn set_vim_mode(enabled: bool, cx: &mut App) {
    AppState::update_settings(cx, |s| s.keymap.vim_mode = enabled);
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

/// Watch the config directory and reload `settings.toml` / `keymap.toml`
/// when they are edited outside FerroIDE (or in an editor tab).
fn watch_config(paths: &Paths, cx: &mut App) -> Option<ferro_project::FileWatcher> {
    let dir = paths.config_dir.clone();
    if let Err(err) = std::fs::create_dir_all(&dir) {
        tracing::warn!("cannot create {}: {err}", dir.display());
        return None;
    }
    let (tx, rx) = async_channel::unbounded::<Vec<PathBuf>>();
    let filter = Arc::new(ferro_project::IgnoreFilter::new(&dir));
    let watcher =
        ferro_project::FileWatcher::new(&dir, filter, Duration::from_millis(100), move |events| {
            let paths = events
                .into_iter()
                .filter_map(|event| match event {
                    ferro_services::ProjectEvent::Added(p)
                    | ferro_services::ProjectEvent::Changed(p) => Some(p),
                    _ => None,
                })
                .collect::<Vec<_>>();
            if !paths.is_empty() {
                let _ = tx.try_send(paths);
            }
        });
    let watcher = match watcher {
        Ok(watcher) => watcher,
        Err(err) => {
            tracing::warn!("settings will not reload when edited: {err:#}");
            return None;
        }
    };
    cx.spawn(async move |cx| {
        while let Ok(changed) = rx.recv().await {
            let _ = cx.update(|cx| {
                let paths = &AppState::global(cx).paths;
                let (settings_file, keymap_file) = (paths.settings_file(), paths.keymap_file());
                if changed.contains(&settings_file) {
                    reload_settings_from_disk(cx);
                }
                if changed.contains(&keymap_file) {
                    reload_keymap_from_disk(cx);
                }
            });
        }
    })
    .detach();
    Some(watcher)
}

/// Apply `settings.toml` as it is on disk now. Our own saves read back
/// unchanged and do nothing.
pub fn reload_settings_from_disk(cx: &mut App) {
    let state = AppState::global(cx);
    let loaded = state.paths.load_settings();
    let old = state.settings.clone();
    if loaded == old {
        return;
    }
    tracing::info!("settings.toml changed on disk; applying it");
    let state = cx.global_mut::<AppState>();
    state.settings = loaded.clone();
    state.reapply_project_settings();
    if loaded.locale != old.locale {
        ferro_i18n::set_locale(loaded.locale);
    }
    if loaded.appearance != old.appearance || loaded.editor.font_size != old.editor.font_size {
        apply_appearance(cx);
    }
    if loaded.keymap != old.keymap {
        reinstall_keymap(cx);
    } else {
        menus::install(cx);
    }
    cx.refresh_windows();
}

/// Apply `keymap.toml` as it is on disk now.
pub fn reload_keymap_from_disk(cx: &mut App) {
    let loaded = AppState::global(cx).paths.load_user_keymap();
    if loaded == AppState::global(cx).user_keymap {
        return;
    }
    tracing::info!("keymap.toml changed on disk; applying it");
    cx.global_mut::<AppState>().user_keymap = loaded;
    reinstall_keymap(cx);
}

fn register_app_actions(cx: &mut App) {
    cx.on_action(|_: &ThemeFerroDark, cx| set_theme(themes::FERRO_DARK, cx));
    cx.on_action(|_: &ThemeFerroLight, cx| set_theme(themes::FERRO_LIGHT, cx));
    cx.on_action(|_: &ThemeHighContrast, cx| set_theme(themes::HIGH_CONTRAST, cx));
    cx.on_action(|_: &LanguageEnglish, cx| set_locale(Locale::En, cx));
    cx.on_action(|_: &LanguagePolish, cx| set_locale(Locale::Pl, cx));
    cx.on_action(|_: &KeymapVsCode, cx| set_keymap(KeymapPreset::VsCode, cx));
    cx.on_action(|_: &KeymapJetBrains, cx| set_keymap(KeymapPreset::JetBrains, cx));
    cx.on_action(|_: &KeymapZed, cx| set_keymap(KeymapPreset::Zed, cx));
    cx.on_action(|_: &KeymapSublime, cx| set_keymap(KeymapPreset::Sublime, cx));
    cx.on_action(|_: &KeymapEmacs, cx| set_keymap(KeymapPreset::Emacs, cx));
    cx.on_action(|_: &ToggleVimMode, cx| {
        let enabled = !AppState::settings(cx).keymap.vim_mode;
        set_vim_mode(enabled, cx);
    });
    cx.on_action(|_: &ZoomInUi, cx| zoom_ui(Some(1.0), cx));
    cx.on_action(|_: &ZoomOutUi, cx| zoom_ui(Some(-1.0), cx));
    cx.on_action(|_: &ResetUiZoom, cx| zoom_ui(None, cx));
    cx.on_action(|_: &ToggleSoftWrap, cx| {
        AppState::update_settings(cx, |s| s.editor.soft_wrap = !s.editor.soft_wrap);
    });
    cx.on_action(|_: &ToggleWhitespace, cx| {
        AppState::update_settings(cx, |s| s.editor.show_whitespace = !s.editor.show_whitespace);
    });
    cx.on_action(|_: &ToggleCompactMode, cx| {
        AppState::update_settings(cx, |s| s.appearance.compact = !s.appearance.compact);
    });
    cx.on_action(|_: &Quit, cx| cx.quit());
}
