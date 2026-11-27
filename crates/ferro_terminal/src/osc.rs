//! A scanner for shell-integration marks (OSC 133) in the PTY byte stream.
//!
//! `alacritty_terminal`'s escape-sequence handler ignores OSC 133, so the
//! reader looks for it itself. The scanner only *observes* the bytes: all of
//! them still go to the emulator (which drops the unknown OSC), and the
//! scanner reports the offset just past each mark's terminator, so the core
//! feeds the bytes up to there and records the mark at the cursor the
//! emulator has reached. Its state survives chunk boundaries, so a mark split
//! across two reads is still found, and nothing is held back from the
//! emulator.

/// A shell-integration mark (FinalTerm / OSC 133).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// `A`: the prompt starts.
    PromptStart,
    /// `B`: the prompt ends and the command line (typed input) starts.
    CommandStart,
    /// `C`: the command was accepted; its output starts.
    OutputStart,
    /// `D[;code]`: the command finished, with its exit code when the shell
    /// reports it.
    CommandFinished(Option<i32>),
}

/// Longest OSC body kept; longer ones (OSC 52 clipboard data, …) are not
/// marks and are skipped without buffering.
const MAX_BODY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    #[default]
    Ground,
    /// After ESC.
    Escape,
    /// Inside an OSC body.
    Osc,
    /// After ESC inside an OSC body (maybe the `ESC \` terminator).
    OscEscape,
}

/// Finds OSC 133 marks; feed it every byte of the stream in order.
#[derive(Debug, Default)]
pub struct OscScanner {
    state: State,
    body: Vec<u8>,
    overflow: bool,
}

impl OscScanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Scan `bytes`; returns each mark with the offset just past its
    /// terminator (the bytes the emulator must have seen before the mark is
    /// placed).
    pub fn scan(&mut self, bytes: &[u8]) -> Vec<(usize, Mark)> {
        let mut marks = Vec::new();
        for (i, &b) in bytes.iter().enumerate() {
            match self.state {
                State::Ground => {
                    if b == 0x1b {
                        self.state = State::Escape;
                    }
                }
                State::Escape => self.after_escape(b),
                State::Osc => match b {
                    0x07 => {
                        if let Some(mark) = self.finish() {
                            marks.push((i + 1, mark));
                        }
                    }
                    0x1b => self.state = State::OscEscape,
                    // CAN and SUB abort the sequence.
                    0x18 | 0x1a => self.state = State::Ground,
                    _ => self.push(b),
                },
                State::OscEscape => {
                    if b == b'\\' {
                        if let Some(mark) = self.finish() {
                            marks.push((i + 1, mark));
                        }
                    } else {
                        // ESC inside the body ends it and starts a new escape.
                        self.after_escape(b);
                    }
                }
            }
        }
        marks
    }

    fn after_escape(&mut self, b: u8) {
        match b {
            b']' => {
                self.state = State::Osc;
                self.body.clear();
                self.overflow = false;
            }
            0x1b => self.state = State::Escape,
            _ => self.state = State::Ground,
        }
    }

    fn push(&mut self, b: u8) {
        if self.body.len() < MAX_BODY {
            self.body.push(b);
        } else {
            self.overflow = true;
        }
    }

    fn finish(&mut self) -> Option<Mark> {
        self.state = State::Ground;
        if self.overflow {
            return None;
        }
        parse_mark(&self.body)
    }
}

/// Parse an OSC body such as `133;D;1` or `133;A;cl=m`.
fn parse_mark(body: &[u8]) -> Option<Mark> {
    let rest = body.strip_prefix(b"133;")?;
    let mut params = rest.split(|&b| b == b';');
    let kind = params.next()?;
    match kind {
        b"A" => Some(Mark::PromptStart),
        b"B" => Some(Mark::CommandStart),
        b"C" => Some(Mark::OutputStart),
        b"D" => {
            let code = params
                .next()
                .and_then(|p| std::str::from_utf8(p).ok())
                .and_then(|p| p.trim().parse::<i32>().ok());
            Some(Mark::CommandFinished(code))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_marks_with_both_terminators() {
        let mut scanner = OscScanner::new();
        let input = b"\x1b]133;A\x07$ \x1b]133;B\x1b\\ls\r\n\x1b]133;C\x07out\x1b]133;D;2\x07";
        let marks = scanner.scan(input);
        let kinds: Vec<_> = marks.iter().map(|(_, m)| *m).collect();
        assert_eq!(
            kinds,
            [
                Mark::PromptStart,
                Mark::CommandStart,
                Mark::OutputStart,
                Mark::CommandFinished(Some(2))
            ]
        );
        // The offsets point just past each terminator.
        assert_eq!(marks[0].0, 8);
        assert_eq!(&input[marks[0].0..marks[0].0 + 2], b"$ ");
        assert_eq!(marks[3].0, input.len());
    }

    #[test]
    fn marks_split_across_chunks_are_found() {
        let input = b"ab\x1b]133;D;127\x1b\\cd\x1b]133;A\x07";
        // Every split point gives the same marks at the same stream offsets.
        for split in 0..=input.len() {
            let mut scanner = OscScanner::new();
            let mut marks: Vec<(usize, Mark)> = scanner.scan(&input[..split]);
            marks.extend(
                scanner
                    .scan(&input[split..])
                    .into_iter()
                    .map(|(o, m)| (o + split, m)),
            );
            assert_eq!(
                marks,
                [
                    (15, Mark::CommandFinished(Some(127))),
                    (input.len(), Mark::PromptStart)
                ],
                "split at {split}"
            );
        }
        // Byte by byte, too.
        let mut scanner = OscScanner::new();
        let count: usize = input.iter().map(|b| scanner.scan(&[*b]).len()).sum();
        assert_eq!(count, 2);
    }

    #[test]
    fn other_sequences_and_bad_marks_are_ignored() {
        let mut scanner = OscScanner::new();
        let long = format!("\x1b]52;c;{}\x07", "A".repeat(500));
        assert!(scanner.scan(long.as_bytes()).is_empty());
        assert!(
            scanner
                .scan(b"\x1b]0;title\x07\x1b[31m\x1b]133;Z\x07")
                .is_empty()
        );
        // An aborted sequence does not swallow the next mark.
        let marks = scanner.scan(b"\x1b]133;A\x18\x1b]133;D\x07");
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].1, Mark::CommandFinished(None));
        // ESC inside a body starts a new escape.
        let marks = scanner.scan(b"\x1b]133;\x1b]133;C\x07");
        assert_eq!(marks[0].1, Mark::OutputStart);
        // Extra parameters (`aid=`, `cl=`) are fine, a bad code is `None`.
        let marks = scanner.scan(b"\x1b]133;A;cl=m\x07\x1b]133;D;x\x07");
        assert_eq!(marks[0].1, Mark::PromptStart);
        assert_eq!(marks[1].1, Mark::CommandFinished(None));
    }
}
