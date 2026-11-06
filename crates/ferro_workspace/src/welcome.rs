//! The welcome screen: recent projects and New / Open / Get from VCS.

use ferro_services::{RecentProject, RecentProjectsService};
use ferro_ui::{FerroIcon, components, tr, tr_args};
use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _, IntoElement,
    KeyContext, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window,
    component::{
        ActiveTheme as _, Icon, Selectable as _, Sizable as _, TitleBar,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputEvent, InputState},
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};

use crate::actions::*;

pub struct WelcomeScreen {
    recent: Vec<RecentProject>,
    filter: Entity<InputState>,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl WelcomeScreen {
    pub fn new(
        recent: &dyn RecentProjectsService,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(tr("welcome-search")));
        let subscription = cx.subscribe(&filter, |_, _, _: &InputEvent, cx| cx.notify());
        Self {
            recent: recent.recent(),
            filter,
            focus_handle: cx.focus_handle(),
            _subscription: subscription,
        }
    }

    fn visible_projects(&self, cx: &gpui_kit::App) -> Vec<RecentProject> {
        let query = self.filter.read(cx).value().to_lowercase();
        self.recent
            .iter()
            .filter(|p| {
                query.is_empty()
                    || p.name.to_lowercase().contains(&query)
                    || p.path.to_string_lossy().to_lowercase().contains(&query)
            })
            .cloned()
            .collect()
    }
}

