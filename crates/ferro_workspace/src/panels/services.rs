//! The Services tool window: run configurations, Docker containers and
//! databases as one tree, with start/stop for run configurations.

use std::collections::HashSet;

use ferro_services::{RunState, ServiceKind, ServiceNode, ServiceState, Services};
use ferro_ui::{FerroIcon, components, tr};
use gpui_kit::{
    AnyView, App, AppContext as _, ClickEvent, Context, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window,
    component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex},
    div,
    prelude::FluentBuilder as _,
    px,
};

use super::common::{self, NodePath, ROW_HEIGHT};

pub struct ServicesView {
    services: Services,
    tree: Vec<ServiceNode>,
    collapsed: HashSet<NodePath>,
    rows: Vec<NodePath>,
    selected: Option<usize>,
    message: Option<SharedString>,
    focus_handle: FocusHandle,
}

pub fn build(services: Services, _window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| ServicesView::new(services, cx)).into()
}

fn kids(node: &ServiceNode) -> &[ServiceNode] {
    &node.children
}

impl ServicesView {
    pub fn new(services: Services, cx: &mut Context<Self>) -> Self {
        let events = services.run.subscribe();
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                let alive = this.update(cx, |this, cx| {
                    if let ferro_services::RunEvent::StateChanged { .. } = event {
                        this.reload();
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        let mut this = Self {
            services,
            tree: Vec::new(),
            collapsed: HashSet::new(),
            rows: Vec::new(),
            selected: None,
            message: None,
            focus_handle: cx.focus_handle(),
        };
        this.reload();
        this
    }

    pub fn reload(&mut self) {
        self.tree = self.services.run.services();
        let collapsed = &self.collapsed;
        self.rows = common::flatten(&self.tree, &kids, &|p| collapsed.contains(p), &|_| true);
        if self.selected.is_some_and(|ix| ix >= self.rows.len()) {
            self.selected = None;
        }
    }

    pub fn row_names(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter_map(|p| common::node_at(&self.tree, p, &kids))
            .map(|n| n.name.clone())
            .collect()
    }

    fn node(&self, ix: usize) -> Option<&ServiceNode> {
        self.rows
            .get(ix)
            .and_then(|p| common::node_at(&self.tree, p, &kids))
    }

    /// The run configuration id of the selected row, if it is one.
    fn selected_config(&self) -> Option<String> {
        let node = self.selected.and_then(|ix| self.node(ix))?;
        (node.kind == ServiceKind::RunConfiguration).then(|| {
            self.services
                .run
                .configurations()
                .into_iter()
                .find(|c| c.name == node.name)
                .map(|c| c.id)
        })?
    }

    pub fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.selected = Some(ix);
        cx.notify();
    }

    /// Start (or stop, when running) the selected run configuration.
    pub fn start_stop(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected_config() else {
            return;
        };
        self.message = None;
        if matches!(self.services.run.state(&id), RunState::Running { .. }) {
            self.services.run.stop(&id);
        } else if let Err(err) = self.services.run.start(&id, false) {
            self.message = Some(err.to_string().into());
        }
        self.reload();
        cx.notify();
    }

    fn toggle(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(path) = self.rows.get(ix).cloned() {
            if !self.collapsed.remove(&path) {
                self.collapsed.insert(path);
            }
            self.reload();
            cx.notify();
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if common::has_command_modifier(&event.keystroke) {
            return;
        }
        let key = event.keystroke.key.as_str();
        if let Some(ix) = common::nav(key, self.selected, self.rows.len()) {
            self.select(ix, cx);
        } else if let Some(ix) = self.selected {
            let has_children = self.node(ix).is_some_and(|n| !n.children.is_empty());
            match key {
                "enter" if has_children => self.toggle(ix, cx),
                "enter" => self.start_stop(cx),
                "left" | "right" if has_children => self.toggle(ix, cx),
                _ => return,
            }
        } else {
            return;
        }
        cx.stop_propagation();
    }
}

impl Focusable for ServicesView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ServicesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, selected_bg, hover_bg, success, danger) = (
            theme.muted_foreground,
            theme.list_active,
            theme.list_hover,
            theme.success,
            theme.danger,
        );
        let config = self.selected_config();
        let running = config
            .as_ref()
            .is_some_and(|id| matches!(self.services.run.state(id), RunState::Running { .. }));
        let toolbar = common::toolbar("services-toolbar", cx)
            .child(
                common::tool_button("services-start", FerroIcon::Run, tr("action-run"))
                    .disabled(config.is_none() || running)
                    .on_click(cx.listener(|this, _, _, cx| this.start_stop(cx))),
            )
            .child(
                common::tool_button("services-stop", FerroIcon::Stop, tr("action-stop"))
                    .disabled(!running)
                    .on_click(cx.listener(|this, _, _, cx| this.start_stop(cx))),
            )
            .child(
                common::tool_button("services-refresh", FerroIcon::Refresh, tr("panels-refresh"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.reload();
                        cx.notify();
                    })),
            )
            .child(div().flex_1())
            .when_some(self.message.clone(), |el, m| {
                el.child(div().text_color(danger).child(m))
            });

