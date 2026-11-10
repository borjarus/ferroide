//! User settings and small pieces of persisted UI state.
//!
//! Global settings live in `<config>/FerroIDE/settings.toml`, keymap
//! overrides in `<config>/FerroIDE/keymap.toml`, and per-project window
//! layouts in `<data>/FerroIDE/layouts/<key>.json`. Project-level settings,
//! JSON Schema and migrations arrive in Stage 4 (see
//! `docs/FerroIDE_plan.md`).
//!
//! Every read tolerates a missing or broken file by falling back to defaults:
//! a corrupt settings file must never stop the IDE from starting.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result};
use ferro_i18n::Locale;
use ferro_keymap::{KeymapPreset, UserKeymap};
use serde::{Deserialize, Deserializer, Serialize};

pub const APP_DIR: &str = "FerroIDE";

/// Default UI font size in pixels; the UI zoom scales it.
pub const DEFAULT_UI_FONT_SIZE: f32 = 14.0;
pub const DEFAULT_EDITOR_FONT_SIZE: f32 = 13.0;
pub const MIN_UI_FONT_SIZE: f32 = 10.0;
pub const MAX_UI_FONT_SIZE: f32 = 24.0;

/// Built-in theme names, as registered with the theme registry.
pub mod themes {
    pub const FERRO_DARK: &str = "Ferro Dark";
    pub const FERRO_LIGHT: &str = "Ferro Light";
    pub const HIGH_CONTRAST: &str = "Ferro High Contrast";
    pub const ALL: [&str; 3] = [FERRO_DARK, FERRO_LIGHT, HIGH_CONTRAST];
}

/// Global user settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub appearance: Appearance,
    #[serde(deserialize_with = "keymap_or_stage1_preset")]
    pub keymap: KeymapSettings,
    pub locale: Locale,
    pub editor: EditorSettings,
}

/// `[keymap]` in `settings.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct KeymapSettings {
    /// The scheme; the user's `keymap.toml` overrides apply on top.
    pub base: KeymapPreset,
    /// Modal editing on top of any scheme. Stored now; the modal layer
    /// itself arrives with the editor core (Stage 3).
    pub vim_mode: bool,
}

/// Stage 1 wrote `keymap = "intellij"`; Stage 2 writes a `[keymap]` table.
fn keymap_or_stage1_preset<'de, D: Deserializer<'de>>(d: D) -> Result<KeymapSettings, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Either {
        Table(KeymapSettings),
        Preset(KeymapPreset),
    }
    Ok(match Either::deserialize(d)? {
        Either::Table(table) => table,
        Either::Preset(base) => KeymapSettings {
            base,
            vim_mode: false,
        },
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme: String,
    pub ui_font_size: f32,
    /// Compact mode: smaller toolbar, menu behind a hamburger button.
    pub compact: bool,
    /// Animations can be disabled for slow machines (power-save mode).
    pub animations: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorSettings {
    pub font_size: f32,
    /// Open editor tabs above this limit close the oldest unpinned one.
    pub tab_limit: usize,
    /// Spaces per indentation level.
    pub tab_size: usize,
    pub soft_wrap: bool,
    pub line_numbers: bool,
    /// Type and parameter hints shown inline.
    pub inlay_hints: bool,
    /// The enclosing scopes stay at the top while scrolling.
    pub sticky_lines: bool,
    pub breadcrumbs: bool,
    /// Change markers in the gutter (added/modified/deleted lines).
    pub vcs_markers: bool,
    /// A single click opens a preview tab, replaced by the next one.
    pub preview_tabs: bool,
    /// When modified files are saved without an explicit Save.
    pub autosave: Autosave,
    /// Draw spaces and tabs as dots and arrows.
    pub show_whitespace: bool,
}

/// When the editor saves modified files by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Autosave {
    /// Only Save / Save All write files.
    Off,
    /// When the editor or the window loses focus, and when a tab closes
    /// (as IntelliJ IDEA does).
    #[default]
    OnFocusChange,
    /// Also one second after typing stops.
    AfterDelay,
}

impl Autosave {
    pub const ALL: [Autosave; 3] = [Autosave::Off, Autosave::OnFocusChange, Autosave::AfterDelay];

    /// Stable id, as written in `settings.toml`.
    pub fn id(self) -> &'static str {
        match self {
            Autosave::Off => "off",
            Autosave::OnFocusChange => "on_focus_change",
            Autosave::AfterDelay => "after_delay",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.id() == id)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            appearance: Appearance::default(),
            keymap: KeymapSettings::default(),
            locale: Locale::from_env(),
            editor: EditorSettings::default(),
        }
    }
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: themes::FERRO_DARK.to_string(),
            ui_font_size: DEFAULT_UI_FONT_SIZE,
            compact: false,
            animations: true,
        }
    }
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            font_size: DEFAULT_EDITOR_FONT_SIZE,
            tab_limit: 10,
            tab_size: 4,
            soft_wrap: false,
            line_numbers: true,
            inlay_hints: true,
            sticky_lines: true,
            breadcrumbs: true,
            vcs_markers: true,
            preview_tabs: true,
            autosave: Autosave::default(),
            show_whitespace: false,
        }
    }
}

