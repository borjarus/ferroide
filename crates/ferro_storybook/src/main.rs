//! FerroIDE storybook: every FerroIDE component on mock data, including the
//! loading, empty and error states and extreme inputs.
//!
//! `cargo run -p ferro_storybook`

use std::time::Duration;

use ferro_services::{Services, mock::MockConfig};
use ferro_settings::Paths;
use ferro_ui::{FerroIcon, components, tr};
use ferro_workspace::{
    AppState,
    actions::*,
    tool_window::{ToolWindowKind, ToolWindowPanel},
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    WindowBounds, WindowOptions,
    component::{
        ActiveTheme as _, Icon, Selectable as _, Sizable as _, TitleBar,
        button::{Button, ButtonVariants as _},
        h_flex, v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px, size,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Story {
    Chrome,
    StatusBar,
    States,
    Icons,
    ToolWindows,
    Colors,
    Windows,
}

impl Story {
    const ALL: [Story; 7] = [
        Story::Chrome,
        Story::StatusBar,
        Story::States,
        Story::Icons,
        Story::ToolWindows,
        Story::Colors,
        Story::Windows,
    ];

    fn title(self) -> &'static str {
        match self {
            Story::Chrome => "Stripes & toolbar",
            Story::StatusBar => "Status bar",
            Story::States => "Loading / empty / error",
            Story::Icons => "Icons",
            Story::ToolWindows => "Tool windows",
            Story::Colors => "Theme tokens",
            Story::Windows => "Windows",
        }
    }
}

/// Mock flavors for the tool-window story.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Flavor {
    Fast,
    Slow,
    Failing,
}

struct Storybook {
    story: Story,
    tool: ToolWindowKind,
    flavor: Flavor,
    panel: Option<Entity<ToolWindowPanel>>,
    active_stripe: usize,
}

impl Storybook {
    fn services(flavor: Flavor) -> Services {
        Services::mock(match flavor {
            Flavor::Fast => MockConfig::default(),
            Flavor::Slow => MockConfig {
                latency: Duration::from_secs(2),
                ..MockConfig::default()
            },
            Flavor::Failing => MockConfig {
                failure_rate: 1.0,
                ..MockConfig::default()
            },
        })
    }

