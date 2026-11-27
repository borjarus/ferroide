//! Clickable file locations in the Run console and terminal output.
//!
//! The UI calls [`find_links`] on every plain-text line (ANSI sequences
//! already stripped), underlines the ranges it returns, and on a click calls
//! [`resolve_link`] to turn the printed location into a file on disk.
//!
//! Parsing is hand-written and runs in two passes:
//!
//! 1. Format-specific matchers for the shapes whose path cannot be found by
//!    splitting on whitespace: Python's `File "…", line N` (the path is
//!    quoted and may contain spaces), .NET's `in C:\My Projects\y.cs:line N`
//!    and JVM stack frames `at pkg.Class.method(File.java:N)`, which only
//!    print a file name and are kept as [`LinkKind::JavaFrame`] with the
//!    package so the file can be found by searching the project.
//! 2. A generic pass that splits the line into tokens at whitespace, quotes,
//!    brackets, `<>`, commas and `=`, and accepts `path:line[:col]` (Rust,
//!    Go, gcc/clang, JS stacks, `file://` URLs) and `path(line[,col]):`
//!    (MSBuild, tsc). A generic match never overlaps a format-specific one.
//!
//! False positives cost more than a missed link (a bogus underline on a
//! timestamp is noise on every line), so the generic pass insists on a
//! plausible file name: the last path segment must have an extension of
//! 1–10 ASCII alphanumerics containing a letter, which rules out times
//! (`12:30:45`), versions (`1.2.3:4`) and IP addresses (`127.0.0.1:8080`).
//! A colon inside the path (other than a drive letter's) rejects it, which
//! rules out `std::fmt::Display`, `node:internal/…` and `host:port`;
//! anything inside a `scheme://` URL other than `file://` is never a link;
//! and a bare host such as `example.com:443` is rejected by a short list of
//! domain suffixes. A path without a line number is not a link.

use std::ffi::OsStr;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// How a [`Link`]'s path is found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkKind {
    /// A path as printed: absolute, or relative to the working directory or
    /// a project root.
    Path,
    /// A JVM stack frame, which only prints the file name: the file is
    /// searched for under the project roots, preferring one in the
    /// directory of `package` (dot-separated, empty for the default
    /// package).
    JavaFrame { package: String },
}

/// A file location found in a line of output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// Byte range in the line to underline: the path with its line (and
    /// column) suffix.
    pub range: Range<usize>,
    /// The path as printed; `file://` URLs are converted to a path.
    pub path: String,
    /// 1-based, as printed.
    pub line: Option<u32>,
    /// 1-based, as printed.
    pub column: Option<u32>,
    pub kind: LinkKind,
}

/// File locations in `line`, in order of appearance and non-overlapping.
pub fn find_links(line: &str) -> Vec<Link> {
    let urls = url_spans(line);
    let mut links: Vec<Link> = Vec::new();
    let candidates = python_links(line)
        .into_iter()
        .chain(dotnet_links(line))
        .chain(java_links(line))
        .chain(generic_links(line));
    for link in candidates {
        let taken = urls.iter().any(|url| overlaps(url, &link.range))
            || links
                .iter()
                .any(|other| overlaps(&other.range, &link.range));
        if !taken {
            links.push(link);
        }
    }
    links.sort_by_key(|link| link.range.start);
    links
}

/// The file `link` points at, with its 0-based line and column (0 when not
/// printed). Relative paths are tried against `cwd`, then each of `roots`;
/// JVM frames are searched for under `roots` (or `cwd` when there are
/// none). `None` when no such file exists.
pub fn resolve_link(
    link: &Link,
    cwd: Option<&Path>,
    roots: &[PathBuf],
) -> Option<(PathBuf, u32, u32)> {
    let line = link.line.unwrap_or(0).saturating_sub(1);
    let column = link.column.unwrap_or(0).saturating_sub(1);
    let path = match &link.kind {
        LinkKind::Path => resolve_path(&link.path, cwd, roots)?,
        LinkKind::JavaFrame { package } => find_jvm_source(package, &link.path, cwd, roots)?,
    };
    Some((path, line, column))
}

// ---------------------------------------------------------------------------
// Format-specific matchers
// ---------------------------------------------------------------------------

