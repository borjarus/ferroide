//! The Terminal tool window: sessions as tabs, each a PTY behind
//! `TerminalService` (Stage 8, `ferro_terminal`) drawn from snapshots of
//! its visible screen.
//!
//! The view owns no emulator state. It measures its size in cells and
//! reports it (`resize`), turns keys, pasted text and wheel scrolling into
//! service calls, and redraws when the service says the screen changed.
//! Selection is kept in absolute lines, so it stays on the same text while
//! output scrolls; copying takes the selected part that is on screen.
//! Ctrl+click opens a file path under the pointer (compiler errors, stack
//! traces) in the editor. With shell integration (OSC 133) every command
//! gets a marker in the left margin, green or red by its exit code, and
//! Ctrl+Shift+↑/↓ jumps between commands.

use std::{cell::Cell, collections::HashMap, rc::Rc};

use ferro_services::{
    AnsiColor, CursorShape, NewTerminal, Services, ShellInfo, StyledSpan, TerminalEvent,
    TerminalSession, TerminalSnapshot, line_text,
};
use ferro_ui::{FerroIcon, components, tr};
use gpui_kit::{
    AnyView, App, AppContext as _, Bounds, ClipboardItem, Context, FocusHandle, Focusable,
    FontWeight, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render,
    ScrollEvent, SharedString, StatefulInteractiveElement as _, Styled as _, Window, canvas,
    component::{
        ActiveTheme as _, Icon, Selectable as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        menu::{DropdownMenu as _, PopupMenuItem},
        v_flex,
    },
    div, font,
    prelude::FluentBuilder as _,
    px, rgb,
};

use super::{common, terminal_input};
use crate::requests::OpenLocation;

/// Horizontal padding inside the terminal area and the command marker
/// margin on its left.
const PADDING: f32 = 6.0;
const MARKER_WIDTH: f32 = 4.0;

/// A position on the screen: absolute line and column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CellPos {
    pub line: i64,
    pub col: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSelection {
    pub anchor: CellPos,
    pub head: CellPos,
}

impl TerminalSelection {
    fn ordered(&self) -> (CellPos, CellPos) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}

pub struct TerminalView {
    services: Services,
    sessions: Vec<TerminalSession>,
    active: Option<u64>,
    snapshot: Option<TerminalSnapshot>,
    /// Exit codes of sessions whose shell ended.
    exited: HashMap<u64, Option<i32>>,
    /// The size last reported per session.
    sizes: HashMap<u64, (u16, u16)>,
    selection: Option<TerminalSelection>,
    selecting: bool,
    shells: Vec<ShellInfo>,
    focus_handle: FocusHandle,
    /// Width of a cell and height of a line, measured on first render.
    cell: Option<(Pixels, Pixels)>,
    /// The terminal area's bounds, written while painting.
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Whether the view had focus in the last render (cursor style).
    focused: bool,
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
        Indexed(ix) => {
            let (r, g, b) = AnsiColor::indexed_rgb(ix);
            rgb(u32::from_be_bytes([0, r, g, b])).into()
        }
        Rgb(r, g, b) => rgb(u32::from_be_bytes([0, r, g, b])).into(),
    }
}

/// Foreground and background of a span, with inverse video and dim text
/// applied. A background of `None` is the terminal's own.
pub fn span_colors(span: &StyledSpan, cx: &App) -> (Hsla, Option<Hsla>) {
    let fg = ansi_color(span.fg, cx);
    let bg = (span.bg != AnsiColor::Default).then(|| ansi_color(span.bg, cx));
    let (mut fg, bg) = if span.inverse {
        (bg.unwrap_or(cx.theme().background), Some(fg))
    } else {
        (fg, bg)
    };
    if span.dim {
        fg = fg.opacity(0.6);
    }
    (fg, bg)
}

