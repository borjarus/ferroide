//! The Problems tool window: the project's diagnostics grouped by file, with
//! severity filters, sorting and keyboard navigation.

use std::{
    collections::{BTreeMap, HashSet},
    ops::Range,
    path::PathBuf,
};

use ferro_services::{Diagnostic, Services, Severity};
use ferro_ui::{FerroIcon, components, tr, tr_args};
use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, ClickEvent, Context, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, ScrollStrategy,
    SharedString, StatefulInteractiveElement as _, Styled as _, UniformListScrollHandle, Window,
    component::{
        ActiveTheme as _, Icon, Selectable as _, Sizable as _, h_flex, spinner::Spinner, v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px, uniform_list,
};

use super::common::{self, ROW_HEIGHT};
use crate::requests::OpenLocation;

enum State {
    Loading,
    Loaded,
    Failed(SharedString),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProblemRow {
    /// A file header; index into `groups`.
    File(usize),
    /// A diagnostic; index into `diagnostics`.
    Item(usize),
}

struct Group {
    path: PathBuf,
    relative: SharedString,
    items: Vec<usize>,
    worst: Severity,
}

pub struct ProblemsView {
    services: Services,
    state: State,
    diagnostics: Vec<Diagnostic>,
    groups: Vec<Group>,
    rows: Vec<ProblemRow>,
    /// Hidden severities.
    hidden: HashSet<Severity>,
    sort_by_severity: bool,
    collapsed: HashSet<PathBuf>,
    selected: Option<usize>,
    focus_handle: FocusHandle,
    scroll: UniformListScrollHandle,
}

pub fn build(services: Services, _window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| ProblemsView::new(services, cx)).into()
}

/// Shown severity buckets: Info covers hints.
fn bucket(severity: Severity) -> Severity {
    match severity {
        Severity::Hint => Severity::Info,
        other => other,
    }
}

impl ProblemsView {
    pub fn new(services: Services, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            services,
            state: State::Loading,
            diagnostics: Vec::new(),
            groups: Vec::new(),
            rows: Vec::new(),
            hidden: HashSet::new(),
            sort_by_severity: false,
            collapsed: HashSet::new(),
            selected: None,
            focus_handle: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
        };
        this.refresh(cx);
        this
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.state = State::Loading;
        let task = cx.background_spawn(self.services.language.project_diagnostics());
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(diagnostics) => this.set_diagnostics(diagnostics),
                    Err(err) => this.state = State::Failed(err.to_string().into()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn set_diagnostics(&mut self, diagnostics: Vec<Diagnostic>) {
        self.diagnostics = diagnostics;
        self.state = State::Loaded;
        self.rebuild();
    }

    fn rebuild(&mut self) {
        let root = self.services.project.roots().first().cloned();
        let mut by_file: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
        for (ix, d) in self.diagnostics.iter().enumerate() {
            if !self.hidden.contains(&bucket(d.severity)) {
                by_file.entry(d.path.clone()).or_default().push(ix);
            }
        }
        let diagnostics = &self.diagnostics;
        let mut groups: Vec<Group> = by_file
            .into_iter()
            .map(|(path, mut items)| {
                items.sort_by_key(|&ix| {
                    let d = &diagnostics[ix];
                    (d.range.start, d.severity)
                });
                let worst = items
                    .iter()
                    .map(|&ix| diagnostics[ix].severity)
                    .min()
                    .unwrap_or(Severity::Hint);
                Group {
                    relative: common::relative(&path, root.as_deref()).into(),
                    path,
                    items,
                    worst,
                }
            })
            .collect();
        if self.sort_by_severity {
            groups.sort_by_key(|g| g.worst);
            for g in &mut groups {
                g.items
                    .sort_by_key(|&ix| (diagnostics[ix].severity, diagnostics[ix].range.start));
            }
        }
        let mut rows = Vec::new();
        for (gi, group) in groups.iter().enumerate() {
            rows.push(ProblemRow::File(gi));
            if !self.collapsed.contains(&group.path) {
                rows.extend(group.items.iter().map(|&ix| ProblemRow::Item(ix)));
            }
        }
        self.groups = groups;
        self.rows = rows;
        if self.selected.is_some_and(|ix| ix >= self.rows.len()) {
            self.selected = None;
        }
    }

    pub fn toggle_severity(&mut self, severity: Severity, cx: &mut Context<Self>) {
        let severity = bucket(severity);
        if !self.hidden.remove(&severity) {
            self.hidden.insert(severity);
        }
        self.rebuild();
        cx.notify();
    }

    pub fn toggle_sort(&mut self, cx: &mut Context<Self>) {
        self.sort_by_severity = !self.sort_by_severity;
        self.rebuild();
        cx.notify();
    }

    pub fn toggle_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.collapsed.remove(&path) {
            self.collapsed.insert(path);
        }
        self.rebuild();
        cx.notify();
    }

    /// Number of file groups shown.
    pub fn group_count(&self) -> usize {
        self.groups.len()
    }