        let body = if self.tree.is_empty() {
            components::empty_state(FerroIcon::Services, tr("panels-services-empty"), None, cx)
                .into_any_element()
        } else {
            v_flex()
                .id("services-tree")
                .size_full()
                .overflow_y_scroll()
                .children(self.rows.iter().enumerate().filter_map(|(ix, path)| {
                    let node = common::node_at(&self.tree, path, &kids)?;
                    let has_children = !node.children.is_empty();
                    let open = !self.collapsed.contains(path);
                    let icon = match node.kind {
                        ServiceKind::Group => {
                            if open {
                                FerroIcon::FolderOpen
                            } else {
                                FerroIcon::Folder
                            }
                        }
                        ServiceKind::RunConfiguration => FerroIcon::Run,
                        ServiceKind::Docker => FerroIcon::Container,
                        ServiceKind::Database => FerroIcon::Database,
                    };
                    let (state_color, state_key) = match node.state {
                        ServiceState::Running => (Some(success), Some("panels-services-running")),
                        ServiceState::Stopped => (Some(muted), Some("panels-services-stopped")),
                        ServiceState::Failed => (Some(danger), Some("panels-services-failed")),
                        ServiceState::None => (None, None),
                    };
                    Some(
                        h_flex()
                            .id(ix)
                            .h(px(ROW_HEIGHT))
                            .pl(px(4. + (path.len() - 1) as f32 * 14.))
                            .pr_2()
                            .gap_1()
                            .w_full()
                            .text_sm()
                            .cursor_pointer()
                            .aria_label(SharedString::from(node.name.clone()))
                            .when(Some(ix) == self.selected, |el| el.bg(selected_bg))
                            .hover(|el| el.bg(hover_bg))
                            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                                this.focus_handle.focus(window, cx);
                                this.select(ix, cx);
                                if has_children {
                                    this.toggle(ix, cx);
                                } else if event.click_count() >= 2 {
                                    this.start_stop(cx);
                                }
                            }))
                            .child(div().w(px(14.)).flex_none().when(has_children, |el| {
                                el.child(
                                    Icon::from(if open {
                                        FerroIcon::ChevronDown
                                    } else {
                                        FerroIcon::ChevronRight
                                    })
                                    .xsmall()
                                    .text_color(muted),
                                )
                            }))
                            .child(Icon::from(icon).small().text_color(muted))
                            .when_some(state_color, |el, c| {
                                el.child(div().size(px(7.)).rounded_full().bg(c))
                            })
                            .child(div().truncate().child(node.name.clone()))
                            .when_some(state_key, |el, key| {
                                el.child(div().text_xs().text_color(muted).child(tr(key)))
                            })
                            .child(div().flex_1())
                            .when_some(node.detail.clone(), |el, d| {
                                el.child(div().text_xs().text_color(muted).truncate().child(d))
                            })
                            .into_any_element(),
                    )
                }))
                .into_any_element()
        };
        v_flex()
            .id("services")
            .track_focus(&self.focus_handle)
            .aria_label(tr("tool-services"))
            .size_full()
            .on_key_down(cx.listener(Self::on_key_down))
            .child(toolbar)
            .child(div().flex_1().min_h_0().child(body))
    }
}
