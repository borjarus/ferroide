//! The Project tool window: a lazily loaded, virtualized file tree.
//!
//! Only expanded directories are fetched, and only visible rows are
//! rendered. Rows are a flat list of `Arc`-shared entries; expanding or
//! collapsing a directory splices its subtree in or out instead of
//! rebuilding the list, so trees with a million rows stay responsive.
//!
//! Keyboard: arrows, Home/End, Page Up/Down, Enter (open / toggle), F2
//! (rename), Delete. Typing starts a speed search (IntelliJ): matching rows
//! are highlighted, Up/Down jump between matches, Esc ends it.

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

use ferro_services::{ChangeKind, Entry, EntryKind, ProjectEvent, Services, fuzzy};
use ferro_ui::{FerroIcon, tr, tr_args};
use gpui_kit::{
    AnyElement, App, AppContext as _, ClickEvent, ClipboardItem, Context, Entity, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, ParentElement as _,
    Render, ScrollStrategy, SharedString, StatefulInteractiveElement as _, Styled as _,
    UniformListScrollHandle, Window,
    component::{
        ActiveTheme as _, Icon, Sizable as _, WindowExt as _,
        dialog::DialogButtonProps,
        h_flex,
        input::{Input, InputEvent, InputState},
        menu::{ContextMenuExt as _, PopupMenuItem},
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px, uniform_list,
};

use super::common::{self, ROW_HEIGHT};
use crate::{
    actions::NewFile,
    requests::{OpenDiff, OpenLocation},
};

type EntryRef = Arc<Entry>;

/// A context-menu command run on the tree.
type MenuCommand = dyn Fn(&mut ProjectTree, &mut Window, &mut Context<ProjectTree>);

enum Children {
    Loading,
    Loaded(Vec<EntryRef>),
    Failed(SharedString),
}

#[derive(Clone)]
enum RowKind {
    Entry(EntryRef),
    /// The children of this directory are being fetched.
    Loading(PathBuf),
    Failed(PathBuf, SharedString),
}

#[derive(Clone)]
struct Row {
    depth: u16,
    kind: RowKind,
}

impl Row {
    fn entry(&self) -> Option<&EntryRef> {
        match &self.kind {
            RowKind::Entry(e) => Some(e),
            _ => None,
        }
    }

    fn path(&self) -> &Path {
        match &self.kind {
            RowKind::Entry(e) => &e.path,
            RowKind::Loading(p) | RowKind::Failed(p, _) => p,
        }
    }
}

/// What the inline edit bar is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditKind {
    Rename(PathBuf),
    NewDirectory(PathBuf),
}

struct Editing {
    kind: EditKind,
    input: Entity<InputState>,
}

/// The payload of a dragged tree row.
#[derive(Clone)]
pub struct DraggedEntry {
    pub path: PathBuf,
    pub name: SharedString,
}

/// Drag preview: the entry name in a pill.
struct DragPreview(SharedString);

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(theme.popover)
            .border_1()
            .border_color(theme.drag_border)
            .text_sm()
            .child(self.0.clone())
    }
}

struct SpeedSearch {
    query: String,
    /// Row indices matching the query, ascending.
    matches: Vec<usize>,
}

pub struct ProjectTree {
    services: Services,
    roots: Vec<PathBuf>,
    root_entries: Vec<EntryRef>,
    children: HashMap<PathBuf, Children>,
    expanded: HashSet<PathBuf>,
    rows: Vec<Row>,
    selected: Option<PathBuf>,
    /// Cached index of `selected` in `rows`; validated before use.
    selected_ix: Option<usize>,
    search: Option<SpeedSearch>,
    editing: Option<Editing>,
    /// VCS status of changed files, for coloring.
    changes: HashMap<PathBuf, ChangeKind>,
    /// The generation of the newest listing request per directory.
    pending_loads: HashMap<PathBuf, u64>,
    load_generation: u64,
    /// Last operation's error, shown under the tree.
    message: Option<SharedString>,
    focus_handle: FocusHandle,
    scroll: UniformListScrollHandle,
}

