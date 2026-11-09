//! The Debug tool window: debugger toolbar, threads and frames, variables,
//! breakpoints and the debug console. Stage 10 wires it to DAP; until then
//! it shows the mock's paused session.

use std::collections::HashSet;

use ferro_services::{
    Breakpoint, DebugEvent, DebugState, DebugThread, Services, TextPos, Variable,
};
use ferro_ui::{FerroIcon, components, tr, tr_args};
use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, ClickEvent, Context, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    component::{
        ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
        h_flex,
        input::{Input, InputEvent, InputState},
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};

use super::common::{self, NodePath, ROW_HEIGHT};
use crate::requests::OpenLocation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugTab {
    Frames,
    Console,
    Breakpoints,
}

/// One line of the debug console.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleLine {
    pub text: SharedString,
    pub error: bool,
}

pub struct DebugView {
    services: Services,
    tab: DebugTab,
    state: DebugState,
    threads: Vec<DebugThread>,
    /// Selected thread id and frame index.
    frame: Option<(u64, usize)>,
    show_library_frames: bool,
    variables: Result<Vec<Variable>, SharedString>,
    expanded: HashSet<NodePath>,
    breakpoints: Vec<Breakpoint>,
    console: Vec<ConsoleLine>,
    input: Entity<InputState>,
    /// Feedback for toolbar buttons the mock cannot perform.
    message: Option<SharedString>,
    focus_handle: FocusHandle,
    console_scroll: ScrollHandle,
}

pub fn build(services: Services, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| DebugView::new(services, window, cx)).into()
}

fn kids(v: &Variable) -> &[Variable] {
    &v.children
}

