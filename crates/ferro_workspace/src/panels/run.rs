//! The Run tool window: one tab per run configuration that ran, each with
//! its console and its test tree.
//!
//! The console keeps the program's ANSI colors (stderr in red when the
//! program does not color it), turns file paths and stack-trace frames into
//! links (`ferro_services::links`: Rust, Java/Kotlin, Python, JS/TS, Go, C#
//! and compiler output), and filters lines by text. The test tree follows
//! the run live (`RunEvent::Tests`); a test opens at its definition, and
//! **Rerun Failed Tests** starts a run of only the failed ones.

use std::{collections::HashSet, ops::Range};

use ferro_services::{
    RunEvent, RunState, Services, StyledSpan, TerminalLine, TestNode, TestStatus, links,
    parse_ansi, strip_ansi,
};
use ferro_ui::{FerroIcon, components, tr, tr_args};
use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, ClickEvent, Context, Entity, FocusHandle, Focusable,
    FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render,
    ScrollStrategy, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    UniformListScrollHandle, Window,
    component::{
        ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputEvent, InputState},
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px, uniform_list,
};

use super::common::{self, NodePath, ROW_HEIGHT};
use crate::requests::OpenLocation;

/// Console lines kept per run; older ones are dropped.
const MAX_LINES: usize = 50_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunTab {
    Console,
    Tests,
}

/// One console line: styled spans and its plain text (for links and the
/// filter).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleLine {
    pub spans: TerminalLine,
    pub text: String,
    pub error: bool,
}

impl ConsoleLine {
    pub fn new(raw: &str, error: bool) -> Self {
        let mut spans = parse_ansi(raw);
        // stderr without colors of its own is shown in red.
        if error {
            for span in &mut spans {
                if span.fg == ferro_services::AnsiColor::Default {
                    span.fg = ferro_services::AnsiColor::Red;
                }
            }
        }
        Self {
            text: strip_ansi(raw).replace('\r', ""),
            spans,
            error,
        }
    }
}

/// A run shown as a tab.
#[derive(Debug, Clone)]
pub struct RunEntry {
    /// The run configuration's id; `None` for results from before this
    /// window opened.
    pub id: Option<String>,
    pub name: String,
    pub state: RunState,
    pub lines: Vec<ConsoleLine>,
    pub tests: Vec<TestNode>,
    /// The test tab was shown automatically once.
    tests_shown: bool,
}

pub struct RunView {
    services: Services,
    runs: Vec<RunEntry>,
    active: usize,
    tab: RunTab,
    collapsed: HashSet<NodePath>,
    hide_passed: bool,
    rows: Vec<NodePath>,
    selected: Option<usize>,
    filter: Entity<InputState>,
    filter_text: String,
    /// Console lines that pass the filter (all when it is empty).
    visible: Vec<usize>,
    follow: bool,
    focus_handle: FocusHandle,
    console_scroll: UniformListScrollHandle,
    tests_scroll: UniformListScrollHandle,
    _filter_subscription: Subscription,
}

pub fn build(services: Services, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| RunView::new(services, window, cx)).into()
}

fn kids(node: &TestNode) -> &[TestNode] {
    &node.children
}

/// Passed/failed/ignored counts of the leaf tests.
pub fn summary(nodes: &[TestNode]) -> (usize, usize, usize) {
    let mut counts = (0, 0, 0);
    fn walk(nodes: &[TestNode], c: &mut (usize, usize, usize)) {
        for node in nodes {
            if node.children.is_empty() {
                match node.status {
                    TestStatus::Passed => c.0 += 1,
                    TestStatus::Failed => c.1 += 1,
                    TestStatus::Ignored => c.2 += 1,
                    _ => {}
                }
            } else {
                walk(&node.children, c);
            }
        }
    }
    walk(nodes, &mut counts);
    counts
}

fn status_icon(status: TestStatus) -> FerroIcon {
    match status {
        TestStatus::Passed => FerroIcon::TestPassed,
        TestStatus::Failed => FerroIcon::TestFailed,
        TestStatus::Ignored => FerroIcon::TestIgnored,
        TestStatus::Running => FerroIcon::Loading,
        TestStatus::NotRun => FerroIcon::Test,
    }
}