/// The tree's top-level nodes. A single root shows the project's name;
/// several roots show their folder names.
fn root_entries(services: &Services, roots: &[PathBuf]) -> Vec<EntryRef> {
    roots
        .iter()
        .enumerate()
        .map(|(ix, root)| {
            let name = match root.file_name() {
                Some(name) if ix > 0 || roots.len() > 1 => name.to_string_lossy().into_owned(),
                _ => services.project.name(),
            };
            Arc::new(Entry {
                path: root.clone(),
                name,
                kind: EntryKind::Directory,
                size: 0,
            })
        })
        .collect()
}

impl ProjectTree {
    pub fn new(services: Services, cx: &mut Context<Self>) -> Self {
        let roots = services.project.roots();
        let root_entries = root_entries(&services, &roots);
        let mut this = Self {
            services,
            roots: roots.clone(),
            root_entries,
            children: HashMap::new(),
            expanded: HashSet::new(),
            rows: Vec::new(),
            selected: None,
            selected_ix: None,
            search: None,
            editing: None,
            changes: HashMap::new(),
            pending_loads: HashMap::new(),
            load_generation: 0,
            message: None,
            focus_handle: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
        };
        for root in roots {
            this.expanded.insert(root.clone());
            this.load(root, cx);
        }
        this.rebuild_rows();
        this.watch(cx);
        this.load_changes(cx);
        this
    }

    // -----------------------------------------------------------------------
    // Loading and the row model
    // -----------------------------------------------------------------------

