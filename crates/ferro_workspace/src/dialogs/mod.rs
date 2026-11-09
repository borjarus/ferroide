//! Dialogs. This module holds the About dialog; each larger dialog has its
//! own submodule. Settings live in `crate::settings_window`.

pub mod clone_repo;
pub mod new_file;
pub mod rename;
pub mod run_config;

use ferro_ui::{tr, tr_args};
use gpui_kit::{
    Context, ParentElement as _, Styled as _, Window,
    component::{ActiveTheme as _, WindowExt as _, v_flex},
    div, px,
};

use crate::{app_state::AppState, workspace::Workspace};

pub fn open_about(_: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    window.open_dialog(cx, |dialog, _, cx| {
        let took = AppState::global(cx)
            .last_theme_switch
            .map(|d| format!("{:.1} ms", d.as_secs_f64() * 1000.0))
            .unwrap_or_else(|| "—".into());
        dialog.title("FerroIDE").w(px(420.)).child(
            v_flex()
                .gap_2()
                .child(tr_args(
                    "about-version",
                    &[("version", env!("CARGO_PKG_VERSION").into())],
                ))
                .child(
                    div()
                        .text_color(cx.theme().muted_foreground)
                        .child(tr("about-stage")),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(tr_args("about-theme-switch", &[("took", took.into())])),
                ),
        )
    });
}
