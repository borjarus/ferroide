//! The terminal emulator without a PTY: bytes in, snapshots out.
//!
//! [`TerminalCore`] wraps `alacritty_terminal`'s [`Term`] (grid, scrollback,
//! modes) and its escape-sequence parser, adds shell integration (OSC 133
//! marks found by [`OscScanner`]) and turns the visible screen into the
//! contract's [`TerminalSnapshot`]. The PTY session feeds it from its reader
//! thread; tests feed it directly.
//!
//! # Absolute lines
//!
//! The contract numbers lines from the oldest line kept in the scrollback,
//! so a line's number stays put while output scrolls and only drops when
//! the oldest lines fall off. Internally every line gets a *stable* number,
//! `dropped + history + screen_line`, where `dropped` counts lines that left
//! the scrollback; marks are stored with it and converted back on
//! [`TerminalCore::snapshot`]. That only works if every dropped line is
//! counted, and `alacritty_terminal` drops the oldest lines silently once the
//! history is full. So the [`Term`] gets [`SLACK`] lines more history than
//! the user asked for, the input is fed in pieces that cannot scroll more
//! than half of that, and after each piece the core trims the history back
//! to the limit itself, counting what it removed. A cleared history
//! (`ESC [3J`, `ESC c`) counts as dropped too. Reflowing the history on a
//! width change can still move marks by a few lines.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Instant,
};

use alacritty_terminal::{
    event::{Event, EventListener, WindowSize},
    grid::{Dimensions, Row, Scroll},
    index::{Column, Line},
    term::{
        Config, Term, TermMode,
        cell::{Cell, Flags},
    },
    vte::ansi::{self, Color, NamedColor, Processor, Rgb},
};
use ferro_services::{
    AnsiColor, CommandBlock, CursorShape, StyledSpan, TerminalCursor, TerminalLine, TerminalModes,
    TerminalSnapshot,
};

use crate::osc::{Mark, OscScanner};

/// Extra history the [`Term`] may hold beyond the scrollback limit between
/// two trims (see the module docs).
pub const SLACK: usize = 256;

/// Nominal cell size in pixels, for programs that ask for the text area
/// size in pixels (the PTY's `TIOCGWINSZ`, `CSI 14 t`).
pub const CELL_WIDTH: u16 = 8;
pub const CELL_HEIGHT: u16 = 16;

/// Commands kept at most, whatever the scrollback holds.
const MAX_COMMANDS: usize = 10_000;

/// Something the session must pass on after [`TerminalCore::advance`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreEvent {
    /// The program set the title (`OSC 0/2`), or reset it (`None`).
    Title(Option<String>),
    Bell,
    /// A command finished (`OSC 133;D`), with its exit code if reported.
    CommandFinished(Option<i32>),
}

/// What feeding bytes produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoreOutput {
    /// Answers to the program's queries (device attributes, cursor
    /// position, …) that must be written back to the PTY.
    pub replies: Vec<u8>,
    pub events: Vec<CoreEvent>,
}

/// Collects the [`Term`]'s events until the core handles them.
#[derive(Clone, Default)]
struct Listener(Arc<Mutex<Vec<Event>>>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }
}

#[derive(Debug, Clone, Copy)]
struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// A command block with stable line numbers (see the module docs).
#[derive(Debug, Clone, Copy)]
struct Block {
    prompt: u64,
    output: Option<u64>,
    exit_code: Option<i32>,
    finished: bool,
}

/// A terminal emulator fed with bytes: screen, scrollback, modes, title and
/// shell-integration command blocks.
pub struct TerminalCore {
    term: Term<Listener>,
    listener: Listener,
    parser: Processor,
    scanner: OscScanner,
    scrollback: usize,
    /// Lines that left the scrollback so far.
    dropped: u64,
    /// History length of the main screen when last seen.
    main_history: usize,
    /// The main screen was resized while hidden: take its history length as
    /// it is next time instead of reading a change as a clear.
    resync_history: bool,
    commands: VecDeque<Block>,
    title: Option<String>,
    exited: Option<i32>,
    prompt_ready: bool,
}

