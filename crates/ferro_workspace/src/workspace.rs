//! The IDE window: title bar with the main toolbar, tool-window stripes, the
//! dock area (tool windows around the editor tabs) and the status bar.

use std::{collections::HashMap, rc::Rc, time::Duration};

use ferro_services::{Branch, RunConfiguration, RunEvent, RunState, Services, TaskProgress};
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
        menu::AppMenuBar,
        notification::Notification,
    },
    div, px,
};

use crate::{
    actions::*,
    app_state::AppState,
    editor::{EDITOR_PANEL_NAME, EditorEvent, EditorPanel, EditorTabState},
    tool_window::{ToolWindowKind, ToolWindowPanel},
    tool_windows::{OpenFile, ToolWindowBody},
};

/// Bumped when the persisted layout schema changes; older layouts are
/// discarded instead of mis-restored.
pub const LAYOUT_VERSION: usize = 1;

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
        panel_handle(EditorPanel::open(
            state.path,
            state.pinned,
            &services,
            window,
            cx,
        ))
    });
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
    /// Docks closed by "Hide All Tool Windows", reopened by the next press.
    hidden_docks: Vec<DockPlacement>,
    pub(crate) app_menu_bar: Entity<AppMenuBar>,
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
        cx.default_global::<WorkspaceServices>()
            .0
            .insert(dock_area.entity_id(), services.clone());

        let project_key = services
            .project
            .roots()
            .first()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| services.project.name());

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
            hidden_docks: Vec::new(),
            app_menu_bar: AppMenuBar::new(cx),
            status: StatusData::default(),
            selected_run: 0,
            run_state: RunState::Idle,
            save_task: None,
            _subscriptions: subscriptions,
        };

        if !this.restore_layout(window, cx) {
            this.install_default_layout(window, cx);
        }
        this.start_service_feeds(cx);
        this.focus_handle.focus(window, cx);
        this
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let kind = panel.read(cx).kind();
        if let ToolWindowBody::Project(tree) = panel.read(cx).body() {
            let tree = tree.clone();
            self._subscriptions.push(cx.subscribe_in(
                &tree,
                window,
                |this, _, event: &OpenFile, window, cx| this.open_file(event.0.clone(), window, cx),
            ));
        }
        self.tool_windows.insert(kind, panel);
    }

    fn adopt_editor(&mut self, editor: Entity<EditorPanel>, cx: &mut Context<Self>) {
        self._subscriptions.push(
            cx.subscribe(&editor, |this, editor, event: &EditorEvent, cx| {
                match event {
                    EditorEvent::Activated => this.mark_active_editor(&editor),
                    EditorEvent::CursorMoved => {}
                }
                cx.notify();
            }),
        );
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

        let root = self.services.project.roots()[0].clone();
        let mut center = DockLayout::tabs();
        for rel in ["src/main.rs", "src/lib.rs", "Cargo.toml"] {
            let editor = EditorPanel::open(root.join(rel), false, &self.services, window, cx);
            self.adopt_editor(editor.clone(), cx);
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
            // IntelliJ opens with only the Project tool window visible.
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
        let state = AppState::global(cx).paths.load_layout(&self.project_key);
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
            self.adopt_editor(editor, cx);
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
        self.editors
            .retain(|e| alive.contains(&PanelId::from(e.entity_id())));
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
                if let Err(err) = AppState::global(cx)
                    .paths
                    .save_layout(&self.project_key, &value)
                {
                    tracing::warn!("could not save layout: {err:#}");
                }
            }
            Err(err) => tracing::warn!("could not serialize layout: {err}"),
        }
    }

    pub(crate) fn reset_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        AppState::global(cx).paths.delete_layout(&self.project_key);
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

    /// IntelliJ stripe behavior: show (and focus) the tool window, or hide
    /// it if it is already the visible one.
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
    pub fn open_file(
        &mut self,
        path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let area = self.dock_area.clone();
        if let Some(existing) = self
            .editors
            .iter()
            .find(|e| e.read(cx).path() == path)
            .cloned()
        {
            let id = PanelId::from(existing.entity_id());
            area.update(cx, |area, cx| area.select_panel(id, window, cx));
            self.mark_active_editor(&existing);
            existing.focus_handle(cx).focus(window, cx);
            return;
        }

        let group = self.active_group(cx);
        let editor = EditorPanel::open(path, false, &self.services, window, cx);
        self.adopt_editor(editor.clone(), cx);
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
        editor.focus_handle(cx).focus(window, cx);
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
        self.adopt_editor(copy.clone(), cx);
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

    pub(crate) fn stop_run(&mut self, cx: &mut Context<Self>) {
        if let Some(config) = self.run_configs.get(self.selected_run) {
            self.services.run.stop(&config.id);
        }
        cx.notify();
    }

    pub(crate) fn notify_info(&self, key: &str, window: &mut Window, cx: &mut App) {
        window.push_notification(Notification::info(tr(key)), cx);
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
            .on_action(cx.listener(|this, _: &FindAction, window, cx| {
                crate::find_action::open(this, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SearchEverywhere, window, cx| {
                crate::find_action::open(this, window, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                crate::dialogs::open_settings(this, window, cx)
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
            .on_action(cx.listener(|this, _: &Run, window, cx| this.start_run(false, window, cx)))
            .on_action(cx.listener(|this, _: &Debug, window, cx| this.start_run(true, window, cx)))
            .on_action(cx.listener(|this, _: &Stop, _, cx| this.stop_run(cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(fg)
            .child(self.render_title_bar(window, cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_stripe(crate::tool_window::StripeSlot::LeftTop, window, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.dock_area.clone()),
                    )
                    .child(self.render_stripe(crate::tool_window::StripeSlot::Right, window, cx)),
            )
            .child(self.render_status_bar(window, cx))
    }
}
