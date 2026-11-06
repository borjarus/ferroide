//! The Ferro themes and UI scaling.
//!
//! Themes are ordinary GPUI Kit theme files (`assets/themes/ferro.json`), so
//! user themes in the same format can be added later. Colors are referenced
//! through GPUI Kit's semantic tokens (`background`, `muted_foreground`,
//! `tab_active`, …); FerroIDE code never names a raw color.

use std::time::{Duration, Instant};

use gpui_kit::{
    App,
    component::{Theme, ThemeRegistry},
    px,
};

pub use ferro_settings::themes::{ALL as THEME_NAMES, FERRO_DARK, FERRO_LIGHT, HIGH_CONTRAST};

const FERRO_THEMES: &str = include_str!("../../../assets/themes/ferro.json");

/// Register the Ferro themes. Idempotent.
pub fn init(cx: &mut App) {
    if let Err(err) = ThemeRegistry::global_mut(cx).load_themes_from_str(FERRO_THEMES) {
        // The file is embedded and covered by a test; this only fires if a
        // broken theme file was committed.
        panic!("built-in Ferro themes failed to parse: {err:#}");
    }
}

/// Switch to a registered theme by name and refresh every window. Returns
/// how long the switch took, which the Stage 1 budget caps at 50 ms.
///
/// An unknown name falls back to Ferro Dark.
pub fn apply_theme(name: &str, cx: &mut App) -> Duration {
    let started = Instant::now();
    let registry = ThemeRegistry::global(cx);
    let config = registry
        .themes()
        .get(name)
        .or_else(|| registry.themes().get(FERRO_DARK))
        .cloned();
    if let Some(config) = config {
        // `apply_config` replaces the font sizes with the theme's; keep the
        // user's UI zoom across a theme switch.
        let font_size = Theme::global(cx).font_size;
        Theme::update(cx, |theme| {
            theme.apply_config(&config);
            theme.font_size = font_size;
        });
    }
    started.elapsed()
}

/// The name of the active theme.
pub fn current_theme(cx: &App) -> String {
    Theme::global(cx).theme_name().to_string()
}

/// Set the UI font size, which scales the whole interface (sizes are in
/// rems), and the editor's monospace size.
pub fn set_font_sizes(ui: f32, editor: f32, cx: &mut App) {
    Theme::update(cx, |theme| {
        theme.font_size = px(ui);
        theme.mono_font_size = px(editor);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ferro_theme_file_parses_with_every_expected_theme() {
        let set: serde_json::Value = serde_json::from_str(FERRO_THEMES).unwrap();
        let names: Vec<&str> = set["themes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, THEME_NAMES);
    }
}