    fn rebuild_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.panel = Some(ToolWindowPanel::build(
            self.tool,
            Self::services(self.flavor),
            window,
            cx,
        ));
        cx.notify();
    }

    fn story_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.story == Story::ToolWindows && self.panel.is_none() {
            self.rebuild_panel(window, cx);
        }
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        match self.story {
            Story::Chrome => v_flex()
                .gap_4()
                .child(section("Tool-window stripe (click to activate)", muted))
                .child(
                    v_flex()
                        .w(px(40.))
                        .gap_1()
                        .p_1()
                        .bg(theme.title_bar)
                        .children(ToolWindowKind::ALL.iter().enumerate().map(|(ix, kind)| {
                            components::stripe_button(
                                SharedString::from(format!("s-{ix}")),
                                kind.icon(),
                                kind.title(),
                                self.active_stripe == ix,
                                cx.listener(move |this, _, _, cx| {
                                    this.active_stripe = ix;
                                    cx.notify();
                                }),
                            )
                        })),
                )
                .child(section("Toolbar widgets", muted))
                .child(
                    h_flex()
                        .gap_1()
                        .p_1()
                        .bg(theme.title_bar)
                        .child(components::toolbar_widget("w1", None, "demo-app"))
                        .child(components::toolbar_widget(
                            "w2",
                            Some(FerroIcon::Branch),
                            "feature/FERRO-101-a-very-long-branch-name",
                        ))
                        .child(div().flex_1())
                        .child(components::toolbar_widget("w3", None, "All tests"))
                        .child(components::icon_button("i1", FerroIcon::Run, "Run"))
                        .child(components::icon_button("i2", FerroIcon::Debug, "Debug"))
                        .child(components::icon_button("i3", FerroIcon::Stop, "Stop"))
                        .child(components::icon_button("i4", FerroIcon::Search, "Search"))
                        .child(components::icon_button(
                            "i5",
                            FerroIcon::Settings,
                            "Settings",
                        )),
                )
                .into_any_element(),
            Story::StatusBar => v_flex()
                .gap_4()
                .child(section("Status items and progress", muted))
                .child(
                    h_flex()
                        .h(px(26.))
                        .px_2()
                        .gap_3()
                        .text_xs()
                        .bg(theme.status_bar)
                        .text_color(muted)
                        .child(components::progress_bar(0.42, px(100.), cx))
                        .child("Indexing… 42%")
                        .child(div().flex_1())
                        .child(components::status_item("c", "128:17"))
                        .child(components::status_item("l", "LF"))
                        .child(components::status_item("e", "UTF-8"))
                        .child(components::status_item("lang", "Rust"))
                        .child(
                            components::status_item("b", "main")
                                .icon(Icon::from(FerroIcon::Branch)),
                        )
                        .child(
                            components::status_item("m", "212 MB")
                                .icon(Icon::from(FerroIcon::Memory)),
                        ),
                )
                .child(section("Progress values", muted))
                .children([0.0, 0.25, 0.5, 1.0, 1.5].map(|f| {
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .child(components::progress_bar(f, px(160.), cx))
                        .child(format!("{f}"))
                }))
                .into_any_element(),
            Story::States => h_flex()
                .gap_4()
                .h(px(220.))
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .border_1()
                        .border_color(theme.border)
                        .child(components::empty_state(
                            FerroIcon::Loading,
                            tr("tree-loading"),
                            None,
                            cx,
                        )),
                )
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .border_1()
                        .border_color(theme.border)
                        .child(components::empty_state(
                            FerroIcon::Database,
                            tr("database-empty"),
                            None,
                            cx,
                        )),
                )
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .border_1()
                        .border_color(theme.border)
                        .child(components::empty_state(
                            FerroIcon::Error,
                            tr("editor-load-failed"),
                            Some("simulated failure: reading a file".into()),
                            cx,
                        )),
                )
                .into_any_element(),
            Story::Icons => h_flex()
                .flex_wrap()
                .gap_2()
                .children(FerroIcon::ALL.iter().map(|icon| {
                    v_flex()
                        .w(px(110.))
                        .h(px(64.))
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme.border)
                        .child(Icon::from(*icon).size(px(20.)))
                        .child(div().text_xs().text_color(muted).child(format!("{icon:?}")))
                }))
                .into_any_element(),
            Story::ToolWindows => {
                let tool = self.tool;
                let flavor = self.flavor;
                v_flex()
                    .size_full()
                    .gap_2()
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap_1()
                            .children(ToolWindowKind::ALL.iter().map(|kind| {
                                let kind = *kind;
                                Button::new(kind.id())
                                    .small()
                                    .label(kind.title())
                                    .selected(kind == tool)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.tool = kind;
                                        this.rebuild_panel(window, cx);
                                    }))
                            })),
                    )
                    .child(
                        h_flex().gap_1().children(
                            [
                                (Flavor::Fast, "Fast mocks"),
                                (Flavor::Slow, "2 s latency"),
                                (Flavor::Failing, "Always failing"),
                            ]
                            .into_iter()
                            .map(|(f, label)| {
                                Button::new(label)
                                    .small()
                                    .ghost()
                                    .label(label)
                                    .selected(f == flavor)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.flavor = f;
                                        this.rebuild_panel(window, cx);
                                    }))
                            }),
                        ),
                    )
                    .child(
                        div()
                            .w(px(420.))
                            .flex_1()
                            .min_h(px(320.))
                            .border_1()
                            .border_color(theme.border)
                            .when_some(self.panel.clone(), |el, panel| el.child(panel)),
                    )
                    .into_any_element()
            }
            Story::Colors => {
                let tokens = [
                    ("background", theme.background),
                    ("foreground", theme.foreground),
                    ("muted", theme.muted),
                    ("muted_foreground", theme.muted_foreground),
                    ("border", theme.border),
                    ("primary", theme.primary),
                    ("accent", theme.accent),
                    ("title_bar", theme.title_bar),
                    ("status_bar", theme.status_bar),
                    ("tab_bar", theme.tab_bar),
                    ("tab_active", theme.tab_active),
                    ("list_active", theme.list_active),
                    ("selection", theme.selection),
                    ("success", theme.success),
                    ("warning", theme.warning),
                    ("danger", theme.danger),
                ];
                v_flex()
                    .gap_3()
                    .child(
                        h_flex().gap_1().children([
                            Button::new("t1")
                                .small()
                                .label(tr("action-theme-dark"))
                                .on_click(|_, w, cx| {
                                    w.dispatch_action(Box::new(ThemeFerroDark), cx)
                                }),
                            Button::new("t2")
                                .small()
                                .label(tr("action-theme-light"))
                                .on_click(|_, w, cx| {
                                    w.dispatch_action(Box::new(ThemeFerroLight), cx)
                                }),
                            Button::new("t3")
                                .small()
                                .label(tr("action-theme-high-contrast"))
                                .on_click(|_, w, cx| {
                                    w.dispatch_action(Box::new(ThemeHighContrast), cx)
                                }),
                        ]),
                    )
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap_2()
                            .children(tokens.into_iter().map(|(name, color)| {
                                v_flex()
                                    .w(px(120.))
                                    .gap_1()
                                    .child(
                                        div()
                                            .h(px(36.))
                                            .rounded(px(4.))
                                            .border_1()
                                            .border_color(theme.border)
                                            .bg(color),
                                    )
                                    .child(div().text_xs().text_color(muted).child(name))
                            })),
                    )
                    .into_any_element()
            }
            Story::Windows => v_flex()
                .gap_2()
                .child(section("Open full windows on mock services", muted))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("open-ws")
                                .primary()
                                .label("Workspace")
                                .on_click(|_, _, cx| {
                                    let _ = ferro_workspace::open_workspace(
                                        ferro_workspace::mock_services(),
                                        cx,
                                    );
                                }),
                        )
                        .child(
                            Button::new("open-slow")
                                .label("Workspace (2 s latency)")
                                .on_click(|_, _, cx| {
                                    let _ = ferro_workspace::open_workspace(
                                        Storybook::services(Flavor::Slow),
                                        cx,
                                    );
                                }),
                        )
                        .child(
                            Button::new("open-fail")
                                .label("Workspace (failing services)")
                                .on_click(|_, _, cx| {
                                    let _ = ferro_workspace::open_workspace(
                                        Storybook::services(Flavor::Failing),
                                        cx,
                                    );
                                }),
                        )
                        .child(
                            Button::new("open-welcome")
                                .label("Welcome screen")
                                .on_click(|_, _, cx| {
                                    let _ = ferro_workspace::open_welcome(cx);
                                }),
                        ),
                )
                .into_any_element(),
        }
    }
}

