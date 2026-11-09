//! Tool windows: the panels around the editor (Project, Commit, Terminal, …).
//!
//! Each tool window is a dock panel, so it can be dragged between the left,
//! right and bottom docks, zoomed, and persisted with the layout. Since
//! Stage 2 every body is a full component on mock services (see `panels`
//! and `vcs`).

use ferro_services::Services;
use ferro_ui::{FerroIcon, tr};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Window,
    component::{
        ActiveTheme as _, Icon,
        dock::{BasePanel, DockPlacement, Panel, PanelControl, PanelEvent},
    },
    div,
};

use crate::panels::{self, ToolWindowBody};

/// Every tool window FerroIDE has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolWindowKind {
    Project,
    Commit,
    Structure,
    Run,
    Debug,
    Problems,
    Git,
    Terminal,
    Todo,
    Notifications,
    Database,
    Build,
    Services,
}

impl ToolWindowKind {
    /// In status-bar order: within each dock, the toggles appear in this
    /// order.
    pub const ALL: [ToolWindowKind; 13] = [
        ToolWindowKind::Project,
        ToolWindowKind::Commit,
        ToolWindowKind::Git,
        ToolWindowKind::Structure,
        ToolWindowKind::Database,
        ToolWindowKind::Terminal,
        ToolWindowKind::Run,
        ToolWindowKind::Debug,
        ToolWindowKind::Problems,
        ToolWindowKind::Todo,
        ToolWindowKind::Build,
        ToolWindowKind::Services,
        ToolWindowKind::Notifications,
    ];

    /// Stable id: the dock's panel name, the persisted key and the element id.
    pub fn id(self) -> &'static str {
        match self {
            Self::Project => "ferro.tool.project",
            Self::Commit => "ferro.tool.commit",
            Self::Structure => "ferro.tool.structure",
            Self::Run => "ferro.tool.run",
            Self::Debug => "ferro.tool.debug",
            Self::Problems => "ferro.tool.problems",
            Self::Git => "ferro.tool.git",
            Self::Terminal => "ferro.tool.terminal",
            Self::Todo => "ferro.tool.todo",
            Self::Notifications => "ferro.tool.notifications",
            Self::Database => "ferro.tool.database",
            Self::Build => "ferro.tool.build",
            Self::Services => "ferro.tool.services",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.id() == id)
    }

    pub fn title_key(self) -> &'static str {
        match self {
            Self::Project => "tool-project",
            Self::Commit => "tool-commit",
            Self::Structure => "tool-structure",
            Self::Run => "tool-run",
            Self::Debug => "tool-debug",
            Self::Problems => "tool-problems",
            Self::Git => "tool-git",
            Self::Terminal => "tool-terminal",
            Self::Todo => "tool-todo",
            Self::Notifications => "tool-notifications",
            Self::Database => "tool-database",
            Self::Build => "tool-build",
            Self::Services => "tool-services",
        }
    }

    pub fn title(self) -> SharedString {
        tr(self.title_key())
    }

    pub fn icon(self) -> FerroIcon {
        match self {
            Self::Project => FerroIcon::Project,
            Self::Commit => FerroIcon::Commit,
            Self::Structure => FerroIcon::Structure,
            Self::Run => FerroIcon::Run,
            Self::Debug => FerroIcon::Debug,
            Self::Problems => FerroIcon::Problems,
            Self::Git => FerroIcon::Git,
            Self::Terminal => FerroIcon::Terminal,
            Self::Todo => FerroIcon::Todo,
            Self::Notifications => FerroIcon::Notifications,
            Self::Database => FerroIcon::Database,
            Self::Build => FerroIcon::Build,
            Self::Services => FerroIcon::Services,
        }
    }

    /// The action that toggles it (also the source of its shortcut).
    pub fn action_id(self) -> &'static str {
        match self {
            Self::Project => "ferro::ToggleProject",
            Self::Commit => "ferro::ToggleCommit",
            Self::Structure => "ferro::ToggleStructure",
            Self::Run => "ferro::ToggleRun",
            Self::Debug => "ferro::ToggleDebug",
            Self::Problems => "ferro::ToggleProblems",
            Self::Git => "ferro::ToggleGit",
            Self::Terminal => "ferro::ToggleTerminal",
            Self::Todo => "ferro::ToggleTodo",
            Self::Notifications => "ferro::ToggleNotifications",
            Self::Database => "ferro::ToggleDatabase",
            Self::Build => "ferro::ToggleBuild",
            Self::Services => "ferro::ToggleServices",
        }
    }

    /// Where the tool window docks by default (the Zed model, plan §8
    /// Stage 1): left — Project, Git/Commit, Outline/Structure, Database;
    /// bottom — Terminal, Run, Debug, Problems, TODO, Build, Services;
    /// right — Notifications. The user can move it to another dock from its
    /// status-bar toggle; the layout remembers that.
    pub fn default_placement(self) -> DockPlacement {
        match self {
            Self::Project | Self::Commit | Self::Git | Self::Structure | Self::Database => {
                DockPlacement::Left
            }
            Self::Terminal
            | Self::Run
            | Self::Debug
            | Self::Problems
            | Self::Todo
            | Self::Build
            | Self::Services => DockPlacement::Bottom,
            Self::Notifications => DockPlacement::Right,
        }
    }
}

/// A dockable tool window.
pub struct ToolWindowPanel {
    kind: ToolWindowKind,
    focus_handle: FocusHandle,
    body: ToolWindowBody,
}

impl ToolWindowPanel {
    pub fn new(
        kind: ToolWindowKind,
        services: Services,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            kind,
            focus_handle: cx.focus_handle(),
            body: panels::build(kind, services, window, cx),
        }
    }

    pub fn build(
        kind: ToolWindowKind,
        services: Services,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| Self::new(kind, services, window, cx))
    }

    pub fn kind(&self) -> ToolWindowKind {
        self.kind
    }

    pub fn body(&self) -> &ToolWindowBody {
        &self.body
    }
}

impl BasePanel for ToolWindowPanel {
    fn panel_name(&self) -> &'static str {
        self.kind.id()
    }

    /// Tool windows are hidden with their status-bar toggle, never closed
    /// from the tab bar, as in Zed.
    fn closable(&self, _: &App) -> bool {
        false
    }

    fn zoomable(&self, _: &App) -> bool {
        true
    }
}

impl Panel for ToolWindowPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(self.kind.title())
    }

    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(Icon::from(self.kind.icon()).text_color(muted))
            .child(self.kind.title())
    }

    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        Some(PanelControl::Menu)
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl EventEmitter<PanelEvent> for ToolWindowPanel {}

impl Focusable for ToolWindowPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.body
            .focus_handle(cx)
            .unwrap_or_else(|| self.focus_handle.clone())
    }
}

impl Render for ToolWindowPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id(self.kind.id())
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().muted)
            .child(self.body.view().clone())
    }
}