impl TerminalCore {
    /// A blank screen of `cols` × `rows` keeping `scrollback` lines of
    /// history.
    pub fn new(cols: u16, rows: u16, scrollback: usize) -> Self {
        let listener = Listener::default();
        let config = Config {
            scrolling_history: scrollback + SLACK,
            ..Config::default()
        };
        let size = Size {
            cols: cols.max(1) as usize,
            rows: rows.max(1) as usize,
        };
        Self {
            term: Term::new(config, &size, listener.clone()),
            listener,
            parser: Processor::new(),
            scanner: OscScanner::new(),
            scrollback,
            dropped: 0,
            main_history: 0,
            resync_history: false,
            commands: VecDeque::new(),
            title: None,
            exited: None,
            prompt_ready: false,
        }
    }

    /// Feed output of the program.
    pub fn advance(&mut self, bytes: &[u8]) -> CoreOutput {
        let mut out = CoreOutput::default();
        let mut start = 0;
        for (end, mark) in self.scanner.scan(bytes) {
            self.feed(&bytes[start..end]);
            start = end;
            self.drain_events(&mut out);
            self.apply_mark(mark, &mut out);
        }
        self.feed(&bytes[start..]);
        self.drain_events(&mut out);
        out
    }

    /// When a synchronized update (`CSI ? 2026 h`) must be ended because
    /// the program did not end it; call [`Self::flush_sync`] then.
    pub fn sync_deadline(&self) -> Option<Instant> {
        self.parser.sync_timeout().sync_timeout()
    }

    /// End a pending synchronized update, showing what it buffered.
    pub fn flush_sync(&mut self) -> CoreOutput {
        let mut out = CoreOutput::default();
        self.parser.stop_sync(&mut self.term);
        self.observe_history();
        self.drain_events(&mut out);
        out
    }

    pub fn cols(&self) -> u16 {
        self.term.grid().columns() as u16
    }

    pub fn rows(&self) -> u16 {
        self.term.grid().screen_lines() as u16
    }

    /// The size for the PTY.
    pub fn window_size(&self) -> WindowSize {
        WindowSize {
            num_lines: self.rows(),
            num_cols: self.cols(),
            cell_width: CELL_WIDTH,
            cell_height: CELL_HEIGHT,
        }
    }

