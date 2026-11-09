//! The Run tool window: the console of the selected run configuration and
//! the results of the last test run as a tree.

use std::{collections::HashSet, ops::Range};

use ferro_services::{RunEvent, RunState, Services, TestNode, TestStatus};
use ferro_ui::{FerroIcon, components, tr, tr_args};
use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, ClickEvent, Context, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, ScrollHandle,
    ScrollStrategy, SharedString, StatefulInteractiveElement as _, Styled as _,
    UniformListScrollHandle, Window,
    component::{
        ActiveTheme as _, Icon, Selectable as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex, v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px, uniform_list,
};

use super::common::{self, NodePath, ROW_HEIGHT};
use crate::requests::OpenLocation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunTab {
    Console,
    Tests,
}

pub struct RunView {
    services: Services,
    tab: RunTab,
    lines: Vec<SharedString>,
    tests: Vec<TestNode>,
    collapsed: HashSet<NodePath>,
    hide_passed: bool,
    rows: Vec<NodePath>,
    selected: Option<usize>,
    focus_handle: FocusHandle,
    console_scroll: ScrollHandle,
    tests_scroll: UniformListScrollHandle,
}

pub fn build(services: Services, cx: &mut App) -> AnyView {
    cx.new(|cx| RunView::new(services, cx)).into()
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

impl RunView {
    pub fn new(services: Services, cx: &mut Context<Self>) -> Self {
        let events = services.run.subscribe();
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                let alive = this.update(cx, |this, cx| {
                    match event {
                        RunEvent::Output { line, .. } => this.lines.push(line.into()),
                        RunEvent::StateChanged { state, .. } => {
                            if let RunState::Finished { exit_code } = state {
                                this.lines
                                    .push(tr_args("run-finished", &[("code", exit_code.into())]));
                                this.reload_tests();
                            }
                        }
                    }
                    this.console_scroll.scroll_to_bottom();
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        let mut this = Self {
            services,
            tab: RunTab::Console,
            lines: Vec::new(),
            tests: Vec::new(),
            collapsed: HashSet::new(),
            hide_passed: false,
            rows: Vec::new(),
            selected: None,
            focus_handle: cx.focus_handle(),
            console_scroll: ScrollHandle::new(),
            tests_scroll: UniformListScrollHandle::new(),
        };
        this.reload_tests();
        this
    }

    pub fn reload_tests(&mut self) {
        self.tests = self.services.run.test_results();
        self.rebuild();
    }

    /// Replace the test tree (tests and benchmarks).
    pub fn set_tests(&mut self, tests: Vec<TestNode>) {
        self.tests = tests;
        self.rebuild();
    }

    fn rebuild(&mut self) {
        let hide_passed = self.hide_passed;
        let collapsed = &self.collapsed;
        self.rows = common::flatten(&self.tests, &kids, &|p| collapsed.contains(p), &|n| {
            !(hide_passed && n.status == TestStatus::Passed)
        });
        if self.selected.is_some_and(|ix| ix >= self.rows.len()) {
            self.selected = None;
        }
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
        summary(&self.tests)
    }

    fn node(&self, row: usize) -> Option<&TestNode> {
        self.rows
            .get(row)
            .and_then(|p| common::node_at(&self.tests, p, &kids))
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
                let node = common::node_at(&self.tests, path, &kids)?;
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

    fn render_console(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.lines.is_empty() {
            return components::empty_state(FerroIcon::Run, tr("run-empty"), None, cx)
                .into_any_element();
        }
        v_flex()
            .id("run-console")
            .track_scroll(&self.console_scroll)
            .size_full()
            .p_2()
            .overflow_y_scroll()
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(cx.theme().mono_font_size)
            .children(self.lines.iter().cloned().map(|line| div().child(line)))
            .into_any_element()
    }

    fn render_tests(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.tests.is_empty() {
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
                        let is_panic = line.contains("panicked");
                        div().when(is_panic, |el| el.text_color(danger)).child(line)
                    })),
            )
            .into_any_element()
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
        let toolbar = common::toolbar("run-toolbar", cx)
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
            .child(toolbar)
            .child(div().flex_1().min_h_0().child(body))
    }
}
