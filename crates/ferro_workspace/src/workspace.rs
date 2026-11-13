//! The IDE window in the Zed layout (plan §8, Stage 1): a slim title bar,
//! the dock area — the editor panes with three docks around them (left,
//! right, bottom) — and the status bar, whose icons toggle the dock panels.

use std::{collections::HashMap, path::PathBuf, rc::Rc, time::Duration};

use ferro_db::{Location, NavLocation};
use ferro_services::{
    Branch, ProjectEvent, RunConfiguration, RunEvent, RunState, Services, TaskProgress, TextPos,
};
use ferro_ui::tr;
use gpui_kit::{
    App, AppContext as _, Context, Entity, EntityId, FocusHandle, Focusable, Global,
    InteractiveElement as _, IntoElement, KeyContext, ParentElement as _, Render, Styled as _,
    Subscription, Task, WeakEntity, Window,
    component::{
        ActiveTheme as _, Placement, WindowExt as _,
        dock::{
            DockArea, DockAreaState, DockEvent, DockLayout, DockPlacement, DockSkin,
            DockToggleEvent, InsertTarget, PaneRef, PanelId, PanelStyle, panel_handle,
            register_panel,
        },
        notification::Notification,
    },
    div, px,
};

use crate::{
    actions::*,
    app_state::AppState,
    editor::{EDITOR_PANEL_NAME, EditorCommand, EditorEvent, EditorPanel, EditorTabState},
    quick_open::QuickOpenMode,
    requests::{OpenDiff, OpenLocation, OpenMerge, RevealInProject},
    settings_window::SettingsPage,
    tool_window::{ToolWindowKind, ToolWindowPanel},
};

/// Bumped when the persisted layout schema changes; older layouts are
/// discarded instead of restored incorrectly.
///
/// 2: the Zed layout — tool windows moved between docks.
pub const LAYOUT_VERSION: usize = 2;

/// How many places Back remembers.
const NAV_HISTORY_LIMIT: usize = 100;

const DOCK_PLACEMENTS: [DockPlacement; 3] = [
    DockPlacement::Left,
    DockPlacement::Bottom,
    DockPlacement::Right,
];
const ALL_PLACEMENTS: [DockPlacement; 4] = [
    DockPlacement::Center,
    DockPlacement::Left,
    DockPlacement::Bottom,
    DockPlacement::Right,
];

/// Services of every open workspace, keyed by its dock area, so panel
/// builders that restore a persisted layout can reach them.
#[derive(Default)]
struct WorkspaceServices(HashMap<EntityId, Services>);
impl Global for WorkspaceServices {}

fn services_for(area: EntityId, cx: &App) -> Services {
    cx.try_global::<WorkspaceServices>()
        .and_then(|registry| registry.0.get(&area).cloned())
        .unwrap_or_else(|| Services::mock(Default::default()))
}

struct BuildersRegistered;
impl Global for BuildersRegistered {}

/// Register the panel builders the dock uses to restore a saved layout.
fn register_builders(cx: &mut App) {
    if cx.has_global::<BuildersRegistered>() {
        return;
    }
    cx.set_global(BuildersRegistered);
    crate::vcs::register_panels(cx, services_for);
    for kind in ToolWindowKind::ALL {
        register_panel(cx, kind.id(), move |ctx, window, cx| {
            let services = services_for(ctx.dock_area().entity_id(), cx);
            panel_handle(ToolWindowPanel::build(kind, services, window, cx))
        });
    }
    register_panel(cx, EDITOR_PANEL_NAME, |ctx, window, cx| {
        let services = services_for(ctx.dock_area().entity_id(), cx);
        let state = match ctx.info() {
            gpui_kit::component::dock::PanelInfo::Panel(value) => {
                serde_json::from_value::<EditorTabState>(value.clone()).ok()
            }
            _ => None,
        };
        let state = state.unwrap_or_else(|| EditorTabState {
            path: services.project.roots()[0].join("README.md"),
            pinned: false,
        });
        let editor = EditorPanel::open(state.path, state.pinned, &services, window, cx);
        // Put the caret where it was when the window closed.
        let key = project_key(&services);
        let path = editor.read(cx).path().to_path_buf();
        if let Ok(Some((line, column))) = AppState::global(cx).db.cursor(&key, &path) {
            editor.update(cx, |e, cx| e.go_to(TextPos::new(line, column), window, cx));
        }
        panel_handle(editor)
    });
}

/// The key the project's state is stored under: its first root.
pub(crate) fn project_key(services: &Services) -> String {
    services
        .project
        .roots()
        .first()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| services.project.name())
}

/// A place in the code Back and Forward return to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavEntry {
    pub path: PathBuf,
    pub position: Option<ferro_services::TextPos>,
}

/// Mutable status-bar data fed by the services.
#[derive(Default)]
pub struct StatusData {
    pub branch: Option<Branch>,
    pub branches: Vec<Branch>,
    pub task: Option<TaskProgress>,
    pub memory_mb: Option<u64>,
}

pub struct Workspace {
    pub(crate) services: Services,
    project_key: String,
    focus_handle: FocusHandle,
    pub(crate) dock_area: Entity<DockArea>,
    skin: Rc<DockSkin>,
    pub(crate) tool_windows: HashMap<ToolWindowKind, Entity<ToolWindowPanel>>,
    /// Open editors, least recently activated first.
    pub(crate) editors: Vec<Entity<EditorPanel>>,
    pub(crate) active_editor: Option<WeakEntity<EditorPanel>>,
    last_tool_window: Option<ToolWindowKind>,
    /// Paths of closed editor tabs, most recent last, for Reopen Closed Tab.
    closed_tabs: Vec<PathBuf>,
    /// Docks closed by "Hide All Tool Windows", reopened by the next press.
    hidden_docks: Vec<DockPlacement>,
    /// Back/Forward navigation history, most recent last.
    nav_back: Vec<NavEntry>,
    nav_forward: Vec<NavEntry>,
    pub(crate) status: StatusData,
    pub(crate) run_configs: Vec<RunConfiguration>,
    pub(crate) selected_run: usize,
    pub(crate) run_state: RunState,
    save_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(services: Services, window: &mut Window, cx: &mut Context<Self>) -> Self {
        register_builders(cx);
        let (dock_area, skin) = DockSkin::dock_area("workspace", Some(LAYOUT_VERSION), window, cx);
        skin.set_panel_style(PanelStyle::TabBar, cx);
        skin.set_close_button_visible(true, cx);
        // Docks are toggled from the status bar, not from the tab bars.
        skin.set_toggle_button_visible(false, cx);
        cx.default_global::<WorkspaceServices>()
            .0
            .insert(dock_area.entity_id(), services.clone());

        let project_key = project_key(&services);

        let mut subscriptions = vec![
            cx.subscribe_in(
                &dock_area,
                window,
                |this, _, event: &DockEvent, window, cx| {
                    if let DockEvent::LayoutChanged = event {
                        this.on_layout_changed(window, cx);
                    }
                },
            ),
            cx.subscribe(&dock_area, |_, _, _: &DockToggleEvent, cx| cx.notify()),
        ];
        subscriptions.push(cx.on_release(|this, cx| {
            let id = this.dock_area.entity_id();
            if cx.has_global::<WorkspaceServices>() {
                cx.global_mut::<WorkspaceServices>().0.remove(&id);
            }
        }));

        let mut this = Self {
            run_configs: services.run.configurations(),
            services,
            project_key,
            focus_handle: cx.focus_handle(),
            dock_area,
            skin,
            tool_windows: HashMap::new(),
            editors: Vec::new(),
            active_editor: None,
            last_tool_window: None,
            closed_tabs: Vec::new(),
            hidden_docks: Vec::new(),
            nav_back: Vec::new(),
            nav_forward: Vec::new(),
            status: StatusData::default(),
            selected_run: 0,
            run_state: RunState::Idle,
            save_task: None,
            _subscriptions: subscriptions,
        };

        if !this.restore_layout(window, cx) {
            this.install_default_layout(window, cx);
        }
        this.restore_history(cx);
        for root in this.services.project.roots() {
            AppState::load_project_settings(&root, cx);
        }
        this.watch_project(cx);
        this.start_service_feeds(cx);
        this.focus_handle.focus(window, cx);
        this
    }