    /// Resize the screen (the history reflows to the new width).
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let size = Size {
            cols: cols.max(1) as usize,
            rows: rows.max(1) as usize,
        };
        self.term.resize(size);
        // Lines moved between the screen and the history keep their
        // absolute numbers, so this is not a clear.
        self.resync_history = true;
        self.observe_history();
    }

    /// Scroll the view: positive `lines` go up into the history,
    /// `i32::MIN` returns to the bottom.
    pub fn scroll(&mut self, lines: i32) {
        let scroll = if lines == i32::MIN {
            Scroll::Bottom
        } else {
            Scroll::Delta(lines)
        };
        self.term.scroll_display(scroll);
    }

    /// Scroll so that absolute line `line` is at the top of the screen
    /// (as far as the history allows).
    pub fn scroll_to_line(&mut self, line: i64) {
        let grid = self.term.grid();
        let history = grid.history_size() as i64;
        let target = (history - line).clamp(0, history);
        let delta = target - grid.display_offset() as i64;
        if delta != 0 {
            self.term.scroll_display(Scroll::Delta(
                delta.clamp(i32::MIN as i64 + 1, i32::MAX as i64) as i32,
            ));
        }
    }

    /// How many lines the view is scrolled up into the history.
    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    /// The process exited with `code` (`-1` when unknown).
    pub fn set_exited(&mut self, code: i32) {
        self.exited = Some(code);
    }

    pub fn exited(&self) -> Option<i32> {
        self.exited
    }

    /// The title the program set, if any.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Whether the shell showed its first prompt (`OSC 133;B`), so typed
    /// input reaches its command line.
    pub fn prompt_ready(&self) -> bool {
        self.prompt_ready
    }

    /// The visible screen.
    pub fn snapshot(&self) -> TerminalSnapshot {
        let grid = self.term.grid();
        let mode = *self.term.mode();
        let rows = grid.screen_lines();
        let cols = grid.columns();
        let offset = grid.display_offset();
        let lines = (0..rows)
            .map(|row| line_spans(&grid[Line(row as i32 - offset as i32)], cols))
            .collect();

        let point = grid.cursor.point;
        let row = point.line.0 + offset as i32;
        let cursor = (0..rows as i32).contains(&row).then(|| {
            let mut col = point.column.0;
            // On the right half of a wide character, show it on the left.
            if col > 0 && grid[point].flags.contains(Flags::WIDE_CHAR_SPACER) {
                col -= 1;
            }
            let shape = if !mode.contains(TermMode::SHOW_CURSOR) {
                CursorShape::Hidden
            } else {
                match self.term.cursor_style().shape {
                    ansi::CursorShape::Block | ansi::CursorShape::HollowBlock => CursorShape::Block,
                    ansi::CursorShape::Underline => CursorShape::Underline,
                    ansi::CursorShape::Beam => CursorShape::Bar,
                    ansi::CursorShape::Hidden => CursorShape::Hidden,
                }
            };
            TerminalCursor {
                row: row as u16,
                col: col as u16,
                shape,
            }
        });

        let alt_screen = mode.contains(TermMode::ALT_SCREEN);
        let history_len = grid.history_size();
        // The alternate screen covers the main screen's visible lines.
        let first_line = if alt_screen {
            self.main_history as i64
        } else {
            history_len as i64 - offset as i64
        };
        let absolute = |stable: u64| stable as i64 - self.dropped as i64;
        let commands = self
            .commands
            .iter()
            .map(|b| CommandBlock {
                prompt_line: absolute(b.prompt),
                output_line: b.output.map(absolute),
                exit_code: b.exit_code,
                finished: b.finished,
            })
            .collect();

        TerminalSnapshot {
            lines,
            cursor,
            cols: cols as u16,
            rows: rows as u16,
            display_offset: offset,
            history_len,
            first_line,
            commands,
            modes: TerminalModes {
                app_cursor: mode.contains(TermMode::APP_CURSOR),
                bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
                alt_screen,
                mouse_reporting: mode.intersects(TermMode::MOUSE_MODE),
            },
            title: self.title.clone(),
            exited: self.exited,
        }
    }

    /// Feed bytes without marks, in pieces that cannot overflow the slack.
    fn feed(&mut self, bytes: &[u8]) {
        let budget = SLACK / 2;
        let cols = self.term.grid().columns().max(1);
        let (mut cost, mut printable, mut start) = (0, 0, 0);
        for (i, &b) in bytes.iter().enumerate() {
            match b {
                // Line feeds and escapes (index, scroll up, …) may scroll.
                b'\n' | 0x0b | 0x0c | 0x1b => cost += 1,
                0x20.. => {
                    // A line's worth of text may wrap and scroll.
                    printable += 1;
                    if printable >= cols {
                        printable = 0;
                        cost += 1;
                    }
                }
                _ => {}
            }
            if cost >= budget {
                self.parser.advance(&mut self.term, &bytes[start..=i]);
                self.observe_history();
                (cost, start) = (0, i + 1);
            }
        }
        if start < bytes.len() {
            self.parser.advance(&mut self.term, &bytes[start..]);
            self.observe_history();
        }
    }

    /// Trim the main screen's history to the limit and count what left it.
    fn observe_history(&mut self) {
        if self.term.mode().contains(TermMode::ALT_SCREEN) {
            return;
        }
        let grid = self.term.grid_mut();
        let mut history = grid.history_size();
        if std::mem::take(&mut self.resync_history) {
            // Moved by a resize, not cleared.
        } else if history < self.main_history {
            // Cleared (`ESC [3J`, `ESC c`).
            self.dropped += (self.main_history - history) as u64;
        }
        if history > self.scrollback {
            let excess = history - self.scrollback;
            grid.update_history(self.scrollback);
            grid.update_history(self.scrollback + SLACK);
            self.dropped += excess as u64;
            history = self.scrollback;
        }
        self.main_history = history;
        while self
            .commands
            .front()
            .is_some_and(|b| b.prompt < self.dropped)
        {
            self.commands.pop_front();
        }
    }

    /// The stable number of the cursor's line, `None` on the alternate
    /// screen (marks there are meaningless).
    fn cursor_line(&self) -> Option<u64> {
        if self.term.mode().contains(TermMode::ALT_SCREEN) {
            return None;
        }
        let line = self.term.grid().cursor.point.line.0.max(0) as u64;
        Some(self.dropped + self.main_history as u64 + line)
    }

    fn apply_mark(&mut self, mark: Mark, out: &mut CoreOutput) {
        // A mark inside a synchronized update: show what came before it, so
        // the cursor is where the mark belongs.
        if self.parser.sync_bytes_count() > 0 {
            self.parser.stop_sync(&mut self.term);
            self.observe_history();
            self.drain_events(out);
        }
        if mark == Mark::CommandStart {
            self.prompt_ready = true;
        }
        let Some(line) = self.cursor_line() else {
            return;
        };
        match mark {
            Mark::PromptStart => {
                if let Some(last) = self.commands.back_mut() {
                    // The same prompt drawn again (a theme and our script
                    // both marking it, a redraw after a resize).
                    if !last.finished && last.output.is_none() && last.prompt == line {
                        return;
                    }
                    // A new prompt ends whatever ran before (Ctrl+C, a shell
                    // without `D`).
                    last.finished = true;
                }
                self.commands.push_back(Block {
                    prompt: line,
                    output: None,
                    exit_code: None,
                    finished: false,
                });
                if self.commands.len() > MAX_COMMANDS {
                    self.commands.pop_front();
                }
            }
            Mark::CommandStart => {}
            Mark::OutputStart => {
                if let Some(last) = self.commands.back_mut()
                    && !last.finished
                    && last.output.is_none()
                {
                    last.output = Some(line);
                }
            }
            Mark::CommandFinished(code) => {
                if let Some(last) = self.commands.back_mut()
                    && !last.finished
                {
                    last.finished = true;
                    last.exit_code = code;
                    out.events.push(CoreEvent::CommandFinished(code));
                }
            }
        }
    }

    fn drain_events(&mut self, out: &mut CoreOutput) {
        let events = std::mem::take(
            &mut *self
                .listener
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for event in events {
            match event {
                Event::Title(title) => {
                    if self.title.as_deref() != Some(title.as_str()) {
                        self.title = Some(title.clone());
                        out.events.push(CoreEvent::Title(Some(title)));
                    }
                }
                Event::ResetTitle => {
                    if self.title.take().is_some() {
                        out.events.push(CoreEvent::Title(None));
                    }
                }
                Event::Bell => out.events.push(CoreEvent::Bell),
                Event::PtyWrite(text) => out.replies.extend_from_slice(text.as_bytes()),
                Event::ColorRequest(index, format) if index < 256 => {
                    let (r, g, b) = palette_rgb(index as u8);
                    out.replies
                        .extend_from_slice(format(Rgb { r, g, b }).as_bytes());
                }
                Event::TextAreaSizeRequest(format) => {
                    out.replies
                        .extend_from_slice(format(self.window_size()).as_bytes());
                }
                // The clipboard (OSC 52) stays out of reach of programs, the
                // theme's foreground and background are the view's business.
                _ => {}
            }
        }
    }
}