impl Settings {
    /// Clamp values a hand-edited file may have pushed out of range.
    pub fn sanitized(mut self) -> Self {
        self.appearance.ui_font_size = self
            .appearance
            .ui_font_size
            .clamp(MIN_UI_FONT_SIZE, MAX_UI_FONT_SIZE);
        self.editor.font_size = self.editor.font_size.clamp(6.0, 72.0);
        self.editor.tab_limit = self.editor.tab_limit.clamp(1, 100);
        self.editor.tab_size = self.editor.tab_size.clamp(1, 16);
        if !themes::ALL.contains(&self.appearance.theme.as_str()) {
            self.appearance.theme = themes::FERRO_DARK.to_string();
        }
        self
    }

    pub fn from_toml(text: &str) -> Result<Self> {
        Ok(toml::from_str::<Settings>(text)?.sanitized())
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }
}

/// Where settings and state are stored. Tests point it at a temp directory.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl Paths {
    /// The platform's standard directories, or `./.ferroide` when the
    /// platform has none (e.g. a minimal container).
    pub fn standard() -> Self {
        let fallback = PathBuf::from(".ferroide");
        Self {
            config_dir: dirs::config_dir()
                .map(|d| d.join(APP_DIR))
                .unwrap_or_else(|| fallback.join("config")),
            data_dir: dirs::data_local_dir()
                .map(|d| d.join(APP_DIR))
                .unwrap_or_else(|| fallback.join("data")),
        }
    }

    pub fn under(root: &Path) -> Self {
        Self {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
        }
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join("settings.toml")
    }

    pub fn keymap_file(&self) -> PathBuf {
        self.config_dir.join("keymap.toml")
    }

    /// Load the user's keymap overrides; missing or broken gives none.
    pub fn load_user_keymap(&self) -> UserKeymap {
        let path = self.keymap_file();
        match fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|err| {
                tracing::warn!("ignoring invalid keymap file {}: {err:#}", path.display());
                UserKeymap::default()
            }),
            Err(_) => UserKeymap::default(),
        }
    }

    pub fn save_user_keymap(&self, keymap: &UserKeymap) -> Result<()> {
        let header = "# FerroIDE keymap overrides, applied on top of the scheme chosen in\n\
                      # settings.toml ([keymap] base). An entry replaces the scheme's\n\
                      # bindings of its action; empty `keys` unbinds the action.\n\n";
        let body = toml::to_string_pretty(keymap)?;
        write_atomic(&self.keymap_file(), format!("{header}{body}").as_bytes())
    }

    pub fn layout_file(&self, key: &str) -> PathBuf {
        self.data_dir
            .join("layouts")
            .join(format!("{}.json", sanitize_key(key)))
    }

    /// Load settings; a missing file gives defaults, a broken one is logged
    /// and gives defaults too.
    pub fn load_settings(&self) -> Settings {
        let path = self.settings_file();
        match fs::read_to_string(&path) {
            Ok(text) => Settings::from_toml(&text).unwrap_or_else(|err| {
                tracing::warn!("ignoring invalid settings file {}: {err:#}", path.display());
                Settings::default()
            }),
            Err(_) => Settings::default(),
        }
    }

    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        write_atomic(&self.settings_file(), settings.to_toml()?.as_bytes())
    }

    /// Load a persisted window layout as raw JSON. The UI layer owns its
    /// schema; this crate only stores it.
    pub fn load_layout(&self, key: &str) -> Option<serde_json::Value> {
        let text = fs::read_to_string(self.layout_file(key)).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save_layout(&self, key: &str, layout: &serde_json::Value) -> Result<()> {
        write_atomic(
            &self.layout_file(key),
            serde_json::to_string_pretty(layout)?.as_bytes(),
        )
    }

    pub fn delete_layout(&self, key: &str) {
        let _ = fs::remove_file(self.layout_file(key));
    }
}

