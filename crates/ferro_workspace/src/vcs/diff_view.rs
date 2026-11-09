//! The diff viewer: a file's working copy against `HEAD`, side by side or
//! unified, as a tab in the editor area. Built on GPUI Kit's patch viewer.
//!
//! Keyboard: F7 / Shift+F7 jump to the next / previous change; the diff body
//! itself scrolls with the arrow keys once focused.

use std::path::{Path, PathBuf};

use ferro_services::Services;
use ferro_ui::{FerroIcon, components, tr, tr_args};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EntityId, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window,
    component::{
        ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _,
        button::{Button, ButtonGroup},
        diff::{Diff, DiffFile, DiffMode, DiffState},
        dock::{
            BasePanel, DockPlacement, Panel, PanelControl, PanelEvent, PanelId, PanelInfo,
            PanelState, panel_handle, register_panel,
        },
        h_flex,
        spinner::Spinner,
        v_flex,
    },
    div,
};
use serde::{Deserialize, Serialize};

use crate::workspace::Workspace;

pub const DIFF_PANEL_NAME: &str = "ferro.diff";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DiffTabState {
    path: PathBuf,
    #[serde(default)]
    split: bool,
}

enum Content {
    Loading,
    Loaded {
        state: Entity<DiffState>,
        additions: usize,
        deletions: usize,
    },
    /// The file has no changes against `HEAD`.
    Unchanged,
    Failed(SharedString),
}

pub struct DiffPanel {
    path: PathBuf,
    services: Services,
    content: Content,
    mode: DiffMode,
    focus_handle: FocusHandle,
}

