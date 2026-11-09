//! The Commit tool window: working-tree changes grouped by changelist, with
//! checkboxes, a commit message and the Commit button (IntelliJ's
//! non-modal commit).
//!
//! Keyboard: Up/Down move the selection, Space toggles the checkbox (of a
//! file or a whole changelist), Enter opens the diff (the merge view for a
//! conflicted file), Left/Right collapse and expand a changelist.

use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
};

use ferro_services::{ChangeKind, FileChange, Services};
use ferro_ui::{FerroIcon, components, tr, tr_args};
use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, ClickEvent, Context, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    component::{
        ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _,
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
        h_flex,
        input::{InputEvent, Textarea, TextareaState},
        notification::Notification,
        spinner::Spinner,
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};

use crate::{
    requests::{OpenDiff, OpenMerge},
    vcs::{VcsGeneration, change_color, repository_changed},
};

enum Load {
    Loading,
    Loaded,
    Failed(SharedString),
}

/// A visible row: a changelist header or a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Group(String),
    /// Index into `changes`.
    File(usize),
}

pub struct CommitView {
    services: Services,
    load: Load,
    changes: Vec<FileChange>,
    checked: HashSet<PathBuf>,
    collapsed: HashSet<String>,
    rows: Vec<Row>,
    selected: Option<usize>,
    message: Entity<TextareaState>,
    amend: bool,
    committing: bool,
    /// The last commit error, shown under the message.
    error: Option<SharedString>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl CommitView {
    pub fn new(services: Services, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(tr("vcs-commit-message-placeholder"))
                .auto_grow(3, 10)
        });
        let subscriptions = vec![
            cx.subscribe(&message, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.error = None;
                    cx.notify();
                }
            }),
            cx.observe_global::<VcsGeneration>(|this, cx| this.refresh(cx)),
        ];
        let mut this = Self {
            services,
            load: Load::Loading,
            changes: Vec::new(),
            checked: HashSet::new(),
            collapsed: HashSet::new(),
            rows: Vec::new(),
            selected: None,
            message,
            amend: false,
            committing: false,
            error: None,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.refresh(cx);
        this
    }

    /// Reload the changes. Checked files stay checked; new files are
    /// checked (except unversioned ones, as in IntelliJ).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.changes.is_empty() {
            self.load = Load::Loading;
        }
        let task = cx.background_spawn(self.services.vcs.changes());
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(changes) => {
                        let known: HashSet<PathBuf> =
                            this.changes.iter().map(|c| c.path.clone()).collect();
                        for change in &changes {
                            if !known.contains(&change.path) && change.kind != ChangeKind::Untracked
                            {
                                this.checked.insert(change.path.clone());
                            }
                        }
                        this.checked
                            .retain(|p| changes.iter().any(|c| &c.path == p));
                        this.changes = changes;
                        this.load = Load::Loaded;
                    }
                    Err(err) => this.load = Load::Failed(err.to_string().into()),
                }
                this.rebuild_rows();
                cx.notify();
            });
        })
        .detach();
    }

    fn groups(&self) -> BTreeMap<&str, Vec<usize>> {
        let mut groups: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (ix, change) in self.changes.iter().enumerate() {
            groups
                .entry(change.changelist.as_str())
                .or_default()
                .push(ix);
        }
        groups
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        // The default changelist first, then the others by name.
        let mut groups: Vec<(String, Vec<usize>)> = self
            .groups()
            .into_iter()
            .map(|(name, files)| (name.to_string(), files))
            .collect();
        groups.sort_by_key(|(name, _)| (name != "Changes", name.clone()));
        for (name, files) in groups {
            let collapsed = self.collapsed.contains(&name);
            rows.push(Row::Group(name));
            if !collapsed {
                rows.extend(files.into_iter().map(Row::File));
            }
        }
        self.rows = rows;
        if let Some(sel) = self.selected
            && sel >= self.rows.len()
        {
            self.selected = self.rows.len().checked_sub(1);
        }
    }

    pub fn changes(&self) -> &[FileChange] {
        &self.changes
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn is_loaded(&self) -> bool {
        matches!(self.load, Load::Loaded)
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    pub fn is_checked(&self, path: &std::path::Path) -> bool {
        self.checked.contains(path)
    }

    pub fn set_checked(&mut self, path: &std::path::Path, checked: bool, cx: &mut Context<Self>) {
        if checked {
            self.checked.insert(path.to_path_buf());
        } else {
            self.checked.remove(path);
        }
        cx.notify();
    }

    /// Check or uncheck every file of a changelist.
    pub fn set_group_checked(&mut self, group: &str, checked: bool, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self
            .changes
            .iter()
            .filter(|c| c.changelist == group)
            .map(|c| c.path.clone())
            .collect();
        for path in paths {
            self.set_checked(&path, checked, cx);
        }
    }

    /// `Some(true)` all checked, `Some(false)` none, `None` some.
    fn group_state(&self, group: &str) -> Option<bool> {
        let files: Vec<&FileChange> = self
            .changes
            .iter()
            .filter(|c| c.changelist == group)
            .collect();
        let checked = files
            .iter()
            .filter(|c| self.checked.contains(&c.path))
            .count();
        match checked {
            0 => Some(false),
            n if n == files.len() => Some(true),
            _ => None,
        }
    }

    pub fn toggle_group(&mut self, group: &str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(group) {
            self.collapsed.insert(group.to_string());
        }
        self.rebuild_rows();
        cx.notify();
    }

    pub fn set_message(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.message.update(cx, |state, cx| {
            state.set_value(text.to_string(), window, cx)
        });
    }

    fn message_text(&self, cx: &App) -> String {
        self.message.read(cx).value().to_string()
    }

    fn checked_paths(&self) -> Vec<PathBuf> {
        self.changes
            .iter()
            .filter(|c| self.checked.contains(&c.path))
            .map(|c| c.path.clone())
            .collect()
    }

    /// Whether the Commit button is enabled.
    pub fn can_commit(&self, cx: &App) -> bool {
        !self.committing
            && self.is_loaded()
            && !self.message_text(cx).trim().is_empty()
            && !self.checked_paths().is_empty()
    }

    pub fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_commit(cx) {
            return;
        }
        if self.amend {
            window.push_notification(Notification::info(tr("vcs-amend-not-available")), cx);
        }
        let message = self.message_text(cx);
        let paths = self.checked_paths();
        self.committing = true;
        self.error = None;
        let task = cx.background_spawn(self.services.vcs.commit(&message, &paths));
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.committing = false;
                match result {
                    Ok(id) => {
                        let short: String = id.chars().take(8).collect();
                        window.push_notification(
                            Notification::success(tr_args(
                                "vcs-committed",
                                &[("hash", short.into()), ("count", paths.len().into())],
                            )),
                            cx,
                        );
                        this.message
                            .update(cx, |state, cx| state.set_value("", window, cx));
                        this.amend = false;
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

    /// Open the selected file: its diff, or the merge view when conflicted.
    fn open_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        match self.rows.get(ix).cloned() {
            Some(Row::File(file)) => {
                let change = &self.changes[file];
                if change.kind == ChangeKind::Conflicted {
                    window.dispatch_action(
                        Box::new(OpenMerge {
                            path: change.path.clone(),
                        }),
                        cx,
                    );
                } else {
                    window.dispatch_action(
                        Box::new(OpenDiff {
                            path: change.path.clone(),
                        }),
                        cx,
                    );
                }
            }
            Some(Row::Group(name)) => self.toggle_group(&name, cx),
            None => {}
        }
    }

    fn toggle_row(&mut self, ix: usize, cx: &mut Context<Self>) {
        match self.rows.get(ix).cloned() {
            Some(Row::File(file)) => {
                let path = self.changes[file].path.clone();
                let checked = !self.checked.contains(&path);
                self.set_checked(&path, checked, cx);
            }
            Some(Row::Group(name)) => {
                let checked = self.group_state(&name) != Some(true);
                self.set_group_checked(&name, checked, cx);
            }
            None => {}
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = &event.keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.platform {
            return;
        }
        let count = self.rows.len();
        if count == 0 {
            return;
        }
        let selected = self.selected.unwrap_or(0);
        match event.keystroke.key.as_str() {
            "down" => self.selected = Some((selected + 1).min(count - 1)),
            "up" => self.selected = Some(selected.saturating_sub(1)),
            "home" => self.selected = Some(0),
            "end" => self.selected = Some(count - 1),
            "space" => self.toggle_row(selected, cx),
            "enter" => self.open_row(selected, window, cx),
            "left" | "right" => {
                if let Some(Row::Group(name)) = self.rows.get(selected).cloned() {
                    let collapse = event.keystroke.key == "left";
                    if collapse != self.collapsed.contains(&name) {
                        self.toggle_group(&name, cx);
                    }
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn show_selected_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ix = self
            .selected
            .filter(|ix| matches!(self.rows.get(*ix), Some(Row::File(_))))
            .or_else(|| self.rows.iter().position(|r| matches!(r, Row::File(_))));
        if let Some(ix) = ix {
            self.open_row(ix, window, cx);
        }
    }

    fn render_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let all_collapsed = self.groups().keys().all(|g| self.collapsed.contains(*g));
        h_flex()
            .gap_1()
            .px_1()
            .child(
                components::icon_button(
                    "vcs-commit-refresh",
                    FerroIcon::Refresh,
                    tr("vcs-refresh"),
                )
                .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
            .child(
                components::icon_button("vcs-commit-diff", FerroIcon::Diff, tr("vcs-show-diff"))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.show_selected_diff(window, cx)),
                    ),
            )
            .child(
                components::icon_button("vcs-commit-rollback", FerroIcon::Undo, tr("vcs-rollback"))
                    .on_click(|_, window, cx| {
                        window.push_notification(
                            Notification::info(tr("vcs-rollback-not-available")),
                            cx,
                        )
                    }),
            )
            .child(div().flex_1())
            .child(
                components::icon_button(
                    "vcs-commit-expand",
                    if all_collapsed {
                        FerroIcon::ExpandAll
                    } else {
                        FerroIcon::CollapseAll
                    },
                    if all_collapsed {
                        tr("vcs-expand-all")
                    } else {
                        tr("vcs-collapse-all")
                    },
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if all_collapsed {
                        this.collapsed.clear();
                    } else {
                        this.collapsed = this.groups().keys().map(|g| g.to_string()).collect();
                    }
                    this.rebuild_rows();
                    cx.notify();
                })),
            )
    }

    fn render_row(&self, ix: usize, row: &Row, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, selected_bg, hover_bg) =
            (theme.muted_foreground, theme.list_active, theme.list_hover);
        let is_selected = self.selected == Some(ix);
        let base = h_flex()
            .id(("vcs-commit-row", ix))
            .h(px(24.))
            .px_2()
            .gap_1()
            .w_full()
            .text_sm()
            .cursor_pointer()
            .when(is_selected, |el| el.bg(selected_bg))
            .hover(|el| el.bg(hover_bg))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.selected = Some(ix);
                this.focus_handle.focus(window, cx);
                if event.click_count() >= 2 {
                    this.open_row(ix, window, cx);
                }
                cx.notify();
            }));
        match row {
            Row::Group(name) => {
                let open = !self.collapsed.contains(name);
                let count = self
                    .changes
                    .iter()
                    .filter(|c| &c.changelist == name)
                    .count();
                let state = self.group_state(name);
                let group = name.clone();
                base.aria_label(SharedString::from(format!("{name}, {count}")))
                    .child(
                        Icon::from(if open {
                            FerroIcon::ChevronDown
                        } else {
                            FerroIcon::ChevronRight
                        })
                        .xsmall()
                        .text_color(muted),
                    )
                    .child(
                        Checkbox::new(("vcs-group-check", ix))
                            .checked(state == Some(true))
                            .accessibility_label(SharedString::from(name.clone()))
                            .tab_stop(false)
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                this.set_group_checked(&group, *checked, cx)
                            })),
                    )
                    .when(state.is_none(), |el| {
                        el.child(div().text_xs().text_color(muted).child("–"))
                    })
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(tr_args("vcs-file-count", &[("count", count.into())])),
                    )
                    .into_any_element()
            }
            Row::File(file) => {
                let change = &self.changes[*file];
                let (dir, name) = match change.relative.rsplit_once('/') {
                    Some((dir, name)) => (dir.to_string(), name.to_string()),
                    None => (String::new(), change.relative.clone()),
                };
                let color = change_color(change.kind, cx);
                let path = change.path.clone();
                base.pl(px(28.))
                    .aria_label(SharedString::from(format!(
                        "{} {}",
                        change.relative,
                        tr(kind_key(change.kind))
                    )))
                    .child(
                        Checkbox::new(("vcs-file-check", ix))
                            .checked(self.checked.contains(&change.path))
                            .accessibility_label(SharedString::from(change.relative.clone()))
                            .tab_stop(false)
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                this.set_checked(&path, *checked, cx)
                            })),
                    )
                    .child(
                        Icon::from(FerroIcon::for_file(&name))
                            .small()
                            .text_color(muted),
                    )
                    .child(div().text_color(color).truncate().child(name))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(muted)
                            .child(dir),
                    )
                    .child(
                        div()
                            .w(px(14.))
                            .text_xs()
                            .text_color(color)
                            .child(change.kind.letter()),
                    )
                    .into_any_element()
            }
        }
    }
}