/// The styled text of one grid row, trailing blanks trimmed.
fn line_spans(row: &Row<Cell>, cols: usize) -> TerminalLine {
    let blank_tail = (0..cols)
        .rev()
        .take_while(|&c| is_blank(&row[Column(c)]))
        .count();
    let end = cols - blank_tail;
    let mut spans: TerminalLine = Vec::new();
    for c in 0..end {
        let cell = &row[Column(c)];
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        let style = cell_style(cell);
        let span = match spans.last_mut() {
            Some(last) if last.same_style(&style) => last,
            _ => {
                spans.push(style);
                spans.last_mut().expect("just pushed")
            }
        };
        if cell.flags.contains(Flags::HIDDEN) {
            span.text.push(' ');
        } else {
            span.text.push(if cell.c == '\0' { ' ' } else { cell.c });
            if let Some(zerowidth) = cell.zerowidth() {
                span.text.extend(zerowidth);
            }
        }
    }
    spans
}

/// A cell that shows nothing: a space without background, inverse or
/// underline.
fn is_blank(cell: &Cell) -> bool {
    (cell.c == ' ' || cell.c == '\0')
        && cell.zerowidth().is_none()
        && matches!(cell.bg, Color::Named(NamedColor::Background))
        && !cell
            .flags
            .intersects(Flags::INVERSE | Flags::ALL_UNDERLINES)
}