    /// Number of diagnostics shown (not collapsed away by filters).
    pub fn shown_count(&self) -> usize {
        self.groups.iter().map(|g| g.items.len()).sum()
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn is_loaded(&self) -> bool {
        matches!(self.state, State::Loaded)
    }

    fn count(&self, severity: Severity) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| bucket(d.severity) == severity)
            .count()
    }

    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.selected = Some(ix);
        self.scroll.scroll_to_item(ix, ScrollStrategy::Center);
        cx.notify();
    }

    /// Open the diagnostic at row `ix`, or fold the file header.
    pub fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        match self.rows.get(ix).copied() {
            Some(ProblemRow::File(gi)) => {
                let path = self.groups[gi].path.clone();
                self.toggle_file(path, cx);
            }
            Some(ProblemRow::Item(di)) => {
                let d = &self.diagnostics[di];
                window.dispatch_action(
                    Box::new(OpenLocation::at(d.path.clone(), d.range.start)),
                    cx,
                );
            }
            None => {}
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        if common::has_command_modifier(&event.keystroke) {
            return;
        }
        if let Some(ix) = common::nav(key, self.selected, self.rows.len()) {
            self.select(ix, cx);
        } else if key == "enter" {
            if let Some(ix) = self.selected {
                self.activate(ix, window, cx);
            }
        } else if key == "left" || key == "right" {
            let group = match self.selected.and_then(|ix| self.rows.get(ix)) {
                Some(ProblemRow::File(gi)) => Some(*gi),
                _ => None,
            };
            if let Some(gi) = group {
                let path = self.groups[gi].path.clone();
                let collapsed = self.collapsed.contains(&path);
                if (key == "left") != collapsed {
                    self.toggle_file(path, cx);
                }
            }
        } else {
            return;
        }
        cx.stop_propagation();
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let (muted, selected_bg, hover_bg) =
            (theme.muted_foreground, theme.list_active, theme.list_hover);
        range
            .filter_map(|ix| self.rows.get(ix).copied().map(|r| (ix, r)))
            .map(|(ix, row)| {
                let base = h_flex()
                    .id(ix)
                    .h(px(ROW_HEIGHT))
                    .px_2()
                    .gap_1()
                    .w_full()
                    .text_sm()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .cursor_pointer()
                    .when(Some(ix) == self.selected, |el| el.bg(selected_bg))
                    .hover(|el| el.bg(hover_bg))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.focus_handle.focus(window, cx);
                        this.select(ix, cx);
                        this.activate(ix, window, cx);
                    }));
                match row {
                    ProblemRow::File(gi) => {
                        let group = &self.groups[gi];
                        let open = !self.collapsed.contains(&group.path);
                        base.aria_label(group.relative.clone())
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
                                Icon::from(FerroIcon::for_file(&group.relative))
                                    .small()
                                    .text_color(muted),
                            )
                            .child(div().truncate().child(group.relative.clone()))
                            .child(div().text_xs().text_color(muted).child(tr_args(
                                "panels-problems-count",
                                &[("count", group.items.len().into())],
                            )))
                            .into_any_element()
                    }
                    ProblemRow::Item(di) => {
                        let d = &self.diagnostics[di];
                        let color = common::severity_color(d.severity, cx);
                        let location =
                            format!("{}:{}", d.range.start.line + 1, d.range.start.column + 1);
                        base.pl(px(28.))
                            .aria_label(SharedString::from(d.message.clone()))
                            .child(
                                Icon::from(common::severity_icon(d.severity))
                                    .small()
                                    .text_color(color),
                            )
                            .child(div().flex_1().truncate().child(d.message.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(format!("{} {}", d.source, location)),
                            )
                            .into_any_element()
                    }
                }
            })
            .collect()
    }
}

impl Focusable for ProblemsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ProblemsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let filter = |id: &'static str,
                      severity: Severity,
                      icon: FerroIcon,
                      key: &str,
                      this: &Self,
                      cx: &mut Context<Self>| {
            let label = tr_args(key, &[("count", this.count(severity).into())]);
            common::tool_button(id, icon, label.clone())
                .label(label)
                .selected(!this.hidden.contains(&severity))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_severity(severity, cx)))
        };
        let toolbar = common::toolbar("problems-toolbar", cx)
            .child(filter(
                "problems-errors",
                Severity::Error,
                FerroIcon::Error,
                "panels-problems-errors",
                self,
                cx,
            ))
            .child(filter(
                "problems-warnings",
                Severity::Warning,
                FerroIcon::Warning,
                "panels-problems-warnings",
                self,
                cx,
            ))
            .child(filter(
                "problems-infos",
                Severity::Info,
                FerroIcon::Info,
                "panels-problems-infos",
                self,
                cx,
            ))
            .child(div().flex_1())
            .child(
                common::tool_button(
                    "problems-sort",
                    FerroIcon::Filter,
                    tr("panels-problems-sort-severity"),
                )
                .selected(self.sort_by_severity)
                .on_click(cx.listener(|this, _, _, cx| this.toggle_sort(cx))),
            )
            .child(
                common::tool_button("problems-refresh", FerroIcon::Refresh, tr("panels-refresh"))
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            );
        let body = match &self.state {
            State::Loading => h_flex()
                .size_full()
                .justify_center()
                .gap_2()
                .text_color(cx.theme().muted_foreground)
                .child(Spinner::new().small())
                .child(tr("panels-loading"))
                .into_any_element(),
            State::Failed(err) => components::empty_state(
                FerroIcon::Error,
                tr("panels-load-failed"),
                Some(err.clone()),
                cx,
            )
            .into_any_element(),
            State::Loaded if self.rows.is_empty() => {
                components::empty_state(FerroIcon::Ok, tr("panels-problems-empty"), None, cx)
                    .into_any_element()
            }
            State::Loaded => uniform_list(
                "problems-rows",
                self.rows.len(),
                cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx)),
            )
            .track_scroll(&self.scroll)
            .size_full()
            .into_any_element(),
        };
        v_flex()
            .id("problems")
            .track_focus(&self.focus_handle)
            .aria_label(tr("tool-problems"))
            .size_full()
            .on_key_down(cx.listener(Self::on_key_down))
            .child(toolbar)
            .child(div().flex_1().min_h_0().child(body))
    }
}
