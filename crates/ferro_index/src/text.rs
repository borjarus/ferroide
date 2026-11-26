//! Text search over files (Find in Files) with ripgrep's libraries, file
//! masks, and replacement.
//!
//! Files are searched in parallel: workers take files in path order from a
//! shared cursor and stop taking new ones once the limit is reached, so the
//! files searched are always a prefix of the list and the first `limit`
//! matches in path order are exact.

use std::{
    fs,
    ops::Range,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

use anyhow::{Context as _, Result, bail};
use ferro_services::{TextMatch, TextQuery};
use globset::{GlobBuilder, GlobMatcher};
use grep_matcher::Matcher as _;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, SearcherBuilder, sinks::Lossy};
use regex::{Regex, RegexBuilder};

/// Files above this size are skipped by text search (generated or binary).
pub const MAX_SEARCH_FILE: u64 = 8 * 1024 * 1024;

/// The regular expression a query stands for: literal text is escaped,
/// whole words get word boundaries.
pub fn query_pattern(query: &TextQuery) -> String {
    let mut pattern = if query.regex {
        query.text.clone()
    } else {
        regex::escape(&query.text)
    };
    if query.whole_word {
        pattern = format!(r"\b(?:{pattern})\b");
    }
    pattern
}

/// [`query_pattern`] compiled with the query's case rule.
pub fn query_regex(query: &TextQuery) -> Result<Regex> {
    RegexBuilder::new(&query_pattern(query))
        .case_insensitive(!query.case_sensitive)
        .multi_line(true)
        .build()
        .context("invalid regular expression")
}

// ---------------------------------------------------------------------------
// File masks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Rule {
    Glob(GlobMatcher),
    Contains(String),
}

impl Rule {
    fn parse(text: &str) -> Result<Self> {
        if !text.contains(['*', '?', '[', '{']) {
            return Ok(Rule::Contains(text.replace('\\', "/")));
        }
        let text = text.replace('\\', "/");
        // Without a separator a glob names files anywhere (`*.rs`).
        let pattern = if text.contains('/') {
            text.trim_start_matches('/').to_string()
        } else {
            format!("**/{text}")
        };
        let glob = GlobBuilder::new(&pattern)
            .literal_separator(true)
            .case_insensitive(cfg!(windows))
            .build()
            .with_context(|| format!("invalid file mask `{text}`"))?;
        Ok(Rule::Glob(glob.compile_matcher()))
    }

    fn matches(&self, relative: &str) -> bool {
        match self {
            Rule::Glob(glob) => glob.is_match(relative),
            Rule::Contains(text) => relative.contains(text.as_str()),
        }
    }
}

/// A parsed file mask: `*.rs, src/**, !target/**`. An empty mask matches
/// every file.
#[derive(Debug, Clone, Default)]
pub struct FileMask {
    include: Vec<Rule>,
    exclude: Vec<Rule>,
}

