//! The 3-way merge view: ours (left), the result (center) and theirs
//! (right), with the common base on demand, for a file with conflicts.
//!
//! Each conflict is resolved with Accept Left / Accept Both / Accept Right /
//! Ignore (keep the base); Apply writes the result once nothing is left.
//! Keyboard: F7 / Shift+F7 move to the next / previous conflict; every
//! button is reachable with Tab.

use std::path::{Path, PathBuf};

use ferro_services::Services;
use ferro_ui::{FerroIcon, components, tr, tr_args};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EntityId, EventEmitter, FocusHandle,
    Focusable, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _,
    Render, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    component::{
        ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, WindowExt as _,
        button::{Button, ButtonVariants as _},
        dock::{
            BasePanel, Panel, PanelControl, PanelEvent, PanelInfo, PanelState, panel_handle,
            register_panel,
        },
        h_flex,
        notification::Notification,
        spinner::Spinner,
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};
use serde::{Deserialize, Serialize};

use crate::{
    vcs::{
        diff_view::find_panel,
        merge_model::{Conflict, MergeModel, Resolution, Segment},
        repository_changed,
    },
    workspace::Workspace,
};

pub const MERGE_PANEL_NAME: &str = "ferro.merge";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MergeTabState {
    path: PathBuf,
}

enum Content {
    Loading,
    Loaded(MergeModel),
    /// The file has no (more) conflicts.
    NoConflict,
    Failed(SharedString),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Ours,
    Base,
    Result,
    Theirs,
}

pub struct MergePanel {
    path: PathBuf,
    services: Services,
    content: Content,
    /// The current conflict.
    current: usize,
    show_base: bool,
    applying: bool,
    applied: bool,
    error: Option<SharedString>,
    scrolls: [ScrollHandle; 4],
    focus_handle: FocusHandle,
}

