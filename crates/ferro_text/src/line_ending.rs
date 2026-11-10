//! Line separators. Buffers always hold `\n`; the file's own separator is
//! detected on load and written back on save.

use std::borrow::Cow;

/// A line separator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LineEnding {
    #[default]
    Lf,
    CrLf,
    /// Classic Mac OS.
    Cr,
}

impl LineEnding {
    pub const ALL: [LineEnding; 3] = [LineEnding::Lf, LineEnding::CrLf, LineEnding::Cr];

    /// The platform's convention, used for files without any line break.
    pub fn native() -> Self {
        if cfg!(windows) {
            LineEnding::CrLf
        } else {
            LineEnding::Lf
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
            LineEnding::Cr => "\r",
        }
    }

    /// Status bar label.
    pub fn label(self) -> &'static str {
        match self {
            LineEnding::Lf => "LF",
            LineEnding::CrLf => "CRLF",
            LineEnding::Cr => "CR",
        }
    }
}

/// What a text uses to separate lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineEndingStats {
    pub lf: usize,
    pub crlf: usize,
    pub cr: usize,
}

impl LineEndingStats {
    pub fn of(text: &str) -> Self {
        let bytes = text.as_bytes();
        let (mut lf, mut crlf, mut cr) = (0, 0, 0);
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\r' if bytes.get(i + 1) == Some(&b'\n') => {
                    crlf += 1;
                    i += 1;
                }
                b'\r' => cr += 1,
                b'\n' => lf += 1,
                _ => {}
            }
            i += 1;
        }
        Self { lf, crlf, cr }
    }

    /// The most frequent separator; ties prefer LF, then CRLF. `None`
    /// without any line break.
    pub fn dominant(&self) -> Option<LineEnding> {
        if self.lf + self.crlf + self.cr == 0 {
            return None;
        }
        Some(if self.lf >= self.crlf && self.lf >= self.cr {
            LineEnding::Lf
        } else if self.crlf >= self.cr {
            LineEnding::CrLf
        } else {
            LineEnding::Cr
        })
    }

    /// More than one kind of separator is used.
    pub fn is_mixed(&self) -> bool {
        [self.lf, self.crlf, self.cr]
            .iter()
            .filter(|n| **n > 0)
            .count()
            > 1
    }
}

/// The separator to keep for `text`: the dominant one, or the platform's
/// when the text has a single line.
pub fn detect(text: &str) -> LineEnding {
    LineEndingStats::of(text)
        .dominant()
        .unwrap_or_else(LineEnding::native)
}

/// Turn every `\r\n` and lone `\r` into `\n`.
pub fn normalize(text: &str) -> Cow<'_, str> {
    if !text.contains('\r') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

/// Turn the `\n` of a normalized text into `ending`.
pub fn denormalize(text: &str, ending: LineEnding) -> Cow<'_, str> {
    match ending {
        LineEnding::Lf => Cow::Borrowed(text),
        _ if !text.contains('\n') => Cow::Borrowed(text),
        _ => Cow::Owned(text.replace('\n', ending.as_str())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_dominant_and_mixed() {
        let stats = LineEndingStats::of("a\r\nb\r\nc\nd");
        assert_eq!(stats.dominant(), Some(LineEnding::CrLf));
        assert!(stats.is_mixed());
        assert_eq!(detect("a\rb\r"), LineEnding::Cr);
        assert_eq!(detect("single line"), LineEnding::native());
    }

    #[test]
    fn round_trip() {
        let original = "a\r\nb\r\n\r\nc";
        let normalized = normalize(original);
        assert_eq!(normalized, "a\nb\n\nc");
        assert_eq!(denormalize(&normalized, LineEnding::CrLf), original);
        assert_eq!(normalize("x\ry\r\n"), "x\ny\n");
    }
}