impl FileMask {
    pub fn parse(mask: &str) -> Result<Self> {
        let mut out = FileMask::default();
        // Commas inside `{a,b}` belong to the glob.
        let mut parts = Vec::new();
        let (mut depth, mut start) = (0usize, 0);
        for (i, c) in mask.char_indices() {
            match c {
                '{' => depth += 1,
                '}' => depth = depth.saturating_sub(1),
                ',' | ';' if depth == 0 => {
                    parts.push(&mask[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        parts.push(&mask[start..]);
        for part in parts.into_iter().map(str::trim).filter(|p| !p.is_empty()) {
            match part.strip_prefix('!') {
                Some(rest) if !rest.trim().is_empty() => {
                    out.exclude.push(Rule::parse(rest.trim())?)
                }
                Some(_) => bail!("`!` needs a pattern after it"),
                None => out.include.push(Rule::parse(part)?),
            }
        }
        Ok(out)
    }

    pub fn is_empty(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty()
    }

    /// Whether a file (its `/`-separated relative path) passes the mask.
    pub fn matches(&self, relative: &str) -> bool {
        (self.include.is_empty() || self.include.iter().any(|r| r.matches(relative)))
            && !self.exclude.iter().any(|r| r.matches(relative))
    }
}

// ---------------------------------------------------------------------------
// Searching
// ---------------------------------------------------------------------------

/// Read the file into `buf` (reused between files), or fail when it is
/// too large to search.
fn read_bounded(path: &std::path::Path, buf: &mut Vec<u8>) -> std::io::Result<()> {
    use std::io::Read as _;
    let mut file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    if len > MAX_SEARCH_FILE {
        return Err(std::io::Error::other("too large"));
    }
    buf.clear();
    buf.reserve(len as usize + 1);
    file.read_to_end(buf)?;
    Ok(())
}

/// A file to search and how results show it.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub path: PathBuf,
    pub shown: String,
}

/// Lines matching `query` in `files` (in their order), at most `limit`.
pub fn search_files(
    files: &[Candidate],
    query: &TextQuery,
    limit: usize,
) -> Result<Vec<TextMatch>> {
    if query.text.is_empty() || files.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    // Validate with the `regex` crate first: its message is the one users
    // see elsewhere (preview, replace), and both use the same syntax.
    query_regex(query)?;
    // Most files of a large project do not match: one pass of the regex
    // over the whole file rejects them faster than the line-oriented
    // searcher, which then runs only on files that do match.
    let pattern = query_pattern(query);
    let prefilter = regex::bytes::RegexBuilder::new(&pattern)
        .case_insensitive(!query.case_sensitive)
        // Line anchors need multi-line mode, which costs the literal
        // optimizations; only patterns with anchors pay for it.
        .multi_line(pattern.contains(['^', '$']))
        .build()
        .context("invalid regular expression")?;
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(!query.case_sensitive)
        .line_terminator(Some(b'\n'))
        .build(&query_pattern(query))
        .context("invalid regular expression")?;

    let cursor = AtomicUsize::new(0);
    let found = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, Vec<TextMatch>)>> = Mutex::new(Vec::new());
    let threads = thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 16)
        .min(files.len());

    thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                let mut searcher = SearcherBuilder::new()
                    .binary_detection(BinaryDetection::quit(0))
                    .line_number(true)
                    .build();
                let mut local = Vec::new();
                let mut bytes = Vec::new();
                loop {
                    if found.load(Ordering::Relaxed) >= limit {
                        break;
                    }
                    let ix = cursor.fetch_add(1, Ordering::Relaxed);
                    let Some(file) = files.get(ix) else { break };
                    // Reading the whole file and searching the slice is
                    // several times faster than the searcher's streaming
                    // reader, and the files are bounded by MAX_SEARCH_FILE.
                    if read_bounded(&file.path, &mut bytes).is_err() {
                        continue;
                    }
                    if !prefilter.is_match(&bytes) {
                        continue;
                    }
                    let mut lines = Vec::new();
                    let sink = Lossy(|number, line: &str| {
                        let line = line.trim_end_matches(['\n', '\r']);
                        let mut ranges: Vec<Range<usize>> = Vec::new();
                        let _ = matcher.find_iter(line.as_bytes(), |m| {
                            if m.start() < m.end() && m.end() <= line.len() {
                                ranges.push(m.start()..m.end());
                            }
                            true
                        });
                        if !ranges.is_empty() {
                            lines.push(TextMatch {
                                path: file.path.clone(),
                                relative: file.shown.clone(),
                                line: number.saturating_sub(1) as u32,
                                line_text: line.to_string(),
                                ranges,
                            });
                        }
                        Ok(true)
                    });
                    if searcher.search_slice(&matcher, &bytes, sink).is_err() {
                        continue;
                    }
                    if !lines.is_empty() {
                        found.fetch_add(lines.len(), Ordering::Relaxed);
                        local.push((ix, lines));
                    }
                }
                results
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .append(&mut local);
            });
        }
    });

    let mut results = results.into_inner().unwrap_or_else(|e| e.into_inner());
    results.sort_by_key(|(ix, _)| *ix);
    let mut out: Vec<TextMatch> = results.into_iter().flat_map(|(_, lines)| lines).collect();
    out.truncate(limit);
    Ok(out)
}

// ---------------------------------------------------------------------------
// Replacing
// ---------------------------------------------------------------------------

/// Replaces the matches of a query. In regex mode the replacement may name
/// groups (`$1`, `${name}`); otherwise it is inserted literally.
#[derive(Debug, Clone)]
pub struct Replacer {
    regex: Regex,
    replacement: String,
    expand: bool,
}

impl Replacer {
    pub fn new(query: &TextQuery, replacement: &str) -> Result<Self> {
        Ok(Self {
            regex: query_regex(query)?,
            replacement: replacement.to_string(),
            expand: query.regex,
        })
    }

    /// `text` with every match replaced, and the number of replacements.
    pub fn apply(&self, text: &str) -> (String, usize) {
        let count = self.regex.find_iter(text).filter(|m| !m.is_empty()).count();
        if count == 0 {
            return (text.to_string(), 0);
        }
        let out = if self.expand {
            self.regex.replace_all(text, self.replacement.as_str())
        } else {
            self.regex
                .replace_all(text, regex::NoExpand(self.replacement.as_str()))
        };
        (out.into_owned(), count)
    }