impl DebugView {
    pub fn new(services: Services, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder(tr("panels-debug-evaluate")));
        cx.subscribe_in(
            &input,
            window,
            |this, input, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let expression = input.read(cx).value().to_string();
                    input.update(cx, |input, cx| input.set_value("", window, cx));
                    this.evaluate(&expression, cx);
                }
            },
        )
        .detach();

        let events = services.debug.subscribe();
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                let alive = this.update(cx, |this, cx| {
                    match event {
                        DebugEvent::BreakpointsChanged => {
                            this.breakpoints = this.services.debug.breakpoints();
                        }
                        DebugEvent::StateChanged(_) => this.reload(),
                    }
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
            tab: DebugTab::Frames,
            state: DebugState::NotStarted,
            threads: Vec::new(),
            frame: None,
            show_library_frames: false,
            variables: Ok(Vec::new()),
            expanded: HashSet::new(),
            breakpoints: Vec::new(),
            console: Vec::new(),
            input,
            message: None,
            focus_handle: cx.focus_handle(),
            console_scroll: ScrollHandle::new(),
        };
        this.reload();
        this
    }

    /// Re-read state, threads, breakpoints and console from the service.
    pub fn reload(&mut self) {
        let debug = &self.services.debug;
        self.state = debug.state();
        self.threads = debug.threads();
        self.breakpoints = debug.breakpoints();
        self.console = debug
            .console()
            .into_iter()
            .map(|text| ConsoleLine {
                text: text.into(),
                error: false,
            })
            .collect();
        let first = self
            .threads
            .iter()
            .find(|t| t.paused && !t.frames.is_empty())
            .map(|t| (t.id, 0));
        if self.frame.is_none_or(|(id, ix)| {
            !self
                .threads
                .iter()
                .any(|t| t.id == id && ix < t.frames.len())
        }) {
            self.frame = first;
        }
        self.load_variables();
    }

    fn load_variables(&mut self) {
        self.variables = match self.frame {
            Some((thread, frame)) => self
                .services
                .debug
                .variables(thread, frame)
                .map_err(|e| e.to_string().into()),
            None => Ok(Vec::new()),
        };
    }

    pub fn set_tab(&mut self, tab: DebugTab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    pub fn select_frame(
        &mut self,
        thread: u64,
        frame: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.frame = Some((thread, frame));
        self.expanded.clear();
        self.load_variables();
        let location = self
            .threads
            .iter()
            .find(|t| t.id == thread)
            .and_then(|t| t.frames.get(frame))
            .and_then(|f| f.path.clone().map(|p| (p, f.line)));
        if let Some((path, line)) = location {
            let open = OpenLocation::at(path, TextPos::new(line, 0)).focus(false);
            window.dispatch_action(Box::new(open), cx);
        }
        cx.notify();
    }

    /// Visible variable rows (paths into the variables tree).
    pub fn variable_rows(&self) -> Vec<NodePath> {
        match &self.variables {
            Ok(vars) => common::flatten(vars, &kids, &|p| !self.expanded.contains(p), &|_| true),
            Err(_) => Vec::new(),
        }
    }

    pub fn toggle_variable(&mut self, path: NodePath, cx: &mut Context<Self>) {
        if !self.expanded.remove(&path) {
            self.expanded.insert(path);
        }
        cx.notify();
    }

    /// The variables of the selected frame.
    pub fn variables(&self) -> &[Variable] {
        self.variables.as_deref().unwrap_or(&[])
    }

    pub fn breakpoint_count(&self) -> usize {
        self.breakpoints.len()
    }

    pub fn console_lines(&self) -> &[ConsoleLine] {
        &self.console
    }

    pub fn evaluate(&mut self, expression: &str, cx: &mut Context<Self>) {
        let expression = expression.trim();
        if expression.is_empty() {
            return;
        }
        self.console.push(ConsoleLine {
            text: format!("> {expression}").into(),
            error: false,
        });
        match self.services.debug.evaluate(expression) {
            Ok(value) => self.console.push(ConsoleLine {
                text: value.into(),
                error: false,
            }),
            Err(err) => self.console.push(ConsoleLine {
                text: err.to_string().into(),
                error: true,
            }),
        }
        self.console_scroll.scroll_to_bottom();
        cx.notify();
    }

    fn step(&mut self, label: SharedString, cx: &mut Context<Self>) {
        self.message = Some(tr_args(
            "panels-debug-mock-step",
            &[("step", label.to_string().into())],
        ));
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.tab != DebugTab::Frames || common::has_command_modifier(&event.keystroke) {
            return;
        }
        // Up/Down move between frames of the selected thread.
        let Some((thread, frame)) = self.frame else {
            return;
        };
        let len = self
            .threads
            .iter()
            .find(|t| t.id == thread)
            .map(|t| t.frames.len())
            .unwrap_or(0);
        if let Some(next) = common::nav(&event.keystroke.key, Some(frame), len) {
            self.frame = Some((thread, next));
            self.expanded.clear();
            self.load_variables();
            cx.notify();
            cx.stop_propagation();
        }
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let paused = self.state == DebugState::Paused;
        let active = self.state != DebugState::NotStarted;
        let step = |id: &'static str,
                    icon: FerroIcon,
                    key: &'static str,
                    enabled: bool,
                    cx: &mut Context<Self>| {
            common::tool_button(id, icon, tr(key))
                .disabled(!enabled)
                .on_click(cx.listener(move |this, _, _, cx| this.step(tr(key), cx)))
        };
        let tab =
            |id: &'static str, t: DebugTab, key: &str, this: &Self, cx: &mut Context<Self>| {
                Button::new(id)
                    .ghost()
                    .xsmall()
                    .label(tr(key))
                    .selected(this.tab == t)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_tab(t, cx)))
            };
        common::toolbar("debug-toolbar", cx)
            .child(step(
                "debug-resume",
                FerroIcon::Run,
                "panels-debug-resume",
                paused,
                cx,
            ))
            .child(step(
                "debug-pause",
                FerroIcon::Pause,
                "panels-debug-pause",
                active && !paused,
                cx,
            ))
            .child(step(
                "debug-step-over",
                FerroIcon::StepOver,
                "panels-debug-step-over",
                paused,
                cx,
            ))
            .child(step(
                "debug-step-into",
                FerroIcon::StepInto,
                "panels-debug-step-into",
                paused,
                cx,
            ))
            .child(step(
                "debug-step-out",
                FerroIcon::StepOut,
                "panels-debug-step-out",
                paused,
                cx,
            ))
            .child(step(
                "debug-stop",
                FerroIcon::Stop,
                "panels-debug-stop",
                active,
                cx,
            ))
            .child(div().w(px(8.)))
            .child(tab(
                "debug-tab-frames",
                DebugTab::Frames,
                "panels-debug-frames",
                self,
                cx,
            ))
            .child(tab(
                "debug-tab-console",
                DebugTab::Console,
                "panels-debug-console",
                self,
                cx,
            ))
            .child(tab(
                "debug-tab-breakpoints",
                DebugTab::Breakpoints,
                "panels-debug-breakpoints",
                self,
                cx,
            ))
            .child(div().flex_1())
            .when_some(self.message.clone(), |el, m| el.child(m))
    }

    fn row(
        id: impl Into<gpui_kit::ElementId>,
        depth: usize,
        selected: bool,
        cx: &App,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let theme = cx.theme();
        h_flex()
            .id(id)
            .h(px(ROW_HEIGHT))
            .pl(px(6. + depth as f32 * 14.))
            .pr_2()
            .gap_1()
            .w_full()
            .text_sm()
            .whitespace_nowrap()
            .overflow_hidden()
            .cursor_pointer()
            .when(selected, |el| el.bg(theme.list_active))
            .hover(|el| el.bg(theme.list_hover))
    }

    fn render_frames(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let mut rows: Vec<AnyElement> = Vec::new();
        for thread in &self.threads {
            let state = if thread.paused {
                tr("panels-debug-paused")
            } else {
                tr("panels-debug-running")
            };
            rows.push(
                Self::row(
                    SharedString::from(format!("thread-{}", thread.id)),
                    0,
                    false,
                    cx,
                )
                .child(
                    Icon::from(if thread.paused {
                        FerroIcon::Pause
                    } else {
                        FerroIcon::Run
                    })
                    .small()
                    .text_color(muted),
                )
                .child(div().child(thread.name.clone()))
                .child(div().text_xs().text_color(muted).child(state))
                .into_any_element(),
            );
            let mut hidden = 0;
            for (ix, frame) in thread.frames.iter().enumerate() {
                if frame.is_library && !self.show_library_frames {
                    hidden += 1;
                    continue;
                }
                let thread_id = thread.id;
                let selected = self.frame == Some((thread_id, ix));
                let location = frame
                    .path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|n| format!("{}:{}", n.to_string_lossy(), frame.line + 1))
                    .unwrap_or_default();
                rows.push(
                    Self::row(
                        SharedString::from(format!("frame-{thread_id}-{ix}")),
                        1,
                        selected,
                        cx,
                    )
                    .aria_label(SharedString::from(frame.name.clone()))
                    .when(frame.is_library, |el| el.text_color(muted))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.focus_handle.focus(window, cx);
                        this.select_frame(thread_id, ix, window, cx)
                    }))
                    .child(Icon::from(FerroIcon::StackFrame).small().text_color(muted))
                    .child(div().flex_1().truncate().child(frame.name.clone()))
                    .child(div().text_xs().text_color(muted).child(location))
                    .into_any_element(),
                );
            }
            if hidden > 0 {
                rows.push(
                    Self::row(
                        SharedString::from(format!("library-{}", thread.id)),
                        1,
                        false,
                        cx,
                    )
                    .text_color(muted)
                    .text_xs()
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.show_library_frames = true;
                        cx.notify();
                    }))
                    .child(tr_args(
                        "panels-debug-library-frames",
                        &[("count", hidden.into())],
                    ))
                    .into_any_element(),
                );
            }
        }
        let frames = v_flex()
            .id("debug-frames")
            .w(gpui_kit::relative(0.45))
            .h_full()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(cx.theme().border)
            .children(rows);

        let variables = match &self.variables {
            Err(err) => components::empty_state(
                FerroIcon::Error,
                tr("panels-load-failed"),
                Some(err.clone()),
                cx,
            )
            .into_any_element(),
            Ok(vars) if vars.is_empty() => components::empty_state(
                FerroIcon::Variable,
                tr("panels-debug-no-variables"),
                None,
                cx,
            )
            .into_any_element(),
            Ok(vars) => {
                let rows = self.variable_rows();
                v_flex()
                    .id("debug-variables")
                    .size_full()
                    .overflow_y_scroll()
                    .children(rows.into_iter().enumerate().filter_map(|(ix, path)| {
                        let var = common::node_at(vars, &path, &kids)?;
                        let has_children = !var.children.is_empty();
                        let open = self.expanded.contains(&path);
                        let depth = path.len() - 1;
                        let path_for_click = path.clone();
                        Some(
                            Self::row(("variable", ix), depth, false, cx)
                                .aria_label(SharedString::from(var.name.clone()))
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    if has_children {
                                        this.toggle_variable(path_for_click.clone(), cx);
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
                                .child(Icon::from(FerroIcon::Variable).small().text_color(muted))
                                .child(div().child(var.name.clone()))
                                .child(div().text_color(muted).child("="))
                                .child(div().flex_1().truncate().child(var.value.clone()))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(var.type_name.clone()),
                                )
                                .into_any_element(),
                        )
                    }))
                    .into_any_element()
            }
        };
        h_flex()
            .size_full()
            .child(frames)
            .child(div().flex_1().h_full().child(variables))
            .into_any_element()
    }

    fn render_console(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (danger, mono, size, border) = (
            theme.danger,
            theme.mono_font_family.clone(),
            theme.mono_font_size,
            theme.border,
        );
        v_flex()
            .size_full()
            .child(
                v_flex()
                    .id("debug-console")
                    .track_scroll(&self.console_scroll)
                    .flex_1()
                    .min_h_0()
                    .p_2()
                    .overflow_y_scroll()
                    .font_family(mono)
                    .text_size(size)
                    .children(self.console.iter().map(|line| {
                        div()
                            .when(line.error, |el| el.text_color(danger))
                            .child(line.text.clone())
                    })),
            )
            .child(
                div().p_1().border_t_1().border_color(border).child(
                    Input::new(&self.input)
                        .small()
                        .aria_label(tr("panels-debug-evaluate")),
                ),
            )
            .into_any_element()
    }

    fn render_breakpoints(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.breakpoints.is_empty() {
            return components::empty_state(
                FerroIcon::Breakpoint,
                tr("panels-debug-no-breakpoints"),
                None,
                cx,
            )
            .into_any_element();
        }
        let theme = cx.theme();
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        let root = self.services.project.roots().first().cloned();
        v_flex()
            .id("debug-breakpoints")
            .size_full()
            .overflow_y_scroll()
            .children(self.breakpoints.iter().enumerate().map(|(ix, bp)| {
                let label = format!(
                    "{}:{}",
                    common::relative(&bp.path, root.as_deref()),
                    bp.line + 1
                );
                let (path, line, enabled) = (bp.path.clone(), bp.line, bp.enabled);
                let open_path = bp.path.clone();
                let remove_path = bp.path.clone();
                Self::row(("breakpoint", ix), 0, false, cx)
                    .aria_label(SharedString::from(label.clone()))
                    .on_click(cx.listener(move |_, event: &ClickEvent, window, cx| {
                        if event.click_count() >= 2 {
                            window.dispatch_action(
                                Box::new(OpenLocation::at(
                                    open_path.clone(),
                                    TextPos::new(line, 0),
                                )),
                                cx,
                            );
                        }
                    }))
                    .child(
                        Checkbox::new(("breakpoint-enabled", ix))
                            .checked(enabled)
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                this.services
                                    .debug
                                    .set_breakpoint_enabled(&path, line, *checked);
                                this.breakpoints = this.services.debug.breakpoints();
                                cx.notify();
                            })),
                    )
                    .child(
                        Icon::from(FerroIcon::Breakpoint)
                            .small()
                            .text_color(if enabled { danger } else { muted }),
                    )
                    .child(div().flex_1().truncate().child(label))
                    .when_some(bp.condition.clone(), |el, c| {
                        el.child(div().text_xs().text_color(muted).child(c))
                    })
                    .child(
                        common::tool_button(
                            ("breakpoint-remove", ix),
                            FerroIcon::Close,
                            tr("panels-debug-remove-breakpoint"),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.services.debug.toggle_breakpoint(&remove_path, line);
                            this.breakpoints = this.services.debug.breakpoints();
                            cx.notify();
                        })),
                    )
            }))
            .into_any_element()
    }
}

impl Focusable for DebugView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for DebugView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if self.state == DebugState::NotStarted && self.tab != DebugTab::Breakpoints {
            components::empty_state(FerroIcon::Debug, tr("debug-empty"), None, cx)
                .into_any_element()
        } else {
            match self.tab {
                DebugTab::Frames => self.render_frames(cx),
                DebugTab::Console => self.render_console(cx),
                DebugTab::Breakpoints => self.render_breakpoints(cx),
            }
        };
        v_flex()
            .id("debug")
            .track_focus(&self.focus_handle)
            .aria_label(tr("tool-debug"))
            .size_full()
            .on_key_down(cx.listener(Self::on_key_down))
            .child(self.render_toolbar(cx))
            .child(div().flex_1().min_h_0().child(body))
    }
}
