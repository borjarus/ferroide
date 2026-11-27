//! The Build tool window: every build tool project found in the project
//! (Cargo, Maven, Gradle, npm / pnpm / yarn, Go, CMake, dotnet) with its
//! tasks, like IntelliJ's Gradle and Maven tool windows.
//!
//! A task runs as a temporary run configuration in the Run tool window
//! (double click, Enter, ▶) or in a new terminal session. Both go through
//! the workspace ([`RunBuildTask`]), which checks that the project is
//! trusted first.

use std::{collections::HashSet, ops::Range};

use ferro_services::{BuildProject, BuildTask, Services};
use ferro_ui::{FerroIcon, components, tr, tr_args};
use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, ClickEvent, Context, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, ScrollStrategy,
    SharedString, StatefulInteractiveElement as _, Styled as _, UniformListScrollHandle, Window,
    component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex},
    div,
    prelude::FluentBuilder as _,
    px, uniform_list,
};

use super::common::{self, ROW_HEIGHT};
use crate::requests::RunBuildTask;

/// A visible row: a project, or one of its tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildRow {
    Project(usize),
    Task(usize, usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Load {
    Loading,
    Loaded,
    Failed(String),
}

pub struct BuildToolsView {
    services: Services,
    projects: Vec<BuildProject>,
    load: Load,
    collapsed: HashSet<usize>,
    rows: Vec<BuildRow>,
    selected: Option<usize>,
    focus_handle: FocusHandle,
    scroll: UniformListScrollHandle,
}

pub fn build(services: Services, _window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| BuildToolsView::new(services, cx)).into()
}

/// The display name of a build tool id.
pub fn tool_name(tool: &str) -> &str {
    match tool {
        "cargo" => "Cargo",
        "maven" => "Maven",
        "gradle" => "Gradle",
        "npm" => "npm",
        "pnpm" => "pnpm",
        "yarn" => "Yarn",
        "go" => "Go",
        "cmake" => "CMake",
        "dotnet" => ".NET",
        other => other,
    }
}

impl BuildToolsView {
    pub fn new(services: Services, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            services,
            projects: Vec::new(),
            load: Load::Loading,
            collapsed: HashSet::new(),
            rows: Vec::new(),
            selected: None,
            focus_handle: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
        };
        this.refresh(cx);
        this
    }

    /// Discover the projects again (after a manifest changed).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.load = Load::Loading;
        let task = cx.background_spawn(self.services.run.build_projects());
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(projects) => {
                        this.projects = projects;
                        this.load = Load::Loaded;
                    }
                    Err(err) => this.load = Load::Failed(format!("{err:#}")),
                }
                this.rebuild();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn is_loaded(&self) -> bool {
        self.load == Load::Loaded
    }

    pub fn projects(&self) -> &[BuildProject] {
        &self.projects
    }

    pub fn rows(&self) -> &[BuildRow] {
        &self.rows
    }

    fn rebuild(&mut self) {
        self.rows.clear();
        for (p, project) in self.projects.iter().enumerate() {
            self.rows.push(BuildRow::Project(p));
            if !self.collapsed.contains(&p) {
                self.rows
                    .extend((0..project.tasks.len()).map(|t| BuildRow::Task(p, t)));
            }
        }
        if self.selected.is_some_and(|ix| ix >= self.rows.len()) {
            self.selected = None;
        }
    }

    pub fn set_all_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.collapsed = if collapsed {
            (0..self.projects.len()).collect()
        } else {
            HashSet::new()
        };
        self.rebuild();
        cx.notify();
    }

    fn toggle(&mut self, project: usize, cx: &mut Context<Self>) {
        if !self.collapsed.remove(&project) {
            self.collapsed.insert(project);
        }
        self.rebuild();
        cx.notify();
    }

    pub fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.selected = Some(ix);
        self.scroll.scroll_to_item(ix, ScrollStrategy::Center);
        cx.notify();
    }

    /// The task of the selected row.
    pub fn selected_task(&self) -> Option<&BuildTask> {
        match self.rows.get(self.selected?)? {
            BuildRow::Task(p, t) => self.projects.get(*p)?.tasks.get(*t),
            BuildRow::Project(_) => None,
        }
    }

    /// Run the selected task.
    pub fn run_selected(&mut self, in_terminal: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(task) = self.selected_task().cloned() {
            window.dispatch_action(Box::new(RunBuildTask { task, in_terminal }), cx);
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if common::has_command_modifier(&event.keystroke) {
            return;
        }
        let key = event.keystroke.key.as_str();
        if let Some(ix) = common::nav(key, self.selected, self.rows.len()) {
            self.select(ix, cx);
        } else if let Some(ix) = self.selected {
            match (key, self.rows[ix]) {
                ("enter", BuildRow::Task(..)) => self.run_selected(false, window, cx),
                ("enter", BuildRow::Project(p)) => self.toggle(p, cx),
                ("left", BuildRow::Project(p)) if !self.collapsed.contains(&p) => {
                    self.toggle(p, cx)
                }
                ("right", BuildRow::Project(p)) if self.collapsed.contains(&p) => {
                    self.toggle(p, cx)
                }
                _ => return,
            }
        } else {
            return;
        }
        cx.stop_propagation();
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let (muted, selected_bg, hover_bg, success) = (
            theme.muted_foreground,
            theme.list_active,
            theme.list_hover,
            theme.success,
        );
        let root = self.services.project.roots().into_iter().next();
        range
            .filter_map(|ix| Some((ix, *self.rows.get(ix)?)))
            .map(|(ix, row)| {
                let base = h_flex()
                    .id(ix)
                    .h(px(ROW_HEIGHT))
                    .pr_2()
                    .gap_1()
                    .w_full()
                    .text_sm()
                    .cursor_pointer()
                    .when(Some(ix) == self.selected, |el| el.bg(selected_bg))
                    .hover(|el| el.bg(hover_bg));
                match row {
                    BuildRow::Project(p) => {
                        let project = &self.projects[p];
                        let open = !self.collapsed.contains(&p);
                        let manifest = common::relative(&project.manifest, root.as_deref());
                        base.pl(px(4.))
                            .aria_label(SharedString::from(project.name.clone()))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.focus_handle.focus(window, cx);
                                this.select(ix, cx);
                                this.toggle(p, cx);
                            }))
                            .child(
                                Icon::from(if open {
                                    FerroIcon::ChevronDown
                                } else {
                                    FerroIcon::ChevronRight
                                })
                                .xsmall()
                                .text_color(muted),
                            )
                            .child(Icon::from(FerroIcon::Build).small().text_color(muted))
                            .child(div().flex_none().child(format!(
                                "{} · {}",
                                tool_name(&project.tool),
                                project.name
                            )))
                            .child(
                                div()
                                    .flex_1()
                                    .truncate()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(manifest),
                            )
                            .into_any_element()
                    }
                    BuildRow::Task(p, t) => {
                        let task = &self.projects[p].tasks[t];
                        let icon = if task.is_test {
                            FerroIcon::Test
                        } else {
                            FerroIcon::Run
                        };
                        base.pl(px(36.))
                            .aria_label(SharedString::from(task.name.clone()))
                            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                                this.focus_handle.focus(window, cx);
                                this.select(ix, cx);
                                if event.click_count() >= 2 {
                                    this.run_selected(false, window, cx);
                                }
                            }))
                            .child(Icon::from(icon).small().text_color(success))
                            .child(div().flex_none().child(task.name.clone()))
                            .child(div().flex_1().truncate().text_xs().text_color(muted).child(
                                if task.description.is_empty() {
                                    task.command.clone()
                                } else {
                                    task.description.clone()
                                },
                            ))
                            .into_any_element()
                    }
                }
            })
            .collect()
    }
}