impl MergePanel {
    pub fn open(path: PathBuf, services: Services, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let mut this = Self {
                path,
                services,
                content: Content::Loading,
                current: 0,
                show_base: false,
                applying: false,
                applied: false,
                error: None,
                scrolls: Default::default(),
                focus_handle: cx.focus_handle(),
            };
            this.reload(cx);
            this
        })
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.content = Content::Loading;
        let task = cx.background_spawn(self.services.vcs.conflicts());
        let path = self.path.clone();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.content = match result {
                    Err(err) => Content::Failed(err.to_string().into()),
                    Ok(conflicts) => match conflicts.iter().find(|c| c.path == path) {
                        None => Content::NoConflict,
                        Some(conflict) => {
                            match MergeModel::parse(&ferro_services::mock::conflict_markers(
                                conflict,
                            )) {
                                Ok(mut model) => {
                                    if model.ours_label.is_empty() {
                                        model.ours_label = conflict.ours_label.clone();
                                    }
                                    if model.theirs_label.is_empty() {
                                        model.theirs_label = conflict.theirs_label.clone();
                                    }
                                    Content::Loaded(model)
                                }
                                Err(err) => Content::Failed(err.to_string().into()),
                            }
                        }
                    },
                };
                cx.notify();
            });
        })
        .detach();
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn model(&self) -> Option<&MergeModel> {
        match &self.content {
            Content::Loaded(model) => Some(model),
            _ => None,
        }
    }

    pub fn is_applied(&self) -> bool {
        self.applied
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    pub fn current(&self) -> usize {
        self.current
    }

    /// Resolve the current conflict and move to the next unresolved one.
    pub fn resolve(&mut self, ix: usize, resolution: Option<Resolution>, cx: &mut Context<Self>) {
        if let Content::Loaded(model) = &mut self.content {
            model.resolve(ix, resolution);
            self.error = None;
            if resolution.is_some()
                && let Some(next) = model.next_unresolved(ix)
            {
                self.go_to(next, cx);
            }
        }
        cx.notify();
    }

    pub fn go_to(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(count) = self.model().map(MergeModel::conflict_count) else {
            return;
        };
        if count == 0 {
            return;
        }
        self.current = ix.min(count - 1);
        if let Some(seg) = self.model().and_then(|m| m.segment_of(self.current)) {
            for scroll in &self.scrolls {
                scroll.scroll_to_item(seg);
            }
        }
        cx.notify();
    }

    fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(count) = self.model().map(MergeModel::conflict_count) else {
            return;
        };
        if count == 0 {
            return;
        }
        let next = if forward {
            (self.current + 1) % count
        } else {
            (self.current + count - 1) % count
        };
        self.go_to(next, cx);
    }

    pub fn set_show_base(&mut self, show: bool, cx: &mut Context<Self>) {
        self.show_base = show;
        cx.notify();
    }

    /// Write the merged result and mark the file resolved.
    pub fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.model().and_then(MergeModel::result_text) else {
            return;
        };
        self.applying = true;
        let task = cx.background_spawn(self.services.vcs.resolve_conflict(&self.path, &text));
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.applying = false;
                match result {
                    Ok(()) => {
                        this.applied = true;
                        window.push_notification(
                            Notification::success(tr_args(
                                "vcs-merge-applied",
                                &[("file", this.file_name().into())],
                            )),
                            cx,
                        );
                        repository_changed(cx);
                    }
                    Err(err) => this.error = Some(err.to_string().into()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let m = &event.keystroke.modifiers;
        if event.keystroke.key == "f7" && !m.control && !m.alt && !m.platform {
            self.step(!m.shift, cx);
            cx.stop_propagation();
        }
    }

    fn render_pane(&self, pane: Pane, model: &MergeModel, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border, warning, success, accent) = (
            theme.muted_foreground,
            theme.border,
            theme.warning,
            theme.success,
            theme.accent,
        );
        let mono = theme.mono_font_family.clone();
        let mono_size = theme.mono_font_size;
        let title: SharedString = match pane {
            Pane::Ours => tr_args(
                "vcs-merge-ours",
                &[("label", model.ours_label.clone().into())],
            ),
            Pane::Theirs => tr_args(
                "vcs-merge-theirs",
                &[("label", model.theirs_label.clone().into())],
            ),
            Pane::Base => tr("vcs-merge-base"),
            Pane::Result => tr("vcs-merge-result"),
        };
        let scroll_ix = match pane {
            Pane::Ours => 0,
            Pane::Base => 1,
            Pane::Result => 2,
            Pane::Theirs => 3,
        };
        let lines = |lines: &[String]| {
            v_flex().children(
                lines
                    .iter()
                    .map(|l| div().min_h(px(18.)).whitespace_nowrap().child(l.clone())),
            )
        };
        let mut conflict_ix = 0;
        let segments: Vec<AnyElement> = model
            .segments
            .iter()
            .enumerate()
            .map(|(seg, segment)| match segment {
                Segment::Common(common) => div()
                    .id(("vcs-merge-seg", seg))
                    .child(lines(common))
                    .into_any_element(),
                Segment::Conflict(conflict) => {
                    let ix = conflict_ix;
                    conflict_ix += 1;
                    let resolved = conflict.resolution.is_some();
                    let color = if resolved { success } else { warning };
                    let body = match pane {
                        Pane::Ours => lines(&conflict.ours).into_any_element(),
                        Pane::Theirs => lines(&conflict.theirs).into_any_element(),
                        Pane::Base => lines(&conflict.base).into_any_element(),
                        Pane::Result => match conflict.resolved_lines() {
                            Some(resolved) => lines(&resolved).into_any_element(),
                            None => div()
                                .italic()
                                .text_color(muted)
                                .child(tr("vcs-merge-unresolved"))
                                .into_any_element(),
                        },
                    };
                    v_flex()
                        .id(("vcs-merge-seg", seg))
                        .my_1()
                        .border_l_2()
                        .border_color(color)
                        .bg(color.opacity(0.12))
                        .when(ix == self.current, |el| {
                            el.border_1().border_l_2().border_color(accent)
                        })
                        .when(pane == Pane::Result, |el| {
                            el.child(self.render_conflict_actions(ix, conflict, cx))
                        })
                        .child(div().pl_1().child(body))
                        .on_click(cx.listener(move |this, _, _, cx| this.go_to(ix, cx)))
                        .into_any_element()
                }
            })
            .collect();
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .border_r_1()
            .border_color(border)
            .child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(muted)
                    .border_b_1()
                    .border_color(border)
                    .truncate()
                    .child(title.clone()),
            )
            .child(
                v_flex()
                    .id(("vcs-merge-pane", scroll_ix))
                    .flex_1()
                    .min_h_0()
                    .p_1()
                    .overflow_scroll()
                    .track_scroll(&self.scrolls[scroll_ix])
                    .aria_label(title)
                    .font_family(mono)
                    .text_size(mono_size)
                    .children(segments),
            )
            .into_any_element()
    }

    fn render_conflict_actions(
        &self,
        ix: usize,
        conflict: &Conflict,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let current = conflict.resolution;
        let button = |id: &'static str,
                      key: &'static str,
                      resolution: Resolution,
                      cx: &mut Context<Self>| {
            Button::new((id, ix))
                .xsmall()
                .ghost()
                .label(tr(key))
                .selected(current == Some(resolution))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let next = (current != Some(resolution)).then_some(resolution);
                    this.resolve(ix, next, cx)
                }))
        };
        h_flex()
            .gap_1()
            .font_family(cx.theme().font_family.clone())
            .child(button(
                "vcs-merge-accept-left",
                "vcs-merge-accept-left",
                Resolution::Ours,
                cx,
            ))
            .child(button(
                "vcs-merge-accept-both",
                "vcs-merge-accept-both",
                Resolution::Both,
                cx,
            ))
            .child(button(
                "vcs-merge-accept-right",
                "vcs-merge-accept-right",
                Resolution::Theirs,
                cx,
            ))
            .child(button(
                "vcs-merge-ignore",
                "vcs-merge-ignore",
                Resolution::Base,
                cx,
            ))
    }
}