    /// The text that replaces the match at `range` of `line` (for the
    /// preview of one match).
    pub fn replacement_for(&self, line: &str, range: Range<usize>) -> String {
        if !self.expand {
            return self.replacement.clone();
        }
        let mut out = String::new();
        match self.regex.captures_at(line, range.start) {
            Some(caps) if caps.get(0).is_some_and(|m| m.start() == range.start) => {
                caps.expand(&self.replacement, &mut out);
            }
            _ => out.push_str(&self.replacement),
        }
        out
    }

    /// `line` with the matches at `ranges` replaced (the preview row).
    pub fn preview_line(&self, line: &str, ranges: &[Range<usize>]) -> String {
        let mut out = String::with_capacity(line.len());
        let mut last = 0;
        for r in ranges {
            if r.start < last || r.end > line.len() {
                continue;
            }
            out.push_str(&line[last..r.start]);
            out.push_str(&self.replacement_for(line, r.clone()));
            last = r.end;
        }
        out.push_str(&line[last..]);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(text: &str) -> TextQuery {
        TextQuery {
            text: text.into(),
            ..TextQuery::default()
        }
    }

    #[test]
    fn masks_take_globs_text_and_exclusions() {
        let mask = FileMask::parse("*.rs, !target/**").unwrap();
        assert!(mask.matches("src/main.rs"));
        assert!(mask.matches("main.rs"));
        assert!(!mask.matches("src/main.ts"));
        assert!(!mask.matches("target/debug/build.rs"));
        let mask = FileMask::parse("src/").unwrap();
        assert!(mask.matches("src/a.txt"));
        assert!(!mask.matches("docs/a.txt"));
        let mask = FileMask::parse("src/**/*.{ts,tsx}; !*.test.ts").unwrap();
        assert!(mask.matches("src/ui/app.tsx"));
        assert!(!mask.matches("src/ui/app.test.ts"));
        assert!(FileMask::parse("").unwrap().is_empty());
        assert!(FileMask::parse("!").is_err());
        assert!(FileMask::parse("src/[a").is_err());
    }

    #[test]
    fn literal_replacement_is_not_expanded() {
        let replacer = Replacer::new(&query("a.b"), "$1").unwrap();
        assert_eq!(replacer.apply("a.b axb a.b"), ("$1 axb $1".into(), 2));
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn regex_replacement_uses_groups() {
        let q = TextQuery {
            text: r"(\w+)\s*=\s*(\d+)".into(),
            regex: true,
            ..TextQuery::default()
        };
        let replacer = Replacer::new(&q, "$2 -> $1").unwrap();
        assert_eq!(replacer.apply("x = 1; y=22").0, "1 -> x; 22 -> y");
        assert_eq!(
            replacer.preview_line("x = 1; y=22", &[7..11]),
            "x = 1; 22 -> y"
        );
    }

    #[test]
    fn whole_words_and_case() {
        let q = TextQuery {
            text: "foo".into(),
            whole_word: true,
            case_sensitive: true,
            ..TextQuery::default()
        };
        let replacer = Replacer::new(&q, "bar").unwrap();
        assert_eq!(replacer.apply("foo food Foo foo").0, "bar food Foo bar");
    }

    #[test]
    fn search_finds_lines_in_path_order_and_honors_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let mut files = Vec::new();
        for i in 0..40 {
            let path = dir.path().join(format!("f{i:02}.txt"));
            fs::write(&path, format!("alpha\nneedle {i}\r\nomega needle needle\n")).unwrap();
            files.push(Candidate {
                path,
                shown: format!("f{i:02}.txt"),
            });
        }
        fs::write(dir.path().join("bin.dat"), b"needle\0\0binary").unwrap();
        files.push(Candidate {
            path: dir.path().join("bin.dat"),
            shown: "bin.dat".into(),
        });
        let all = search_files(&files, &query("NEEDLE"), 10_000).unwrap();
        assert_eq!(all.len(), 80, "binary files are skipped");
        assert_eq!(all[0].relative, "f00.txt");
        assert_eq!(all[0].line, 1);
        assert_eq!(all[0].line_text, "needle 0", "line breaks are trimmed");
        assert_eq!(all[1].ranges, vec![6..12, 13..19]);
        let first = search_files(&files, &query("needle"), 5).unwrap();
        assert_eq!(first, all[..5].to_vec());
        let bad = TextQuery {
            text: "(".into(),
            regex: true,
            ..TextQuery::default()
        };
        assert!(search_files(&files, &bad, 5).is_err());
    }
}