/// The style of a cell, as an empty span.
fn cell_style(cell: &Cell) -> StyledSpan {
    let (fg, dim_color) = map_color(cell.fg);
    let (bg, _) = map_color(cell.bg);
    let flags = cell.flags;
    StyledSpan {
        text: String::new(),
        fg,
        bold: flags.contains(Flags::BOLD),
        bg,
        italic: flags.contains(Flags::ITALIC),
        underline: flags.intersects(Flags::ALL_UNDERLINES),
        dim: flags.contains(Flags::DIM) || dim_color,
        inverse: flags.contains(Flags::INVERSE),
    }
}

/// The contract's color for an emulator color, and whether it is a dim
/// variant (shown as the normal color, dimmed).
fn map_color(color: Color) -> (AnsiColor, bool) {
    match color {
        Color::Spec(rgb) => (AnsiColor::Rgb(rgb.r, rgb.g, rgb.b), false),
        Color::Indexed(ix) => (AnsiColor::from_index(ix), false),
        Color::Named(named) => {
            let ix = named as usize;
            if ix < 16 {
                (AnsiColor::from_index(ix as u8), false)
            } else {
                match named {
                    NamedColor::DimBlack
                    | NamedColor::DimRed
                    | NamedColor::DimGreen
                    | NamedColor::DimYellow
                    | NamedColor::DimBlue
                    | NamedColor::DimMagenta
                    | NamedColor::DimCyan
                    | NamedColor::DimWhite => {
                        let base = ix - NamedColor::DimBlack as usize;
                        (AnsiColor::from_index(base as u8), true)
                    }
                    NamedColor::DimForeground => (AnsiColor::Default, true),
                    // Foreground, background, cursor, bright foreground.
                    _ => (AnsiColor::Default, false),
                }
            }
        }
    }
}

/// RGB of a palette index (xterm's defaults), for programs that ask.
fn palette_rgb(ix: u8) -> (u8, u8, u8) {
    const BASE: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 0, 0),
        (0, 205, 0),
        (205, 205, 0),
        (0, 0, 238),
        (205, 0, 205),
        (0, 205, 205),
        (229, 229, 229),
        (127, 127, 127),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (92, 92, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    match BASE.get(ix as usize) {
        Some(&rgb) => rgb,
        None => AnsiColor::indexed_rgb(ix),
    }
}

#[cfg(test)]
mod tests;