/// Register the builder that restores merge tabs from a saved layout.
pub(super) fn register(cx: &mut App, services_for: fn(EntityId, &App) -> Services) {
    register_panel(cx, MERGE_PANEL_NAME, move |ctx, _, cx| {
        let services = services_for(ctx.dock_area().entity_id(), cx);
        let path = match ctx.info() {
            PanelInfo::Panel(value) => serde_json::from_value::<MergeTabState>(value.clone())
                .ok()
                .map(|s| s.path),
            _ => None,
        }
        .unwrap_or_default();
        panel_handle(MergePanel::open(path, services, cx))
    });
}

/// Open the merge view for `path`; `None` opens the first conflicted file
/// or reports that there are no conflicts.
pub fn open(
    workspace: &mut Workspace,
    path: Option<PathBuf>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let Some(path) = path else {
        let task = cx.background_spawn(workspace.services.vcs.conflicts());
        cx.spawn_in(window, async move |this, cx| {
            let conflicts = task.await;
            let _ = this.update_in(cx, |this, window, cx| match conflicts {
                Ok(conflicts) => match conflicts.first() {
                    Some(first) => open(this, Some(first.path.clone()), window, cx),
                    None => this.notify_info("vcs-merge-no-conflicts", window, cx),
                },
                Err(err) => window.push_notification(Notification::error(err.to_string()), cx),
            });
        })
        .detach();
        return;
    };
    if let Some(id) = find_panel::<MergePanel>(workspace, cx, |p| p.path == path) {
        workspace
            .dock_area
            .update(cx, |area, cx| area.select_panel(id, window, cx));
        return;
    }
    let panel = MergePanel::open(path, workspace.services.clone(), cx);
    workspace.add_center_panel(panel.clone(), window, cx);
    panel.focus_handle(cx).focus(window, cx);
}

impl BasePanel for MergePanel {
    fn panel_name(&self) -> &'static str {
        MERGE_PANEL_NAME
    }

    fn closable(&self, _: &App) -> bool {
        true
    }

    fn zoomable(&self, _: &App) -> bool {
        true
    }

    fn dump(&self, _: &App) -> PanelState {
        PanelState {
            panel_name: MERGE_PANEL_NAME.to_string(),
            children: Vec::new(),
            info: PanelInfo::panel(
                serde_json::to_value(MergeTabState {
                    path: self.path.clone(),
                })
                .unwrap_or_default(),
            ),
        }
    }
}

