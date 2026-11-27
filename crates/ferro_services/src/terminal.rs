//! Terminal sessions. Stage 2 rendered them statically; Stage 8 adds a PTY
//! and a terminal emulator (`ferro_terminal`) behind the same contract:
//! input ([`TerminalService::write`]), size, scrollback, a snapshot of the
//! visible screen, and shell integration (OSC 133 command boundaries with
//! their exit codes).

use std::path::PathBuf;

use crate::Result;

/// The 16 ANSI colors, so themes decide the actual shades, plus the
/// 256-color palette and true color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AnsiColor {
    /// The terminal's foreground (or no background, for a background).
    #[default]
    Default,
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
    /// An index of the 256-color palette above the 16 named colors
    /// (16..=255; smaller indices are mapped to the named colors).
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl AnsiColor {
    /// The named color for indices 0..16, `Indexed` otherwise.
    pub fn from_index(ix: u8) -> Self {
        if ix < 8 {
            color(ix, false)
        } else if ix < 16 {
            color(ix - 8, true)
        } else {
            AnsiColor::Indexed(ix)
        }
    }

    /// RGB of a 256-color palette index above 15 (the 6×6×6 cube and the
    /// gray ramp).
    pub fn indexed_rgb(ix: u8) -> (u8, u8, u8) {
        match ix {
            16..=231 => {
                let ix = ix - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                (level(ix / 36), level((ix / 6) % 6), level(ix % 6))
            }
            232..=255 => {
                let v = 8 + (ix - 232) * 10;
                (v, v, v)
            }
            _ => (128, 128, 128),
        }
    }
}

/// A run of text with one style.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StyledSpan {
    pub text: String,
    pub fg: AnsiColor,
    pub bold: bool,
    /// `Default` is no background.
    pub bg: AnsiColor,
    pub italic: bool,
    pub underline: bool,
    pub dim: bool,
    /// Foreground and background swapped (SGR 7).
    pub inverse: bool,
}

impl StyledSpan {
    /// Whether `other` draws the same way, so the two can be merged.
    pub fn same_style(&self, other: &StyledSpan) -> bool {
        self.fg == other.fg
            && self.bold == other.bold
            && self.bg == other.bg
            && self.italic == other.italic
            && self.underline == other.underline
            && self.dim == other.dim
            && self.inverse == other.inverse
    }
}

pub type TerminalLine = Vec<StyledSpan>;

/// The plain text of a styled line.
pub fn line_text(line: &[StyledSpan]) -> String {
    line.iter().map(|s| s.text.as_str()).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSession {
    pub id: u64,
    pub title: String,
    /// The whole output, for services without an emulator (the mock); a
    /// PTY session leaves it empty and serves [`TerminalService::snapshot`].
    pub lines: Vec<TerminalLine>,
}

/// A shell the terminal can start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellInfo {
    /// `pwsh`, `powershell`, `cmd`, `bash`, `zsh`, `fish`, `sh`, …
    pub id: String,
    /// Shown in the New Terminal menu, e.g. "PowerShell 7".
    pub name: String,
    pub program: PathBuf,
    pub args: Vec<String>,
}

/// How to open a terminal session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewTerminal {
    /// A [`ShellInfo::id`]; `None` is the default shell (settings, then
    /// the system's).
    pub shell: Option<String>,
    /// `None` is the first project root.
    pub cwd: Option<PathBuf>,
    /// `None` names it "Local", "Local (2)", …
    pub title: Option<String>,
    /// A command typed into the shell once it starts (e.g. a build tool
    /// task run in the terminal).
    pub command: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Bar,
    /// The application hid it.
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCursor {
    /// Row of the visible screen, 0-based.
    pub row: u16,
    pub col: u16,
    pub shape: CursorShape,
}

/// Terminal modes that change how the view encodes input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TerminalModes {
    /// DECCKM: arrows send `ESC O A` instead of `ESC [ A`.
    pub app_cursor: bool,
    /// Pasted text is wrapped in `ESC [200~` … `ESC [201~`.
    pub bracketed_paste: bool,
    /// A full-screen program (vim, less) is on the alternate screen.
    pub alt_screen: bool,
    /// The program asked for mouse reports.
    pub mouse_reporting: bool,
}

/// One shell command found through shell integration (OSC 133). Lines are
/// absolute: 0 is the oldest line kept in the scrollback, so they stay
/// valid while the screen scrolls (see [`TerminalSnapshot::first_line`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandBlock {
    /// Where the prompt starts (OSC 133;A).
    pub prompt_line: i64,
    /// Where the output starts (OSC 133;C), once the command runs.
    pub output_line: Option<i64>,
    /// Exit code (OSC 133;D;code), once it finished; `None` while running
    /// or when the shell does not report it (cmd).
    pub exit_code: Option<i32>,
    pub finished: bool,
}