/// Message id of a change kind's name.
pub fn kind_key(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Modified => "vcs-kind-modified",
        ChangeKind::Added => "vcs-kind-added",
        ChangeKind::Deleted => "vcs-kind-deleted",
        ChangeKind::Renamed => "vcs-kind-renamed",
        ChangeKind::Untracked => "vcs-kind-untracked",
        ChangeKind::Conflicted => "vcs-kind-conflicted",
    }
}

impl Focusable for CommitView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for CommitView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, danger, border) = (theme.muted_foreground, theme.danger, theme.border);

        let list: AnyElement = match &self.load {
            Load::Loading => h_flex()
                .flex_1()
                .justify_center()
                .gap_2()
                .text_sm()
                .text_color(muted)
                .child(Spinner::new().small())
                .child(tr("vcs-loading-changes"))
                .into_any_element(),
            Load::Failed(err) => v_flex()
                .flex_1()
                .child(components::empty_state(
                    FerroIcon::Error,
                    tr("vcs-changes-failed"),
                    Some(err.clone()),
                    cx,
                ))
                .child(
                    h_flex().justify_center().pb_2().child(
                        Button::new("vcs-commit-retry")
                            .small()
                            .label(tr("vcs-retry"))
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
                )
                .into_any_element(),
            Load::Loaded if self.changes.is_empty() => div()
                .flex_1()
                .child(components::empty_state(
                    FerroIcon::Ok,
                    tr("vcs-no-changes"),
                    None,
                    cx,
                ))
                .into_any_element(),
            Load::Loaded => {
                let rows: Vec<AnyElement> = self
                    .rows
                    .clone()
                    .iter()
                    .enumerate()
                    .map(|(ix, row)| self.render_row(ix, row, cx))
                    .collect();
                v_flex()
                    .id("vcs-commit-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_focus(&self.focus_handle)
                    .aria_label(tr("vcs-changes"))
                    .on_key_down(cx.listener(Self::on_key_down))
                    .children(rows)
                    .into_any_element()
            }
        };

        let can_commit = self.can_commit(cx);
        let checked = self.checked_paths().len();
        v_flex()
            .id("vcs-commit")
            .key_context("CommitView")
            .size_full()
            .child(self.render_toolbar(cx))
            .child(list)
            .child(
                v_flex()
                    .gap_2()
                    .p_2()
                    .border_t_1()
                    .border_color(border)
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Checkbox::new("vcs-amend")
                                    .label(tr("vcs-amend"))
                                    .checked(self.amend)
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.amend = *checked;
                                        cx.notify();
                                    })),
                            )
                            .child(div().flex_1())
                            .child(div().text_xs().text_color(muted).child(tr_args(
                                "vcs-selected-count",
                                &[("count", checked.into())],
                            ))),
                    )
                    .child(
                        Textarea::new(&self.message)
                            .aria_label(tr("vcs-commit-message"))
                            .tab_index(0),
                    )
                    .when_some(self.error.clone(), |el, error| {
                        el.child(
                            h_flex()
                                .id("vcs-commit-error")
                                .gap_1()
                                .text_xs()
                                .text_color(danger)
                                .child(Icon::from(FerroIcon::Error).xsmall())
                                .child(error),
                        )
                    })
                    .child(
                        h_flex().gap_2().child(
                            Button::new("vcs-commit-button")
                                .primary()
                                .small()
                                .loading(self.committing)
                                .label(tr("vcs-commit"))
                                .disabled(!can_commit)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.commit(window, cx)),
                                ),
                        ),
                    ),
            )
    }
}

pub fn build(services: Services, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| CommitView::new(services, window, cx)).into()
}