/// Python: `File "path", line N` — the path is quoted and may contain spaces.
fn python_links(line: &str) -> Vec<Link> {
    let mut out = Vec::new();
    for (ix, marker) in line.match_indices("File \"") {
        let path_start = ix + marker.len();
        let Some(close) = line[path_start..].find('"').map(|i| i + path_start) else {
            continue;
        };
        let path = &line[path_start..close];
        // `<stdin>`, `<frozen importlib._bootstrap>`, `<string>`.
        if path.trim().is_empty() || path.starts_with('<') {
            continue;
        }
        let suffix = "\", line ";
        if !line[close..].starts_with(suffix) {
            continue;
        }
        let Some((number, end)) = number_at(line, close + suffix.len()) else {
            continue;
        };
        out.push(Link {
            range: path_start..end,
            path: path.to_string(),
            line: Some(number),
            column: None,
            kind: LinkKind::Path,
        });
    }
    out
}

/// .NET stack traces: `at A.B() in C:\My Projects\y.cs:line 42` — the path
/// runs from ` in ` to `:line`, spaces included.
fn dotnet_links(line: &str) -> Vec<Link> {
    let mut out = Vec::new();
    for (ix, marker) in line.match_indices(":line ") {
        let Some((number, end)) = number_at(line, ix + marker.len()) else {
            continue;
        };
        let Some(path_start) = line[..ix].rfind(" in ").map(|i| i + 4) else {
            continue;
        };
        let path = &line[path_start..ix];
        if !plausible_path(path) {
            continue;
        }
        out.push(Link {
            range: path_start..end,
            path: path.to_string(),
            line: Some(number),
            column: None,
            kind: LinkKind::Path,
        });
    }
    out
}

/// JVM frames: `at [module/]pkg.Class.method(File.ext:N)`. Only the
/// `File.ext:N` part is underlined.
fn java_links(line: &str) -> Vec<Link> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    for (open, _) in line.match_indices('(') {
        let Some(close) = line[open..].find(')').map(|i| i + open) else {
            continue;
        };
        let inner = &line[open + 1..close];
        let Some(colon) = inner.rfind(':') else {
            continue;
        };
        let file = &inner[..colon];
        if file.contains(['/', '\\', ':', ' ']) || !plausible_path(file) {
            continue;
        }
        let Some((number, end)) = number_at(line, open + 1 + colon + 1) else {
            continue;
        };
        if end != close {
            continue;
        }
        // The qualified method name directly before `(`, after whitespace.
        let name_start = bytes[..open]
            .iter()
            .rposition(u8::is_ascii_whitespace)
            .map_or(0, |i| i + 1);
        let qualified = &line[name_start..open];
        let before = line[..name_start].trim_end();
        let after_at = before
            .strip_suffix("at")
            .is_some_and(|rest| rest.is_empty() || rest.ends_with(|c: char| c.is_whitespace()));
        if !after_at {
            continue;
        }
        let Some(package) = jvm_package(qualified) else {
            continue;
        };
        out.push(Link {
            range: open + 1..close,
            path: file.to_string(),
            line: Some(number),
            column: None,
            kind: LinkKind::JavaFrame { package },
        });
    }
    out
}

/// The package of a qualified JVM method name (`java.base/java.lang.Thread.run`
/// → `java.lang`): everything before the class, which is the first segment
/// starting with an uppercase letter, or the second-to-last segment.
fn jvm_package(qualified: &str) -> Option<String> {
    let qualified = qualified.rsplit('/').next().unwrap_or(qualified);
    let segments: Vec<&str> = qualified.split('.').collect();
    if segments.len() < 2 || segments.iter().any(|s| s.is_empty()) {
        return None;
    }
    let types = &segments[..segments.len() - 1];
    let class = types
        .iter()
        .position(|s| s.starts_with(|c: char| c.is_uppercase()))
        .unwrap_or(types.len() - 1);
    Some(segments[..class].join("."))
}

// ---------------------------------------------------------------------------
// Generic `path:line[:col]` and `path(line[,col]):`
// ---------------------------------------------------------------------------

/// Characters that end a path in the generic pass.
fn is_delimiter(b: u8) -> bool {
    b.is_ascii_whitespace()
        || matches!(
            b,
            b'"' | b'\''
                | b'`'
                | b'('
                | b')'
                | b'['
                | b']'
                | b'{'
                | b'}'
                | b'<'
                | b'>'
                | b','
                | b'='
                | b'|'
        )
}

fn generic_links(line: &str) -> Vec<Link> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if is_delimiter(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && !is_delimiter(bytes[i]) {
            i += 1;
        }
        // Delimiters are ASCII, so `start..i` is on char boundaries.
        if let Some(link) = parse_token(line, start, i) {
            out.push(link);
        }
    }
    out
}

