//! Bounded project walks, shared by build tool discovery, test location
//! and report collection.
//!
//! `ignore`'s walker respects `.gitignore`, `.ignore` and hidden files;
//! on top of that, dependency and output folders are skipped by name even
//! when nothing ignores them (a fresh clone before `git init`, a vendored
//! tree), and the walk stops after `max_files` matches so a huge
//! repository cannot stall a run.

use std::path::{Path, PathBuf};

/// Folders never worth descending into.
pub const SKIPPED: &[&str] = &[
    "target",
    "node_modules",
    ".git",
    "build",
    "dist",
    "out",
    "obj",
    ".venv",
    "venv",
    "__pycache__",
    ".gradle",
    ".idea",
    ".vs",
    ".mvn",
    ".next",
    ".tox",
];

/// Files under `root` (at most `max_depth` levels down) for which `keep`
/// holds, at most `max_files` of them, in a stable order. `skipped` are
/// folder names not descended into; `ignores` whether `.gitignore` and
/// hidden files are respected (report folders are usually ignored).
pub fn files(
    root: &Path,
    max_depth: usize,
    max_files: usize,
    skipped: &'static [&'static str],
    ignores: bool,
    mut keep: impl FnMut(&Path) -> bool,
) -> Vec<PathBuf> {
    let walker = ignore::WalkBuilder::new(root)
        .max_depth(Some(max_depth))
        .standard_filters(ignores)
        .require_git(false)
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(move |entry| {
            entry.depth() == 0
                || !entry.file_type().is_some_and(|t| t.is_dir())
                || !entry
                    .file_name()
                    .to_str()
                    .is_some_and(|n| skipped.contains(&n))
        })
        .build();
    let mut out = Vec::new();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if keep(entry.path()) {
            out.push(entry.into_path());
            if out.len() >= max_files {
                tracing::debug!(root = %root.display(), "walk stopped at {max_files} files");
                break;
            }
        }
    }
    out
}

/// Files with one of `extensions` (without dots) under `root`.
pub fn sources(root: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    files(root, 24, 50_000, SKIPPED, true, |p| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| extensions.contains(&e))
    })
}