/// The text of `line` between columns `start..end` (by characters, which
/// is what the emulator's cells are, wide characters aside).
fn slice_cols(text: &str, start: usize, end: usize) -> String {
    text.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

/// Byte offset of column `col` in `text`.
fn byte_of_col(text: &str, col: usize) -> usize {
    text.char_indices()
        .nth(col)
        .map(|(ix, _)| ix)
        .unwrap_or(text.len())
}

impl TerminalView {
    pub fn new(services: Services, cx: &mut Context<Self>) -> Self {
        let events = services.terminal.subscribe();
        cx.spawn(async move |this, cx| {
            while let Ok(first) = events.recv().await {
                // A burst of output becomes one redraw.
                let mut batch = vec![first];
                while let Ok(more) = events.try_recv() {
                    batch.push(more);
                }
                if this
                    .update(cx, |this, cx| this.on_events(batch, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let shells = services.terminal.shells();
        let mut this = Self {
            services,
            sessions: Vec::new(),
            active: None,
            snapshot: None,
            exited: HashMap::new(),
            sizes: HashMap::new(),
            selection: None,
            selecting: false,
            shells,
            focus_handle: cx.focus_handle(),
            cell: None,
            bounds: Rc::new(Cell::new(None)),
            focused: false,
        };
        this.sync();
        this
    }

    fn on_events(&mut self, events: Vec<TerminalEvent>, cx: &mut Context<Self>) {
        let mut refresh = false;
        for event in events {
            match event {
                TerminalEvent::Changed(id) | TerminalEvent::CommandFinished { id, .. } => {
                    refresh |= Some(id) == self.active;
                }
                TerminalEvent::TitleChanged { .. } => {
                    self.sync();
                    refresh = true;
                }
                TerminalEvent::Exited { id, code } => {
                    self.exited.insert(id, code);
                    refresh = true;
                }
                TerminalEvent::Bell(_) => {}
            }
        }
        if refresh {
            self.refresh_snapshot();
            cx.notify();
        }
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
        self.sizes
            .retain(|id, _| self.sessions.iter().any(|s| s.id == *id));
        if (had_sessions && added.is_some()) || !self.active_exists() {
            self.active = added
                .filter(|_| had_sessions)
                .or_else(|| self.sessions.first().map(|s| s.id));
            self.selection = None;
        }
        self.refresh_snapshot();
    }

    fn refresh_snapshot(&mut self) {
        self.snapshot = self
            .active
            .and_then(|id| self.services.terminal.snapshot(id));
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

    /// The visible screen of the active session.
    pub fn snapshot(&self) -> Option<&TerminalSnapshot> {
        self.snapshot.as_ref()
    }

    pub fn new_session(&mut self, cx: &mut Context<Self>) {
        let session = self.services.terminal.new_session();
        self.sync();
        self.active = Some(session.id);
        self.refresh_snapshot();
        cx.notify();
    }

    /// Open a session with a chosen shell (or other options).
    pub fn open(&mut self, options: NewTerminal, cx: &mut Context<Self>) -> anyhow::Result<()> {
        let session = self.services.terminal.open(options)?;
        self.sync();
        self.active = Some(session.id);
        self.refresh_snapshot();
        cx.notify();
        Ok(())
    }

    pub fn close_session(&mut self, id: u64, cx: &mut Context<Self>) {
        self.services.terminal.close_session(id);
        self.exited.remove(&id);
        self.sync();
        cx.notify();
    }

    pub fn activate(&mut self, id: u64, cx: &mut Context<Self>) {
        self.active = Some(id);
        self.selection = None;
        self.refresh_snapshot();
        cx.notify();
    }

    /// Send bytes to the active session and return to the bottom, as
    /// terminals do on input.
    pub fn send(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        let Some(id) = self.active else { return };
        if self.snapshot.as_ref().is_some_and(|s| s.display_offset > 0) {
            self.services.terminal.scroll(id, i32::MIN);
        }
        self.selection = None;
        self.services.terminal.write(id, bytes);
        self.refresh_snapshot();
        cx.notify();
    }

    pub fn paste(&mut self, text: &str, cx: &mut Context<Self>) {
        let modes = self.snapshot.as_ref().map(|s| s.modes).unwrap_or_default();
        self.send(&terminal_input::encode_paste(text, modes), cx);
    }

    pub fn scroll_lines(&mut self, lines: i32, cx: &mut Context<Self>) {
        let Some(id) = self.active else { return };
        self.services.terminal.scroll(id, lines);
        self.refresh_snapshot();
        cx.notify();
    }

    pub fn selection(&self) -> Option<TerminalSelection> {
        self.selection
    }

    pub fn set_selection(&mut self, selection: Option<TerminalSelection>, cx: &mut Context<Self>) {
        self.selection = selection;
        cx.notify();
    }

    /// The selected text that is on screen, lines joined with `\n`.
    pub fn selected_text(&self) -> Option<String> {
        let selection = self.selection?;
        let snapshot = self.snapshot.as_ref()?;
        let (start, end) = selection.ordered();
        let mut out = Vec::new();
        for line in start.line..=end.line {
            let row = line - snapshot.first_line;
            let Some(text) = usize::try_from(row)
                .ok()
                .and_then(|row| snapshot.lines.get(row))
                .map(|l| line_text(l))
            else {
                continue;
            };
            let from = if line == start.line {
                start.col as usize
            } else {
                0
            };
            let to = if line == end.line {
                end.col as usize + 1
            } else {
                usize::MAX
            };
            out.push(slice_cols(&text, from, to).trim_end().to_string());
        }
        (!out.is_empty()).then(|| out.join("\n"))
    }

    pub fn copy(&mut self, cx: &mut Context<Self>) -> bool {
        match self.selected_text() {
            Some(text) if !text.is_empty() => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                true
            }
            _ => false,
        }
    }

    /// Scroll to the previous (`-1`) or next (`1`) command's prompt.
    pub fn jump_to_command(&mut self, direction: i32, cx: &mut Context<Self>) {
        let (Some(id), Some(snapshot)) = (self.active, self.snapshot.as_ref()) else {
            return;
        };
        let top = snapshot.first_line;
        let target = if direction < 0 {
            snapshot
                .commands
                .iter()
                .rev()
                .map(|c| c.prompt_line)
                .find(|&l| l < top)
        } else {
            snapshot
                .commands
                .iter()
                .map(|c| c.prompt_line)
                .find(|&l| l > top)
        };
        match target {
            Some(line) => self.services.terminal.scroll_to_line(id, line),
            None if direction > 0 => self.services.terminal.scroll(id, i32::MIN),
            None => return,
        }
        self.refresh_snapshot();
        cx.notify();
    }

    /// The file location under `pos`, if its line shows one.
    pub fn link_at(&self, pos: CellPos) -> Option<(std::path::PathBuf, u32, u32)> {
        let snapshot = self.snapshot.as_ref()?;
        let row = usize::try_from(pos.line - snapshot.first_line).ok()?;
        let text = line_text(snapshot.lines.get(row)?);
        let byte = byte_of_col(&text, pos.col as usize);
        let roots = self.services.project.roots();
        ferro_services::links::find_links(&text)
            .into_iter()
            .find(|link| link.range.start <= byte && byte < link.range.end)
            .and_then(|link| {
                ferro_services::links::resolve_link(
                    &link,
                    roots.first().map(|r| r.as_path()),
                    &roots,
                )
            })
    }

    /// The cell under a window position.
    fn cell_at(&self, position: Point<Pixels>) -> Option<CellPos> {
        let (bounds, (cell_w, line_h), snapshot) =
            (self.bounds.get()?, self.cell?, self.snapshot.as_ref()?);
        let x = position.x - bounds.origin.x - px(PADDING + MARKER_WIDTH);
        let y = position.y - bounds.origin.y - px(PADDING);
        let col = (x / cell_w).floor().max(0.) as u16;
        let row = (y / line_h)
            .floor()
            .clamp(0., snapshot.rows.saturating_sub(1) as f32) as i64;
        Some(CellPos {
            line: snapshot.first_line + row,
            col: col.min(snapshot.cols.saturating_sub(1)),
        })
    }

    /// Report a new size in cells for the area `bounds`.
    fn on_bounds(&mut self, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        let (Some(id), Some((cell_w, line_h))) = (self.active, self.cell) else {
            return;
        };
        let width = bounds.size.width - px(2. * PADDING + MARKER_WIDTH);
        let height = bounds.size.height - px(2. * PADDING);
        let cols = (width / cell_w).floor().max(2.) as u16;
        let rows = (height / line_h).floor().max(1.) as u16;
        if self.sizes.get(&id) != Some(&(cols, rows)) {
            self.sizes.insert(id, (cols, rows));
            self.services.terminal.resize(id, cols, rows);
            self.refresh_snapshot();
            cx.notify();
        }
    }

    fn measure(&mut self, window: &mut Window, cx: &App) {
        if self.cell.is_some() {
            return;
        }
        let theme = cx.theme();
        let font_id = window
            .text_system()
            .resolve_font(&font(theme.mono_font_family.clone()));
        let size = theme.mono_font_size;
        if let Ok(advance) = window.text_system().advance(font_id, size, 'm') {
            self.cell = Some((advance.width, (size * 1.35).round()));
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let m = keystroke.modifiers;
        let key = keystroke.key.as_str();
        let secondary = m.control || m.platform;
        let handled = match key {
            // Copy and paste.
            "c" if secondary && m.shift => {
                self.copy(cx);
                true
            }
            "c" if m.platform || (m.control && self.selection.is_some()) => self.copy(cx),
            "v" if (secondary && m.shift) || m.platform || (cfg!(windows) && m.control) => {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.paste(&text, cx);
                }
                true
            }
            // History.
            "pageup" if m.shift => {
                let rows = self.snapshot.as_ref().map_or(10, |s| s.rows as i32);
                self.scroll_lines(rows.max(2) - 1, cx);
                true
            }
            "pagedown" if m.shift => {
                let rows = self.snapshot.as_ref().map_or(10, |s| s.rows as i32);
                self.scroll_lines(-(rows.max(2) - 1), cx);
                true
            }
            "up" if m.control && m.shift => {
                self.jump_to_command(-1, cx);
                true
            }
            "down" if m.control && m.shift => {
                self.jump_to_command(1, cx);
                true
            }
            _ => false,
        };
        if handled {
            cx.stop_propagation();
            return;
        }
        let modes = self.snapshot.as_ref().map(|s| s.modes).unwrap_or_default();
        if let Some(bytes) = terminal_input::encode_key(keystroke, modes) {
            self.send(&bytes, cx);
            cx.stop_propagation();
        }
        let _ = window;
    }

    fn on_scroll(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((_, line_h)) = self.cell else { return };
        let lines = (event.delta.pixel_delta(line_h).y / line_h).round() as i32;
        if lines == 0 {
            return;
        }
        let alt_screen = self.snapshot.as_ref().is_some_and(|s| s.modes.alt_screen);
        if alt_screen {
            // Full-screen programs scroll with the arrow keys.
            let key: &[u8] = if lines > 0 { b"\x1b[A" } else { b"\x1b[B" };
            let bytes = key.repeat(lines.unsigned_abs() as usize);
            if let Some(id) = self.active {
                self.services.terminal.write(id, &bytes);
            }
        } else {
            self.scroll_lines(lines * 3, cx);
        }
        cx.stop_propagation();
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window, cx);
        let Some(pos) = self.cell_at(event.position) else {
            return;
        };
        if event.modifiers.control || event.modifiers.platform {
            if let Some((path, line, col)) = self.link_at(pos) {
                window.dispatch_action(
                    Box::new(OpenLocation::at(
                        path,
                        ferro_services::TextPos::new(line, col),
                    )),
                    cx,
                );
            }
            return;
        }
        if event.click_count >= 2 {
            self.select_word(pos, cx);
            return;
        }
        self.selection = Some(TerminalSelection {
            anchor: pos,
            head: pos,
        });
        self.selecting = true;
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        if let (Some(pos), Some(selection)) =
            (self.cell_at(event.position), self.selection.as_mut())
            && selection.head != pos
        {
            selection.head = pos;
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.selecting = false;
        if self.selection.is_some_and(|s| s.anchor == s.head) {
            self.selection = None;
            cx.notify();
        }
    }

    fn select_word(&mut self, pos: CellPos, cx: &mut Context<Self>) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        let Some(line) = usize::try_from(pos.line - snapshot.first_line)
            .ok()
            .and_then(|row| snapshot.lines.get(row))
        else {
            return;
        };
        let chars: Vec<char> = line_text(line).chars().collect();
        let col = pos.col as usize;
        let is_word = |c: char| !c.is_whitespace() && !"\"'`()[]{}<>|;,".contains(c);
        if !chars.get(col).is_some_and(|&c| is_word(c)) {
            return;
        }
        let start = (0..=col)
            .rev()
            .take_while(|&i| is_word(chars[i]))
            .last()
            .unwrap_or(col);
        let end = (col..chars.len())
            .take_while(|&i| is_word(chars[i]))
            .last()
            .unwrap_or(col);
        self.selection = Some(TerminalSelection {
            anchor: CellPos {
                line: pos.line,
                col: start as u16,
            },
            head: CellPos {
                line: pos.line,
                col: end as u16,
            },
        });
        cx.notify();
    }

    fn render_line(
        &self,
        row: usize,
        line: &[StyledSpan],
        line_h: Pixels,
        cx: &App,
    ) -> impl IntoElement + use<> {
        h_flex()
            .id(("terminal-row", row))
            .h(line_h)
            .line_height(line_h)
            .whitespace_nowrap()
            .children(line.iter().map(|span| {
                let (fg, bg) = span_colors(span, cx);
                div()
                    .text_color(fg)
                    .when_some(bg, |el, bg| el.bg(bg))
                    .when(span.bold, |el| el.font_weight(FontWeight::BOLD))
                    .when(span.italic, |el| el.italic())
                    .when(span.underline, |el| el.underline())
                    .child(SharedString::from(span.text.clone()))
            }))
    }

    fn render_screen(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return components::empty_state(
                FerroIcon::Terminal,
                tr("panels-terminal-empty"),
                None,
                cx,
            )
            .into_any_element();
        };
        let theme = cx.theme();
        let (cell_w, line_h) = self.cell.unwrap_or((px(8.), px(18.)));
        let (fg, selection_bg, success, danger, muted) = (
            theme.foreground,
            theme.selection,
            theme.success,
            theme.danger,
            theme.muted_foreground,
        );
        let left = px(PADDING + MARKER_WIDTH);
        let top = px(PADDING);
        let row_top = |row: i64| top + line_h * row as f32;
        let mut overlays = Vec::new();

        // Selection.
        if let Some(selection) = self.selection {
            let (start, end) = selection.ordered();
            for line in start.line.max(snapshot.first_line)
                ..=end.line.min(snapshot.first_line + snapshot.rows as i64 - 1)
            {
                let row = line - snapshot.first_line;
                let from = if line == start.line { start.col } else { 0 };
                let to = if line == end.line {
                    end.col + 1
                } else {
                    snapshot.cols
                };
                overlays.push(
                    div()
                        .absolute()
                        .top(row_top(row))
                        .left(left + cell_w * from as f32)
                        .w(cell_w * to.saturating_sub(from) as f32)
                        .h(line_h)
                        .bg(selection_bg)
                        .into_any_element(),
                );
            }
        }

        // Command markers (shell integration).
        for command in &snapshot.commands {
            let row = command.prompt_line - snapshot.first_line;
            if row < 0 || row >= snapshot.rows as i64 {
                continue;
            }
            let color = match (command.finished, command.exit_code) {
                (true, Some(0)) => success,
                (true, Some(_)) => danger,
                _ => muted.opacity(0.5),
            };
            overlays.push(
                div()
                    .absolute()
                    .top(row_top(row) + px(2.))
                    .left(px(1.))
                    .w(px(MARKER_WIDTH - 1.))
                    .h(line_h - px(4.))
                    .rounded_sm()
                    .bg(color)
                    .into_any_element(),
            );
        }

        // Cursor.
        let focused = self.focused;
        if let Some(cursor) = snapshot.cursor
            && cursor.shape != CursorShape::Hidden
        {
            let x = left + cell_w * cursor.col as f32;
            let y = row_top(cursor.row as i64);
            let el = div().absolute().left(x);
            let el = match (cursor.shape, focused) {
                (CursorShape::Bar, _) => el.top(y).w(px(2.)).h(line_h).bg(fg),
                (CursorShape::Underline, _) => {
                    el.top(y + line_h - px(2.)).w(cell_w).h(px(2.)).bg(fg)
                }
                (_, true) => el.top(y).w(cell_w).h(line_h).bg(fg.opacity(0.45)),
                (_, false) => el.top(y).w(cell_w).h(line_h).border_1().border_color(fg),
            };
            overlays.push(el.into_any_element());
        }

        let bounds = self.bounds.clone();
        let entity = cx.entity().downgrade();
        div()
            .id("terminal-screen")
            .relative()
            .size_full()
            .overflow_hidden()
            .cursor_text()
            .font_family(theme.mono_font_family.clone())
            .text_size(theme.mono_font_size)
            .on_scroll(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .child(
                canvas(
                    move |b, _, cx| {
                        if bounds.get() != Some(b) {
                            bounds.set(Some(b));
                            let entity = entity.clone();
                            cx.defer(move |cx| {
                                let _ = entity.update(cx, |this, cx| this.on_bounds(b, cx));
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .children(overlays)
            .child(
                v_flex().absolute().top(top).left(left).children(
                    snapshot
                        .lines
                        .iter()
                        .enumerate()
                        .map(|(row, line)| self.render_line(row, line, line_h, cx)),
                ),
            )
            .when(snapshot.exited.is_some(), |el| {
                el.child(
                    div()
                        .absolute()
                        .bottom(px(4.))
                        .right(px(8.))
                        .px_2()
                        .rounded_sm()
                        .bg(theme.secondary)
                        .text_xs()
                        .text_color(muted)
                        .child(ferro_ui::tr_args(
                            "panels-terminal-exited",
                            &[("code", snapshot.exited.unwrap_or(-1).into())],
                        )),
                )
            })
            .into_any_element()
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.measure(window, cx);
        self.focused = self.focus_handle.is_focused(window);
        // Sessions may change from outside (New Terminal action).
        if self.services.terminal.sessions().len() != self.sessions.len() {
            self.sync();
        }
        let mut tabs = common::toolbar("terminal-tabs", cx);
        for session in &self.sessions {
            let id = session.id;
            let exited = self.exited.contains_key(&id);
            tabs = tabs.child(
                h_flex()
                    .child(
                        Button::new(SharedString::from(format!("terminal-tab-{id}")))
                            .ghost()
                            .xsmall()
                            .icon(Icon::from(FerroIcon::Terminal))
                            .label(SharedString::from(if exited {
                                format!("{} ✕", session.title)
                            } else {
                                session.title.clone()
                            }))
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
        let entity = cx.entity().downgrade();
        let shells = self.shells.clone();
        tabs = tabs
            .child(
                common::tool_button("terminal-new", FerroIcon::Add, tr("action-new-terminal"))
                    .on_click(cx.listener(|this, _, _, cx| this.new_session(cx))),
            )
            .when(shells.len() > 1, |el| {
                el.child(
                    Button::new("terminal-shells")
                        .ghost()
                        .xsmall()
                        .icon(Icon::from(FerroIcon::ChevronDown))
                        .tooltip(tr("panels-terminal-choose-shell"))
                        .dropdown_menu(move |mut menu, _, _| {
                            for shell in &shells {
                                let entity = entity.clone();
                                let id = shell.id.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(SharedString::from(shell.name.clone()))
                                        .on_click(move |_, window, cx| {
                                            let id = id.clone();
                                            let _ = entity.update(cx, |this, cx| {
                                                let opened = this.open(
                                                    NewTerminal {
                                                        shell: Some(id),
                                                        ..Default::default()
                                                    },
                                                    cx,
                                                );
                                                if let Err(err) = opened {
                                                    tracing::warn!(
                                                        "cannot open a terminal: {err:#}"
                                                    );
                                                }
                                                this.focus_handle.focus(window, cx);
                                            });
                                        }),
                                );
                            }
                            menu
                        }),
                )
            })
            .child(div().flex_1())
            .child(
                common::tool_button(
                    "terminal-prev-command",
                    FerroIcon::ArrowUp,
                    tr("panels-terminal-prev-command"),
                )
                .on_click(cx.listener(|this, _, _, cx| this.jump_to_command(-1, cx))),
            )
            .child(
                common::tool_button(
                    "terminal-next-command",
                    FerroIcon::ArrowDown,
                    tr("panels-terminal-next-command"),
                )
                .on_click(cx.listener(|this, _, _, cx| this.jump_to_command(1, cx))),
            );

        v_flex()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .aria_label(tr("tool-terminal"))
            .size_full()
            .bg(cx.theme().background)
            .on_key_down(cx.listener(Self::on_key_down))
            .child(tabs)
            .child(div().flex_1().min_h_0().child(self.render_screen(cx)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_slice_by_characters() {
        assert_eq!(slice_cols("zażółć gęślą", 2, 6), "żółć");
        assert_eq!(byte_of_col("aż b", 2), 3);
        assert_eq!(byte_of_col("ab", 9), 2);
    }

    #[test]
    fn selections_are_ordered() {
        let a = CellPos { line: 5, col: 3 };
        let b = CellPos { line: 2, col: 9 };
        let s = TerminalSelection { anchor: a, head: b };
        assert_eq!(s.ordered(), (b, a));
    }
}