/// A link in the token `line[start..end]`.
fn parse_token(line: &str, start: usize, end: usize) -> Option<Link> {
    let token = &line[start..end];
    let (prefix, url) = match token.strip_prefix("file://") {
        // `file:///C:/x` → `C:/x`; `file:///home/x` → `/home/x`.
        Some(rest) => {
            let b = rest.as_bytes();
            let drive = b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':';
            (if drive { 8 } else { 7 }, true)
        }
        None => (0, false),
    };
    let body = &token[prefix..];
    let b = body.as_bytes();
    for (ix, _) in body.match_indices(':') {
        if !b.get(ix + 1).is_some_and(u8::is_ascii_digit) {
            continue;
        }
        let path = &body[..ix];
        if !plausible_path(path) {
            continue;
        }
        let (number, mut tail) = number_at(body, ix + 1)?;
        let mut column = None;
        if b.get(tail) == Some(&b':') && b.get(tail + 1).is_some_and(u8::is_ascii_digit) {
            let (col, after) = number_at(body, tail + 1)?;
            column = Some(col);
            tail = after;
        }
        // `x.rs:3abc` and `a.jar:4.13` are not locations; `x.rs:3:`,
        // `x.rs:3.` and `x.rs:3;` are.
        let decimal = b.get(tail) == Some(&b'.') && b.get(tail + 1).is_some_and(u8::is_ascii_digit);
        if decimal || b.get(tail).is_some_and(u8::is_ascii_alphanumeric) {
            return None;
        }
        let path = if url {
            percent_decode(path)
        } else {
            path.to_string()
        };
        return Some(Link {
            range: start..start + prefix + tail,
            path,
            line: Some(number),
            column,
            kind: LinkKind::Path,
        });
    }
    if url || !plausible_path(body) {
        return None;
    }
    let (number, column, len) = paren_suffix(&line[end..])?;
    Some(Link {
        range: start..end + len,
        path: body.to_string(),
        line: Some(number),
        column,
        kind: LinkKind::Path,
    })
}

/// MSBuild / tsc: `(12,5):` or `(12):` right after the path. Returns the
/// line, the column and the length through `)`. The colon after `)` is
/// required, so a call like `Math.max(1,2)` is not taken for a location.
fn paren_suffix(rest: &str) -> Option<(u32, Option<u32>, usize)> {
    if !rest.starts_with('(') {
        return None;
    }
    let (number, mut at) = number_at(rest, 1)?;
    let mut column = None;
    if rest[at..].starts_with(',') {
        let (col, after) = number_at(rest, at + 1)?;
        column = Some(col);
        at = after;
    }
    if !rest[at..].starts_with("):") {
        return None;
    }
    Some((number, column, at + 1))
}

/// Domain suffixes that make a separator-less `name.ext:N` a host and port.
const HOST_SUFFIXES: &[&str] = &[
    "com",
    "org",
    "net",
    "io",
    "dev",
    "local",
    "localhost",
    "internal",
    "lan",
];

/// Whether `path` looks like a file: its last segment has an extension of
/// 1–10 ASCII alphanumerics with a letter, and it has no colon except a
/// drive letter's.
fn plausible_path(path: &str) -> bool {
    let b = path.as_bytes();
    if b.is_empty() {
        return false;
    }
    for (i, &c) in b.iter().enumerate() {
        let drive = i == 1 && b[0].is_ascii_alphabetic() && matches!(b.get(2), Some(b'\\' | b'/'));
        if c == b':' && !drive {
            return false;
        }
    }
    let segment = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let Some(dot) = segment.rfind('.') else {
        return false;
    };
    let extension = &segment[dot + 1..];
    let extension_ok = (1..=10).contains(&extension.len())
        && extension.bytes().all(|c| c.is_ascii_alphanumeric())
        && extension.bytes().any(|c| c.is_ascii_alphabetic());
    if !extension_ok {
        return false;
    }
    let bare = !path.contains(['/', '\\']);
    !(bare
        && HOST_SUFFIXES
            .iter()
            .any(|suffix| extension.eq_ignore_ascii_case(suffix)))
}

