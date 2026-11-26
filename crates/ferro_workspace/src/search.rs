//! Navigation and search in the window (plan §8, Stage 7): options of the
//! project index, bookmarks, and the history of Replace in Files that
//! `ferro::UndoReplaceInFiles` reverts in one operation.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use ferro_db::Bookmark;
use ferro_index::{IndexOptions, TrigramMode};
use ferro_services::TextPos;
use ferro_settings::TrigramIndex;
use ferro_ui::{tr, tr_args};
use gpui_kit::{
    App, Context, EntityId, Global, Window,
    component::{WindowExt as _, notification::Notification},
};

use crate::{
    app_state::AppState, editor::providers::same_path, requests::OpenLocation, workspace::Workspace,
};

/// Replace operations kept for undo, per window.
const REPLACE_HISTORY: usize = 10;

/// The project index of a project on disk: stored under `<data>/index`,
/// trigrams as `[search] trigram_index` says.
pub fn index_options(roots: &[PathBuf], cx: &App) -> IndexOptions {
    let state = AppState::global(cx);
    let trigrams = match state.settings.search.trigram_index {
        TrigramIndex::Auto => TrigramMode::Auto,
        TrigramIndex::On => TrigramMode::On,
        TrigramIndex::Off => TrigramMode::Off,
    };
    IndexOptions {
        db_path: state
            .persist
            .then(|| ferro_index::db_path_for(&state.paths.data_dir, roots)),
        trigrams,
        ..IndexOptions::default()
    }
}

// ---------------------------------------------------------------------------
// Bookmarks
// ---------------------------------------------------------------------------

/// Bookmarks per project key, read from the state database once.
#[derive(Default)]
struct BookmarkStore(HashMap<String, Vec<Bookmark>>);
impl Global for BookmarkStore {}

fn load(project: &str, cx: &mut App) {
    if cx
        .try_global::<BookmarkStore>()
        .is_some_and(|s| s.0.contains_key(project))
    {
        return;
    }
    let marks = AppState::global(cx)
        .db
        .bookmarks(project)
        .unwrap_or_else(|err| {
            tracing::warn!("could not read bookmarks: {err:#}");
            Vec::new()
        });
    cx.default_global::<BookmarkStore>()
        .0
        .insert(project.to_string(), marks);
}

/// Every bookmark of a project, by file and line.
pub fn bookmarks(project: &str, cx: &mut App) -> Vec<Bookmark> {
    load(project, cx);
    cx.global::<BookmarkStore>().0[project].clone()
}

/// Bookmarked lines of one file (for the editor's glyph margin). Empty
/// until the project's bookmarks were loaded by the window.
pub fn bookmarked_lines(project: &str, path: &Path, cx: &App) -> Vec<u32> {
    cx.try_global::<BookmarkStore>()
        .and_then(|s| s.0.get(project))
        .map(|marks| {
            marks
                .iter()
                .filter(|b| same_path(&b.path, path))
                .map(|b| b.line)
                .collect()
        })
        .unwrap_or_default()
}

/// Load a window's bookmarks (at startup).
pub fn init_bookmarks(project: &str, cx: &mut App) {
    load(project, cx);
}

/// Bookmark the caret's line of the active editor, or remove its bookmark.
pub fn toggle_bookmark(workspace: &mut Workspace, cx: &mut Context<Workspace>) -> Option<bool> {
    let editor = workspace.active_editor_entity()?;
    let (path, caret, text) = {
        let e = editor.read(cx);
        (e.path().to_path_buf(), e.caret(cx)?, e.text(cx)?)
    };
    let snippet = text.lines().nth(caret.line as usize).unwrap_or_default();
    let project = workspace.project_key().to_string();
    let added = match AppState::global(cx)
        .db
        .toggle_bookmark(&project, &path, caret.line, snippet)
    {
        Ok(added) => added,
        Err(err) => {
            tracing::warn!("could not change a bookmark: {err:#}");
            return None;
        }
    };
    if cx.has_global::<BookmarkStore>() {
        cx.global_mut::<BookmarkStore>().0.remove(&project);
    }
    load(&project, cx);
    editor.update(cx, |_, cx| cx.notify());
    Some(added)
}

/// The bookmark after (or before) `current` in file and line order,
/// wrapping around.
pub fn next_bookmark<'a>(
    marks: &'a [Bookmark],
    current: Option<(&Path, u32)>,
    forward: bool,
) -> Option<&'a Bookmark> {
    if marks.is_empty() {
        return None;
    }
    let mut sorted: Vec<&Bookmark> = marks.iter().collect();
    sorted.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
    let Some((path, line)) = current else {
        return Some(if forward {
            sorted[0]
        } else {
            sorted[sorted.len() - 1]
        });
    };
    let key = |b: &Bookmark| (b.path.clone(), b.line);
    let here = (path.to_path_buf(), line);
    if forward {
        sorted
            .iter()
            .find(|b| key(b) > here)
            .or(sorted.first())
            .copied()
    } else {
        sorted
            .iter()
            .rev()
            .find(|b| key(b) < here)
            .or(sorted.last())
            .copied()
    }
}

