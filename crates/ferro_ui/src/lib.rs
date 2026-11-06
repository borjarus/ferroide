//! FerroIDE look and feel: themes, icons, typography and shared components.
//!
//! Everything visual that more than one crate needs lives here, so the rest
//! of the UI never hard-codes a color, an icon path or a font size.

pub mod assets;
pub mod components;
pub mod icons;
pub mod theme;

pub use assets::FerroAssets;
pub use icons::FerroIcon;

/// Translate a message id into a `SharedString` for the UI.
pub fn tr(key: &str) -> gpui_kit::SharedString {
    ferro_i18n::t(key).into()
}

/// Translate with Fluent arguments.
pub fn tr_args(key: &str, args: &[(&str, ferro_i18n::Arg)]) -> gpui_kit::SharedString {
    ferro_i18n::t_args(key, args).into()
}

/// Initialize the FerroIDE UI layer: GPUI Kit, the Ferro themes and fonts.
/// Call once, after `gpui_kit::init`.
pub fn init(cx: &mut gpui_kit::App) {
    theme::init(cx);
}