fn section(title: &'static str, muted: gpui_kit::Hsla) -> impl IntoElement {
    div().text_xs().text_color(muted).child(title)
}

impl Render for Storybook {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, side, border, fg) = (
            theme.background,
            theme.muted,
            theme.border,
            theme.foreground,
        );
        let body = self.story_body(window, cx);
        v_flex()
            .size_full()
            .bg(bg)
            .text_color(fg)
            .child(TitleBar::new().child(div().text_sm().child("FerroIDE Storybook")))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        v_flex()
                            .w(px(220.))
                            .h_full()
                            .p_2()
                            .gap_1()
                            .bg(side)
                            .border_r_1()
                            .border_color(border)
                            .children(Story::ALL.iter().map(|story| {
                                let story = *story;
                                Button::new(story.title())
                                    .ghost()
                                    .small()
                                    .w_full()
                                    .label(story.title())
                                    .selected(story == self.story)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.story = story;
                                        cx.notify();
                                    }))
                            })),
                    )
                    .child(
                        div()
                            .id("story")
                            .flex_1()
                            .h_full()
                            .p_4()
                            .overflow_y_scroll()
                            .child(body),
                    ),
            )
    }
}

fn main() {
    tracing_subscriber::fmt().init();
    gpui_kit::application()
        .with_assets(ferro_ui::FerroAssets)
        .run(|cx: &mut App| {
            gpui_kit::init(cx);
            ferro_ui::init(cx);
            let paths = Paths::standard();
            AppState::init(paths.load_settings(), paths, false, cx);
            ferro_workspace::init(cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(gpui_kit::Bounds::centered(
                    None,
                    size(px(1200.), px(800.)),
                    cx,
                ))),
                ..TitleBar::window_options()
            };
            gpui_kit::open_window(options, cx, |_, cx| {
                cx.new(|_| Storybook {
                    story: Story::Chrome,
                    tool: ToolWindowKind::Project,
                    flavor: Flavor::Fast,
                    panel: None,
                    active_stripe: 0,
                })
            })
            .expect("open storybook window");
        });
}
