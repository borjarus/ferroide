//! Small dialogs: quick Settings (the full Settings window comes in Stage 2)
//! and About.

use ferro_i18n::Locale;
use ferro_keymap::KeymapPreset;
use ferro_settings::themes;
use ferro_ui::{tr, tr_args};
use gpui_kit::{
    Action, App, Context, IntoElement, ParentElement as _, SharedString, Styled as _, Window,
    component::{
        ActiveTheme as _, Selectable as _, Sizable as _, WindowExt as _,
        button::{Button, ButtonGroup, ButtonVariants as _},
        h_flex, v_flex,
    },
    div, px,
};

use crate::{actions::*, app_state::AppState, workspace::Workspace};

fn choice(
    id: &'static str,
    label: impl Into<SharedString>,
    selected: bool,
    action: impl Action + Clone,
) -> Button {
    Button::new(id)
        .small()
        .label(label)
        .selected(selected)
        .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
}

fn section(title: SharedString, body: impl IntoElement, cx: &App) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(title),
        )
        .child(body)
}

pub fn open_settings(_: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    window.open_dialog(cx, |dialog, _, cx| {
        let settings = AppState::settings(cx).clone();
        let theme = settings.appearance.theme.as_str();
        dialog.title(tr("settings-title")).w(px(520.)).child(
            v_flex()
                .gap_4()
                .child(section(
                    tr("settings-theme"),
                    ButtonGroup::new("theme")
                        .child(choice(
                            "dark",
                            tr("action-theme-dark"),
                            theme == themes::FERRO_DARK,
                            ThemeFerroDark,
                        ))
                        .child(choice(
                            "light",
                            tr("action-theme-light"),
                            theme == themes::FERRO_LIGHT,
                            ThemeFerroLight,
                        ))
                        .child(choice(
                            "hc",
                            tr("action-theme-high-contrast"),
                            theme == themes::HIGH_CONTRAST,
                            ThemeHighContrast,
                        )),
                    cx,
                ))
                .child(section(
                    tr("settings-language"),
                    ButtonGroup::new("language")
                        .child(choice(
                            "en",
                            "English",
                            settings.locale == Locale::En,
                            LanguageEnglish,
                        ))
                        .child(choice(
                            "pl",
                            "Polski",
                            settings.locale == Locale::Pl,
                            LanguagePolish,
                        )),
                    cx,
                ))
                .child(section(
                    tr("settings-keymap"),
                    ButtonGroup::new("keymap")
                        .child(choice(
                            "intellij",
                            "IntelliJ IDEA",
                            settings.keymap == KeymapPreset::IntelliJ,
                            KeymapIntelliJ,
                        ))
                        .child(choice(
                            "vscode",
                            "VS Code",
                            settings.keymap == KeymapPreset::VsCode,
                            KeymapVsCode,
                        )),
                    cx,
                ))
                .child(section(
                    tr("settings-ui-size"),
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("zoom-out")
                                .small()
                                .label("−")
                                .on_click(|_, w, cx| w.dispatch_action(Box::new(ZoomOutUi), cx)),
                        )
                        .child(
                            div()
                                .w(px(56.))
                                .text_center()
                                .child(format!("{:.0} px", settings.appearance.ui_font_size)),
                        )
                        .child(
                            Button::new("zoom-in")
                                .small()
                                .label("+")
                                .on_click(|_, w, cx| w.dispatch_action(Box::new(ZoomInUi), cx)),
                        )
                        .child(
                            Button::new("zoom-reset")
                                .small()
                                .ghost()
                                .label(tr("action-reset-ui-zoom"))
                                .on_click(|_, w, cx| w.dispatch_action(Box::new(ResetUiZoom), cx)),
                        ),
                    cx,
                ))
                .child(section(
                    tr("settings-layout"),
                    choice(
                        "compact",
                        tr("action-toggle-compact-mode"),
                        settings.appearance.compact,
                        ToggleCompactMode,
                    ),
                    cx,
                ))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(tr("settings-stage-note")),
                ),
        )
    });
}

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