/// Byte ranges of URLs other than `file://` (`https://host:443/a.js:1`):
/// nothing inside them is a link.
fn url_spans(line: &str) -> Vec<Range<usize>> {
    let b = line.as_bytes();
    let mut spans = Vec::new();
    for (ix, _) in line.match_indices("://") {
        let mut start = ix;
        while start > 0
            && (b[start - 1].is_ascii_alphanumeric() || matches!(b[start - 1], b'+' | b'.' | b'-'))
        {
            start -= 1;
        }
        while start < ix && !b[start].is_ascii_alphabetic() {
            start += 1;
        }
        let scheme = &line[start..ix];
        if scheme.is_empty() || scheme.eq_ignore_ascii_case("file") {
            continue;
        }
        let end = b[ix..]
            .iter()
            .position(|&c| c.is_ascii_whitespace() || matches!(c, b'"' | b'\'' | b'<' | b'>'))
            .map_or(b.len(), |i| i + ix);
        spans.push(start..end);
    }
    spans
}

fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

/// The decimal number starting at byte `at` of `s`, and the byte after it.
fn number_at(s: &str, at: usize) -> Option<(u32, usize)> {
    let b = s.as_bytes();
    let mut end = at;
    while end < b.len() && b[end].is_ascii_digit() {
        end += 1;
    }
    if end == at {
        return None;
    }
    s[at..end].parse().ok().map(|n| (n, end))
}