impl Focusable for WelcomeScreen {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn opened_ago(days: u32) -> SharedString {
    match days {
        0 => tr("welcome-today"),
        1 => tr("welcome-yesterday"),
        n => tr_args("welcome-days-ago", &[("days", n.into())]),
    }
}

impl Render for WelcomeScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, side_bg, border, muted, hover, primary) = (
            theme.background,
            theme.muted,
            theme.border,
            theme.muted_foreground,
            theme.list_hover,
            theme.primary,
        );
        let projects = self.visible_projects(cx);
        let mut context = KeyContext::default();
        context.add("Welcome");

        let action_button =
            |id: &'static str,
             icon: FerroIcon,
             key: &'static str,
             action: fn() -> Box<dyn gpui_kit::Action>| {
                Button::new(id)
                    .icon(Icon::from(icon))
                    .label(tr(key))
                    .on_click(move |_, window, cx| window.dispatch_action(action(), cx))
            };

        v_flex()
            .id("welcome")
            .key_context(context)
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(bg)
            .child(TitleBar::new().child(div().text_sm().child("FerroIDE")))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    // Sidebar
                    .child(
                        v_flex()
                            .w(px(220.))
                            .h_full()
                            .p_4()
                            .gap_3()
                            .bg(side_bg)
                            .border_r_1()
                            .border_color(border)
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .size(px(36.))
                                            .rounded(px(8.))
                                            .bg(primary)
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_color(gpui_kit::white())
                                            .font_weight(gpui_kit::FontWeight::BOLD)
                                            .child("Fe"),
                                    )
                                    .child(
                                        v_flex()
                                            .child(
                                                div()
                                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                                    .child("FerroIDE"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(muted)
                                                    .child(env!("CARGO_PKG_VERSION")),
                                            ),
                                    ),
                            )
                            .child(
                                Button::new("nav-projects")
                                    .ghost()
                                    .small()
                                    .w_full()
                                    .selected(true)
                                    .icon(Icon::from(FerroIcon::Folder))
                                    .label(tr("welcome-projects")),
                            )
                            .child(div().flex_1())
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(
                                        components::icon_button(
                                            "theme-dark",
                                            FerroIcon::ThemeDark,
                                            tr("action-theme-dark"),
                                        )
                                        .on_click(
                                            |_, w, cx| {
                                                w.dispatch_action(Box::new(ThemeFerroDark), cx)
                                            },
                                        ),
                                    )
                                    .child(
                                        components::icon_button(
                                            "theme-light",
                                            FerroIcon::ThemeLight,
                                            tr("action-theme-light"),
                                        )
                                        .on_click(
                                            |_, w, cx| {
                                                w.dispatch_action(Box::new(ThemeFerroLight), cx)
                                            },
                                        ),
                                    )
                                    .child(
                                        components::icon_button(
                                            "theme-hc",
                                            FerroIcon::HighContrast,
                                            tr("action-theme-high-contrast"),
                                        )
                                        .on_click(
                                            |_, w, cx| {
                                                w.dispatch_action(Box::new(ThemeHighContrast), cx)
                                            },
                                        ),
                                    )
                                    .child(div().flex_1())
                                    .child(
                                        Button::new("lang-en")
                                            .ghost()
                                            .xsmall()
                                            .label("EN")
                                            .on_click(|_, w, cx| {
                                                w.dispatch_action(Box::new(LanguageEnglish), cx)
                                            }),
                                    )
                                    .child(
                                        Button::new("lang-pl")
                                            .ghost()
                                            .xsmall()
                                            .label("PL")
                                            .on_click(|_, w, cx| {
                                                w.dispatch_action(Box::new(LanguagePolish), cx)
                                            }),
                                    ),
                            ),
                    )
                    // Main
                    .child(
                        v_flex()
                            .flex_1()
                            .h_full()
                            .p_6()
                            .gap_4()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .child(tr("welcome-title")),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(Input::new(&self.filter).flex_1().cleanable(true))
                                    .child(action_button(
                                        "welcome-new",
                                        FerroIcon::NewProject,
                                        "action-new-project",
                                        || Box::new(NewProject),
                                    ))
                                    .child(action_button(
                                        "welcome-open",
                                        FerroIcon::OpenProject,
                                        "action-open-project",
                                        || Box::new(OpenProject),
                                    ))
                                    .child(action_button(
                                        "welcome-vcs",
                                        FerroIcon::GetFromVcs,
                                        "action-get-from-vcs",
                                        || Box::new(GetFromVcs),
                                    )),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(tr("welcome-recent")),
                            )
                            .child(
                                v_flex()
                                    .id("recent-projects")
                                    .flex_1()
                                    .overflow_y_scroll()
                                    .gap_1()
                                    .when(projects.is_empty(), |el| {
                                        el.child(components::empty_state(
                                            FerroIcon::Folder,
                                            tr("welcome-no-projects"),
                                            None,
                                            cx,
                                        ))
                                    })
                                    .children(projects.into_iter().enumerate().map(
                                        |(ix, project)| {
                                            let initial: SharedString = project
                                                .name
                                                .chars()
                                                .next()
                                                .map(|c| c.to_uppercase().to_string())
                                                .unwrap_or_default()
                                                .into();
                                            h_flex()
                                                .id(("recent", ix))
                                                .p_2()
                                                .gap_3()
                                                .rounded(px(6.))
                                                .cursor_pointer()
                                                .hover(|el| el.bg(hover))
                                                .on_click(|_, window, cx| {
                                                    window
                                                        .dispatch_action(Box::new(OpenProject), cx)
                                                })
                                                .child(
                                                    div()
                                                        .size(px(32.))
                                                        .flex_none()
                                                        .rounded(px(6.))
                                                        .bg(primary.opacity(0.25))
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .font_weight(gpui_kit::FontWeight::BOLD)
                                                        .child(initial),
                                                )
                                                .child(
                                                    v_flex()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .child(
                                                            div()
                                                                .truncate()
                                                                .child(project.name.clone()),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(muted)
                                                                .truncate()
                                                                .child(
                                                                    project
                                                                        .path
                                                                        .to_string_lossy()
                                                                        .into_owned(),
                                                                ),
                                                        ),
                                                )
                                                .child(
                                                    v_flex()
                                                        .items_end()
                                                        .text_xs()
                                                        .text_color(muted)
                                                        .child(opened_ago(project.opened_days_ago))
                                                        .when_some(
                                                            project.branch.clone(),
                                                            |el, b| el.child(b),
                                                        ),
                                                )
                                        },
                                    )),
                            ),
                    ),
            )
    }
}