    fn watch(&mut self, cx: &mut Context<Self>) {
        let events = self.services.project.subscribe();
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                let alive = this.update(cx, |this, cx| this.on_project_event(event, cx));
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Pick up root folders added to or removed from the project.
    fn reload_roots(&mut self, cx: &mut Context<Self>) {
        let roots = self.services.project.roots();
        let removed: Vec<PathBuf> = self
            .roots
            .iter()
            .filter(|r| !roots.contains(r))
            .cloned()
            .collect();
        for root in &removed {
            self.children.retain(|dir, _| !dir.starts_with(root));
            self.expanded.retain(|dir| !dir.starts_with(root));
        }
        let added: Vec<PathBuf> = roots
            .iter()
            .filter(|r| !self.roots.contains(r))
            .cloned()
            .collect();
        self.root_entries = root_entries(&self.services, &roots);
        self.roots = roots;
        for root in added {
            self.expanded.insert(root.clone());
            self.load(root, cx);
        }
        self.rebuild_rows();
        cx.notify();
    }

    fn on_project_event(&mut self, event: ProjectEvent, cx: &mut Context<Self>) {
        let path = match &event {
            ProjectEvent::Added(p) | ProjectEvent::Removed(p) | ProjectEvent::Changed(p) => p,
            ProjectEvent::RootsChanged => {
                self.reload_roots(cx);
                return;
            }
        };
        if let ProjectEvent::Removed(removed) = &event {
            // Forget the removed subtree.
            self.children.retain(|dir, _| !dir.starts_with(removed));
            self.expanded.retain(|dir| !dir.starts_with(removed));
        }
        if let Some(parent) = path.parent().map(Path::to_path_buf)
            && self.children.contains_key(&parent)
        {
            self.load(parent, cx);
        }
        self.load_changes(cx);
    }

    fn load_changes(&mut self, cx: &mut Context<Self>) {
        let task = cx.background_spawn(self.services.vcs.changes());
        cx.spawn(async move |this, cx| {
            let changes = task.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.changes = changes.into_iter().map(|c| (c.path, c.kind)).collect();
                cx.notify();
            });
        })
        .detach();
    }

    /// Fetch the children of `dir`; keeps showing the old ones meanwhile.
    fn load(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        if !self.children.contains_key(&dir) {
            self.children.insert(dir.clone(), Children::Loading);
        }
        // Only the latest listing of a directory counts: an older, slower
        // answer must not overwrite a newer one.
        self.load_generation += 1;
        let generation = self.load_generation;
        self.pending_loads.insert(dir.clone(), generation);
        let task = cx.background_spawn(self.services.project.children(&dir));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.pending_loads.get(&dir) != Some(&generation) {
                    return;
                }
                this.pending_loads.remove(&dir);
                let children = match result {
                    Ok(entries) => Children::Loaded(entries.into_iter().map(Arc::new).collect()),
                    Err(err) => Children::Failed(err.to_string().into()),
                };
                this.set_children(dir, children);
                cx.notify();
            });
        })
        .detach();
    }

    fn set_children(&mut self, dir: PathBuf, children: Children) {
        self.children.insert(dir.clone(), children);
        self.refresh_dir_rows(&dir);
    }

    /// Insert already listed children (tests and benchmarks).
    pub fn insert_children(&mut self, dir: PathBuf, entries: Vec<Entry>) {
        self.set_children(
            dir,
            Children::Loaded(entries.into_iter().map(Arc::new).collect()),
        );
    }

    /// Store listed children and mark `dir` expanded without touching the
    /// rows; call [`Self::rebuild_rows`] afterwards (benchmarks of huge
    /// trees).
    pub fn preload_children(&mut self, dir: PathBuf, entries: Vec<Entry>) {
        self.expanded.insert(dir.clone());
        self.children.insert(
            dir,
            Children::Loaded(entries.into_iter().map(Arc::new).collect()),
        );
    }

    /// Render the rows in `range`, as the list does for the visible part
    /// (benchmarks).
    pub fn render_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> usize {
        self.render_rows(range, cx).len()
    }

    /// Rows of `dir`'s visible subtree, at `depth`.
    fn subtree_rows(&self, dir: &Path, depth: u16, out: &mut Vec<Row>) {
        match self.children.get(dir) {
            None | Some(Children::Loading) => out.push(Row {
                depth,
                kind: RowKind::Loading(dir.to_path_buf()),
            }),
            Some(Children::Failed(err)) => out.push(Row {
                depth,
                kind: RowKind::Failed(dir.to_path_buf(), err.clone()),
            }),
            Some(Children::Loaded(entries)) => {
                for entry in entries {
                    out.push(Row {
                        depth,
                        kind: RowKind::Entry(entry.clone()),
                    });
                    if entry.is_dir() && self.expanded.contains(&entry.path) {
                        self.subtree_rows(&entry.path, depth + 1, out);
                    }
                }
            }
        }
    }

    /// Rebuild every row. Used at start; later changes are incremental.
    pub fn rebuild_rows(&mut self) {
        let mut rows = Vec::with_capacity(self.rows.len());
        for root in &self.root_entries {
            rows.push(Row {
                depth: 0,
                kind: RowKind::Entry(root.clone()),
            });
            if self.expanded.contains(&root.path) {
                self.subtree_rows(&root.path, 1, &mut rows);
            }
        }
        self.rows = rows;
        self.after_rows_changed();
    }

    /// The range of rows below row `ix` that belong to its subtree.
    fn descendants(&self, ix: usize) -> Range<usize> {
        let depth = self.rows[ix].depth;
        let end = self.rows[ix + 1..]
            .iter()
            .position(|r| r.depth <= depth)
            .map(|n| ix + 1 + n)
            .unwrap_or(self.rows.len());
        ix + 1..end
    }

    fn row_of(&self, path: &Path) -> Option<usize> {
        if let Some(ix) = self.selected_ix
            && self.rows.get(ix).is_some_and(|r| r.path() == path)
            && self.rows[ix].entry().is_some()
        {
            return Some(ix);
        }
        self.rows
            .iter()
            .position(|r| r.entry().is_some_and(|e| e.path == path))
    }

    /// Replace the rows under `dir` with its current subtree, if `dir` is
    /// visible and expanded.
    fn refresh_dir_rows(&mut self, dir: &Path) {
        if !self.expanded.contains(dir) {
            return;
        }
        let Some(ix) = self.row_of(dir) else { return };
        let range = self.descendants(ix);
        let mut fresh = Vec::new();
        self.subtree_rows(dir, self.rows[ix].depth + 1, &mut fresh);
        self.rows.splice(range, fresh);
        self.after_rows_changed();
    }

    fn after_rows_changed(&mut self) {
        self.selected_ix = self.selected.as_ref().and_then(|p| {
            self.rows
                .iter()
                .position(|r| r.entry().is_some_and(|e| e.path == *p))
        });
        if let Some(search) = &self.search {
            let query = search.query.clone();
            self.update_search_matches(&query);
        }
    }

    pub fn expand(&mut self, dir: &Path, cx: &mut Context<Self>) {
        if !self.expanded.insert(dir.to_path_buf()) {
            return;
        }
        if !self.children.contains_key(dir) {
            self.load(dir.to_path_buf(), cx);
        }
        self.refresh_dir_rows(dir);
        cx.notify();
    }

    pub fn collapse(&mut self, dir: &Path, cx: &mut Context<Self>) {
        if !self.expanded.remove(dir) {
            return;
        }
        if let Some(ix) = self.row_of(dir) {
            let range = self.descendants(ix);
            self.rows.drain(range);
            self.after_rows_changed();
        }
        cx.notify();
    }

    pub fn toggle(&mut self, dir: &Path, cx: &mut Context<Self>) {
        if self.expanded.contains(dir) {
            self.collapse(dir, cx);
        } else {
            self.expand(dir, cx);
        }
    }

    pub fn is_expanded(&self, dir: &Path) -> bool {
        self.expanded.contains(dir)
    }

    /// Number of visible rows (for tests and the storybook).
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// The names of the visible rows (tests).
    pub fn row_names(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter_map(|r| r.entry().map(|e| e.name.clone()))
            .collect()
    }

    pub fn selected(&self) -> Option<&Path> {
        self.selected.as_deref()
    }

    pub fn message(&self) -> Option<&SharedString> {
        self.message.as_ref()
    }

    // -----------------------------------------------------------------------
    // Selection and keyboard
    // -----------------------------------------------------------------------

    fn select_ix(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(entry) = self.rows.get(ix).and_then(Row::entry) {
            self.selected = Some(entry.path.clone());
            self.selected_ix = Some(ix);
            self.scroll.scroll_to_item(ix, ScrollStrategy::Center);
            cx.notify();
        }
    }

    pub fn select_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        if let Some(ix) = self.row_of(path) {
            self.select_ix(ix, cx);
        }
    }

    fn selected_entry(&self) -> Option<EntryRef> {
        self.selected_ix
            .and_then(|ix| self.rows.get(ix))
            .and_then(Row::entry)
            .cloned()
    }

    /// Select `path`, expanding (and loading) its parent directories.
    pub fn reveal(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        for ancestor in path
            .ancestors()
            .skip(1)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            if !self.roots.iter().any(|root| ancestor.starts_with(root)) {
                continue;
            }
            let ancestor = ancestor.to_path_buf();
            if self.expanded.insert(ancestor.clone()) && !self.children.contains_key(&ancestor) {
                self.load(ancestor.clone(), cx);
            }
            self.refresh_dir_rows(&ancestor);
        }
        self.selected = Some(path);
        self.after_rows_changed();
        if let Some(ix) = self.selected_ix {
            self.scroll.scroll_to_item(ix, ScrollStrategy::Center);
        }
        cx.notify();
    }

    /// Open a file (focused, permanent) or toggle a directory.
    fn activate_entry(&mut self, entry: &EntryRef, window: &mut Window, cx: &mut Context<Self>) {
        if entry.is_dir() {
            self.toggle(&entry.path, cx);
        } else {
            window.dispatch_action(Box::new(OpenLocation::file(entry.path.clone())), cx);
        }
    }

    fn on_row_click(
        &mut self,
        ix: usize,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window, cx);
        let Some(row) = self.rows.get(ix).cloned() else {
            return;
        };
        match row.kind {
            RowKind::Entry(entry) => {
                self.select_ix(ix, cx);
                if entry.is_dir() {
                    self.toggle(&entry.path, cx);
                } else if event.click_count() >= 2 {
                    window.dispatch_action(Box::new(OpenLocation::file(entry.path.clone())), cx);
                } else {
                    // Single click: a preview tab, focus stays in the tree.
                    let preview = OpenLocation::file(entry.path.clone())
                        .preview(true)
                        .focus(false);
                    window.dispatch_action(Box::new(preview), cx);
                    self.focus_handle.focus(window, cx);
                }
            }
            RowKind::Failed(dir, _) => self.load(dir, cx),
            RowKind::Loading(_) => {}
        }
    }

    /// Handle a key press while the tree is focused. Returns whether the key
    /// was used.
    pub fn handle_key(
        &mut self,
        key: &str,
        text: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.editing.is_some() {
            return false;
        }
        // Speed search: Up/Down cycle the matches, Backspace edits, Esc ends.
        if let Some(search) = &mut self.search {
            match key {
                "escape" => {
                    self.search = None;
                    cx.notify();
                    return true;
                }
                "backspace" => {
                    let mut query = search.query.clone();
                    query.pop();
                    if query.is_empty() {
                        self.search = None;
                        cx.notify();
                    } else {
                        self.set_search(&query, cx);
                    }
                    return true;
                }
                "up" | "down" => {
                    self.jump_to_match(key == "down", cx);
                    return true;
                }
                _ => {}
            }
        }
        match key {
            "up" | "down" | "home" | "end" | "pageup" | "pagedown" => {
                if let Some(ix) = common::nav(key, self.selected_ix, self.rows.len()) {
                    self.select_ix(ix, cx);
                }
                true
            }
            "right" => {
                if let Some(entry) = self.selected_entry().filter(|e| e.is_dir()) {
                    if self.is_expanded(&entry.path) {
                        if let Some(ix) = self.selected_ix {
                            self.select_ix(ix + 1, cx);
                        }
                    } else {
                        self.expand(&entry.path, cx);
                    }
                }
                true
            }
            "left" => {
                if let Some(entry) = self.selected_entry() {
                    if entry.is_dir() && self.is_expanded(&entry.path) {
                        self.collapse(&entry.path, cx);
                    } else if let Some(parent) = entry.path.parent() {
                        self.select_path(parent, cx);
                    }
                }
                true
            }
            "enter" => {
                if let Some(entry) = self.selected_entry() {
                    self.activate_entry(&entry, window, cx);
                }
                true
            }
            "f2" => {
                if let Some(entry) = self.selected_entry() {
                    self.start_edit(EditKind::Rename(entry.path.clone()), window, cx);
                }
                true
            }
            "delete" => {
                if let Some(entry) = self.selected_entry() {
                    self.confirm_delete(entry.path.clone(), window, cx);
                }
                true
            }
            _ => match text {
                Some(text) if !text.is_empty() && text.chars().all(|c| !c.is_control()) => {
                    let query = self
                        .search
                        .as_ref()
                        .map(|s| s.query.clone())
                        .unwrap_or_default()
                        + text;
                    self.set_search(&query, cx);
                    true
                }
                _ => false,
            },
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if common::has_command_modifier(keystroke) {
            return;
        }
        let text = keystroke.key_char.as_deref();
        if self.handle_key(&keystroke.key, text, window, cx) {
            cx.stop_propagation();
        }
    }

    // -----------------------------------------------------------------------
    // Speed search
    // -----------------------------------------------------------------------

    /// Start or update the speed search and select the first match at or
    /// after the selection.
    pub fn set_search(&mut self, query: &str, cx: &mut Context<Self>) {
        self.search = Some(SpeedSearch {
            query: query.to_string(),
            matches: Vec::new(),
        });
        self.update_search_matches(query);
        let start = self.selected_ix.unwrap_or(0);
        let target = self.search.as_ref().and_then(|s| {
            s.matches
                .iter()
                .find(|&&ix| ix >= start)
                .or(s.matches.first())
                .copied()
        });
        if let Some(ix) = target {
            self.select_ix(ix, cx);
        }
        cx.notify();
    }

    pub fn search_query(&self) -> Option<&str> {
        self.search.as_ref().map(|s| s.query.as_str())
    }

    fn update_search_matches(&mut self, query: &str) {
        let matches = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                row.entry()
                    .is_some_and(|e| fuzzy::fuzzy_match(query, &e.name).is_some())
            })
            .map(|(ix, _)| ix)
            .collect();
        if let Some(search) = &mut self.search {
            search.matches = matches;
        }
    }

    fn jump_to_match(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(search) = &self.search else { return };
        let current = self.selected_ix.unwrap_or(0);
        let next = if forward {
            search
                .matches
                .iter()
                .find(|&&ix| ix > current)
                .or(search.matches.first())
        } else {
            search
                .matches
                .iter()
                .rev()
                .find(|&&ix| ix < current)
                .or(search.matches.last())
        }
        .copied();
        if let Some(ix) = next {
            self.select_ix(ix, cx);
        }
    }

    // -----------------------------------------------------------------------
    // File operations
    // -----------------------------------------------------------------------

    /// Show the inline name field for a rename or a new directory.
    pub fn start_edit(&mut self, kind: EditKind, window: &mut Window, cx: &mut Context<Self>) {
        let initial = match &kind {
            EditKind::Rename(path) => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            EditKind::NewDirectory(_) => String::new(),
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(tr("panels-project-name-placeholder"))
                .default_value(initial)
        });
        cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.commit_edit(window, cx);
            }
        })
        .detach();
        input.update(cx, |input, cx| input.focus(window, cx));
        self.editing = Some(Editing { kind, input });
        cx.notify();
    }

    pub fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = None;
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    fn commit_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editing) = self.editing.take() else {
            return;
        };
        let name = editing.input.read(cx).value().trim().to_string();
        self.focus_handle.focus(window, cx);
        if name.is_empty() {
            cx.notify();
            return;
        }
        match editing.kind {
            EditKind::Rename(path) => self.rename_entry(path, &name, cx),
            EditKind::NewDirectory(parent) => self.create_directory(parent, &name, cx),
        }
    }

    fn run_operation(
        &mut self,
        op: impl std::future::Future<Output = ferro_services::Result<()>> + Send + 'static,
        select: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.message = None;
        let task = cx.background_spawn(op);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        if let Some(path) = select {
                            this.selected = Some(path);
                            this.after_rows_changed();
                        }
                    }
                    Err(err) => this.message = Some(err.to_string().into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn rename_entry(&mut self, path: PathBuf, new_name: &str, cx: &mut Context<Self>) {
        let Some(parent) = path.parent() else { return };
        let to = parent.join(new_name);
        if to == path {
            return;
        }
        let op = self.services.project.rename(&path, &to);
        self.run_operation(op, Some(to), cx);
    }

    pub fn create_directory(&mut self, parent: PathBuf, name: &str, cx: &mut Context<Self>) {
        let path = parent.join(name);
        self.expand(&parent, cx);
        let op = self.services.project.create(&path, EntryKind::Directory);
        self.run_operation(op, Some(path), cx);
    }

    pub fn delete_entry(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let op = self.services.project.delete(&path);
        self.run_operation(op, path.parent().map(Path::to_path_buf), cx);
    }

    fn confirm_delete(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        let name: SharedString = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
            .into();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            let path = path.clone();
            alert
                .confirm()
                .title(tr("panels-project-delete-title"))
                .description(tr_args(
                    "panels-project-delete-body",
                    &[("name", name.to_string().into())],
                ))
                .show_cancel(true)
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(tr("panels-project-delete"))
                        .on_ok(move |_, _, cx| {
                            let path = path.clone();
                            let _ = this.update(cx, |this, cx| this.delete_entry(path, cx));
                            true
                        }),
                )
        });
    }

    /// Whether `from` may be moved into directory `to_dir`.
    pub fn can_move(from: &Path, to_dir: &Path) -> bool {
        !to_dir.starts_with(from) && from.parent() != Some(to_dir)
    }

    /// Move `from` into `to_dir` (drag & drop). Returns whether a move was
    /// started.
    pub fn move_entry(&mut self, from: &Path, to_dir: &Path, cx: &mut Context<Self>) -> bool {
        if !Self::can_move(from, to_dir) {
            return false;
        }
        let Some(name) = from.file_name() else {
            return false;
        };
        let to = to_dir.join(name);
        self.expand(to_dir, cx);
        let op = self.services.project.rename(from, &to);
        self.run_operation(op, Some(to), cx);
        true
    }

    fn copy_path(&self, path: &Path, relative: bool, cx: &mut App) {
        let text = if relative {
            common::relative(path, self.roots.first().map(PathBuf::as_path))
        } else {
            path.to_string_lossy().into_owned()
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    // -----------------------------------------------------------------------
    // Rendering
    // -----------------------------------------------------------------------

    fn change_color(&self, path: &Path, cx: &App) -> Option<gpui_kit::Hsla> {
        let theme = cx.theme();
        self.changes.get(path).map(|kind| match kind {
            ChangeKind::Modified | ChangeKind::Renamed => theme.blue,
            ChangeKind::Added => theme.green,
            ChangeKind::Untracked => theme.red_light,
            ChangeKind::Conflicted => theme.red,
            ChangeKind::Deleted => theme.muted_foreground,
        })
    }

    fn context_menu_for(
        this: gpui_kit::WeakEntity<Self>,
        entry: EntryRef,
        changed: bool,
        menu: gpui_kit::component::menu::PopupMenu,
    ) -> gpui_kit::component::menu::PopupMenu {
        let dir = if entry.is_dir() {
            entry.path.clone()
        } else {
            entry
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default()
        };
        let item = |label: SharedString, f: Box<MenuCommand>| {
            let this = this.clone();
            let f = std::rc::Rc::new(f);
            PopupMenuItem::new(label).on_click(move |_, window, cx| {
                let f = f.clone();
                let _ = this.update(cx, |tree, cx| f(tree, window, cx));
            })
        };
        let mut menu = menu;
        if !entry.is_dir() {
            menu = menu.menu(
                tr("panels-project-open"),
                Box::new(OpenLocation::file(entry.path.clone())),
            );
        }
        menu = menu
            .menu(tr("action-new-file"), Box::new(NewFile))
            .item(item(tr("panels-project-new-directory"), {
                let dir = dir.clone();
                Box::new(move |tree, window, cx| {
                    tree.start_edit(EditKind::NewDirectory(dir.clone()), window, cx)
                })
            }))
            .separator()
            .item(item(tr("panels-project-rename"), {
                let path = entry.path.clone();
                Box::new(move |tree, window, cx| {
                    tree.start_edit(EditKind::Rename(path.clone()), window, cx)
                })
            }))
            .item(item(tr("panels-project-delete"), {
                let path = entry.path.clone();
                Box::new(move |tree, window, cx| tree.confirm_delete(path.clone(), window, cx))
            }))
            .separator()
            .item(item(tr("panels-project-copy-path"), {
                let path = entry.path.clone();
                Box::new(move |tree, _, cx| tree.copy_path(&path, false, cx))
            }))
            .item(item(tr("panels-project-copy-relative-path"), {
                let path = entry.path.clone();
                Box::new(move |tree, _, cx| tree.copy_path(&path, true, cx))
            }));
        if changed {
            menu = menu.separator().menu(
                tr("action-show-diff"),
                Box::new(OpenDiff {
                    path: entry.path.clone(),
                }),
            );
        }
        menu
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let (muted, selected_bg, hover_bg, danger, drop_bg, accent) = (
            theme.muted_foreground,
            theme.list_active,
            theme.list_hover,
            theme.danger,
            theme.drop_target,
            theme.primary,
        );
        let query = self.search.as_ref().map(|s| s.query.clone());
        let this = cx.entity().downgrade();
        range
            .filter_map(|ix| self.rows.get(ix).cloned().map(|row| (ix, row)))
            .map(|(ix, row)| {
                let indent = px(8. + row.depth as f32 * 14.);
                let is_selected = Some(ix) == self.selected_ix;
                let base = h_flex()
                    .id(ix)
                    .h(px(ROW_HEIGHT))
                    .pl(indent)
                    .pr_2()
                    .gap_1()
                    .w_full()
                    .text_sm()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .cursor_pointer()
                    .when(is_selected, |el| el.bg(selected_bg))
                    .hover(|el| el.bg(hover_bg))
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        this.on_row_click(ix, event, window, cx)
                    }));
                match row.kind {
                    RowKind::Entry(entry) => {
                        let is_dir = entry.is_dir();
                        let open = is_dir && self.expanded.contains(&entry.path);
                        let (icon, chevron) = if is_dir {
                            (
                                if open {
                                    FerroIcon::FolderOpen
                                } else {
                                    FerroIcon::Folder
                                },
                                Some(if open {
                                    FerroIcon::ChevronDown
                                } else {
                                    FerroIcon::ChevronRight
                                }),
                            )
                        } else {
                            (FerroIcon::for_file(&entry.name), None)
                        };
                        let name: SharedString = entry.name.clone().into();
                        let color = self.change_color(&entry.path, cx);
                        let label = match &query {
                            Some(q) => {
                                let ranges = fuzzy::fuzzy_match(q, &entry.name)
                                    .map(|m| common::char_ranges(&entry.name, &m.positions))
                                    .unwrap_or_default();
                                common::highlighted(name.clone(), ranges, accent).into_any_element()
                            }
                            None => name.clone().into_any_element(),
                        };
                        let changed = color.is_some();
                        let drop_dir = if is_dir {
                            entry.path.clone()
                        } else {
                            entry
                                .path
                                .parent()
                                .map(Path::to_path_buf)
                                .unwrap_or_default()
                        };
                        let drag = DraggedEntry {
                            path: entry.path.clone(),
                            name: name.clone(),
                        };
                        let drop_dir_check = drop_dir.clone();
                        let entry_for_menu = entry.clone();
                        let this = this.clone();
                        base.aria_label(name.clone())
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, _, _, cx| this.select_ix(ix, cx)),
                            )
                            .on_drag(drag, |dragged: &DraggedEntry, _, _, cx| {
                                cx.new(|_| DragPreview(dragged.name.clone()))
                            })
                            .drag_over::<DraggedEntry>(move |style, _, _, _| style.bg(drop_bg))
                            .can_drop(move |dragged, _, _| {
                                dragged
                                    .downcast_ref::<DraggedEntry>()
                                    .is_some_and(|d| Self::can_move(&d.path, &drop_dir_check))
                            })
                            .on_drop(cx.listener(move |this, dragged: &DraggedEntry, _, cx| {
                                this.move_entry(&dragged.path, &drop_dir, cx);
                            }))
                            .child(div().w(px(14.)).flex_none().when_some(chevron, |el, c| {
                                el.child(Icon::from(c).xsmall().text_color(muted))
                            }))
                            .child(Icon::from(icon).small().text_color(muted))
                            .child(
                                div()
                                    .truncate()
                                    .when_some(color, |el, c| el.text_color(c))
                                    .child(label),
                            )
                            .context_menu(move |menu, _, _| {
                                Self::context_menu_for(
                                    this.clone(),
                                    entry_for_menu.clone(),
                                    changed,
                                    menu,
                                )
                            })
                            .into_any_element()
                    }
                    RowKind::Loading(_) => base
                        .child(div().w(px(14.)).flex_none())
                        .child(Icon::from(FerroIcon::Loading).small().text_color(muted))
                        .child(div().text_color(muted).child(tr("tree-loading")))
                        .into_any_element(),
                    RowKind::Failed(_, err) => base
                        .child(div().w(px(14.)).flex_none())
                        .child(Icon::from(FerroIcon::Error).small().text_color(danger))
                        .child(div().truncate().text_color(danger).child(format!(
                            "{} — {}",
                            tr("tree-load-failed"),
                            err
                        )))
                        .into_any_element(),
                }
            })
            .collect()
    }

    fn render_edit_bar(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let editing = self.editing.as_ref()?;
        let title = match &editing.kind {
            EditKind::Rename(_) => tr("panels-project-rename"),
            EditKind::NewDirectory(parent) => tr_args(
                "panels-project-new-directory-in",
                &[(
                    "dir",
                    common::relative(parent, self.roots.first().map(PathBuf::as_path)).into(),
                )],
            ),
        };
        Some(
            v_flex()
                .id("project-edit")
                .p_1()
                .gap_1()
                .border_b_1()
                .border_color(cx.theme().border)
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.cancel_edit(window, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(common::caption(title, cx))
                .child(Input::new(&editing.input).small()),
        )
    }
}