/// Go to the next or previous bookmark.
pub fn jump_to_bookmark(
    workspace: &mut Workspace,
    forward: bool,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let project = workspace.project_key().to_string();
    let marks = bookmarks(&project, cx);
    let current = workspace.active_editor_entity().and_then(|e| {
        let e = e.read(cx);
        Some((e.path().to_path_buf(), e.caret(cx)?.line))
    });
    let Some(target) = next_bookmark(
        &marks,
        current.as_ref().map(|(p, l)| (p.as_path(), *l)),
        forward,
    ) else {
        window.push_notification(Notification::info(tr("search-no-bookmarks")), cx);
        return;
    };
    let location = OpenLocation::at(target.path.clone(), TextPos::new(target.line, 0));
    workspace.open_location(&location, window, cx);
}

// ---------------------------------------------------------------------------
// Replace in Files: undo
// ---------------------------------------------------------------------------

/// A file changed by Replace in Files, with its text before and after.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplacedFile {
    pub path: PathBuf,
    pub before: String,
    pub after: String,
}

#[derive(Default)]
struct ReplaceHistory(HashMap<EntityId, Vec<Vec<ReplacedFile>>>);
impl Global for ReplaceHistory {}

/// Remember one Replace in Files operation of a window.
pub fn record_replace(workspace: EntityId, files: Vec<ReplacedFile>, cx: &mut App) {
    if files.is_empty() {
        return;
    }
    let history = cx
        .default_global::<ReplaceHistory>()
        .0
        .entry(workspace)
        .or_default();
    history.push(files);
    if history.len() > REPLACE_HISTORY {
        history.remove(0);
    }
}

/// Whether the window has a Replace in Files operation to undo.
pub fn can_undo_replace(workspace: EntityId, cx: &App) -> bool {
    cx.try_global::<ReplaceHistory>()
        .and_then(|h| h.0.get(&workspace))
        .is_some_and(|ops| !ops.is_empty())
}

/// Revert the window's last Replace in Files: every file whose text is
/// still what the replacement produced gets its old text back (open files
/// in their editors, as an undoable edit; the others on disk). Files
/// changed since are left alone and reported.
pub fn undo_replace(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let id = cx.entity_id();
    let Some(files) = cx
        .try_global::<ReplaceHistory>()
        .and_then(|h| h.0.get(&id))
        .and_then(|ops| ops.last().cloned())
    else {
        window.push_notification(Notification::info(tr("search-undo-nothing")), cx);
        return;
    };
    if let Some(ops) = cx.global_mut::<ReplaceHistory>().0.get_mut(&id) {
        ops.pop();
    }
    let mut restored = 0usize;
    let mut changed = 0usize;
    let mut on_disk = Vec::new();
    for file in files {
        let editor = workspace
            .editors
            .iter()
            .find(|e| same_path(e.read(cx).path(), &file.path) && e.read(cx).is_loaded())
            .cloned();
        match editor {
            Some(editor) => {
                if editor.read(cx).text(cx).as_deref() == Some(file.after.as_str()) {
                    editor.update(cx, |e, cx| e.replace_text(file.before, window, cx));
                    restored += 1;
                } else {
                    changed += 1;
                }
            }
            None => on_disk.push(file),
        }
    }
    let project = workspace.services.project.clone();
    cx.spawn_in(window, async move |_, cx| {
        for file in on_disk {
            let current = project.read_bytes(&file.path).await;
            match current {
                Ok(bytes) if bytes == file.after.as_bytes() => {
                    match project
                        .write_file(&file.path, file.before.into_bytes())
                        .await
                    {
                        Ok(()) => restored += 1,
                        Err(err) => {
                            tracing::warn!("undo of a replacement failed: {err:#}");
                            changed += 1;
                        }
                    }
                }
                _ => changed += 1,
            }
        }
        let _ = cx.update(|window, cx| {
            let mut message =
                tr_args("search-undo-done", &[("files", restored.into())]).to_string();
            if changed > 0 {
                message.push(' ');
                message.push_str(&tr_args(
                    "search-undo-changed",
                    &[("files", changed.into())],
                ));
            }
            window.push_notification(Notification::info(message), cx);
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(path: &str, line: u32) -> Bookmark {
        Bookmark {
            path: PathBuf::from(path),
            line,
            snippet: String::new(),
        }
    }

    #[test]
    fn bookmarks_cycle_in_file_and_line_order() {
        let marks = vec![mark("/p/b.rs", 4), mark("/p/a.rs", 10), mark("/p/a.rs", 2)];
        let next = |p: &str, l: u32, f: bool| {
            let b = next_bookmark(&marks, Some((Path::new(p), l)), f).unwrap();
            (b.path.to_string_lossy().into_owned(), b.line)
        };
        assert_eq!(next("/p/a.rs", 2, true), ("/p/a.rs".into(), 10));
        assert_eq!(next("/p/a.rs", 10, true), ("/p/b.rs".into(), 4));
        assert_eq!(next("/p/b.rs", 4, true), ("/p/a.rs".into(), 2), "wraps");
        assert_eq!(
            next("/p/a.rs", 2, false),
            ("/p/b.rs".into(), 4),
            "wraps back"
        );
        assert_eq!(next("/p/a.rs", 5, false), ("/p/a.rs".into(), 2));
        assert_eq!(next_bookmark(&marks, None, true).unwrap().line, 2);
        assert!(next_bookmark(&[], None, true).is_none());
    }
}
