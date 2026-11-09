//! The Terminal tool window: sessions as tabs, rendered statically from the
//! terminal service with ANSI colors mapped to the theme. Input and a PTY
//! arrive in Stage 8.

use ferro_services::{AnsiColor, Services, StyledSpan, TerminalSession};
use ferro_ui::{FerroIcon, components, tr};
use gpui_kit::{
    AnyView, App, AppContext as _, Context, FocusHandle, Focusable, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window,
    component::{
        ActiveTheme as _, Icon, Selectable as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex, v_flex,
    },
    div,
    prelude::FluentBuilder as _,
};

use super::common;

pub struct TerminalView {
    services: Services,
    sessions: Vec<TerminalSession>,
    active: Option<u64>,
    focus_handle: FocusHandle,
    scroll: ScrollHandle,
}

pub fn build(services: Services, _window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| TerminalView::new(services, cx)).into()
}

/// The theme color for an ANSI color.
pub fn ansi_color(color: AnsiColor, cx: &App) -> Hsla {
    let t = cx.theme();
    use AnsiColor::*;
    match color {
        Default => t.foreground,
        Black => t.muted_foreground,
        Red => t.red,
        Green => t.green,
        Yellow => t.yellow,
        Blue => t.blue,
        Magenta => t.magenta,
        Cyan => t.cyan,
        White => t.foreground,
        BrightBlack => t.muted_foreground,
        BrightRed => t.red_light,
        BrightGreen => t.green_light,
        BrightYellow => t.yellow_light,
        BrightBlue => t.blue_light,
        BrightMagenta => t.magenta_light,
        BrightCyan => t.cyan_light,
        BrightWhite => t.foreground,
    }
}

impl TerminalView {
    pub fn new(services: Services, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            services,
            sessions: Vec::new(),
            active: None,
            focus_handle: cx.focus_handle(),
            scroll: ScrollHandle::new(),
        };
        this.sync();
        this
    }

    /// Re-read the sessions; a new one (e.g. from New Terminal) becomes
    /// active.
    fn sync(&mut self) {
        let sessions = self.services.terminal.sessions();
        let added = sessions
            .iter()
            .find(|s| !self.sessions.iter().any(|old| old.id == s.id))
            .map(|s| s.id);
        let had_sessions = !self.sessions.is_empty();
        self.sessions = sessions;
        if (had_sessions && added.is_some()) || !self.active_exists() {
            self.active = added
                .filter(|_| had_sessions)
                .or_else(|| self.sessions.first().map(|s| s.id));
            self.scroll.scroll_to_bottom();
        }
    }

    fn active_exists(&self) -> bool {
        self.active
            .is_some_and(|id| self.sessions.iter().any(|s| s.id == id))
    }

    pub fn session_titles(&self) -> Vec<String> {
        self.sessions.iter().map(|s| s.title.clone()).collect()
    }

    pub fn active_session(&self) -> Option<&TerminalSession> {
        self.sessions.iter().find(|s| Some(s.id) == self.active)
    }

    pub fn new_session(&mut self, cx: &mut Context<Self>) {
        let session = self.services.terminal.new_session();
        self.sync();
        self.active = Some(session.id);
        cx.notify();
    }

    pub fn close_session(&mut self, id: u64, cx: &mut Context<Self>) {
        self.services.terminal.close_session(id);
        self.sync();
        cx.notify();
    }

    pub fn activate(&mut self, id: u64, cx: &mut Context<Self>) {
        self.active = Some(id);
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn span(span: &StyledSpan, cx: &App) -> impl IntoElement {
        div()
            .text_color(ansi_color(span.fg, cx))
            .when(span.bold, |el| el.font_weight(FontWeight::BOLD))
            .child(SharedString::from(span.text.clone()))
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Sessions may change from outside (New Terminal action).
        self.sync();
        let theme = cx.theme();
        let (muted, mono, mono_size, border) = (
            theme.muted_foreground,
            theme.mono_font_family.clone(),
            theme.mono_font_size,
            theme.border,
        );
        let mut tabs = common::toolbar("terminal-tabs", cx);
        for session in &self.sessions {
            let id = session.id;
            tabs = tabs.child(
                h_flex()
                    .child(
                        Button::new(SharedString::from(format!("terminal-tab-{id}")))
                            .ghost()
                            .xsmall()
                            .icon(Icon::from(FerroIcon::Terminal))
                            .label(SharedString::from(session.title.clone()))
                            .selected(Some(id) == self.active)
                            .on_click(cx.listener(move |this, _, _, cx| this.activate(id, cx))),
                    )
                    .child(
                        common::tool_button(
                            SharedString::from(format!("terminal-close-{id}")),
                            FerroIcon::Close,
                            tr("panels-terminal-close"),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.close_session(id, cx))),
                    ),
            );
        }
        tabs = tabs.child(
            common::tool_button("terminal-new", FerroIcon::Add, tr("action-new-terminal"))
                .on_click(cx.listener(|this, _, _, cx| this.new_session(cx))),
        );

        let body = match self.active_session() {
            None => {
                components::empty_state(FerroIcon::Terminal, tr("panels-terminal-empty"), None, cx)
                    .into_any_element()
            }
            Some(session) => v_flex()
                .id("terminal-output")
                .track_scroll(&self.scroll)
                .overflow_y_scroll()
                .size_full()
                .p_2()
                .font_family(mono.clone())
                .text_size(mono_size)
                .children(session.lines.iter().map(|line| {
                    h_flex()
                        .min_h(mono_size * 1.4)
                        .whitespace_nowrap()
                        .children(line.iter().map(|span| Self::span(span, cx)))
                }))
                .into_any_element(),
        };

        v_flex()
            .id("terminal")
            .track_focus(&self.focus_handle)
            .aria_label(tr("tool-terminal"))
            .size_full()
            .child(tabs)
            .child(div().flex_1().min_h_0().child(body))
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .border_t_1()
                    .border_color(border)
                    .text_xs()
                    .text_color(muted)
                    .child(Icon::from(FerroIcon::Info).xsmall())
                    .child(tr("panels-terminal-readonly")),
            )
    }
}