    /// Follow root folders being added or removed, and edits of a
    /// project's `.ferroide/settings.toml`.
    fn watch_project(&mut self, cx: &mut Context<Self>) {
        let events = self.services.project.subscribe();
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                let alive = this.update(cx, |this, cx| match &event {
                    ProjectEvent::RootsChanged => {
                        let roots = this.services.project.roots();
                        cx.global_mut::<AppState>()
                            .project_settings
                            .retain(|(root, _)| roots.contains(root));
                        for root in roots {
                            AppState::load_project_settings(&root, cx);
                        }
                        this.schedule_save(cx);
                    }
                    ProjectEvent::Added(path)
                    | ProjectEvent::Changed(path)
                    | ProjectEvent::Removed(path) => {
                        let root = this.services.project.roots().into_iter().find(|root| {
                            *path == ferro_settings::Paths::project_settings_file(root)
                        });
                        if let Some(root) = root {
                            AppState::load_project_settings(&root, cx);
                        }
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Back / Forward and recent files as the project left them.
    fn restore_history(&mut self, cx: &mut Context<Self>) {
        let db = AppState::global(cx).db.clone();
        if let Ok((back, forward)) = db.nav_history(&self.project_key) {
            let entry = |n: NavLocation| NavEntry {
                path: n.path,
                position: n.position.map(|(line, column)| TextPos::new(line, column)),
            };
            if self.nav_back.is_empty() && self.nav_forward.is_empty() {
                self.nav_back = back.into_iter().map(entry).collect();
                self.nav_forward = forward.into_iter().map(entry).collect();
            }
        }
        if let Ok(files) = db.recent_files(&self.project_key)
            && !files.is_empty()
        {
            let id = self.dock_area.entity_id();
            crate::quick_open::seed_recent_files(id, files, cx);
        }
    }

    /// Persist what is not part of the dock layout: Back / Forward, recent
    /// files and the caret of every open file.
    fn save_history(&self, cx: &App) {
        let db = AppState::global(cx).db.clone();
        let key = &self.project_key;
        let nav = |list: &[NavEntry]| -> Vec<NavLocation> {
            list.iter()
                .map(|e| NavLocation {
                    path: e.path.clone(),
                    position: e.position.map(|p| (p.line, p.column)),
                })
                .collect()
        };
        let mut result = db.save_nav_history(key, &nav(&self.nav_back), &nav(&self.nav_forward));
        // Most recent first, the active file at the top.
        let mut recent = crate::quick_open::recent_files(self, cx);
        if let Some(active) = recent.pop() {
            recent.insert(0, active);
        }
        for path in recent.iter().rev() {
            result = result.and(db.touch_recent_file(key, path));
        }
        for editor in &self.editors {
            let editor = editor.read(cx);
            if let Some(caret) = editor.caret(cx) {
                result = result.and(db.save_cursor(
                    key,
                    &Location {
                        path: editor.path().to_path_buf(),
                        line: caret.line,
                        column: caret.column,
                    },
                ));
            }
        }
        if let Err(err) = result {
            tracing::warn!("could not save the workspace state: {err:#}");
        }
    }

    /// Remember a visited place for Recent Locations.
    fn record_location(&self, entry: &NavEntry, cx: &App) {
        let Some(position) = entry.position else {
            return;
        };
        let snippet = self
            .editors
            .iter()
            .find(|e| e.read(cx).path() == entry.path)
            .and_then(|e| e.read(cx).text(cx))
            .and_then(|text| text.lines().nth(position.line as usize).map(str::to_string))
            .unwrap_or_default();
        let location = Location {
            path: entry.path.clone(),
            line: position.line,
            column: position.column,
        };
        if let Err(err) =
            AppState::global(cx)
                .db
                .add_recent_location(&self.project_key, &location, &snippet)
        {
            tracing::warn!("could not record a recent location: {err:#}");
        }
    }

    pub fn project_key(&self) -> &str {
        &self.project_key
    }

    pub fn build(services: Services, window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| Self::new(services, window, cx))
    }

    pub fn project_name(&self) -> String {
        self.services.project.name()
    }

    pub fn dock_area(&self) -> &Entity<DockArea> {
        &self.dock_area
    }

    pub fn skin(&self) -> &Rc<DockSkin> {
        &self.skin
    }

    // -----------------------------------------------------------------------
    // Layout: default, restore, persist
    // -----------------------------------------------------------------------

    fn tool_window(
        &mut self,
        kind: ToolWindowKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ToolWindowPanel> {
        if let Some(panel) = self.tool_windows.get(&kind) {
            return panel.clone();
        }
        let panel = ToolWindowPanel::build(kind, self.services.clone(), window, cx);
        self.adopt_tool_window(panel.clone(), window, cx);
        panel
    }

    fn adopt_tool_window(
        &mut self,
        panel: Entity<ToolWindowPanel>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let kind = panel.read(cx).kind();
        self.tool_windows.insert(kind, panel);
    }

    /// The body of a tool window as its concrete view, creating the tool
    /// window if needed.
    pub fn tool_window_body<T: 'static>(
        &mut self,
        kind: ToolWindowKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<T>> {
        self.tool_window(kind, window, cx)
            .read(cx)
            .body()
            .downcast::<T>()
    }

    fn adopt_editor(
        &mut self,
        editor: Entity<EditorPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self._subscriptions.push(cx.subscribe_in(
            &editor,
            window,
            |this, editor, event: &EditorEvent, window, cx| {
                match event {
                    EditorEvent::Activated => {
                        // Switching tabs is a navigation Back returns from;
                        // navigation through `open_location` has already
                        // made the editor active and recorded itself.
                        if this.active_editor_entity().as_ref() != Some(editor)
                            && let Some(from) = this.current_location(cx)
                        {
                            this.record_location(&from, cx);
                            this.push_back(from);
                        }
                        this.mark_active_editor(editor);
                        this.schedule_save(cx);
                        crate::panels::structure::active_editor_changed(this, cx);
                    }
                    EditorEvent::CursorMoved | EditorEvent::Saved | EditorEvent::Reloaded => {}
                    EditorEvent::SaveFailed(message) => {
                        window.push_notification(Notification::error(message.clone()), cx);
                    }
                    EditorEvent::CompareWithDisk => {
                        let (path, texts) = {
                            let e = editor.read(cx);
                            (e.path().to_path_buf(), e.compare_texts(cx))
                        };
                        if let Some((disk, text)) = texts {
                            let panel = crate::vcs::diff_view::DiffPanel::open_disk_and_editor(
                                path,
                                disk,
                                text,
                                this.services.clone(),
                                cx,
                            );
                            this.add_center_panel(panel.clone(), window, cx);
                            panel.focus_handle(cx).focus(window, cx);
                        }
                    }
                }
                cx.notify();
            },
        ));
        self.editors.push(editor);
    }

    fn mark_active_editor(&mut self, editor: &Entity<EditorPanel>) {
        if let Some(ix) = self.editors.iter().position(|e| e == editor) {
            let editor = self.editors.remove(ix);
            self.editors.push(editor);
        }
        self.active_editor = Some(editor.downgrade());
    }

    fn install_default_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut groups: HashMap<DockPlacement, DockLayout> = HashMap::new();
        for kind in ToolWindowKind::ALL {
            let panel = self.tool_window(kind, window, cx);
            let layout = groups
                .remove(&kind.default_placement())
                .unwrap_or_else(DockLayout::tabs);
            groups.insert(
                kind.default_placement(),
                layout.panel_view(panel_handle(panel), cx),
            );
        }

        let mut center = DockLayout::tabs();
        for path in self.services.project.initial_files() {
            let editor = EditorPanel::open(path, false, &self.services, window, cx);
            self.adopt_editor(editor.clone(), window, cx);
            center = center.panel_view(panel_handle(editor), cx);
        }

        let area = self.dock_area.clone();
        area.update(cx, |area, cx| {
            area.set_center(center.active_index(0), window, cx);
            for placement in DOCK_PLACEMENTS {
                if let Some(layout) = groups.remove(&placement) {
                    area.set_dock(placement, layout.active_index(0), window, cx);
                }
            }
            area.set_dock_size(DockPlacement::Left, px(280.), window, cx);
            area.set_dock_size(DockPlacement::Right, px(300.), window, cx);
            area.set_dock_size(DockPlacement::Bottom, px(220.), window, cx);
            // Like Zed, open with only the left dock (Project) visible.
            for placement in [DockPlacement::Bottom, DockPlacement::Right] {
                if area.is_dock_open(placement) {
                    area.toggle_dock(placement, window, cx);
                }
            }
        });
        if let Some(first) = self.editors.first().cloned() {
            self.mark_active_editor(&first);
        }
    }

    /// Restore the layout saved for this project. Returns false when there
    /// is none or it cannot be used.
    fn restore_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        // Stages 1–3 kept layouts in JSON files; they are imported once.
        let app = AppState::global(cx);
        let state = app
            .db
            .load_layout(&self.project_key)
            .ok()
            .flatten()
            .or_else(|| app.paths.load_layout(&self.project_key));
        let Some(state) = state.and_then(|v| serde_json::from_value::<DockAreaState>(v).ok())
        else {
            return false;
        };
        if state.version != Some(LAYOUT_VERSION) {
            return false;
        }
        let area = self.dock_area.clone();
        let loaded = area.update(cx, |area, cx| area.load(state, window, cx));
        if let Err(err) = loaded {
            tracing::warn!("discarding saved layout: {err:#}");
            return false;
        }
        self.adopt_panels_from_area(window, cx);
        // Tool windows missing from an old layout are created on demand.
        self.tool_windows.len() + self.editors.len() > 0
    }

    /// Pick up the entities the dock created while restoring a layout.
    fn adopt_panels_from_area(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let area = self.dock_area.read(cx);
        let mut tools = Vec::new();
        let mut editors = Vec::new();
        for placement in ALL_PLACEMENTS {
            let Some(tree) = area.layout(placement) else {
                continue;
            };
            for id in tree.panels() {
                let Some(view) = area.panel(id).map(|p| p.view()) else {
                    continue;
                };
                if let Ok(tool) = view.clone().downcast::<ToolWindowPanel>() {
                    tools.push(tool);
                } else if let Ok(editor) = view.downcast::<EditorPanel>() {
                    editors.push(editor);
                }
            }
        }
        for tool in tools {
            self.adopt_tool_window(tool, window, cx);
        }
        for editor in editors {
            self.adopt_editor(editor, window, cx);
        }
        if let Some(last) = self.editors.last().cloned() {
            self.active_editor = Some(last.downgrade());
        }
    }

    fn on_layout_changed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        // Forget editors the dock no longer holds (closed tabs).
        let area = self.dock_area.read(cx);
        let alive: Vec<PanelId> = ALL_PLACEMENTS
            .iter()
            .filter_map(|p| area.layout(*p))
            .flat_map(|tree| tree.panels().collect::<Vec<_>>())
            .collect();
        // A tab closed with its close button keeps its edits: save it.
        let (kept, closed): (Vec<_>, Vec<_>) = std::mem::take(&mut self.editors)
            .into_iter()
            .partition(|e| alive.contains(&PanelId::from(e.entity_id())));
        self.editors = kept;
        for editor in closed {
            if let Some(caret) = editor.read(cx).caret(cx) {
                let location = Location {
                    path: editor.read(cx).path().to_path_buf(),
                    line: caret.line,
                    column: caret.column,
                };
                let _ = AppState::global(cx)
                    .db
                    .save_cursor(&self.project_key, &location);
            }
            save_if_dirty(&editor, cx);
        }
        if self
            .active_editor
            .as_ref()
            .and_then(|e| e.upgrade())
            .is_none_or(|e| !alive.contains(&PanelId::from(e.entity_id())))
        {
            self.active_editor = self.editors.last().map(|e| e.downgrade());
        }
        self.schedule_save(cx);
        cx.notify();
    }

    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        if !AppState::global(cx).persist {
            return;
        }
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let _ = this.update(cx, |this, cx| this.save_layout(cx));
        }));
    }

    pub fn save_layout(&self, cx: &App) {
        let state = self.dock_area.read(cx).dump(cx);
        match serde_json::to_value(&state) {
            Ok(value) => {
                let app = AppState::global(cx);
                if let Err(err) = app.db.save_layout(&self.project_key, &value) {
                    tracing::warn!("could not save layout: {err:#}");
                } else {
                    // The database has it now; drop the Stage 1–3 file.
                    app.paths.delete_layout(&self.project_key);
                }
            }
            Err(err) => tracing::warn!("could not serialize layout: {err}"),
        }
        self.save_history(cx);
    }

    pub(crate) fn reset_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        AppState::global(cx).paths.delete_layout(&self.project_key);
        if let Err(err) = AppState::global(cx).db.delete_layout(&self.project_key) {
            tracing::warn!("could not reset the saved layout: {err:#}");
        }
        let mut groups: HashMap<DockPlacement, DockLayout> = HashMap::new();
        for kind in ToolWindowKind::ALL {
            let panel = self.tool_window(kind, window, cx);
            let layout = groups
                .remove(&kind.default_placement())
                .unwrap_or_else(DockLayout::tabs);
            groups.insert(
                kind.default_placement(),
                layout.panel_view(panel_handle(panel), cx),
            );
        }
        let mut center = DockLayout::tabs();
        for editor in &self.editors {
            center = center.panel_view(panel_handle(editor.clone()), cx);
        }
        let area = self.dock_area.clone();
        area.update(cx, |area, cx| {
            area.set_zoomed_out(window, cx);
            area.set_center(center, window, cx);
            for placement in DOCK_PLACEMENTS {
                if let Some(layout) = groups.remove(&placement) {
                    area.set_dock(placement, layout.active_index(0), window, cx);
                }
                let open = placement == DockPlacement::Left;
                if area.is_dock_open(placement) != open {
                    area.toggle_dock(placement, window, cx);
                }
            }
        });
        self.hidden_docks.clear();
    }

    // -----------------------------------------------------------------------
    // Tool windows
    // -----------------------------------------------------------------------

    fn placement_of(&self, id: PanelId, cx: &App) -> Option<DockPlacement> {
        let area = self.dock_area.read(cx);
        ALL_PLACEMENTS
            .into_iter()
            .find(|p| area.layout(*p).is_some_and(|t| t.panels().any(|x| x == id)))
    }

    /// Whether `id` is the displayed tab of its group.
    fn is_active_in_group(&self, id: PanelId, placement: DockPlacement, cx: &App) -> bool {
        let area = self.dock_area.read(cx);
        let Some(tree) = area.layout(placement) else {
            return false;
        };
        let Some(node) = tree.find_panel_node(id).and_then(|n| tree.find_node(n)) else {
            return false;
        };
        matches!(node.kind(), PaneRef::Tabs { panels, active_ix } if panels.get(active_ix) == Some(&id))
    }

    /// Whether the tool window is on screen.
    pub fn is_tool_window_visible(&self, kind: ToolWindowKind, cx: &App) -> bool {
        let Some(panel) = self.tool_windows.get(&kind) else {
            return false;
        };
        let id = PanelId::from(panel.entity_id());
        let Some(placement) = self.placement_of(id, cx) else {
            return false;
        };
        let open =
            placement == DockPlacement::Center || self.dock_area.read(cx).is_dock_open(placement);
        open && self.is_active_in_group(id, placement, cx)
    }

    /// Status-bar toggle behavior (Zed): show (and focus) the tool window,
    /// or close its dock if it is already the visible one.
    pub fn toggle_tool_window(
        &mut self,
        kind: ToolWindowKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = self.tool_window(kind, window, cx);
        let id = PanelId::from(panel.entity_id());
        let area = self.dock_area.clone();

        match self.placement_of(id, cx) {
            None => {
                // Closed earlier: dock it again where it belongs.
                let placement = kind.default_placement();
                area.update(cx, |area, cx| {
                    area.add_panel_view(panel_handle(panel.clone()), placement, None, window, cx);
                    if !area.is_dock_open(placement) {
                        area.toggle_dock(placement, window, cx);
                    }
                });
                panel.focus_handle(cx).focus(window, cx);
                self.last_tool_window = Some(kind);
            }
            Some(placement) => {
                if self.is_tool_window_visible(kind, cx) && placement != DockPlacement::Center {
                    area.update(cx, |area, cx| area.toggle_dock(placement, window, cx));
                    self.focus_active_editor(window, cx);
                } else {
                    area.update(cx, |area, cx| {
                        area.select_panel(id, window, cx);
                        if placement != DockPlacement::Center && !area.is_dock_open(placement) {
                            area.toggle_dock(placement, window, cx);
                        }
                    });
                    panel.focus_handle(cx).focus(window, cx);
                    self.last_tool_window = Some(kind);
                }
            }
        }
        self.hidden_docks.clear();
        cx.notify();
    }

    /// Show (and focus) a tool window; unlike the toggle, never hides it.
    pub fn show_tool_window(
        &mut self,
        kind: ToolWindowKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_tool_window_visible(kind, cx) {
            self.toggle_tool_window(kind, window, cx);
        } else if let Some(panel) = self.tool_windows.get(&kind) {
            panel.focus_handle(cx).focus(window, cx);
        }
    }

    /// Open or close a whole dock (VS Code/Zed: Ctrl+B, Ctrl+J, Ctrl+Alt+B).
    pub fn toggle_dock(
        &mut self,
        placement: DockPlacement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let area = self.dock_area.clone();
        let opening = !area.read(cx).is_dock_open(placement);
        area.update(cx, |area, cx| area.toggle_dock(placement, window, cx));
        if !opening {
            self.focus_active_editor(window, cx);
        }
        self.hidden_docks.clear();
        cx.notify();
    }

    /// The dock a tool window is in now, or the one it opens in.
    pub fn tool_window_placement(&self, kind: ToolWindowKind, cx: &App) -> DockPlacement {
        self.tool_windows
            .get(&kind)
            .and_then(|panel| self.placement_of(PanelId::from(panel.entity_id()), cx))
            .unwrap_or_else(|| kind.default_placement())
    }

    /// Move a tool window to another dock ("Dock Left / Right / Bottom" on
    /// its status-bar toggle) and show it there.
    pub fn move_tool_window(
        &mut self,
        kind: ToolWindowKind,
        to: DockPlacement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = self.tool_window(kind, window, cx);
        let id = PanelId::from(panel.entity_id());
        let area = self.dock_area.clone();
        if self.placement_of(id, cx) != Some(to) {
            // A tab group in the target dock to join, if it has any panel.
            let group = {
                let area = area.read(cx);
                area.layout(to).and_then(|tree| {
                    let first = tree.panels().find(|p| *p != id)?;
                    tree.find_panel_node(first)
                })
            };
            area.update(cx, |area, cx| match group {
                Some(node) => area.move_panel(
                    id,
                    InsertTarget::Tabs {
                        node,
                        ix: None,
                        activate: true,
                    },
                    window,
                    cx,
                ),
                None => {
                    if area.panel(id).is_some() {
                        area.remove_panel(panel.clone(), window, cx);
                    }
                    area.add_panel_view(panel_handle(panel.clone()), to, None, window, cx);
                }
            });
        }
        area.update(cx, |area, cx| {
            area.select_panel(id, window, cx);
            if !area.is_dock_open(to) {
                area.toggle_dock(to, window, cx);
            }
        });
        panel.focus_handle(cx).focus(window, cx);
        self.last_tool_window = Some(kind);
        self.hidden_docks.clear();
        cx.notify();
    }

    pub(crate) fn hide_all_tool_windows(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let area = self.dock_area.clone();
        let open: Vec<DockPlacement> = DOCK_PLACEMENTS
            .into_iter()
            .filter(|p| area.read(cx).is_dock_open(*p))
            .collect();
        area.update(cx, |area, cx| {
            if open.is_empty() {
                for placement in self.hidden_docks.drain(..) {
                    if !area.is_dock_open(placement) {
                        area.toggle_dock(placement, window, cx);
                    }
                }
            } else {
                for placement in &open {
                    area.toggle_dock(*placement, window, cx);
                }
                self.hidden_docks = open;
            }
        });
        self.focus_active_editor(window, cx);
        cx.notify();
    }

    pub(crate) fn toggle_maximize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let area = self.dock_area.clone();
        if area.read(cx).is_zoomed() {
            area.update(cx, |area, cx| area.set_zoomed_out(window, cx));
            return;
        }
        // The focused tool window, or the last one used, or the editor.
        let target = self
            .last_tool_window
            .and_then(|k| self.tool_windows.get(&k))
            .filter(|p| self.is_tool_window_visible(p.read(cx).kind(), cx))
            .map(|p| PanelId::from(p.entity_id()))
            .or_else(|| {
                self.active_editor_entity()
                    .map(|e| PanelId::from(e.entity_id()))
            });
        let Some(id) = target else { return };
        let Some(placement) = self.placement_of(id, cx) else {
            return;
        };
        let node = area
            .read(cx)
            .layout(placement)
            .and_then(|t| t.find_panel_node(id));
        if let Some(node) = node {
            area.update(cx, |area, cx| area.set_zoomed_in(node, window, cx));
        }
    }

    // -----------------------------------------------------------------------
    // Editors
    // -----------------------------------------------------------------------

    pub fn active_editor_entity(&self) -> Option<Entity<EditorPanel>> {
        self.active_editor.as_ref().and_then(|e| e.upgrade())
    }

    fn focus_active_editor(&self, window: &mut Window, cx: &mut App) {
        if let Some(editor) = self.active_editor_entity() {
            editor.focus_handle(cx).focus(window, cx);
        } else {
            self.focus_handle.focus(window, cx);
        }
    }

    /// The tab group holding the active editor.
    fn active_group(&self, cx: &App) -> Option<gpui_kit::component::dock::NodeId> {
        let editor = self.active_editor_entity()?;
        self.dock_area
            .read(cx)
            .layout(DockPlacement::Center)?
            .find_panel_node(PanelId::from(editor.entity_id()))
    }

    /// Open `path` in the active tab group, or show it if already open.
    pub fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.open_location(&OpenLocation::file(path), window, cx);
    }

    /// Open a file at a position. A preview request replaces the current
    /// preview tab instead of adding one (VS Code/Zed).
    ///
    /// A request that moves focus to the editor is a navigation: the place
    /// it leaves is remembered for Back.
    pub fn open_location(
        &mut self,
        location: &OpenLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if location.focus
            && let Some(from) = self.current_location(cx)
            && (from.path != location.path
                || location.position.is_some_and(|p| Some(p) != from.position))
        {
            self.push_back(from);
            self.nav_forward.clear();
        }
        self.open_location_inner(location, window, cx);
    }

    fn open_location_inner(
        &mut self,
        location: &OpenLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let area = self.dock_area.clone();
        let path = location.path.clone();
        if let Some(existing) = self
            .editors
            .iter()
            .find(|e| e.read(cx).path() == path)
            .cloned()
        {
            let id = PanelId::from(existing.entity_id());
            area.update(cx, |area, cx| area.select_panel(id, window, cx));
            self.mark_active_editor(&existing);
            if !location.preview {
                existing.update(cx, |e, cx| e.set_preview(false, cx));
            }
            if let Some(position) = location.position {
                existing.update(cx, |e, cx| e.go_to(position, window, cx));
            }
            if location.focus {
                existing.focus_handle(cx).focus(window, cx);
            }
            return;
        }

        // A new preview replaces the previous preview tab.
        if location.preview
            && let Some(old) = self
                .editors
                .iter()
                .find(|e| e.read(cx).is_preview())
                .cloned()
        {
            self.close_editor(&old, window, cx);
        }

        let group = self.active_group(cx);
        let editor = EditorPanel::open(path, false, &self.services, window, cx);
        editor.update(cx, |e, cx| {
            e.set_preview(location.preview, cx);
            if let Some(position) = location.position {
                e.go_to(position, window, cx);
            }
        });
        self.adopt_editor(editor.clone(), window, cx);
        let id = PanelId::from(editor.entity_id());
        area.update(cx, |area, cx| {
            area.add_panel_view(
                panel_handle(editor.clone()),
                DockPlacement::Center,
                None,
                window,
                cx,
            );
            if let Some(node) = group {
                area.move_panel(
                    id,
                    InsertTarget::Tabs {
                        node,
                        ix: None,
                        activate: true,
                    },
                    window,
                    cx,
                );
            }
        });
        self.mark_active_editor(&editor);
        self.enforce_tab_limit(window, cx);
        if location.focus {
            editor.focus_handle(cx).focus(window, cx);
        }
    }

    // -----------------------------------------------------------------------
    // Back / Forward
    // -----------------------------------------------------------------------

    /// The active editor's file and caret.
    pub fn current_location(&self, cx: &App) -> Option<NavEntry> {
        let editor = self.active_editor_entity()?;
        let editor = editor.read(cx);
        Some(NavEntry {
            path: editor.path().to_path_buf(),
            position: editor.caret(cx),
        })
    }

    fn push_back(&mut self, entry: NavEntry) {
        if self.nav_back.last() == Some(&entry) {
            return;
        }
        self.nav_back.push(entry);
        if self.nav_back.len() > NAV_HISTORY_LIMIT {
            self.nav_back.remove(0);
        }
    }

    pub fn can_go_back(&self) -> bool {
        !self.nav_back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.nav_forward.is_empty()
    }

    /// Return to the previous place (VS Code: Alt+Left).
    pub fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.nav_back.pop() else {
            return;
        };
        if let Some(here) = self.current_location(cx) {
            self.nav_forward.push(here);
        }
        self.navigate_to(target, window, cx);
    }

    /// Undo a Back (VS Code: Alt+Right).
    pub fn go_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.nav_forward.pop() else {
            return;
        };
        if let Some(here) = self.current_location(cx) {
            self.push_back(here);
        }
        self.navigate_to(target, window, cx);
    }

    fn navigate_to(&mut self, target: NavEntry, window: &mut Window, cx: &mut Context<Self>) {
        let mut location = OpenLocation::file(target.path);
        location.position = target.position;
        self.open_location_inner(&location, window, cx);
        cx.notify();
    }

    /// Add any panel (diff, merge, …) as a tab of the active editor group.
    pub fn add_center_panel<P: gpui_kit::component::dock::Panel>(
        &mut self,
        panel: Entity<P>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let group = self.active_group(cx);
        let id = PanelId::from(panel.entity_id());
        self.dock_area.update(cx, |area, cx| {
            area.add_panel_view(panel_handle(panel), DockPlacement::Center, None, window, cx);
            if let Some(node) = group {
                area.move_panel(
                    id,
                    InsertTarget::Tabs {
                        node,
                        ix: None,
                        activate: true,
                    },
                    window,
                    cx,
                );
            }
        });
    }

    pub(crate) fn reopen_closed_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.closed_tabs.pop() {
            self.open_file(path, window, cx);
        }
    }

    /// Close the least recently used unpinned tabs above the limit.
    fn enforce_tab_limit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let limit = AppState::settings(cx).editor.tab_limit;
        let mut excess = self.editors.len().saturating_sub(limit);
        let candidates: Vec<_> = self
            .editors
            .iter()
            .filter(|e| !e.read(cx).is_pinned())
            .cloned()
            .collect();
        for editor in candidates {
            if excess == 0 {
                break;
            }
            if Some(editor.downgrade()) == self.active_editor {
                continue;
            }
            self.close_editor(&editor, window, cx);
            excess -= 1;
        }
    }

    fn close_editor(
        &mut self,
        editor: &Entity<EditorPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if editor.read(cx).is_pinned() {
            return;
        }
        let editor = editor.clone();
        save_if_dirty(&editor, cx);
        let path = editor.read(cx).path().to_path_buf();
        self.closed_tabs.retain(|p| *p != path);
        self.closed_tabs.push(path);
        if self.closed_tabs.len() > 50 {
            self.closed_tabs.remove(0);
        }
        self.dock_area
            .update(cx, |area, cx| area.remove_panel(editor.clone(), window, cx));
        self.editors.retain(|e| *e != editor);
        if self.active_editor == Some(editor.downgrade()) {
            self.active_editor = self.editors.last().map(|e| e.downgrade());
        }
    }

    pub(crate) fn close_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor_entity() {
            if editor.read(cx).is_pinned() {
                self.notify_info("tab-pinned-cannot-close", window, cx);
                return;
            }
            self.close_editor(&editor, window, cx);
            self.focus_active_editor(window, cx);
        }
    }

    pub(crate) fn close_other_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.active_editor_entity();
        for editor in self.editors.clone() {
            if Some(&editor) != active.as_ref() {
                self.close_editor(&editor, window, cx);
            }
        }
    }

    pub(crate) fn close_all_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for editor in self.editors.clone() {
            self.close_editor(&editor, window, cx);
        }
        self.focus_active_editor(window, cx);
    }

    pub(crate) fn toggle_pin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor_entity() else {
            return;
        };
        let pinned = !editor.read(cx).is_pinned();
        editor.update(cx, |e, cx| e.set_pinned(pinned, cx));
        // Pinned tabs move to the front of their group, as in IntelliJ.
        if pinned && let Some(node) = self.active_group(cx) {
            let id = PanelId::from(editor.entity_id());
            let first_unpinned = self.group_panels(node, cx).iter().position(|p| {
                self.editors
                    .iter()
                    .find(|e| PanelId::from(e.entity_id()) == *p)
                    .is_none_or(|e| !e.read(cx).is_pinned() || e == &editor)
            });
            self.dock_area.update(cx, |area, cx| {
                area.move_panel(
                    id,
                    InsertTarget::Tabs {
                        node,
                        ix: first_unpinned,
                        activate: true,
                    },
                    window,
                    cx,
                )
            });
        }
        self.schedule_save(cx);
        cx.notify();
    }

    fn group_panels(&self, node: gpui_kit::component::dock::NodeId, cx: &App) -> Vec<PanelId> {
        let area = self.dock_area.read(cx);
        area.layout(DockPlacement::Center)
            .and_then(|t| t.find_node(node))
            .map(|n| match n.kind() {
                PaneRef::Tabs { panels, .. } => panels.to_vec(),
                PaneRef::Split { .. } => Vec::new(),
            })
            .unwrap_or_default()
    }

    pub(crate) fn cycle_tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(node) = self.active_group(cx) else {
            return;
        };
        let Some(editor) = self.active_editor_entity() else {
            return;
        };
        let panels = self.group_panels(node, cx);
        let Some(ix) = panels
            .iter()
            .position(|p| *p == PanelId::from(editor.entity_id()))
        else {
            return;
        };
        let next = if forward {
            (ix + 1) % panels.len()
        } else {
            (ix + panels.len() - 1) % panels.len()
        };
        let id = panels[next];
        self.dock_area
            .update(cx, |area, cx| area.select_panel(id, window, cx));
        if let Some(target) = self
            .editors
            .iter()
            .find(|e| PanelId::from(e.entity_id()) == id)
            .cloned()
        {
            self.mark_active_editor(&target);
            target.focus_handle(cx).focus(window, cx);
        }
    }

    /// Open the active file again in a new group beside the current one.
    pub(crate) fn split(
        &mut self,
        placement: Placement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.active_editor_entity() else {
            return;
        };
        let Some(node) = self.active_group(cx) else {
            return;
        };
        let path = editor.read(cx).path().to_path_buf();
        let copy = EditorPanel::open(path, false, &self.services, window, cx);
        self.adopt_editor(copy.clone(), window, cx);
        let id = PanelId::from(copy.entity_id());
        self.dock_area.update(cx, |area, cx| {
            area.add_panel_view(
                panel_handle(copy.clone()),
                DockPlacement::Center,
                None,
                window,
                cx,
            );
            area.move_panel(
                id,
                InsertTarget::Split {
                    node,
                    placement,
                    size: None,
                },
                window,
                cx,
            );
        });
        self.mark_active_editor(&copy);
        copy.focus_handle(cx).focus(window, cx);
    }

    // -----------------------------------------------------------------------
    // Services
    // -----------------------------------------------------------------------

    fn start_service_feeds(&mut self, cx: &mut Context<Self>) {
        // Branch and branch list.
        let current = cx.background_spawn(self.services.vcs.current_branch());
        let all = cx.background_spawn(self.services.vcs.branches());
        cx.spawn(async move |this, cx| {
            let current = current.await.ok();
            let all = all.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.status.branch = current;
                this.status.branches = all;
                cx.notify();
            });
        })
        .detach();

        // Run state.
        let events = self.services.run.subscribe();
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                let alive = this.update(cx, |this, cx| {
                    if let RunEvent::StateChanged { id, state } = event
                        && this
                            .run_configs
                            .get(this.selected_run)
                            .is_some_and(|c| c.id == id)
                    {
                        this.run_state = state;
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();

        // Background tasks (mock indexing).
        let tasks = self.services.status.subscribe_tasks();
        cx.spawn(async move |this, cx| {
            while let Ok(progress) = tasks.recv().await {
                let done = progress.done;
                let alive = this.update(cx, |this, cx| {
                    this.status.task = (!done).then_some(progress);
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();

        // Memory indicator: real resident memory of this process.
        cx.spawn(async move |this, cx| {
            loop {
                let mb =
                    memory_stats::memory_stats().map(|m| (m.physical_mem / (1024 * 1024)) as u64);
                if this
                    .update(cx, |this, cx| {
                        if this.status.memory_mb != mb {
                            this.status.memory_mb = mb;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        })
        .detach();
    }

    pub(crate) fn checkout_branch(&mut self, name: String, cx: &mut Context<Self>) {
        let checkout = cx.background_spawn(self.services.vcs.checkout(&name));
        let current = self.services.vcs.clone();
        cx.spawn(async move |this, cx| {
            let result = checkout.await;
            let branch = cx.background_spawn(current.current_branch()).await.ok();
            let _ = this.update(cx, |this, cx| {
                if result.is_ok() {
                    this.status.branch = branch;
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn start_run(&mut self, debug: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !crate::projects::is_trusted(&self.services, cx) {
            window.push_notification(
                Notification::warning(tr("project-untrusted-run").to_string()),
                cx,
            );
            return;
        }
        let Some(config) = self.run_configs.get(self.selected_run).cloned() else {
            return;
        };
        match self.services.run.start(&config.id, debug) {
            Ok(()) => {
                let kind = if debug {
                    ToolWindowKind::Debug
                } else {
                    ToolWindowKind::Run
                };
                if !self.is_tool_window_visible(kind, cx) {
                    self.toggle_tool_window(kind, window, cx);
                }
            }
            Err(err) => window.push_notification(Notification::warning(err.to_string()), cx),
        }
    }

    /// Re-read the run configurations after they were edited.
    pub(crate) fn reload_run_configs(&mut self, cx: &mut Context<Self>) {
        let selected = self
            .run_configs
            .get(self.selected_run)
            .map(|c| c.id.clone());
        self.run_configs = self.services.run.configurations();
        self.selected_run = selected
            .and_then(|id| self.run_configs.iter().position(|c| c.id == id))
            .unwrap_or(0)
            .min(self.run_configs.len().saturating_sub(1));
        if let Some(config) = self.run_configs.get(self.selected_run) {
            self.run_state = self.services.run.state(&config.id);
        }
        cx.notify();
    }

    pub(crate) fn stop_run(&mut self, cx: &mut Context<Self>) {
        if let Some(config) = self.run_configs.get(self.selected_run) {
            self.services.run.stop(&config.id);
        }
        cx.notify();
    }

    /// Select `path` in the Project tool window, showing it first.
    pub fn reveal_in_project(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_tool_window(ToolWindowKind::Project, window, cx);
        if let Some(tree) = self.tool_window_body::<crate::panels::project::ProjectTree>(
            ToolWindowKind::Project,
            window,
            cx,
        ) {
            tree.update(cx, |tree, cx| tree.reveal(path, cx));
        }
    }

    /// Run an editor command on the active editor.
    pub fn editor_command(
        &mut self,
        command: EditorCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.active_editor_entity() {
            let services = self.services.clone();
            editor.update(cx, |e, cx| e.run_command(command, &services, window, cx));
        }
    }

    /// Save the active editor (File → Save).
    pub fn save_active(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor_entity() {
            editor.update(cx, |e, cx| e.save(cx).detach());
        }
    }

    /// Save every modified editor (File → Save All).
    pub fn save_all(&mut self, cx: &mut Context<Self>) {
        for editor in self.editors.clone() {
            save_if_dirty(&editor, cx);
        }
    }

    pub(crate) fn notify_info(&self, key: &str, window: &mut Window, cx: &mut App) {
        window.push_notification(Notification::info(tr(key)), cx);
    }
}

/// Save `editor` if it has unsaved changes.
fn save_if_dirty(editor: &Entity<EditorPanel>, cx: &mut App) {
    if editor.read(cx).is_dirty() {
        editor.update(cx, |e, cx| e.save(cx).detach());
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut context = KeyContext::default();
        context.add(ferro_keymap::WORKSPACE_CONTEXT);
        let theme = cx.theme();
        let (bg, fg) = (theme.background, theme.foreground);

        div()
            .id("workspace")
            .key_context(context)
            .track_focus(&self.focus_handle)
            .on_modifiers_changed(cx.listener(crate::quick_open::on_modifiers_changed))
            .capture_key_down(
                cx.listener(|this, event, _, cx| crate::quick_open::on_key_down(this, event, cx)),
            )
            .on_action(cx.listener(|this, _: &FindAction, window, cx| {
                crate::quick_open::open(this, QuickOpenMode::Commands, window, cx)
            }))
            .on_action(cx.listener(|this, _: &CommandPalette, window, cx| {
                crate::quick_open::open(this, QuickOpenMode::Commands, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SearchEverywhere, window, cx| {
                crate::quick_open::open(this, QuickOpenMode::Files, window, cx)
            }))
            .on_action(cx.listener(|this, _: &QuickOpen, window, cx| {
                crate::quick_open::open(this, QuickOpenMode::Files, window, cx)
            }))
            .on_action(cx.listener(|this, _: &GoToSymbolInFile, window, cx| {
                crate::quick_open::open(this, QuickOpenMode::SymbolsInFile, window, cx)
            }))
            .on_action(cx.listener(|this, _: &GoToSymbolInWorkspace, window, cx| {
                crate::quick_open::open(this, QuickOpenMode::SymbolsInWorkspace, window, cx)
            }))
            .on_action(cx.listener(|this, _: &GoToLine, window, cx| {
                crate::quick_open::open(this, QuickOpenMode::GoToLine, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FindInFiles, window, cx| {
                crate::find_in_files::open(this, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ReplaceInFiles, window, cx| {
                crate::find_in_files::open(this, true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                crate::settings_window::open(this, SettingsPage::Appearance, window, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenKeymapSettings, window, cx| {
                crate::settings_window::open(this, SettingsPage::Keymap, window, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenSettingsFile, window, cx| {
                crate::settings_window::open_settings_file(this, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NewFile, window, cx| {
                crate::dialogs::new_file::open(this, window, cx)
            }))
            .on_action(cx.listener(|this, _: &EditRunConfigurations, window, cx| {
                crate::dialogs::run_config::open(this, window, cx)
            }))
            .on_action(cx.listener(|this, _: &RenameSymbol, window, cx| {
                crate::dialogs::rename::open(this, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowDiff, window, cx| {
                let path = this
                    .active_editor_entity()
                    .map(|e| e.read(cx).path().to_path_buf());
                match path {
                    Some(path) => crate::vcs::diff_view::open(this, path, window, cx),
                    None => this.notify_info("feature-coming-soon", window, cx),
                }
            }))
            .on_action(cx.listener(|this, _: &ResolveConflicts, window, cx| {
                crate::vcs::merge_view::open(this, None, window, cx)
            }))
            .on_action(cx.listener(|this, request: &OpenLocation, window, cx| {
                this.open_location(request, window, cx)
            }))
            .on_action(cx.listener(|this, request: &OpenDiff, window, cx| {
                crate::vcs::diff_view::open(this, request.path.clone(), window, cx)
            }))
            .on_action(cx.listener(|this, request: &OpenMerge, window, cx| {
                crate::vcs::merge_view::open(this, Some(request.path.clone()), window, cx)
            }))
            .on_action(cx.listener(|this, request: &RevealInProject, window, cx| {
                this.reveal_in_project(request.path.clone(), window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectOpenedFile, window, cx| {
                let path = this
                    .active_editor_entity()
                    .map(|e| e.read(cx).path().to_path_buf());
                if let Some(path) = path {
                    this.reveal_in_project(path, window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &Save, _, cx| this.save_active(cx)))
            .on_action(cx.listener(|this, _: &SaveAll, _, cx| this.save_all(cx)))
            .on_action(cx.listener(|this, _: &ReloadFromDisk, window, cx| {
                if let Some(editor) = this.active_editor_entity() {
                    editor.update(cx, |e, cx| e.reload(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &DuplicateLineDown, window, cx| {
                this.editor_command(EditorCommand::DuplicateLineDown, window, cx)
            }))
            .on_action(cx.listener(|this, _: &DuplicateLineUp, window, cx| {
                this.editor_command(EditorCommand::DuplicateLineUp, window, cx)
            }))
            .on_action(cx.listener(|this, _: &MoveLineUp, window, cx| {
                this.editor_command(EditorCommand::MoveLineUp, window, cx)
            }))
            .on_action(cx.listener(|this, _: &MoveLineDown, window, cx| {
                this.editor_command(EditorCommand::MoveLineDown, window, cx)
            }))
            .on_action(cx.listener(|this, _: &DeleteLine, window, cx| {
                this.editor_command(EditorCommand::DeleteLine, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleLineComment, window, cx| {
                this.editor_command(EditorCommand::ToggleLineComment, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleBlockComment, window, cx| {
                this.editor_command(EditorCommand::ToggleBlockComment, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectNextOccurrence, window, cx| {
                this.editor_command(EditorCommand::SelectNextOccurrence, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectAllOccurrences, window, cx| {
                this.editor_command(EditorCommand::SelectAllOccurrences, window, cx)
            }))
            .on_action(cx.listener(|this, _: &AddCursorAbove, window, cx| {
                this.editor_command(EditorCommand::AddCursorAbove, window, cx)
            }))
            .on_action(cx.listener(|this, _: &AddCursorBelow, window, cx| {
                this.editor_command(EditorCommand::AddCursorBelow, window, cx)
            }))
            .on_action(cx.listener(|this, _: &TriggerCompletion, window, cx| {
                this.editor_command(EditorCommand::TriggerCompletion, window, cx)
            }))
            .on_action(cx.listener(|this, _: &QuickFix, window, cx| {
                this.editor_command(EditorCommand::QuickFix, window, cx)
            }))
            .on_action(cx.listener(|this, _: &QuickDocumentation, window, cx| {
                this.editor_command(EditorCommand::QuickDocumentation, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ParameterInfo, window, cx| {
                this.editor_command(EditorCommand::ParameterInfo, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleBreakpoint, window, cx| {
                this.editor_command(EditorCommand::ToggleBreakpoint, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NextProblem, window, cx| {
                this.editor_command(EditorCommand::NextProblem, window, cx)
            }))
            .on_action(cx.listener(|this, _: &PreviousProblem, window, cx| {
                this.editor_command(EditorCommand::PreviousProblem, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleLeftDock, window, cx| {
                this.toggle_dock(DockPlacement::Left, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleBottomDock, window, cx| {
                this.toggle_dock(DockPlacement::Bottom, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleRightDock, window, cx| {
                this.toggle_dock(DockPlacement::Right, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleServices, window, cx| {
                this.toggle_tool_window(ToolWindowKind::Services, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NewTerminal, window, cx| {
                this.services.terminal.new_session();
                this.show_tool_window(ToolWindowKind::Terminal, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ReopenClosedTab, window, cx| {
                this.reopen_closed_tab(window, cx)
            }))
            .on_action(cx.listener(|this, _: &GetFromVcs, window, cx| {
                crate::dialogs::clone_repo::open(this, window, cx)
            }))
            .on_action(cx.listener(|this, _: &About, window, cx| {
                crate::dialogs::open_about(this, window, cx)
            }))
            .on_action(cx.listener(|this, _: &HideAllToolWindows, window, cx| {
                this.hide_all_tool_windows(window, cx)
            }))
            .on_action(cx.listener(|this, _: &MaximizeToolWindow, window, cx| {
                this.toggle_maximize(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ResetLayout, window, cx| this.reset_layout(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleProject, window, cx| {
                this.toggle_tool_window(ToolWindowKind::Project, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleCommit, window, cx| {
                this.toggle_tool_window(ToolWindowKind::Commit, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleStructure, window, cx| {
                this.toggle_tool_window(ToolWindowKind::Structure, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleRun, window, cx| {
                this.toggle_tool_window(ToolWindowKind::Run, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleDebug, window, cx| {
                this.toggle_tool_window(ToolWindowKind::Debug, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleProblems, window, cx| {
                this.toggle_tool_window(ToolWindowKind::Problems, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleGit, window, cx| {
                this.toggle_tool_window(ToolWindowKind::Git, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleTerminal, window, cx| {
                this.toggle_tool_window(ToolWindowKind::Terminal, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleTodo, window, cx| {
                this.toggle_tool_window(ToolWindowKind::Todo, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleNotifications, window, cx| {
                this.toggle_tool_window(ToolWindowKind::Notifications, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleDatabase, window, cx| {
                this.toggle_tool_window(ToolWindowKind::Database, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleBuild, window, cx| {
                this.toggle_tool_window(ToolWindowKind::Build, window, cx)
            }))
            .on_action(cx.listener(|this, _: &GoBack, window, cx| this.go_back(window, cx)))
            .on_action(cx.listener(|this, _: &GoForward, window, cx| this.go_forward(window, cx)))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| this.close_tab(window, cx)))
            .on_action(
                cx.listener(|this, _: &CloseOtherTabs, window, cx| {
                    this.close_other_tabs(window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &CloseAllTabs, window, cx| this.close_all_tabs(window, cx)),
            )
            .on_action(cx.listener(|this, _: &PinTab, window, cx| this.toggle_pin(window, cx)))
            .on_action(
                cx.listener(|this, _: &NextTab, window, cx| this.cycle_tab(true, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &PreviousTab, window, cx| this.cycle_tab(false, window, cx)),
            )
            .on_action(cx.listener(|this, _: &SplitRight, window, cx| {
                this.split(Placement::Right, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SplitDown, window, cx| {
                this.split(Placement::Bottom, window, cx)
            }))
            .on_action(cx.listener(|this, _: &AddFolderToProject, window, cx| {
                crate::projects::prompt_add_folder(this, window, cx)
            }))
            .on_action(cx.listener(|this, _: &TrustProject, _, cx| {
                crate::projects::trust(&this.services, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Run, window, cx| this.start_run(false, window, cx)))
            .on_action(cx.listener(|this, _: &Debug, window, cx| this.start_run(true, window, cx)))
            .on_action(cx.listener(|this, _: &Stop, _, cx| this.stop_run(cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(fg)
            .child(self.render_title_bar(window, cx))
            .children(crate::projects::render_trust_bar(self, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(self.dock_area.clone()),
            )
            .child(self.render_status_bar(window, cx))
    }
}