/// Make a project path or name usable as a file name.
fn sanitize_key(key: &str) -> String {
    let cleaned: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut cleaned = cleaned.trim_matches('_').to_string();
    if cleaned.is_empty() {
        cleaned = "default".into();
    }
    // A long path would exceed file-name limits; keep the tail plus a hash.
    if cleaned.len() > 80 {
        let hash = key.bytes().fold(0xcbf29ce484222325u64, |h, b| {
            (h ^ b as u64).wrapping_mul(0x100000001b3)
        });
        cleaned = format!("{}-{hash:016x}", &cleaned[cleaned.len() - 60..]);
    }
    cleaned
}

/// Write to a temporary file and rename it over the target, so a crash
/// mid-write never leaves a truncated file.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("path has no parent")?;
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ferro-settings-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn settings_round_trip_through_disk() {
        let paths = Paths::under(&temp_root("roundtrip"));
        let mut settings = Settings::default();
        settings.appearance.theme = themes::FERRO_LIGHT.into();
        settings.locale = Locale::Pl;
        settings.keymap = KeymapSettings {
            base: KeymapPreset::Emacs,
            vim_mode: true,
        };
        settings.editor.soft_wrap = true;
        paths.save_settings(&settings).unwrap();
        assert_eq!(paths.load_settings(), settings);
        let text = fs::read_to_string(paths.settings_file()).unwrap();
        assert!(
            text.contains("[keymap]") && text.contains("base = \"emacs\""),
            "{text}"
        );
    }

    #[test]
    fn stage1_keymap_setting_is_migrated() {
        let settings = Settings::from_toml("keymap = \"intellij\"\nlocale = \"pl\"").unwrap();
        assert_eq!(settings.keymap.base, KeymapPreset::JetBrains);
        assert!(!settings.keymap.vim_mode);
        let settings = Settings::from_toml("").unwrap();
        assert_eq!(settings.keymap.base, KeymapPreset::VsCode);
    }

    #[test]
    fn user_keymap_round_trips_through_disk() {
        let paths = Paths::under(&temp_root("keymap"));
        assert_eq!(paths.load_user_keymap(), UserKeymap::default());
        let mut keymap = UserKeymap::default();
        keymap.set("ferro::QuickOpen", &["ctrl-e".into()]);
        keymap.set("ferro::GoToLine", &[]);
        paths.save_user_keymap(&keymap).unwrap();
        assert_eq!(paths.load_user_keymap(), keymap);
        let text = fs::read_to_string(paths.keymap_file()).unwrap();
        assert!(text.contains("[[binding]]"), "{text}");
    }

    #[test]
    fn broken_or_partial_files_fall_back_to_defaults() {
        let paths = Paths::under(&temp_root("broken"));
        fs::create_dir_all(&paths.config_dir).unwrap();
        fs::write(paths.settings_file(), "this is = = not toml").unwrap();
        assert_eq!(paths.load_settings().appearance, Appearance::default());

        let partial =
            Settings::from_toml("[appearance]\nui_font_size = 99\ntheme = \"Nope\"").unwrap();
        assert_eq!(partial.appearance.ui_font_size, MAX_UI_FONT_SIZE);
        assert_eq!(partial.appearance.theme, themes::FERRO_DARK);
        assert_eq!(partial.editor, EditorSettings::default());
    }

    #[test]
    fn layout_keys_become_safe_file_names() {
        let paths = Paths::under(&temp_root("layout"));
        let key = r"C:\Users\me\projects\demo-app";
        let value = serde_json::json!({"center": [1, 2, 3]});
        paths.save_layout(key, &value).unwrap();
        assert_eq!(paths.load_layout(key), Some(value));
        let file = paths.layout_file(key);
        let name = file.file_name().unwrap().to_str().unwrap();
        assert!(!name.contains(['\\', ':', '/']), "{name}");

        let long = "x/".repeat(200);
        assert!(paths.layout_file(&long).file_name().unwrap().len() < 100);
    }
}