impl Focusable for ProjectTree {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ProjectTree {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, danger, popover) = (
            theme.border,
            theme.muted_foreground,
            theme.danger,
            theme.popover,
        );
        let search = self.search.as_ref().map(|s| {
            let found = !s.matches.is_empty();
            h_flex()
                .px_2()
                .py_1()
                .gap_1()
                .text_xs()
                .bg(popover)
                .border_b_1()
                .border_color(border)
                .child(Icon::from(FerroIcon::Search).xsmall().text_color(muted))
                .child(
                    div()
                        .when(!found, |el| el.text_color(danger))
                        .child(s.query.clone()),
                )
                .child(div().flex_1())
                .child(div().text_color(muted).child(tr_args(
                    "panels-project-matches",
                    &[("count", s.matches.len().into())],
                )))
        });
        v_flex()
            .id("project-tree")
            .key_context("ProjectTree")
            .track_focus(&self.focus_handle)
            .aria_label(tr("tool-project"))
            .size_full()
            .on_key_down(cx.listener(Self::on_key_down))
            .when_some(self.render_edit_bar(cx), |el, bar| el.child(bar))
            .when_some(search, |el, bar| el.child(bar))
            .child(
                uniform_list(
                    "project-tree-rows",
                    self.rows.len(),
                    cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx)),
                )
                .track_scroll(&self.scroll)
                .flex_1()
                .py_1(),
            )
            .when_some(self.message.clone(), |el, message| {
                el.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(danger)
                        .border_t_1()
                        .border_color(border)
                        .child(message),
                )
            })
    }
}
