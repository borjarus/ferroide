//! Search over a real project: files by name (Quick Open), text (Find in
//! Files) and TODO comments.
//!
//! Stage 4 searches the [`FileIndex`] directly, reading files on demand;
//! the persistent, incremental index of Stage 7 replaces it behind the same
//! `SearchService` contract.

use std::{fs, path::Path};

use anyhow::{Context as _, Result};
use ferro_services::{
    FileMatch, ProjectService, SearchService, TextMatch, TextQuery, TodoItem, TodoKind, fuzzy,
};
use futures::{FutureExt as _, future::BoxFuture};
use regex::{Regex, RegexBuilder};

use crate::{FsProjectService, IndexedFile};

/// Files above this size are skipped by text search (generated or binary).
const MAX_SEARCH_FILE: u64 = 4 * 1024 * 1024;
/// TODO collection stops here, so a vendored tree cannot stall the UI.
const MAX_TODOS: usize = 5_000;

pub struct FsSearchService {
    project: FsProjectService,
}

impl FsSearchService {
    pub fn new(project: FsProjectService) -> Self {
        Self { project }
    }
}

/// How a file is shown in results: relative to its root, prefixed with the
/// root's name when the project has several roots.
fn display_path(file: &IndexedFile, multi_root: bool) -> String {
    if multi_root {
        let name = file
            .root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        format!("{name}/{}", file.relative)
    } else {
        file.relative.clone()
    }
}

/// The text of a file if it looks like text and is not too large.
fn read_text(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    if meta.len() > MAX_SEARCH_FILE {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Build the regex for a query: literal text is escaped, whole words get
/// word boundaries.
pub fn query_regex(query: &TextQuery) -> Result<Regex> {
    let mut pattern = if query.regex {
        query.text.clone()
    } else {
        regex::escape(&query.text)
    };
    if query.whole_word {
        pattern = format!(r"\b(?:{pattern})\b");
    }
    RegexBuilder::new(&pattern)
        .case_insensitive(!query.case_sensitive)
        .multi_line(true)
        .build()
        .context("invalid regular expression")
}

fn todo_regex() -> Regex {
    Regex::new(r"\b(TODO|FIXME)\b:?\s*(.*)").expect("a valid pattern")
}

impl SearchService for FsSearchService {
    fn find_files(&self, query: &str, limit: usize) -> BoxFuture<'static, Result<Vec<FileMatch>>> {
        let index = self.project.index().clone();
        let multi_root = self.project.roots().len() > 1;
        let query = query.to_string();
        async move {
            index.ready().await;
            let files = index.snapshot();
            smol::unblock(move || {
                let shown: Vec<String> =
                    files.iter().map(|f| display_path(f, multi_root)).collect();
                let ranked = fuzzy::rank(&query, shown.iter().map(String::as_str), limit);
                Ok(ranked
                    .into_iter()
                    .map(|(ix, m)| FileMatch {
                        path: files[ix].path.clone(),
                        relative: shown[ix].clone(),
                        score: m.score,
                        positions: m.positions,
                    })
                    .collect())
            })
            .await
        }
        .boxed()
    }

    fn find_text(
        &self,
        query: TextQuery,
        limit: usize,
    ) -> BoxFuture<'static, Result<Vec<TextMatch>>> {
        let index = self.project.index().clone();
        let multi_root = self.project.roots().len() > 1;
        async move {
            if query.text.is_empty() {
                return Ok(Vec::new());
            }
            let regex = query_regex(&query)?;
            index.ready().await;
            let files = index.snapshot();
            smol::unblock(move || {
                let filter = query
                    .path_filter
                    .as_deref()
                    .map(str::trim)
                    .filter(|f| !f.is_empty())
                    .map(str::to_string);
                let mut out = Vec::new();
                for file in files.iter() {
                    let shown = display_path(file, multi_root);
                    if filter.as_deref().is_some_and(|f| !shown.contains(f)) {
                        continue;
                    }
                    let Some(text) = read_text(&file.path) else {
                        continue;
                    };
                    if !regex.is_match(&text) {
                        continue;
                    }
                    for (line_ix, line) in text.lines().enumerate() {
                        let ranges: Vec<_> = regex.find_iter(line).map(|m| m.range()).collect();
                        if ranges.is_empty() {
                            continue;
                        }
                        out.push(TextMatch {
                            path: file.path.clone(),
                            relative: shown.clone(),
                            line: line_ix as u32,
                            line_text: line.to_string(),
                            ranges,
                        });
                        if out.len() >= limit {
                            return Ok(out);
                        }
                    }
                }
                Ok(out)
            })
            .await
        }
        .boxed()
    }

    fn todos(&self) -> BoxFuture<'static, Result<Vec<TodoItem>>> {
        let index = self.project.index().clone();
        let multi_root = self.project.roots().len() > 1;
        async move {
            index.ready().await;
            let files = index.snapshot();
            smol::unblock(move || {
                let regex = todo_regex();
                let mut out = Vec::new();
                for file in files.iter() {
                    let Some(text) = read_text(&file.path) else {
                        continue;
                    };
                    if !text.contains("TODO") && !text.contains("FIXME") {
                        continue;
                    }
                    for (line_ix, line) in text.lines().enumerate() {
                        let Some(caps) = regex.captures(line) else {
                            continue;
                        };
                        let kind = if &caps[1] == "TODO" {
                            TodoKind::Todo
                        } else {
                            TodoKind::Fixme
                        };
                        out.push(TodoItem {
                            path: file.path.clone(),
                            relative: display_path(file, multi_root),
                            line: line_ix as u32,
                            kind,
                            text: caps[2].trim().trim_end_matches("*/").trim().to_string(),
                        });
                        if out.len() >= MAX_TODOS {
                            return Ok(out);
                        }
                    }
                }
                Ok(out)
            })
            .await
        }
        .boxed()
    }
}
