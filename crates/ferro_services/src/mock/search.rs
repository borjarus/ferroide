//! Search on mock data: fuzzy file names over the whole project (stress tree
//! included), literal text search, and TODO comments.

use std::{ops::Range, path::PathBuf, sync::Arc};

use anyhow::bail;
use futures::{FutureExt as _, future::BoxFuture};

use super::{Dice, MockConfig, MockProjectService, simulate};
use crate::{FileMatch, Result, SearchService, TextMatch, TextQuery, TodoItem, TodoKind, fuzzy};

pub struct MockSearchService {
    config: MockConfig,
    dice: Dice,
    project: Arc<MockProjectService>,
}

impl MockSearchService {
    pub fn new(config: MockConfig, project: Arc<MockProjectService>) -> Self {
        Self {
            dice: Dice::new(config.seed ^ 0x5ea7c4),
            config,
            project,
        }
    }

    fn should_fail(&self) -> bool {
        self.config.failure_rate > 0.0 && self.dice.roll() < self.config.failure_rate
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Byte ranges of `needle` in `line`, compared char by char so case folding
/// never shifts offsets.
pub(super) fn find_matches(
    line: &str,
    needle: &str,
    case_sensitive: bool,
    whole_word: bool,
) -> Vec<Range<usize>> {
    let needle: Vec<char> = needle.chars().collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let same =
        |a: char, b: char| a == b || (!case_sensitive && a.to_lowercase().eq(b.to_lowercase()));
    let mut out = Vec::new();
    let mut i = 0;
    while i + needle.len() <= chars.len() {
        let hit = needle
            .iter()
            .enumerate()
            .all(|(k, &n)| same(chars[i + k].1, n));
        if hit {
            let end_ix = i + needle.len();
            let before_ok = i == 0 || !is_word_char(chars[i - 1].1);
            let after_ok = end_ix == chars.len() || !is_word_char(chars[end_ix].1);
            if !whole_word || (before_ok && after_ok) {
                let end = chars.get(end_ix).map(|(b, _)| *b).unwrap_or(line.len());
                out.push(chars[i].0..end);
                i = end_ix;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// TODO/FIXME markers in comments.
pub(super) fn todo_in_line(line: &str) -> Option<(TodoKind, usize, String)> {
    let comment = ["//", "#", "<!--", "/*", "--"]
        .iter()
        .filter_map(|m| line.find(m))
        .min()?;
    let rest = &line[comment..];
    for (marker, kind) in [("TODO", TodoKind::Todo), ("FIXME", TodoKind::Fixme)] {
        if let Some(ix) = rest.find(marker) {
            let text = rest[ix + marker.len()..]
                .trim_start_matches([':', ' ', '('])
                .trim_end_matches("-->")
                .trim()
                .to_string();
            return Some((kind, comment + ix, text));
        }
    }
    None
}

const REGEX_META: &[char] = &[
    '.', '*', '+', '?', '(', ')', '[', ']', '{', '}', '|', '^', '$', '\\',
];

impl SearchService for MockSearchService {
    fn find_files(&self, query: &str, limit: usize) -> BoxFuture<'static, Result<Vec<FileMatch>>> {
        let fail = self.should_fail();
        let latency = self.config.latency;
        let project = self.project.clone();
        let query = query.to_string();
        async move {
            simulate(latency, fail, "searching files").await?;
            let demo: Vec<String> = project
                .demo_files()
                .into_iter()
                .filter_map(|(p, _)| project.relative(&p))
                .collect();
            let stress = project.stress_files();
            let candidates = demo.iter().chain(stress.iter());
            let ranked = fuzzy::rank(&query, candidates.clone().map(String::as_str), limit);
            let all: Vec<&String> = candidates.collect();
            Ok(ranked
                .into_iter()
                .map(|(ix, m)| FileMatch {
                    path: project.absolute(all[ix]),
                    relative: all[ix].clone(),
                    score: m.score,
                    positions: m.positions,
                })
                .collect())
        }
        .boxed()
    }

    fn find_text(
        &self,
        query: TextQuery,
        limit: usize,
    ) -> BoxFuture<'static, Result<Vec<TextMatch>>> {
        let fail = self.should_fail();
        let latency = self.config.latency * 2;
        let project = self.project.clone();
        async move {
            simulate(latency, fail, "searching text").await?;
            if query.regex && query.text.contains(REGEX_META) {
                bail!("regular expressions arrive in Stage 7");
            }
            if query.text.is_empty() {
                return Ok(Vec::new());
            }
            let filter = query
                .path_filter
                .as_deref()
                .map(str::trim)
                .filter(|f| !f.is_empty());
            let wanted = |rel: &str| filter.is_none_or(|f| rel.contains(f));

            let mut files: Vec<(PathBuf, String, String)> = project
                .demo_files()
                .into_iter()
                .filter_map(|(p, text)| project.relative(&p).map(|rel| (p, rel, text)))
                .filter(|(_, rel, _)| wanted(rel))
                .collect();
            let mut out = Vec::new();
            let scan = |path: PathBuf, rel: &str, text: &str, out: &mut Vec<TextMatch>| {
                for (line_ix, line) in text.lines().enumerate() {
                    if out.len() >= limit {
                        return;
                    }
                    let ranges =
                        find_matches(line, &query.text, query.case_sensitive, query.whole_word);
                    if !ranges.is_empty() {
                        out.push(TextMatch {
                            path: path.clone(),
                            relative: rel.to_string(),
                            line: line_ix as u32,
                            line_text: line.to_string(),
                            ranges,
                        });
                    }
                }
            };
            for (path, rel, text) in files.drain(..) {
                scan(path, &rel, &text, &mut out);
            }
            // The stress tree is searched only when asked for explicitly.
            if filter.is_some_and(|f| f.starts_with("stress")) {
                for rel in project.stress_files().iter().filter(|r| wanted(r)) {
                    if out.len() >= limit {
                        break;
                    }
                    let path = project.absolute(rel);
                    if let Some(text) = project.file_text(&path) {
                        scan(path, rel, &text, &mut out);
                    }
                }
            }
            Ok(out)
        }
        .boxed()
    }

    fn todos(&self) -> BoxFuture<'static, Result<Vec<TodoItem>>> {
        let fail = self.should_fail();
        let latency = self.config.latency;
        let project = self.project.clone();
        async move {
            simulate(latency, fail, "collecting TODOs").await?;
            let mut out = Vec::new();
            for (path, text) in project.demo_files() {
                let Some(rel) = project.relative(&path) else {
                    continue;
                };
                for (line_ix, line) in text.lines().enumerate() {
                    if let Some((kind, _, text)) = todo_in_line(line) {
                        out.push(TodoItem {
                            path: path.clone(),
                            relative: rel.clone(),
                            line: line_ix as u32,
                            kind,
                            text,
                        });
                    }
                }
            }
            Ok(out)
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block<T>(f: BoxFuture<'static, T>) -> T {
        smol::block_on(f)
    }

    fn search(stress_files: usize) -> MockSearchService {
        let config = MockConfig {
            stress_files,
            ..MockConfig::instant()
        };
        let project = Arc::new(MockProjectService::demo(config.clone()));
        MockSearchService::new(config, project)
    }

    #[test]
    fn find_files_ranks_demo_files_and_the_stress_tree() {
        let search = search(5_000);
        let hits = block(search.find_files("main", 10)).unwrap();
        assert_eq!(hits[0].relative, "src/main.rs");
        assert!(hits[0].path.ends_with("main.rs"));
        let stress = block(search.find_files("file0001", 20)).unwrap();
        assert!(!stress.is_empty());
        assert!(stress.iter().all(|m| m.relative.starts_with("stress/")));
        assert!(block(search.find_files("", 5)).unwrap().len() == 5);
        assert!(block(search.find_files("qqqqzzzz", 5)).unwrap().is_empty());
    }

    #[test]
    fn find_text_honors_case_whole_word_and_path_filter() {
        let search = search(0);
        let query = |text: &str| TextQuery {
            text: text.into(),
            ..TextQuery::default()
        };
        let hits = block(search.find_text(query("greet"), 100)).unwrap();
        assert!(hits.iter().any(|m| m.relative == "src/lib.rs"));
        assert!(hits.iter().any(|m| m.relative == "web/src/app.ts"));
        for m in &hits {
            for r in &m.ranges {
                assert!(m.line_text[r.clone()].eq_ignore_ascii_case("greet"));
            }
        }

        let whole = TextQuery {
            whole_word: true,
            ..query("greet")
        };
        let whole_hits = block(search.find_text(whole, 100)).unwrap();
        assert!(whole_hits.iter().all(|m| !m.line_text.contains("greets_")
            || m.ranges.iter().all(|r| &m.line_text[r.clone()] == "greet")));

        let sensitive = TextQuery {
            case_sensitive: true,
            ..query("HELLO")
        };
        assert!(block(search.find_text(sensitive, 100)).unwrap().is_empty());

        let filtered = TextQuery {
            path_filter: Some("web/".into()),
            ..query("greet")
        };
        let web = block(search.find_text(filtered, 100)).unwrap();
        assert!(!web.is_empty() && web.iter().all(|m| m.relative.starts_with("web/")));

        assert_eq!(block(search.find_text(query("greet"), 2)).unwrap().len(), 2);
        let regex = TextQuery {
            regex: true,
            ..query("gre+t")
        };
        assert!(block(search.find_text(regex, 10)).is_err());
    }

    #[test]
    fn matching_is_safe_on_non_ascii_text() {
        let line = "Zażółć gęślą jaźń — ZAŻÓŁĆ";
        let ranges = find_matches(line, "zażółć", false, true);
        assert_eq!(ranges.len(), 2);
        assert_eq!(&line[ranges[1].clone()], "ZAŻÓŁĆ");
    }

    #[test]
    fn todos_are_found_in_demo_files() {
        let search = search(0);
        let todos = block(search.todos()).unwrap();
        assert!(todos.iter().any(|t| t.relative == "src/config.rs"
            && t.kind == TodoKind::Todo
            && t.text == "retry with exponential backoff"));
        assert!(todos.iter().any(|t| t.kind == TodoKind::Fixme));
        assert_eq!(todo_in_line("let todo = 1; // nothing"), None);
    }
}