impl DiffPanel {
    pub fn open(path: PathBuf, split: bool, services: Services, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let mut this = Self {
                path,
                services,
                content: Content::Loading,
                mode: if split {
                    DiffMode::Split
                } else {
                    DiffMode::Unified
                },
                focus_handle: cx.focus_handle(),
            };
            this.reload(cx);
            this
        })
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.content = Content::Loading;
        let task = cx.background_spawn(self.services.vcs.diff(&self.path));
        let language = crate::editor::language_for(&self.path);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.content = match result {
                    Err(err) => Content::Failed(err.to_string().into()),
                    Ok(diff) => match DiffFile::parse(&diff.patch) {
                        Err(err) => Content::Failed(err.message().clone()),
                        Ok(files) if !files.iter().any(DiffFile::has_changes) => Content::Unchanged,
                        Ok(files) => {
                            let files: Vec<DiffFile> = files
                                .into_iter()
                                .map(|f| f.with_language(language))
                                .collect();
                            let additions = files.iter().map(DiffFile::additions).sum();
                            let deletions = files.iter().map(DiffFile::deletions).sum();
                            let mode = this.mode;
                            let state = cx.new(|cx| DiffState::new(files, cx).with_mode(mode));
                            Content::Loaded {
                                state,
                                additions,
                                deletions,
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

    pub fn is_loaded(&self) -> bool {
        matches!(self.content, Content::Loaded { .. } | Content::Unchanged)
    }

    /// `(additions, deletions)` once loaded.
    pub fn stats(&self) -> Option<(usize, usize)> {
        match self.content {
            Content::Loaded {
                additions,
                deletions,
                ..
            } => Some((additions, deletions)),
            _ => None,
        }
    }

    pub fn is_split(&self) -> bool {
        self.mode == DiffMode::Split
    }

    pub fn set_split(&mut self, split: bool, cx: &mut Context<Self>) {
        self.mode = if split {
            DiffMode::Split
        } else {
            DiffMode::Unified
        };
        if let Content::Loaded { state, .. } = &self.content {
            let mode = self.mode;
            state.update(cx, |state, cx| state.set_mode(mode, cx));
        }
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    pub fn next_change(&mut self, forward: bool, cx: &mut Context<Self>) {
        if let Content::Loaded { state, .. } = &self.content {
            state.update(cx, |state, cx| {
                if forward {
                    state.next_change(cx)
                } else {
                    state.previous_change(cx)
                }
            });
        }
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
            self.next_change(!m.shift, cx);
            cx.stop_propagation();
        }
    }
}

/// Register the builder that restores diff tabs from a saved layout.
pub(super) fn register(cx: &mut App, services_for: fn(EntityId, &App) -> Services) {
    register_panel(cx, DIFF_PANEL_NAME, move |ctx, _, cx| {
        let services = services_for(ctx.dock_area().entity_id(), cx);
        let state = match ctx.info() {
            PanelInfo::Panel(value) => serde_json::from_value::<DiffTabState>(value.clone()).ok(),
            _ => None,
        };
        // A broken entry still restores, as a tab showing the error.
        let state = state.unwrap_or_else(|| DiffTabState {
            path: services
                .project
                .roots()
                .first()
                .cloned()
                .unwrap_or_default(),
            split: false,
        });
        panel_handle(DiffPanel::open(state.path, state.split, services, cx))
    });
}

/// Open the diff of `path` in the editor area, or show its existing tab.
pub fn open(
    workspace: &mut Workspace,
    path: PathBuf,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    if let Some(id) = find_panel::<DiffPanel>(workspace, cx, |p| p.path == path) {
        workspace
            .dock_area
            .update(cx, |area, cx| area.select_panel(id, window, cx));
        return;
    }
    let panel = DiffPanel::open(path, false, workspace.services.clone(), cx);
    workspace.add_center_panel(panel.clone(), window, cx);
    panel.focus_handle(cx).focus(window, cx);
}

/// The id of a center panel of type `T` matching `pred`.
pub(crate) fn find_panel<T: 'static>(
    workspace: &Workspace,
    cx: &App,
    pred: impl Fn(&T) -> bool,
) -> Option<PanelId> {
    let area = workspace.dock_area.read(cx);
    let tree = area.layout(DockPlacement::Center)?;
    tree.panels().find(|id| {
        area.panel(*id)
            .and_then(|p| p.view().downcast::<T>().ok())
            .is_some_and(|p| pred(p.read(cx)))
    })
}

impl BasePanel for DiffPanel {
    fn panel_name(&self) -> &'static str {
        DIFF_PANEL_NAME
    }

    fn closable(&self, _: &App) -> bool {
        true
    }

    fn zoomable(&self, _: &App) -> bool {
        true
    }

    fn dump(&self, _: &App) -> PanelState {
        let state = DiffTabState {
            path: self.path.clone(),
            split: self.is_split(),
        };
        PanelState {
            panel_name: DIFF_PANEL_NAME.to_string(),
            children: Vec::new(),
            info: PanelInfo::panel(serde_json::to_value(state).unwrap_or_default()),
        }
    }
}

impl Panel for DiffPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(tr_args(
            "vcs-diff-tab",
            &[("file", self.file_name().into())],
        ))
    }

    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        h_flex()
            .gap_1()
            .child(Icon::from(FerroIcon::Diff).small().text_color(muted))
            .child(tr_args(
                "vcs-diff-tab",
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

impl EventEmitter<PanelEvent> for DiffPanel {}

impl Focusable for DiffPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for DiffPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, success, danger, border) = (
            theme.muted_foreground,
            theme.success,
            theme.danger,
            theme.border,
        );
        let relative = self
            .services
            .project
            .roots()
            .first()
            .and_then(|root| self.path.strip_prefix(root).ok())
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|| self.path.to_string_lossy().into_owned());
        let split = self.is_split();
        let loaded = matches!(self.content, Content::Loaded { .. });

        let header = h_flex()
            .gap_2()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(border)
            .text_sm()
            .child(
                Icon::from(FerroIcon::for_file(&relative))
                    .small()
                    .text_color(muted),
            )
            .child(div().truncate().child(relative))
            .children(self.stats().map(|(a, d)| {
                h_flex()
                    .gap_1()
                    .text_xs()
                    .child(div().text_color(success).child(format!("+{a}")))
                    .child(div().text_color(danger).child(format!("−{d}")))
            }))
            .child(div().flex_1())
            .child(
                components::icon_button(
                    "vcs-diff-prev",
                    FerroIcon::ArrowUp,
                    tr("vcs-diff-previous-change"),
                )
                .disabled(!loaded)
                .on_click(cx.listener(|this, _, _, cx| this.next_change(false, cx))),
            )
            .child(
                components::icon_button(
                    "vcs-diff-next",
                    FerroIcon::ArrowDown,
                    tr("vcs-diff-next-change"),
                )
                .disabled(!loaded)
                .on_click(cx.listener(|this, _, _, cx| this.next_change(true, cx))),
            )
            .child(
                ButtonGroup::new("vcs-diff-mode")
                    .small()
                    .child(
                        Button::new("vcs-diff-split")
                            .label(tr("vcs-diff-side-by-side"))
                            .selected(split),
                    )
                    .child(
                        Button::new("vcs-diff-unified")
                            .label(tr("vcs-diff-unified"))
                            .selected(!split),
                    )
                    .on_click(cx.listener(|this, selected: &Vec<usize>, _, cx| {
                        if let Some(&ix) = selected.first() {
                            this.set_split(ix == 0, cx);
                        }
                    })),
            );

        let body = match &self.content {
            Content::Loading => h_flex()
                .flex_1()
                .justify_center()
                .gap_2()
                .text_color(muted)
                .child(Spinner::new().small())
                .child(tr("vcs-diff-loading"))
                .into_any_element(),
            Content::Failed(err) => div()
                .flex_1()
                .child(components::empty_state(
                    FerroIcon::Error,
                    tr("vcs-diff-failed"),
                    Some(err.clone()),
                    cx,
                ))
                .into_any_element(),
            Content::Unchanged => div()
                .flex_1()
                .child(components::empty_state(
                    FerroIcon::Ok,
                    tr("vcs-diff-no-changes"),
                    None,
                    cx,
                ))
                .into_any_element(),
            Content::Loaded { state, .. } => div()
                .flex_1()
                .min_h_0()
                .child(Diff::new(state).header_visible(false).size_full())
                .into_any_element(),
        };

        v_flex()
            .id("vcs-diff-panel")
            .key_context("DiffPanel")
            .track_focus(&self.focus_handle)
            .aria_label(tr("vcs-diff"))
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .bg(cx.theme().background)
            .child(header)
            .child(body)
    }
}