impl Focusable for BuildToolsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for BuildToolsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_task = self.selected_task().is_some();
        let toolbar = common::toolbar("build-toolbar", cx)
            .child(
                common::tool_button(
                    "build-refresh",
                    FerroIcon::Refresh,
                    tr("panels-build-refresh"),
                )
                .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
            .child(
                common::tool_button("build-run", FerroIcon::Run, tr("panels-build-run"))
                    .disabled(!has_task)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.run_selected(false, window, cx)),
                    ),
            )
            .child(
                common::tool_button(
                    "build-run-terminal",
                    FerroIcon::Terminal,
                    tr("panels-build-run-in-terminal"),
                )
                .disabled(!has_task)
                .on_click(cx.listener(|this, _, window, cx| this.run_selected(true, window, cx))),
            )
            .child(div().flex_1())
            .child(
                common::tool_button("build-expand", FerroIcon::ExpandAll, tr("vcs-expand-all"))
                    .on_click(cx.listener(|this, _, _, cx| this.set_all_collapsed(false, cx))),
            )
            .child(
                common::tool_button(
                    "build-collapse",
                    FerroIcon::CollapseAll,
                    tr("vcs-collapse-all"),
                )
                .on_click(cx.listener(|this, _, _, cx| this.set_all_collapsed(true, cx))),
            );
        let body = match &self.load {
            Load::Loading if self.projects.is_empty() => {
                components::empty_state(FerroIcon::Loading, tr("panels-build-loading"), None, cx)
                    .into_any_element()
            }
            Load::Failed(err) => components::empty_state(
                FerroIcon::Error,
                tr_args("panels-build-failed", &[("error", err.clone().into())]),
                None,
                cx,
            )
            .into_any_element(),
            _ if self.projects.is_empty() => {
                components::empty_state(FerroIcon::Build, tr("panels-build-empty"), None, cx)
                    .into_any_element()
            }
            _ => uniform_list(
                "build-tasks",
                self.rows.len(),
                cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx)),
            )
            .track_scroll(&self.scroll)
            .size_full()
            .into_any_element(),
        };
        v_flex()
            .id("build-tools")
            .track_focus(&self.focus_handle)
            .aria_label(tr("tool-build"))
            .size_full()
            .on_key_down(cx.listener(Self::on_key_down))
            .child(toolbar)
            .child(div().flex_1().min_h_0().child(body))
    }
}