impl Panel for MergePanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(tr_args(
            "vcs-merge-tab",
            &[("file", self.file_name().into())],
        ))
    }

    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        h_flex()
            .gap_1()
            .child(Icon::from(FerroIcon::Merge).small().text_color(muted))
            .child(tr_args(
                "vcs-merge-tab",
                &[("file", self.file_name().into())],
            ))
    }

    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        Some(PanelControl::Menu)
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl EventEmitter<PanelEvent> for MergePanel {}

impl Focusable for MergePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn status_color(unresolved: usize, cx: &App) -> Hsla {
    if unresolved == 0 {
        cx.theme().success
    } else {
        cx.theme().warning
    }
}

impl Render for MergePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border, danger, background) = (
            theme.muted_foreground,
            theme.border,
            theme.danger,
            theme.background,
        );

        let body: AnyElement = match &self.content {
            Content::Loading => h_flex()
                .flex_1()
                .justify_center()
                .gap_2()
                .text_color(muted)
                .child(Spinner::new().small())
                .child(tr("vcs-merge-loading"))
                .into_any_element(),
            Content::Failed(err) => div()
                .flex_1()
                .child(components::empty_state(
                    FerroIcon::Error,
                    tr("vcs-merge-failed"),
                    Some(err.clone()),
                    cx,
                ))
                .into_any_element(),
            Content::NoConflict => div()
                .flex_1()
                .child(components::empty_state(
                    FerroIcon::Ok,
                    tr("vcs-merge-no-conflicts-in-file"),
                    None,
                    cx,
                ))
                .into_any_element(),
            Content::Loaded(model) => {
                let model = model.clone();
                let total = model.conflict_count();
                let unresolved = model.unresolved_count();
                let can_apply = unresolved == 0 && !self.applying && !self.applied;
                let toolbar = h_flex()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(border)
                    .text_sm()
                    .child(
                        components::icon_button(
                            "vcs-merge-prev",
                            FerroIcon::ArrowUp,
                            tr("vcs-merge-previous-conflict"),
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.step(false, cx))),
                    )
                    .child(
                        components::icon_button(
                            "vcs-merge-next",
                            FerroIcon::ArrowDown,
                            tr("vcs-merge-next-conflict"),
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.step(true, cx))),
                    )
                    .child(
                        div()
                            .id("vcs-merge-counter")
                            .text_color(status_color(unresolved, cx))
                            .child(tr_args(
                                "vcs-merge-counter",
                                &[
                                    ("current", (self.current + 1).min(total).into()),
                                    ("total", total.into()),
                                    ("unresolved", unresolved.into()),
                                ],
                            )),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("vcs-merge-show-base")
                            .small()
                            .ghost()
                            .label(tr("vcs-merge-show-base"))
                            .selected(self.show_base)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let show = !this.show_base;
                                this.set_show_base(show, cx)
                            })),
                    )
                    .child(
                        Button::new("vcs-merge-apply")
                            .small()
                            .primary()
                            .loading(self.applying)
                            .label(if self.applied {
                                tr("vcs-merge-done")
                            } else {
                                tr("vcs-merge-apply")
                            })
                            .disabled(!can_apply)
                            .on_click(cx.listener(|this, _, window, cx| this.apply(window, cx))),
                    );
                let panes = h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_pane(Pane::Ours, &model, cx))
                    .when(self.show_base, |el| {
                        el.child(self.render_pane(Pane::Base, &model, cx))
                    })
                    .child(self.render_pane(Pane::Result, &model, cx))
                    .child(self.render_pane(Pane::Theirs, &model, cx));
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(toolbar)
                    .when_some(self.error.clone(), |el, err| {
                        el.child(
                            h_flex()
                                .px_2()
                                .gap_1()
                                .text_xs()
                                .text_color(danger)
                                .child(Icon::from(FerroIcon::Error).xsmall())
                                .child(err),
                        )
                    })
                    .child(panes)
                    .into_any_element()
            }
        };

        v_flex()
            .id("vcs-merge-panel")
            .key_context("MergePanel")
            .track_focus(&self.focus_handle)
            .aria_label(tr("vcs-merge"))
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .bg(background)
            .child(body)
    }
}
