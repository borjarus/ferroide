//! Bodies of the tool windows.
//!
//! Each tool window body is its own view in its own module, built from the
//! workspace's services. Views ask the workspace for things (open a file at
//! a line, show a diff) by dispatching the actions in [`crate::requests`].

pub mod common;
pub mod debug;
pub mod notifications;
pub mod problems;
pub mod project;
pub mod run;
pub mod services;
pub mod structure;
pub mod terminal;
pub mod todo;

use ferro_services::Services;
use ferro_ui::{FerroIcon, components, tr};
use gpui_kit::{
    AnyView, App, AppContext as _, Context, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window,
    component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex},
    div,
    prelude::FluentBuilder as _,
    px,
};

use crate::tool_window::ToolWindowKind;

/// What a tool window shows: any view.
pub struct ToolWindowBody(AnyView);

impl ToolWindowBody {
    pub fn view(&self) -> &AnyView {
        &self.0
    }

    /// The body as a concrete view, e.g. the Project tree.
    pub fn downcast<T: 'static>(&self) -> Option<Entity<T>> {
        self.0.clone().downcast::<T>().ok()
    }

    /// The body's own focus handle, so focusing the tool window puts the
    /// keyboard in its list rather than on the panel frame.
    pub fn focus_handle(&self, cx: &App) -> Option<FocusHandle> {
        fn of<T: Focusable + 'static>(view: &AnyView, cx: &App) -> Option<FocusHandle> {
            view.clone()
                .downcast::<T>()
                .ok()
                .map(|e| e.read(cx).focus_handle(cx))
        }
        let v = &self.0;
        of::<project::ProjectTree>(v, cx)
            .or_else(|| of::<problems::ProblemsView>(v, cx))
            .or_else(|| of::<terminal::TerminalView>(v, cx))
            .or_else(|| of::<run::RunView>(v, cx))
            .or_else(|| of::<debug::DebugView>(v, cx))
            .or_else(|| of::<structure::StructureView>(v, cx))
            .or_else(|| of::<todo::TodoView>(v, cx))
            .or_else(|| of::<notifications::NotificationsView>(v, cx))
            .or_else(|| of::<services::ServicesView>(v, cx))
    }
}

pub fn build(
    kind: ToolWindowKind,
    services: Services,
    window: &mut Window,
    cx: &mut App,
) -> ToolWindowBody {
    ToolWindowBody(match kind {
        ToolWindowKind::Project => cx.new(|cx| project::ProjectTree::new(services, cx)).into(),
        ToolWindowKind::Run => run::build(services, cx),
        ToolWindowKind::Debug => debug::build(services, window, cx),
        ToolWindowKind::Problems => problems::build(services, window, cx),
        ToolWindowKind::Terminal => terminal::build(services, window, cx),
        ToolWindowKind::Structure => structure::build(services, window, cx),
        ToolWindowKind::Todo => todo::build(services, window, cx),
        ToolWindowKind::Notifications => notifications::build(services, window, cx),
        ToolWindowKind::Services => self::services::build(services, window, cx),
        ToolWindowKind::Commit => crate::vcs::commit::build(services, window, cx),
        ToolWindowKind::Git => crate::vcs::git_log::build(services, window, cx),
        ToolWindowKind::Database | ToolWindowKind::Build => static_panel(kind, cx),
    })
}

/// A body showing fixed mock rows (Database and Build until their stages).
pub fn static_panel(kind: ToolWindowKind, cx: &mut App) -> AnyView {
    cx.new(|_| StaticPanel(kind)).into()
}

pub struct StaticPanel(ToolWindowKind);

impl Render for StaticPanel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        StaticBody(self.0)
    }
}

#[derive(IntoElement)]
pub struct StaticBody(ToolWindowKind);

struct MockRow {
    icon: FerroIcon,
    text: SharedString,
    detail: Option<SharedString>,
}

fn row(icon: FerroIcon, text: impl Into<SharedString>, detail: Option<&str>) -> MockRow {
    MockRow {
        icon,
        text: text.into(),
        detail: detail.map(|d| SharedString::from(d.to_string())),
    }
}

fn mock_rows(kind: ToolWindowKind) -> Result<Vec<MockRow>, (FerroIcon, &'static str)> {
    use FerroIcon as I;
    Ok(match kind {
        ToolWindowKind::Structure => vec![
            row(I::FileCode, "use anyhow::Result", None),
            row(I::FileCode, "use demo_app::{Config, greet}", None),
            row(I::Run, "fn main() -> Result<()>", Some("main.rs:4")),
        ],
        ToolWindowKind::Problems => vec![
            row(
                I::Error,
                "mismatched types: expected `u32`, found `&str`",
                Some("src/config.rs:12"),
            ),
            row(
                I::Warning,
                "unused variable: `retries`",
                Some("src/main.rs:6"),
            ),
            row(I::Info, "Typo: In word 'ferro'", Some("README.md:1")),
        ],
        ToolWindowKind::Git => vec![
            row(
                I::Commit,
                "Add config loading",
                Some("a1b2c3d · Ada L. · 2 h"),
            ),
            row(
                I::Commit,
                "Greet strangers politely",
                Some("9f8e7d6 · Linus T. · yesterday"),
            ),
            row(
                I::Commit,
                "Merge branch 'feature/web'",
                Some("5c4b3a2 · Grace H. · 3 d"),
            ),
            row(I::Commit, "Initial commit", Some("0000001 · Ada L. · 2 w")),
        ],
        ToolWindowKind::Terminal => vec![
            row(I::Terminal, "PS C:\\Projects\\demo-app> cargo run", None),
            row(I::Terminal, "    Finished `dev` profile in 0.08s", None),
            row(I::Terminal, "Hello, FerroIDE", None),
        ],
        ToolWindowKind::Todo => vec![
            row(
                I::Todo,
                "TODO: retry with exponential backoff",
                Some("src/config.rs:18"),
            ),
            row(I::Todo, "FIXME: handle empty names", Some("src/lib.rs:7")),
        ],
        ToolWindowKind::Notifications => vec![
            row(I::Ok, "Indexing finished", Some("demo-app")),
            row(I::Info, "Running on mock services (Stage 1)", None),
        ],
        ToolWindowKind::Build => vec![
            row(I::Build, "cargo build", None),
            row(I::Build, "cargo test", None),
            row(I::Build, "cargo clippy", None),
            row(I::Run, "cargo run", None),
        ],
        ToolWindowKind::Debug => return Err((I::Debug, "debug-empty")),
        ToolWindowKind::Database => return Err((I::Database, "database-empty")),
        ToolWindowKind::Project
        | ToolWindowKind::Run
        | ToolWindowKind::Commit
        | ToolWindowKind::Services => {
            return Err((I::Info, "tool-empty"));
        }
    })
}

impl gpui_kit::RenderOnce for StaticBody {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, hover) = (theme.muted_foreground, theme.list_hover);
        match mock_rows(self.0) {
            Err((icon, key)) => components::empty_state(icon, tr(key), None, cx).into_any_element(),
            Ok(rows) => v_flex()
                .id(self.0.id())
                .size_full()
                .py_1()
                .overflow_y_scroll()
                .children(rows.into_iter().map(|r| {
                    h_flex()
                        .h(px(24.))
                        .px_2()
                        .gap_2()
                        .text_sm()
                        .hover(|el| el.bg(hover))
                        .child(Icon::from(r.icon).small().text_color(muted))
                        .child(div().flex_1().truncate().child(r.text))
                        .when_some(r.detail, |el, d| {
                            el.child(div().text_xs().text_color(muted).child(d))
                        })
                }))
                .into_any_element(),
        }
    }
}
