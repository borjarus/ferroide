//! Terminal sessions. Stage 2 renders them statically; Stage 8 adds a PTY
//! and a terminal emulator behind the same contract.

/// The 16 ANSI colors, so themes decide the actual shades.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnsiColor {
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
}

/// A run of text with one style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledSpan {
    pub text: String,
    pub fg: AnsiColor,
    pub bold: bool,
}

pub type TerminalLine = Vec<StyledSpan>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSession {
    pub id: u64,
    pub title: String,
    pub lines: Vec<TerminalLine>,
}

pub trait TerminalService: Send + Sync + 'static {
    fn sessions(&self) -> Vec<TerminalSession>;
    /// Open a new session and return it.
    fn new_session(&self) -> TerminalSession;
    fn close_session(&self, id: u64);
}

/// Parse text with ANSI SGR escape sequences (`\x1b[1;31m`) into styled
/// spans. Unknown sequences are dropped.
pub fn parse_ansi(text: &str) -> TerminalLine {
    let mut spans = Vec::new();
    let (mut fg, mut bold) = (AnsiColor::Default, false);
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            let mut code = String::new();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    if c == 'm' {
                        if !current.is_empty() {
                            spans.push(StyledSpan {
                                text: std::mem::take(&mut current),
                                fg,
                                bold,
                            });
                        }
                        for part in code.split(';') {
                            match part.parse::<u8>().unwrap_or(0) {
                                0 => (fg, bold) = (AnsiColor::Default, false),
                                1 => bold = true,
                                22 => bold = false,
                                39 => fg = AnsiColor::Default,
                                n @ 30..=37 => fg = color(n - 30, false),
                                n @ 90..=97 => fg = color(n - 90, true),
                                _ => {}
                            }
                        }
                    }
                    break;
                }
                code.push(c);
            }
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        spans.push(StyledSpan {
            text: current,
            fg,
            bold,
        });
    }
    spans
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
}