/// `%20` → space; invalid escapes are kept as printed.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let hex = |c: Option<&u8>| c.and_then(|&c| (c as char).to_digit(16));
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let (Some(high), Some(low)) = (hex(b.get(i + 1)), hex(b.get(i + 2)))
        {
            out.push((high * 16 + low) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

fn resolve_path(path: &str, cwd: Option<&Path>, roots: &[PathBuf]) -> Option<PathBuf> {
    if is_absolute(path) {
        let candidate = Path::new(path);
        if candidate.is_file() {
            return Some(normalize(candidate));
        }
        #[cfg(windows)]
        if let Some(candidate) = msys_path(path)
            && candidate.is_file()
        {
            return Some(normalize(&candidate));
        }
        return None;
    }
    let mut relative = path;
    while let Some(rest) = relative
        .strip_prefix("./")
        .or_else(|| relative.strip_prefix(".\\"))
    {
        relative = rest;
    }
    cwd.into_iter()
        .chain(roots.iter().map(PathBuf::as_path))
        .map(|base| base.join(relative))
        .find(|candidate| candidate.is_file())
        .map(|candidate| normalize(&candidate))
}

/// Absolute on any platform: `/x`, `\\server\share`, `C:\x`, `C:/x`.
fn is_absolute(path: &str) -> bool {
    let b = path.as_bytes();
    Path::new(path).is_absolute()
        || path.starts_with(['/', '\\'])
        || (b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && matches!(b[2], b'/' | b'\\'))
}

/// MSYS / Git Bash: `/c/x/y` → `C:\x\y`.
#[cfg(windows)]
fn msys_path(path: &str) -> Option<PathBuf> {
    let b = path.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b'/' {
        let drive = (b[1] as char).to_ascii_uppercase();
        Some(PathBuf::from(format!("{drive}:\\{}", &path[3..])))
    } else {
        None
    }
}

/// One separator style (the platform's) and no `.` components.
fn normalize(path: &Path) -> PathBuf {
    path.components().collect()
}

/// Directories never searched for JVM sources: build output and
/// dependencies (hidden directories are skipped too).
const SKIPPED_DIRS: &[&str] = &["target", "build", "out", "node_modules"];

/// Upper bound on directory entries visited by one JVM source search.
const MAX_VISITED: usize = 50_000;

/// A file named `file` under `roots` (or `cwd` when there are none),
/// preferring one whose directories end with `package`; otherwise the first
/// one with that name. Breadth is bounded by [`MAX_VISITED`].
fn find_jvm_source(
    package: &str,
    file: &str,
    cwd: Option<&Path>,
    roots: &[PathBuf],
) -> Option<PathBuf> {
    let segments: Vec<&str> = package.split('.').filter(|s| !s.is_empty()).collect();
    let mut bases: Vec<&Path> = roots.iter().map(PathBuf::as_path).collect();
    if bases.is_empty() {
        bases.extend(cwd);
    }
    let mut visited = 0;
    let mut fallback = None;
    for base in bases {
        let mut stack = vec![base.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
            entries.sort_by_key(std::fs::DirEntry::file_name);
            let mut subdirs = Vec::new();
            for entry in entries {
                visited += 1;
                if visited > MAX_VISITED {
                    return fallback;
                }
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if file_type.is_dir() {
                    if !name.starts_with('.') && !SKIPPED_DIRS.contains(&name.as_ref()) {
                        subdirs.push(entry.path());
                    }
                } else if file_type.is_file() && name == file {
                    let path = entry.path();
                    if in_package(&path, &segments) {
                        return Some(normalize(&path));
                    }
                    if fallback.is_none() {
                        fallback = Some(normalize(&path));
                    }
                }
            }
            // Popped in name order.
            stack.extend(subdirs.into_iter().rev());
        }
    }
    fallback
}

/// Whether the directories holding `path` end with `package`'s segments.
fn in_package(path: &Path, package: &[&str]) -> bool {
    let mut dir = path.parent();
    for segment in package.iter().rev() {
        let Some(current) = dir else {
            return false;
        };
        if current.file_name() != Some(OsStr::new(segment)) {
            return false;
        }
        dir = current.parent();
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links(line: &str) -> Vec<Link> {
        find_links(line)
    }

    fn one(line: &str) -> Link {
        let mut found = links(line);
        assert_eq!(found.len(), 1, "{line:?}: {found:?}");
        found.remove(0)
    }

    /// Asserts a single link with this path, line and column.
    fn check(line: &str, path: &str, number: u32, column: Option<u32>) -> Link {
        let link = one(line);
        assert_eq!(link.path, path, "{line:?}");
        assert_eq!(link.line, Some(number), "{line:?}");
        assert_eq!(link.column, column, "{line:?}");
        link
    }

    fn underlined<'a>(line: &'a str, link: &Link) -> &'a str {
        &line[link.range.clone()]
    }

    fn none(line: &str) {
        assert!(links(line).is_empty(), "{line:?}: {:?}", links(line));
    }

    // -- generic ----------------------------------------------------------

    #[test]
    fn generic_paths() {
        check(r"C:\x\y.rs:3:4", r"C:\x\y.rs", 3, Some(4));
        check("C:/x/y.rs:3", "C:/x/y.rs", 3, None);
        check("/a/b.rs:3", "/a/b.rs", 3, None);
        check(r".\src\x.rs:3", r".\src\x.rs", 3, None);
        check("../x/y.py:7", "../x/y.py", 7, None);
        check(
            r"\\server\share\x.rs:12:1",
            r"\\server\share\x.rs",
            12,
            Some(1),
        );
        check("error at .env:3", ".env", 3, None);
    }

    #[test]
    fn exact_range_of_windows_path() {
        let line = r"error: C:\x\y.rs:3:4: oops";
        let link = one(line);
        assert_eq!(link.range, 7..20);
        assert_eq!(underlined(line, &link), r"C:\x\y.rs:3:4");
    }

    #[test]
    fn trailing_punctuation_is_not_part_of_the_link() {
        let line = "see src/x.rs:12.";
        let link = check(line, "src/x.rs", 12, None);
        assert_eq!(underlined(line, &link), "src/x.rs:12");
        let line = "'src/x.rs:3:4';";
        let link = check(line, "src/x.rs", 3, Some(4));
        assert_eq!(underlined(line, &link), "src/x.rs:3:4");
        check("at src/x.rs:5;", "src/x.rs", 5, None);
        check("in a.rs:5, b", "a.rs", 5, None);
        none("src/x.rs:3abc");
    }

    #[test]
    fn several_links_in_order() {
        let found = links("a.rs:1 and b.rs:2:3 then c/d.go:4");
        let paths: Vec<_> = found.iter().map(|l| l.path.as_str()).collect();
        assert_eq!(paths, ["a.rs", "b.rs", "c/d.go"]);
        assert_eq!(found[1].column, Some(3));
    }

    #[test]
    fn ranges_are_byte_offsets_after_non_ascii_text() {
        let line = "błąd → src/żółw.rs:3:4 — źle";
        let link = check(line, "src/żółw.rs", 3, Some(4));
        let start = line.find("src").unwrap();
        assert_eq!(link.range.start, start);
        assert_eq!(underlined(line, &link), "src/żółw.rs:3:4");

        let line = "  File \"/tmp/zażółć.py\", line 2 — ü";
        let link = check(line, "/tmp/zażółć.py", 2, None);
        assert_eq!(underlined(line, &link), "/tmp/zażółć.py\", line 2");
    }

    // -- Rust -------------------------------------------------------------

    #[test]
    fn rust() {
        let line = "  --> src/x.rs:1:2";
        let link = check(line, "src/x.rs", 1, Some(2));
        assert_eq!(link.range, 6..18);

        let line = "thread 'main' panicked at src/x.rs:9:10:";
        let link = check(line, "src/x.rs", 9, Some(10));
        assert_eq!(underlined(line, &link), "src/x.rs:9:10");

        check(
            "             at ./src/main.rs:5:5",
            "./src/main.rs",
            5,
            Some(5),
        );
        check(
            "             at /rustc/abc123/library/core/src/panicking.rs:75:14",
            "/rustc/abc123/library/core/src/panicking.rs",
            75,
            Some(14),
        );
        check(
            "warning: unused variable: `x`  --> crates/a/src/lib.rs:10:9",
            "crates/a/src/lib.rs",
            10,
            Some(9),
        );
    }

    // -- Python -----------------------------------------------------------

    #[test]
    fn python() {
        let line = "  File \"x.py\", line 12, in f";
        let link = check(line, "x.py", 12, None);
        assert_eq!(underlined(line, &link), "x.py\", line 12");
        assert_eq!(link.range, 8..22);

        check(r#"File "C:\x\y.py", line 3"#, r"C:\x\y.py", 3, None);
        let line = r#"  File "/home/me/my project/app.py", line 7, in <module>"#;
        let link = check(line, "/home/me/my project/app.py", 7, None);
        assert_eq!(
            underlined(line, &link),
            "/home/me/my project/app.py\", line 7"
        );
        none("  File \"<stdin>\", line 1, in <module>");
        none("  File \"<frozen importlib._bootstrap>\", line 1007, in _find_and_load");
    }

    // -- JVM --------------------------------------------------------------

    fn java(line: &str, file: &str, number: u32, package: &str) {
        let link = check(line, file, number, None);
        assert_eq!(
            link.kind,
            LinkKind::JavaFrame {
                package: package.into()
            },
            "{line:?}"
        );
        assert_eq!(underlined(line, &link), format!("{file}:{number}"));
    }

    #[test]
    fn java_and_kotlin_frames() {
        java(
            "\tat com.acme.Foo.bar(Foo.java:42)",
            "Foo.java",
            42,
            "com.acme",
        );
        java("at com.acme.FooKt.main(Foo.kt:7)", "Foo.kt", 7, "com.acme");
        java(
            "\tat com.acme.Foo$Inner.lambda$run$0(Foo.java:12)",
            "Foo.java",
            12,
            "com.acme",
        );
        java(
            "\tat java.base/java.lang.Thread.run(Thread.java:833)",
            "Thread.java",
            833,
            "java.lang",
        );
        java("    at Main.main(Main.java:3)", "Main.java", 3, "");
        java("\tat foo.bar.baz.qux(baz.java:3)", "baz.java", 3, "foo.bar");
        java(
            "\tat org.junit.Assert.fail(Assert.java:89) ~[junit-4.13.jar:4.13]",
            "Assert.java",
            89,
            "org.junit",
        );
        let line = "\tat com.acme.Foo.bar(Foo.java:42)";
        assert_eq!(one(line).range, 21..32);
    }

    #[test]
    fn java_frames_without_a_location() {
        none("\tat java.base/jdk.internal.reflect.NativeMethodAccessorImpl.invoke0(Native Method)");
        none("\tat com.acme.Foo.bar(Unknown Source)");
        none("\tat com.acme.Foo.bar(Foo.java)");
        none("Exception in thread \"main\" java.lang.IllegalStateException: boom");
    }

    // -- JS / TS ----------------------------------------------------------

    #[test]
    fn javascript() {
        let line = "    at fn (/abs/x.ts:1:2)";
        let link = check(line, "/abs/x.ts", 1, Some(2));
        assert_eq!(underlined(line, &link), "/abs/x.ts:1:2");
        check("    at /abs/x.js:3:4", "/abs/x.js", 3, Some(4));
        check(
            r"    at Object.<anonymous> (C:\x\y.js:3:4)",
            r"C:\x\y.js",
            3,
            Some(4),
        );
        check("./src/x.tsx:3:4", "./src/x.tsx", 3, Some(4));
        check("ERROR in ./src/App.tsx:12:7", "./src/App.tsx", 12, Some(7));
    }

    #[test]
    fn file_urls() {
        let line = "    at file:///C:/x/y.ts:3:4";
        let link = check(line, "C:/x/y.ts", 3, Some(4));
        assert_eq!(underlined(line, &link), "file:///C:/x/y.ts:3:4");

        let line = "    at async file:///home/u/app.mjs:10:5";
        let link = check(line, "/home/u/app.mjs", 10, Some(5));
        assert_eq!(underlined(line, &link), "file:///home/u/app.mjs:10:5");

        check(
            "at main (file:///C:/My%20Dir/y.ts:3:4)",
            "C:/My Dir/y.ts",
            3,
            Some(4),
        );
    }

    #[test]
    fn node_internals_are_not_links() {
        none("    at node:internal/main/run_main_module:28:49");
        none("    at Module._compile (node:internal/modules/cjs/loader:1105:14)");
        none("    at Module._compile (node:internal/modules/cjs/loader.js:1105:14)");
        none("    at process.processTicksAndRejections (node:internal/process/task_queues:95:5)");
    }

    // -- Go ---------------------------------------------------------------

    #[test]
    fn go() {
        let line = "\t/abs/x.go:12 +0x1d";
        let link = check(line, "/abs/x.go", 12, None);
        assert_eq!(underlined(line, &link), "/abs/x.go:12");
        check("    x_test.go:12: message", "x_test.go", 12, None);
        check("./main.go:5:2: undefined: foo", "./main.go", 5, Some(2));
        check(
            "\t/usr/local/go/src/runtime/panic.go:770 +0x132",
            "/usr/local/go/src/runtime/panic.go",
            770,
            None,
        );
    }

    // -- .NET / MSBuild -----------------------------------------------------

    #[test]
    fn dotnet_stack_traces() {
        let line = r"   at A.B() in C:\x\y.cs:line 42";
        let link = check(line, r"C:\x\y.cs", 42, None);
        assert_eq!(underlined(line, &link), r"C:\x\y.cs:line 42");

        let line = r"   at App.Program.Main(String[] args) in C:\My Projects\y.cs:line 42";
        let link = check(line, r"C:\My Projects\y.cs", 42, None);
        assert_eq!(underlined(line, &link), r"C:\My Projects\y.cs:line 42");

        check(
            "   at Svc.Run() in /home/u/src/Svc.cs:line 7",
            "/home/u/src/Svc.cs",
            7,
            None,
        );
    }

    #[test]
    fn msbuild_and_tsc() {
        let line = r"C:\x\y.cs(12,5): error CS1002: ; expected";
        let link = check(line, r"C:\x\y.cs", 12, Some(5));
        assert_eq!(underlined(line, &link), r"C:\x\y.cs(12,5)");

        let line = "y.cs(12): warning CS0168: unused";
        let link = check(line, "y.cs", 12, None);
        assert_eq!(link.range, 0..8);

        let line = "src/x.ts(3,4): error TS2322: Type 'string' is not assignable";
        let link = check(line, "src/x.ts", 3, Some(4));
        assert_eq!(underlined(line, &link), "src/x.ts(3,4)");

        check(
            r"  Program.cs(7,13): error CS0103: name 'x' [C:\p\App.csproj]",
            "Program.cs",
            7,
            Some(13),
        );
        none("let m = Math.max(1,2);");
    }

    // -- gcc / clang --------------------------------------------------------

    #[test]
    fn gcc_and_clang() {
        let line = "x.c:3:4: error: expected ';' before '}' token";
        let link = check(line, "x.c", 3, Some(4));
        assert_eq!(link.range, 0..7);
        check("src/a.cpp:10:2: warning: unused", "src/a.cpp", 10, Some(2));
        let line = "In file included from inc/a.h:3,";
        let link = check(line, "inc/a.h", 3, None);
        assert_eq!(underlined(line, &link), "inc/a.h:3");
        check("                 from main.c:1:", "main.c", 1, None);
    }

    // -- false positives ------------------------------------------------------

    #[test]
    fn urls_are_not_links() {
        none("http://host:8080");
        none("https://example.com:443/a.js:1");
        none("Listening on http://localhost:3000/app.js:1 now");
        none("see (https://example.com/x.rs:3)");
    }

    #[test]
    fn times_versions_and_hosts_are_not_links() {
        none("12:30:45");
        none("[12:30:45] Starting");
        none("2026-11-27T12:30:45.123Z done");
        none("v1.2.3");
        none("   Compiling serde v1.0.228");
        none("1.2.3:4");
        none("localhost:3000");
        none("127.0.0.1:8080");
        none("connect to example.com:443 failed");
        none("Listening on [::1]:8080");
        none("fe80::1:8080");
        none("impl std::fmt::Display for X");
        none("ratio 3:4");
        none("Note:3 items");
        none("src/main.rs");
        none("loaded from [junit-4.13.jar:4.13]");
    }

    // -- resolve_link ---------------------------------------------------------

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("ferro_links_{}_{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn file(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "x").unwrap();
            normalize(&path)
        }

        fn path(&self, relative: &str) -> PathBuf {
            self.0.join(relative)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn path_link(path: &str, line: Option<u32>, column: Option<u32>) -> Link {
        Link {
            range: 0..0,
            path: path.into(),
            line,
            column,
            kind: LinkKind::Path,
        }
    }

    #[test]
    fn resolves_relative_to_cwd() {
        let dir = TempDir::new();
        let file = dir.file("src/x.rs");
        let link = one("  --> src/x.rs:3:4");
        assert_eq!(
            resolve_link(&link, Some(&dir.0), &[]),
            Some((file.clone(), 2, 3))
        );
        let link = one("at ./src/x.rs:1");
        assert_eq!(
            resolve_link(&link, Some(&dir.0), &[]),
            Some((file.clone(), 0, 0))
        );
        if cfg!(windows) {
            let link = one(r".\src\x.rs:7");
            assert_eq!(resolve_link(&link, Some(&dir.0), &[]), Some((file, 6, 0)));
        }
    }

    #[test]
    fn resolves_relative_to_a_root_when_cwd_misses() {
        let dir = TempDir::new();
        let file = dir.file("app/main.go");
        std::fs::create_dir_all(dir.path("other")).unwrap();
        let link = one("./app/main.go:5:2: undefined: foo");
        let roots = [dir.path("nothing"), dir.0.clone()];
        assert_eq!(
            resolve_link(&link, Some(&dir.path("other")), &roots),
            Some((file, 4, 1))
        );
    }

    #[test]
    fn resolves_absolute_paths() {
        let dir = TempDir::new();
        let file = dir.file("abs/y.py");
        let link = path_link(&file.to_string_lossy(), Some(12), None);
        assert_eq!(resolve_link(&link, None, &[]), Some((file, 11, 0)));
    }

    #[test]
    fn missing_files_are_none() {
        let dir = TempDir::new();
        let link = path_link("src/nope.rs", Some(1), Some(1));
        assert_eq!(
            resolve_link(&link, Some(&dir.0), std::slice::from_ref(&dir.0)),
            None
        );
        let absolute = dir.path("nope.rs");
        let link = path_link(&absolute.to_string_lossy(), Some(1), None);
        assert_eq!(
            resolve_link(&link, Some(&dir.0), std::slice::from_ref(&dir.0)),
            None
        );
        let link = path_link(
            "/rustc/abc123/library/core/src/panicking.rs",
            Some(75),
            Some(14),
        );
        assert_eq!(resolve_link(&link, Some(&dir.0), &[]), None);
    }

    #[test]
    fn line_and_column_saturate() {
        let dir = TempDir::new();
        let file = dir.file("a.rs");
        let link = path_link("a.rs", Some(0), None);
        assert_eq!(resolve_link(&link, Some(&dir.0), &[]), Some((file, 0, 0)));
    }

    #[test]
    fn java_frames_prefer_the_package_directory() {
        let dir = TempDir::new();
        // `aaa` is walked before `src`.
        dir.file("mod/aaa/Foo.java");
        let wanted = dir.file("mod/src/main/java/com/acme/Foo.java");
        let link = one("\tat com.acme.Foo.bar(Foo.java:42)");
        assert_eq!(
            resolve_link(&link, None, std::slice::from_ref(&dir.0)),
            Some((wanted, 41, 0))
        );
    }

    #[test]
    fn java_frames_fall_back_to_the_file_name() {
        let dir = TempDir::new();
        let first = dir.file("mod/aaa/Foo.java");
        dir.file("mod/src/com/acme/Foo.java");
        let link = one("\tat org.other.Foo.bar(Foo.java:3)");
        assert_eq!(
            resolve_link(&link, None, std::slice::from_ref(&dir.0)),
            Some((first, 2, 0))
        );
        // Without roots the working directory is searched.
        assert!(resolve_link(&link, Some(&dir.0), &[]).is_some());
    }

    #[test]
    fn java_search_skips_build_and_hidden_directories() {
        let dir = TempDir::new();
        dir.file("node_modules/com/acme/Bar.java");
        dir.file("target/com/acme/Bar.java");
        dir.file("build/com/acme/Bar.java");
        dir.file("out/Bar.java");
        dir.file(".git/com/acme/Bar.java");
        dir.file(".hidden/Bar.java");
        let link = one("\tat com.acme.Bar.run(Bar.java:1)");
        assert_eq!(
            resolve_link(&link, None, std::slice::from_ref(&dir.0)),
            None
        );
        let found = dir.file("src/com/acme/Bar.java");
        assert_eq!(
            resolve_link(&link, None, std::slice::from_ref(&dir.0)),
            Some((found, 0, 0))
        );
    }
}