/// The visible screen of a session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TerminalSnapshot {
    /// One entry per visible row, `rows` of them.
    pub lines: Vec<TerminalLine>,
    pub cursor: Option<TerminalCursor>,
    pub cols: u16,
    pub rows: u16,
    /// How many lines the view is scrolled up into the history (0 = at the
    /// bottom, following output).
    pub display_offset: usize,
    /// Lines in the scrollback above the screen.
    pub history_len: usize,
    /// Absolute line number of `lines[0]`.
    pub first_line: i64,
    /// Commands seen so far (the most recent last), from shell integration.
    pub commands: Vec<CommandBlock>,
    pub modes: TerminalModes,
    /// The shell's title (OSC 0/2), if it set one.
    pub title: Option<String>,
    /// The process exited with this code (`Some(-1)` when unknown).
    pub exited: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    /// The screen of `id` changed; take a new snapshot.
    Changed(u64),
    TitleChanged {
        id: u64,
        title: String,
    },
    /// A command finished (shell integration).
    CommandFinished {
        id: u64,
        exit_code: Option<i32>,
    },
    Bell(u64),
    /// The shell process exited; the session stays open showing its output.
    Exited {
        id: u64,
        code: Option<i32>,
    },
}

pub trait TerminalService: Send + Sync + 'static {
    fn sessions(&self) -> Vec<TerminalSession>;
    /// Open a new session with the default shell in the project root.
    fn new_session(&self) -> TerminalSession;
    /// Open a session with options. The default ignores them.
    fn open(&self, options: NewTerminal) -> Result<TerminalSession> {
        let _ = options;
        Ok(self.new_session())
    }
    fn close_session(&self, id: u64);
    /// Shells found on this machine, the default first.
    fn shells(&self) -> Vec<ShellInfo> {
        Vec::new()
    }
    /// Send input (keys, pasted text) to the session's process.
    fn write(&self, id: u64, bytes: &[u8]) {
        let _ = (id, bytes);
    }
    /// The view's size in cells changed.
    fn resize(&self, id: u64, cols: u16, rows: u16) {
        let _ = (id, cols, rows);
    }
    /// Scroll the view: positive `lines` go up into the history,
    /// `i32::MIN` returns to the bottom.
    fn scroll(&self, id: u64, lines: i32) {
        let _ = (id, lines);
    }
    /// Scroll so that absolute line `line` is at the top of the screen.
    fn scroll_to_line(&self, id: u64, line: i64) {
        let _ = (id, line);
    }
    /// The visible screen. The default shows every line of
    /// [`TerminalSession::lines`] without a cursor.
    fn snapshot(&self, id: u64) -> Option<TerminalSnapshot> {
        let session = self.sessions().into_iter().find(|s| s.id == id)?;
        let cols = session
            .lines
            .iter()
            .map(|l| line_text(l).chars().count())
            .max()
            .unwrap_or(0)
            .max(80) as u16;
        Some(TerminalSnapshot {
            rows: session.lines.len() as u16,
            cols,
            lines: session.lines,
            ..TerminalSnapshot::default()
        })
    }
    /// Screen changes, titles, finished commands and exits. The default
    /// never sends anything.
    fn subscribe(&self) -> async_channel::Receiver<TerminalEvent> {
        async_channel::unbounded().1
    }
}

/// Remove ANSI escape sequences (CSI, OSC, single-character escapes).
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => skip_osc(&mut chars),
            _ => {}
        }
    }
    out
}

/// Skip an OSC body, which ends with BEL or `ESC \`.
fn skip_osc(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(c) = chars.next() {
        if c == '\x07' {
            break;
        }
        if c == '\x1b' {
            if chars.peek() == Some(&'\\') {
                chars.next();
            }
            break;
        }
    }
}

/// Parse text with ANSI SGR escape sequences (`\x1b[1;31m`) into styled
/// spans. Other sequences, lone escapes and carriage returns are dropped.
pub fn parse_ansi(text: &str) -> TerminalLine {
    let mut spans: Vec<StyledSpan> = Vec::new();
    let mut style = StyledSpan::default();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    fn flush(spans: &mut Vec<StyledSpan>, current: &mut String, style: &StyledSpan) {
        if !current.is_empty() {
            spans.push(StyledSpan {
                text: std::mem::take(current),
                ..style.clone()
            });
        }
    }
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            let mut code = String::new();
            for c in chars.by_ref() {
                if ('\x40'..='\x7e').contains(&c) {
                    if c == 'm' {
                        flush(&mut spans, &mut current, &style);
                        apply_sgr(&code, &mut style);
                    }
                    break;
                }
                code.push(c);
            }
        } else if c == '\x1b' && chars.peek() == Some(&']') {
            chars.next();
            skip_osc(&mut chars);
        } else if c != '\x1b' && c != '\r' {
            current.push(c);
        }
    }
    flush(&mut spans, &mut current, &style);
    spans
}