/// Split styled spans at the byte ranges of links: each piece with the
/// index of the link it belongs to.
pub fn split_at_links(
    spans: &[StyledSpan],
    links: &[Range<usize>],
) -> Vec<(StyledSpan, Option<usize>)> {
    let mut out = Vec::new();
    let mut offset = 0;
    for span in spans {
        let end = offset + span.text.len();
        // Cut points inside this span.
        let mut cuts: Vec<usize> = links
            .iter()
            .flat_map(|r| [r.start, r.end])
            .filter(|&p| p > offset && p < end)
            .collect();
        cuts.sort_unstable();
        cuts.dedup();
        let mut start = offset;
        for cut in cuts.into_iter().chain(std::iter::once(end)) {
            let (Some(text), true) = (span.text.get(start - offset..cut - offset), cut > start)
            else {
                continue;
            };
            let link = links.iter().position(|r| r.start <= start && cut <= r.end);
            out.push((
                StyledSpan {
                    text: text.to_string(),
                    ..span.clone()
                },
                link,
            ));
            start = cut;
        }
        offset = end;
    }
    out
}

impl RunView {
    pub fn new(services: Services, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let events = services.run.subscribe();
        cx.spawn(async move |this, cx| {
            while let Ok(first) = events.recv().await {
                let mut batch = vec![first];
                while let Ok(more) = events.try_recv() {
                    batch.push(more);
                }
                if this
                    .update(cx, |this, cx| {
                        for event in batch {
                            this.on_event(event);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let filter = cx
            .new(|cx| InputState::new(window, cx).placeholder(tr("panels-run-filter-placeholder")));
        let subscription = cx.subscribe(&filter, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.filter_text = input.read(cx).value().to_string();
                this.refilter_inner();
                cx.notify();
            }
        });
        let mut this = Self {
            services,
            runs: Vec::new(),
            active: 0,
            tab: RunTab::Console,
            collapsed: HashSet::new(),
            hide_passed: false,
            rows: Vec::new(),
            selected: None,
            filter,
            filter_text: String::new(),
            visible: Vec::new(),
            follow: true,
            focus_handle: cx.focus_handle(),
            console_scroll: UniformListScrollHandle::new(),
            tests_scroll: UniformListScrollHandle::new(),
            _filter_subscription: subscription,
        };
        // Results of a run from before this window opened (the mock has
        // some), shown on the Tests tab.
        let tests = this.services.run.test_results();
        if !tests.is_empty() {
            this.runs.push(RunEntry {
                id: None,
                name: tr("panels-run-last-results").to_string(),
                state: RunState::Idle,
                lines: Vec::new(),
                tests,
                tests_shown: true,
            });
            this.tab = RunTab::Tests;
            this.rebuild();
        }
        this
    }

    fn entry_index(&mut self, id: &str) -> usize {
        if let Some(ix) = self.runs.iter().position(|r| r.id.as_deref() == Some(id)) {
            return ix;
        }
        let name = self
            .services
            .run
            .configurations()
            .into_iter()
            .find(|c| c.id == id)
            .map(|c| c.name)
            .unwrap_or_else(|| id.to_string());
        self.runs.push(RunEntry {
            id: Some(id.to_string()),
            name,
            state: RunState::Idle,
            lines: Vec::new(),
            tests: Vec::new(),
            tests_shown: false,
        });
        self.runs.len() - 1
    }

    fn on_event(&mut self, event: RunEvent) {
        match event {
            RunEvent::StateChanged { id, state } => {
                let ix = self.entry_index(&id);
                let entry = &mut self.runs[ix];
                let restarted = matches!(state, RunState::Running { .. })
                    && !matches!(entry.state, RunState::Running { .. });
                entry.state = state;
                if restarted {
                    // A new run of the same configuration reuses its tab.
                    entry.lines.clear();
                    entry.tests.clear();
                    entry.tests_shown = false;
                    self.active = ix;
                    self.tab = RunTab::Console;
                    self.follow = true;
                    self.collapsed.clear();
                    self.selected = None;
                }
                if let RunState::Finished { exit_code } = state {
                    entry.lines.push(ConsoleLine::new(
                        &tr_args("run-finished", &[("code", exit_code.into())]),
                        false,
                    ));
                    entry.tests = self.services.run.test_results_for(&id);
                }
                if ix == self.active {
                    self.after_change();
                }
            }
            RunEvent::Output { id, line, error } => {
                let ix = self.entry_index(&id);
                let entry = &mut self.runs[ix];
                entry.lines.push(ConsoleLine::new(&line, error));
                if entry.lines.len() > MAX_LINES {
                    let excess = entry.lines.len() - MAX_LINES;
                    entry.lines.drain(..excess);
                }
                if ix == self.active {
                    self.after_change();
                }
            }
            RunEvent::Tests { id } => {
                let ix = self.entry_index(&id);
                let tests = self.services.run.test_results_for(&id);
                let entry = &mut self.runs[ix];
                entry.tests = tests;
                if !entry.tests.is_empty() && !entry.tests_shown {
                    entry.tests_shown = true;
                    if ix == self.active {
                        self.tab = RunTab::Tests;
                    }
                }
                if ix == self.active {
                    self.rebuild();
                }
            }
            RunEvent::ConfigurationsChanged => {}
        }
    }

    fn after_change(&mut self) {
        self.refilter_inner();
        self.rebuild();
        if self.follow && !self.visible.is_empty() {
            self.console_scroll
                .scroll_to_item(self.visible.len() - 1, ScrollStrategy::Bottom);
        }
    }

    fn entry(&self) -> Option<&RunEntry> {
        self.runs.get(self.active)
    }

    pub fn runs(&self) -> &[RunEntry] {
        &self.runs
    }

    pub fn active_run(&self) -> Option<&RunEntry> {
        self.entry()
    }

    pub fn tab(&self) -> RunTab {
        self.tab
    }

    pub fn activate(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.runs.len() {
            self.active = ix;
            self.collapsed.clear();
            self.selected = None;
            if !self.runs[ix].tests.is_empty() && self.runs[ix].lines.is_empty() {
                self.tab = RunTab::Tests;
            }
            self.after_change();
            cx.notify();
        }
    }

    pub fn close_run(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.runs.len() {
            return;
        }
        if let Some(id) = self.runs[ix].id.clone()
            && matches!(self.runs[ix].state, RunState::Running { .. })
        {
            self.services.run.stop(&id);
        }
        self.runs.remove(ix);
        self.active = self.active.min(self.runs.len().saturating_sub(1));
        self.after_change();
        cx.notify();
    }

    pub fn reload_tests(&mut self) {
        let tests = match self.entry().and_then(|e| e.id.clone()) {
            Some(id) => self.services.run.test_results_for(&id),
            None => self.services.run.test_results(),
        };
        if let Some(entry) = self.runs.get_mut(self.active) {
            entry.tests = tests;
        }
        self.rebuild();
    }

    /// Replace the test tree of the active run (tests and benchmarks).
    pub fn set_tests(&mut self, tests: Vec<TestNode>) {
        if self.runs.is_empty() {
            self.runs.push(RunEntry {
                id: None,
                name: tr("panels-run-last-results").to_string(),
                state: RunState::Idle,
                lines: Vec::new(),
                tests: Vec::new(),
                tests_shown: true,
            });
        }
        if let Some(entry) = self.runs.get_mut(self.active) {
            entry.tests = tests;
        }
        self.rebuild();
    }

    fn tests(&self) -> &[TestNode] {
        self.entry().map(|e| e.tests.as_slice()).unwrap_or(&[])
    }

    fn rebuild(&mut self) {
        let hide_passed = self.hide_passed;
        let collapsed = &self.collapsed;
        let tests = self
            .runs
            .get(self.active)
            .map(|e| e.tests.as_slice())
            .unwrap_or(&[]);
        self.rows = common::flatten(tests, &kids, &|p| collapsed.contains(p), &|n| {
            !(hide_passed && n.status == TestStatus::Passed)
        });
        if self.selected.is_some_and(|ix| ix >= self.rows.len()) {
            self.selected = None;
        }
    }

    /// Show only console lines containing `text` (case-insensitive).
    pub fn set_filter(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.filter.update(cx, |input, cx| {
            input.set_value(text.to_string(), window, cx)
        });
        self.filter_text = text.to_string();
        self.refilter_inner();
        cx.notify();
    }

    fn refilter_inner(&mut self) {
        let filter = self.filter_text.to_lowercase();
        let lines = self
            .runs
            .get(self.active)
            .map(|e| e.lines.as_slice())
            .unwrap_or(&[]);
        self.visible = if filter.is_empty() {
            (0..lines.len()).collect()
        } else {
            lines
                .iter()
                .enumerate()
                .filter(|(_, l)| l.text.to_lowercase().contains(&filter))
                .map(|(ix, _)| ix)
                .collect()
        };
    }

    /// Console lines shown with the current filter.
    pub fn visible_lines(&self) -> Vec<&ConsoleLine> {
        let lines = self.entry().map(|e| e.lines.as_slice()).unwrap_or(&[]);
        self.visible
            .iter()
            .filter_map(|&ix| lines.get(ix))
            .collect()
    }

    pub fn set_tab(&mut self, tab: RunTab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    pub fn set_hide_passed(&mut self, hide: bool, cx: &mut Context<Self>) {
        self.hide_passed = hide;
        self.rebuild();
        cx.notify();
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn summary(&self) -> (usize, usize, usize) {
        summary(self.tests())
    }

    fn node(&self, row: usize) -> Option<&TestNode> {
        self.rows
            .get(row)
            .and_then(|p| common::node_at(self.tests(), p, &kids))
    }

    pub fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.selected = Some(ix);
        self.tests_scroll.scroll_to_item(ix, ScrollStrategy::Center);
        cx.notify();
    }

    /// The output of the selected test.
    pub fn selected_output(&self) -> Vec<String> {
        self.selected
            .and_then(|ix| self.node(ix))
            .map(|n| n.output.clone())
            .unwrap_or_default()
    }

    fn toggle(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(path) = self.rows.get(ix).cloned() {
            if !self.collapsed.remove(&path) {
                self.collapsed.insert(path);
            }
            self.rebuild();
            cx.notify();
        }
    }

    fn open_test(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((path, line)) = self.node(ix).and_then(|n| n.location.clone()) {
            window.dispatch_action(
                Box::new(OpenLocation::at(
                    path,
                    ferro_services::TextPos::new(line, 0),
                )),
                cx,
            );
        }
    }

    /// Start the active run's configuration again.
    pub fn rerun(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.entry().and_then(|e| e.id.clone()) else {
            return;
        };
        if let Err(err) = self.services.run.start(&id, false) {
            crate::notify_warning(window, cx, err.to_string());
        }
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.entry().and_then(|e| e.id.clone()) {
            self.services.run.stop(&id);
        }
        cx.notify();
    }

    /// Run the failed tests of the active run again.
    pub fn rerun_failed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.entry().and_then(|e| e.id.clone()) else {
            return;
        };
        if let Err(err) = self.services.run.rerun_failed(&id) {
            crate::notify_warning(window, cx, err.to_string());
        }
    }

    pub fn clear_console(&mut self, cx: &mut Context<Self>) {
        if let Some(entry) = self.runs.get_mut(self.active) {
            entry.lines.clear();
        }
        self.after_change();
        cx.notify();
    }

    /// Open the file a console link points to.
    pub fn open_link(
        &mut self,
        line: usize,
        link: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.entry() else { return };
        let Some(text) = entry.lines.get(line).map(|l| l.text.clone()) else {
            return;
        };
        let Some(found) = links::find_links(&text).into_iter().nth(link) else {
            return;
        };
        let cwd = entry
            .id
            .as_deref()
            .and_then(|id| self.services.run.working_directory(id));
        let roots = self.services.project.roots();
        let cwd = cwd.or_else(|| roots.first().cloned());
        match links::resolve_link(&found, cwd.as_deref(), &roots) {
            Some((path, line, col)) => window.dispatch_action(
                Box::new(OpenLocation::at(
                    path,
                    ferro_services::TextPos::new(line, col),
                )),
                cx,
            ),
            None => crate::notify_warning(
                window,
                cx,
                tr_args("panels-run-link-missing", &[("path", found.path.into())]).to_string(),
            ),
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab != RunTab::Tests || common::has_command_modifier(&event.keystroke) {
            return;
        }
        let key = event.keystroke.key.as_str();
        if let Some(ix) = common::nav(key, self.selected, self.rows.len()) {
            self.select(ix, cx);
        } else if let Some(ix) = self.selected {
            let has_children = self.node(ix).is_some_and(|n| !n.children.is_empty());
            let collapsed = self.collapsed.contains(&self.rows[ix]);
            match key {
                "enter" if has_children => self.toggle(ix, cx),
                "enter" => self.open_test(ix, window, cx),
                "left" if has_children && !collapsed => self.toggle(ix, cx),
                "right" if has_children && collapsed => self.toggle(ix, cx),
                _ => return,
            }
        } else {
            return;
        }
        cx.stop_propagation();
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let (muted, selected_bg, hover_bg, success, danger) = (
            theme.muted_foreground,
            theme.list_active,
            theme.list_hover,
            theme.success,
            theme.danger,
        );
        range
            .filter_map(|ix| {
                let path = self.rows.get(ix)?;
                let node = common::node_at(self.tests(), path, &kids)?;
                Some((ix, path.len(), node))
            })
            .map(|(ix, depth, node)| {
                let color = match node.status {
                    TestStatus::Passed => success,
                    TestStatus::Failed => danger,
                    _ => muted,
                };
                let has_children = !node.children.is_empty();
                let open = !self.collapsed.contains(&self.rows[ix]);
                h_flex()
                    .id(ix)
                    .h(px(ROW_HEIGHT))
                    .pl(px(4. + depth as f32 * 14.))
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
                        if event.click_count() >= 2 {
                            this.open_test(ix, window, cx);
                        } else if has_children {
                            this.toggle(ix, cx);
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
                    .child(
                        Icon::from(status_icon(node.status))
                            .small()
                            .text_color(color),
                    )
                    .child(div().flex_1().truncate().child(node.name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(format!("{} ms", node.duration_ms)),
                    )
                    .into_any_element()
            })
            .collect()
    }

    fn render_console_rows(
        &mut self,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let link_color = cx.theme().link;
        let line_h = cx.theme().mono_font_size * 1.4;
        let Some(entry) = self.runs.get(self.active) else {
            return Vec::new();
        };
        range
            .filter_map(|row| {
                let ix = *self.visible.get(row)?;
                let line = entry.lines.get(ix)?;
                let found = links::find_links(&line.text);
                let ranges: Vec<Range<usize>> = found.iter().map(|l| l.range.clone()).collect();
                let pieces = split_at_links(&line.spans, &ranges);
                Some(
                    h_flex()
                        .id(("run-console-line", ix))
                        .h(line_h)
                        .whitespace_nowrap()
                        .children(pieces.into_iter().enumerate().map(|(n, (span, link))| {
                            let (fg, bg) = super::terminal::span_colors(&span, cx);
                            let el = div()
                                .id(("run-console-span", n))
                                .text_color(fg)
                                .when_some(bg, |el, bg| el.bg(bg))
                                .when(span.bold, |el| el.font_weight(FontWeight::BOLD))
                                .when(span.underline, |el| el.underline())
                                .child(SharedString::from(span.text));
                            match link {
                                Some(link) => el
                                    .text_color(link_color)
                                    .underline()
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_link(ix, link, window, cx)
                                    }))
                                    .into_any_element(),
                                None => el.into_any_element(),
                            }
                        }))
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn render_console(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.entry().is_none_or(|e| e.lines.is_empty()) {
            return components::empty_state(FerroIcon::Run, tr("run-empty"), None, cx)
                .into_any_element();
        }
        let theme = cx.theme();
        uniform_list(
            "run-console",
            self.visible.len(),
            cx.processor(|this, range: Range<usize>, _, cx| this.render_console_rows(range, cx)),
        )
        .track_scroll(&self.console_scroll)
        .size_full()
        .p_2()
        .font_family(theme.mono_font_family.clone())
        .text_size(theme.mono_font_size)
        .into_any_element()
    }

    fn render_tests(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.tests().is_empty() {
            return components::empty_state(FerroIcon::Test, tr("panels-run-no-tests"), None, cx)
                .into_any_element();
        }
        let theme = cx.theme();
        let (muted, border, danger) = (theme.muted_foreground, theme.border, theme.danger);
        let (passed, failed, ignored) = self.summary();
        let output = self.selected_output();
        h_flex()
            .size_full()
            .child(
                v_flex()
                    .w(gpui_kit::relative(0.5))
                    .h_full()
                    .border_r_1()
                    .border_color(border)
                    .child(common::caption(
                        tr_args(
                            "panels-run-summary",
                            &[
                                ("passed", passed.into()),
                                ("failed", failed.into()),
                                ("ignored", ignored.into()),
                            ],
                        ),
                        cx,
                    ))
                    .child(
                        uniform_list(
                            "run-tests",
                            self.rows.len(),
                            cx.processor(|this, range: Range<usize>, _, cx| {
                                this.render_rows(range, cx)
                            }),
                        )
                        .track_scroll(&self.tests_scroll)
                        .flex_1(),
                    ),
            )
            .child(
                v_flex()
                    .id("run-test-output")
                    .flex_1()
                    .h_full()
                    .p_2()
                    .overflow_y_scroll()
                    .font_family(theme.mono_font_family.clone())
                    .text_size(theme.mono_font_size)
                    .when(output.is_empty(), |el| {
                        el.text_color(muted).child(tr("panels-run-select-test"))
                    })
                    .children(output.into_iter().map(|line| {
                        let is_panic = line.contains("panicked")
                            || line.contains("Error")
                            || line.contains("assert");
                        div().when(is_panic, |el| el.text_color(danger)).child(line)
                    })),
            )
            .into_any_element()
    }

    fn render_run_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (success, danger, muted) = (theme.success, theme.danger, theme.muted_foreground);
        let mut tabs = common::toolbar("run-tabs", cx);
        for (ix, run) in self.runs.iter().enumerate() {
            let (icon, color) = match run.state {
                RunState::Running { .. } => (FerroIcon::Loading, success),
                RunState::Finished { exit_code: 0 } => (FerroIcon::Ok, success),
                RunState::Finished { .. } => (FerroIcon::Error, danger),
                RunState::Idle => (FerroIcon::Run, muted),
            };
            tabs = tabs.child(
                h_flex()
                    .child(
                        Button::new(SharedString::from(format!("run-tab-{ix}")))
                            .ghost()
                            .xsmall()
                            .icon(Icon::from(icon).text_color(color))
                            .label(SharedString::from(run.name.clone()))
                            .selected(ix == self.active)
                            .on_click(cx.listener(move |this, _, _, cx| this.activate(ix, cx))),
                    )
                    .child(
                        common::tool_button(
                            SharedString::from(format!("run-tab-close-{ix}")),
                            FerroIcon::Close,
                            tr("panels-run-close-tab"),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.close_run(ix, cx))),
                    ),
            );
        }
        tabs
    }
}

impl Focusable for RunView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RunView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = |id: &'static str,
                   t: RunTab,
                   label: SharedString,
                   this: &Self,
                   cx: &mut Context<Self>| {
            Button::new(id)
                .ghost()
                .xsmall()
                .label(label)
                .selected(this.tab == t)
                .on_click(cx.listener(move |this, _, _, cx| this.set_tab(t, cx)))
        };
        let running = self
            .entry()
            .is_some_and(|e| matches!(e.state, RunState::Running { .. }));
        let has_config = self.entry().is_some_and(|e| e.id.is_some());
        let failed = self.summary().1 > 0;
        let toolbar = common::toolbar("run-toolbar", cx)
            .child(
                common::tool_button("run-rerun", FerroIcon::Refresh, tr("panels-run-rerun"))
                    .disabled(!has_config || running)
                    .on_click(cx.listener(|this, _, window, cx| this.rerun(window, cx))),
            )
            .child(
                common::tool_button("run-stop", FerroIcon::Stop, tr("action-stop"))
                    .disabled(!running)
                    .on_click(cx.listener(|this, _, _, cx| this.stop(cx))),
            )
            .child(
                common::tool_button(
                    "run-rerun-failed",
                    FerroIcon::TestFailed,
                    tr("action-rerun-failed-tests"),
                )
                .disabled(!has_config || running || !failed)
                .on_click(cx.listener(|this, _, window, cx| this.rerun_failed(window, cx))),
            )
            .child(tab(
                "run-tab-console",
                RunTab::Console,
                tr("panels-run-console"),
                self,
                cx,
            ))
            .child(tab(
                "run-tab-tests",
                RunTab::Tests,
                tr("panels-run-tests"),
                self,
                cx,
            ))
            .child(div().flex_1())
            .when(self.tab == RunTab::Console, |el| {
                el.child(
                    div()
                        .w(px(180.))
                        .child(Input::new(&self.filter).xsmall().cleanable(true)),
                )
                .child(
                    common::tool_button(
                        "run-follow",
                        FerroIcon::ArrowDown,
                        tr("panels-run-follow"),
                    )
                    .selected(self.follow)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.follow = !this.follow;
                        this.after_change();
                        cx.notify();
                    })),
                )
                .child(
                    common::tool_button("run-clear", FerroIcon::Delete, tr("panels-run-clear"))
                        .on_click(cx.listener(|this, _, _, cx| this.clear_console(cx))),
                )
            })
            .when(self.tab == RunTab::Tests, |el| {
                el.child(
                    common::tool_button(
                        "run-hide-passed",
                        FerroIcon::Filter,
                        tr("panels-run-hide-passed"),
                    )
                    .selected(self.hide_passed)
                    .on_click(cx.listener(|this, _, _, cx| {
                        let hide = !this.hide_passed;
                        this.set_hide_passed(hide, cx)
                    })),
                )
            });
        let body = match self.tab {
            RunTab::Console => self.render_console(cx),
            RunTab::Tests => self.render_tests(cx),
        };
        v_flex()
            .id("run")
            .track_focus(&self.focus_handle)
            .aria_label(tr("tool-run"))
            .size_full()
            .on_key_down(cx.listener(Self::on_key_down))
            .when(!self.runs.is_empty(), |el| {
                el.child(self.render_run_tabs(cx))
            })
            .child(toolbar)
            .child(div().flex_1().min_h_0().child(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(text: &str) -> StyledSpan {
        StyledSpan {
            text: text.into(),
            ..Default::default()
        }
    }

    #[test]
    fn spans_are_split_at_link_boundaries() {
        let spans = [span("error at "), span("src/a.rs:3:4 now")];
        let links: Vec<Range<usize>> = std::iter::once(9..21).collect();
        let pieces = split_at_links(&spans, &links);
        let texts: Vec<_> = pieces.iter().map(|(s, l)| (s.text.as_str(), *l)).collect();
        assert_eq!(
            texts,
            [
                ("error at ", None),
                ("src/a.rs:3:4", Some(0)),
                (" now", None)
            ]
        );
    }

    #[test]
    fn stderr_lines_turn_red_unless_colored() {
        let line = ConsoleLine::new("\x1b[33mwarn\x1b[0m plain", true);
        assert_eq!(line.spans[0].fg, ferro_services::AnsiColor::Yellow);
        assert_eq!(line.spans[1].fg, ferro_services::AnsiColor::Red);
        assert_eq!(line.text, "warn plain");
    }
}