fn apply_sgr(code: &str, style: &mut StyledSpan) {
    let parts: Vec<u16> = code
        .split([';', ':'])
        .map(|p| p.parse::<u16>().unwrap_or(0))
        .collect();
    let mut i = 0;
    while i < parts.len() {
        match parts[i] {
            0 => *style = StyledSpan::default(),
            1 => style.bold = true,
            2 => style.dim = true,
            3 => style.italic = true,
            4 => style.underline = true,
            7 => style.inverse = true,
            22 => (style.bold, style.dim) = (false, false),
            23 => style.italic = false,
            24 => style.underline = false,
            27 => style.inverse = false,
            39 => style.fg = AnsiColor::Default,
            49 => style.bg = AnsiColor::Default,
            n @ 30..=37 => style.fg = color((n - 30) as u8, false),
            n @ 90..=97 => style.fg = color((n - 90) as u8, true),
            n @ 40..=47 => style.bg = color((n - 40) as u8, false),
            n @ 100..=107 => style.bg = color((n - 100) as u8, true),
            n @ (38 | 48) => {
                let target = if n == 38 {
                    &mut style.fg
                } else {
                    &mut style.bg
                };
                match parts.get(i + 1) {
                    Some(5) => {
                        if let Some(&ix) = parts.get(i + 2) {
                            *target = AnsiColor::from_index(ix.min(255) as u8);
                        }
                        i += 2;
                    }
                    Some(2) => {
                        if let (Some(&r), Some(&g), Some(&b)) =
                            (parts.get(i + 2), parts.get(i + 3), parts.get(i + 4))
                        {
                            *target = AnsiColor::Rgb(r as u8, g as u8, b as u8);
                        }
                        i += 4;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        i += 1;
    }
}

fn color(ix: u8, bright: bool) -> AnsiColor {
    use AnsiColor::*;
    const NORMAL: [AnsiColor; 8] = [Black, Red, Green, Yellow, Blue, Magenta, Cyan, White];
    const BRIGHT: [AnsiColor; 8] = [
        BrightBlack,
        BrightRed,
        BrightGreen,
        BrightYellow,
        BrightBlue,
        BrightMagenta,
        BrightCyan,
        BrightWhite,
    ];
    if bright {
        BRIGHT[ix as usize]
    } else {
        NORMAL[ix as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_sequences_become_styled_spans() {
        let line = parse_ansi("\x1b[1;32m   Compiling\x1b[0m demo-app \x1b[91merror\x1b[39m!");
        let texts: Vec<_> = line.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["   Compiling", " demo-app ", "error", "!"]);
        assert_eq!(line[0].fg, AnsiColor::Green);
        assert!(line[0].bold);
        assert_eq!(line[1].fg, AnsiColor::Default);
        assert_eq!(line[2].fg, AnsiColor::BrightRed);
        assert_eq!(line[3].fg, AnsiColor::Default);
    }

    #[test]
    fn plain_text_is_one_span() {
        assert_eq!(parse_ansi("ls -la").len(), 1);
        assert!(parse_ansi("").is_empty());
    }

    #[test]
    fn extended_colors_backgrounds_and_attributes() {
        let line = parse_ansi("\x1b[38;5;196;48;2;1;2;3;4;7mx\x1b[24;27;49my\x1b[38;5;9mz");
        assert_eq!(line[0].fg, AnsiColor::Indexed(196));
        assert_eq!(line[0].bg, AnsiColor::Rgb(1, 2, 3));
        assert!(line[0].underline && line[0].inverse);
        assert_eq!(line[1].bg, AnsiColor::Default);
        assert!(!line[1].underline && !line[1].inverse);
        assert_eq!(line[2].fg, AnsiColor::BrightRed);
        assert_eq!(AnsiColor::indexed_rgb(16), (0, 0, 0));
        assert_eq!(AnsiColor::indexed_rgb(231), (255, 255, 255));
    }

    #[test]
    fn strip_removes_csi_and_osc() {
        assert_eq!(
            strip_ansi("\x1b[1;31merror\x1b[0m: \x1b]8;;file:///a\x1b\\link\x1b]8;;\x07!"),
            "error: link!"
        );
    }
}
