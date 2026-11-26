//! Find in Files / Replace in Files with a live preview of the matches.
//!
//! The search is debounced (200 ms) and runs on the background executor;
//! results arrive grouped by file in a virtualized list, with ±3 lines of
//! context for the selected match.
//!
//! Stage 7: the search covers a scope — the project, a directory, the open
//! files or a named scope from `[search.scopes]` — narrowed by a file mask
//! (`*.rs, !target/**`). Replace All substitutes regex groups (`$1`),
//! rewrites open files in their editors (one undoable edit each) and the
//! other files on disk, and records the whole operation so
//! `ferro::UndoReplaceInFiles` reverts it at once.

use std::{
    ops::Range,
    path::{Path, PathBuf},
    time::Duration,
};

use ferro_index::Replacer;
use ferro_services::{SearchScope, Services, TextMatch, TextPos, TextQuery};
use ferro_ui::{FerroIcon, tr, tr_args};
use gpui_kit::{
    AnyElement, App, AppContext as _, ClickEvent, Context, Entity, FocusHandle, Focusable, Global,
    HighlightStyle, InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _, Render,
    Role, ScrollStrategy, SharedString, StatefulInteractiveElement as _, Styled as _, StyledText,
    Subscription, Task, UniformListScrollHandle, WeakEntity, Window,
    component::{
        ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, WindowExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputEvent, InputState},
        notification::Notification,
        spinner::Spinner,
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px, uniform_list,
};

use crate::{
    app_state::AppState, editor::providers::same_path, requests::OpenLocation,
    search::ReplacedFile, workspace::Workspace,
};

const DEBOUNCE: Duration = Duration::from_millis(200);
/// Matching lines fetched per search.
pub const RESULT_LIMIT: usize = 2_000;
const ROW_HEIGHT: f32 = 24.;
const CONTEXT_LINES: u32 = 3;
const CONTEXT: &str = "FindInFiles";

gpui_kit::actions!(
    find_in_files,
    [
        SelectPrevious,
        SelectNext,
        OpenSelected,
        Close,
        ToggleCase,
        ToggleWholeWord,
        ToggleRegex,
        ReplaceAll
    ]
);

struct KeysBound;
impl Global for KeysBound {}

fn bind_keys(cx: &mut App) {
    if cx.has_global::<KeysBound>() {
        return;
    }
    cx.set_global(KeysBound);
    let context = Some(CONTEXT);
    let replace_all = if cfg!(target_os = "macos") {
        "cmd-alt-enter"
    } else {
        "ctrl-alt-enter"
    };
    cx.bind_keys([
        KeyBinding::new("up", SelectPrevious, context),
        KeyBinding::new("down", SelectNext, context),
        KeyBinding::new("enter", OpenSelected, context),
        KeyBinding::new("escape", Close, context),
        KeyBinding::new("alt-c", ToggleCase, context),
        KeyBinding::new("alt-w", ToggleWholeWord, context),
        KeyBinding::new("alt-r", ToggleRegex, context),
        KeyBinding::new(replace_all, ReplaceAll, context),
    ]);
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Byte ranges of literal `needle` in `text`, with the same rules as the
/// search service (char-wise case folding, word boundaries).
pub fn find_ranges(
    text: &str,
    needle: &str,
    case_sensitive: bool,
    whole_word: bool,
) -> Vec<Range<usize>> {
    let needle: Vec<char> = needle.chars().collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let same =
        |a: char, b: char| a == b || (!case_sensitive && a.to_lowercase().eq(b.to_lowercase()));
    let mut out = Vec::new();
    let mut i = 0;
    while i + needle.len() <= chars.len() {
        if needle
            .iter()
            .enumerate()
            .all(|(k, &n)| same(chars[i + k].1, n))
        {
            let end_ix = i + needle.len();
            let before = i == 0 || !is_word_char(chars[i - 1].1);
            let after = end_ix == chars.len() || !is_word_char(chars[end_ix].1);
            if !whole_word || (before && after) {
                let end = chars.get(end_ix).map_or(text.len(), |(b, _)| *b);
                out.push(chars[i].0..end);
                i = end_ix;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// `text` with every range replaced by `replacement`.
pub fn replace_ranges(text: &str, ranges: &[Range<usize>], replacement: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for r in ranges {
        out.push_str(&text[last..r.start]);
        out.push_str(replacement);
        last = r.end;
    }
    out.push_str(&text[last..]);
    out
}

/// Which files Find in Files searches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeChoice {
    Project,
    /// The directory typed in the directory field (relative to the root).
    Directory,
    /// The files open in editors.
    OpenFiles,
    /// A scope from `[search.scopes]`, by name.
    Named(String),
}

/// One row of the result list.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    /// A file heading with its number of matches.
    File {
        path: PathBuf,
        relative: SharedString,
        count: usize,
    },
    /// A matching line: an index into the results.
    Match(usize),
}

/// Group matches (in path order) under file headings.
pub fn build_rows(results: &[TextMatch]) -> Vec<Row> {
    let mut rows = Vec::with_capacity(results.len() + 16);
    let mut ix = 0;
    while ix < results.len() {
        let path = &results[ix].path;
        let end = results[ix..]
            .iter()
            .position(|m| &m.path != path)
            .map_or(results.len(), |n| ix + n);
        rows.push(Row::File {
            path: path.clone(),
            relative: results[ix].relative.clone().into(),
            count: results[ix..end].iter().map(|m| m.ranges.len().max(1)).sum(),
        });
        rows.extend((ix..end).map(Row::Match));
        ix = end;
    }
    rows
}

// ---------------------------------------------------------------------------
// The view
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// No query yet.
    Idle,
    Loading,
    Ready,
    Failed(SharedString),
}

#[derive(Debug, Clone)]
enum Preview {
    None,
    Loading,
    Lines {
        first_line: u32,
        lines: Vec<SharedString>,
        focus: u32,
        ranges: Vec<Range<usize>>,
    },
    Failed(SharedString),
}

pub struct FindInFiles {
    workspace: WeakEntity<Workspace>,
    services: Services,
    query: Entity<InputState>,
    replacement: Entity<InputState>,
    filter: Entity<InputState>,
    directory: Entity<InputState>,
    scope: ScopeChoice,
    /// `[search.scopes]`: name and file mask.
    named_scopes: Vec<(String, String)>,
    root: PathBuf,
    replace_mode: bool,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
    results: Vec<TextMatch>,
    rows: Vec<Row>,
    selected: Option<usize>,
    status: Status,
    generation: u64,
    search_task: Option<Task<()>>,
    preview: Preview,
    preview_task: Option<Task<()>>,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl FindInFiles {
    fn new(
        workspace: &Workspace,
        handle: WeakEntity<Workspace>,
        replace_mode: bool,
        initial: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(tr("search-fif-query"))
                .default_value(initial.unwrap_or_default())
        });
        let replacement =
            cx.new(|cx| InputState::new(window, cx).placeholder(tr("search-fif-replacement")));
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(tr("search-fif-filter")));
        let root = workspace
            .services
            .project
            .roots()
            .first()
            .cloned()
            .unwrap_or_default();
        // The directory scope starts at the active file's folder.
        let start_dir = workspace
            .active_editor_entity()
            .and_then(|e| e.read(cx).path().parent().map(Path::to_path_buf))
            .and_then(|dir| dir.strip_prefix(&root).ok().map(Path::to_path_buf))
            .map(|rel| rel.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let directory = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(tr("search-fif-directory"))
                .default_value(start_dir)
        });
        let named_scopes = AppState::settings(cx)
            .search
            .scopes
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let mut subscriptions = Vec::new();
        for input in [&query, &filter, &directory] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::Change = event {
                        this.schedule_search(window, cx);
                    }
                },
            ));
        }
        subscriptions.push(cx.subscribe_in(
            &replacement,
            window,
            |_, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    cx.notify();
                }
            },
        ));
        let mut this = Self {
            workspace: handle,
            services: workspace.services.clone(),
            query,
            replacement,
            filter,
            directory,
            scope: ScopeChoice::Project,
            named_scopes,
            root,
            replace_mode,
            case_sensitive: false,
            whole_word: false,
            regex: false,
            results: Vec::new(),
            rows: Vec::new(),
            selected: None,
            status: Status::Idle,
            generation: 0,
            search_task: None,
            preview: Preview::None,
            preview_task: None,
            scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        this.schedule_search(window, cx);
        this
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn results(&self) -> &[TextMatch] {
        &self.results
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn selected_match(&self) -> Option<&TextMatch> {
        match self.rows.get(self.selected?)? {
            Row::Match(ix) => self.results.get(*ix),
            Row::File { .. } => None,
        }
    }

    pub fn set_query(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.query.update(cx, |input, cx| {
            input.set_value(text.to_string(), window, cx)
        });
        self.schedule_search(window, cx);
    }

    pub fn set_filter(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.filter.update(cx, |input, cx| {
            input.set_value(text.to_string(), window, cx)
        });
        self.schedule_search(window, cx);
    }

    pub fn set_replacement(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.replacement.update(cx, |input, cx| {
            input.set_value(text.to_string(), window, cx)
        });
        cx.notify();
    }

    pub fn toggle_case(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.case_sensitive = !self.case_sensitive;
        self.schedule_search(window, cx);
    }

    pub fn toggle_whole_word(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.whole_word = !self.whole_word;
        self.schedule_search(window, cx);
    }

    pub fn toggle_regex(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.regex = !self.regex;
        self.schedule_search(window, cx);
    }

    pub fn toggle_replace(&mut self, cx: &mut Context<Self>) {
        self.replace_mode = !self.replace_mode;
        cx.notify();
    }

    fn text_query(&self, cx: &App) -> TextQuery {
        let filter = self.filter.read(cx).value().trim().to_string();
        let scope = match &self.scope {
            ScopeChoice::Project => SearchScope::Project,
            ScopeChoice::Directory => {
                let dir = self.directory.read(cx).value().trim().replace('\\', "/");
                let dir = dir.trim_matches('/');
                if dir.is_empty() {
                    SearchScope::Project
                } else {
                    SearchScope::Directory(self.root.join(dir))
                }
            }
            ScopeChoice::OpenFiles => SearchScope::Files(
                self.workspace
                    .upgrade()
                    .map(|ws| {
                        ws.read(cx)
                            .editors
                            .iter()
                            .map(|e| e.read(cx).path().to_path_buf())
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
            ScopeChoice::Named(name) => self
                .named_scopes
                .iter()
                .find(|(n, _)| n == name)
                .map_or(SearchScope::Project, |(_, mask)| {
                    SearchScope::Mask(mask.clone())
                }),
        };
        TextQuery {
            text: self.query.read(cx).value().to_string(),
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            regex: self.regex,
            path_filter: (!filter.is_empty()).then_some(filter),
            scope,
        }
    }

    pub fn scope(&self) -> &ScopeChoice {
        &self.scope
    }

    /// Search another scope.
    pub fn set_scope(&mut self, scope: ScopeChoice, window: &mut Window, cx: &mut Context<Self>) {
        self.scope = scope;
        self.schedule_search(window, cx);
    }

    pub fn set_directory(&mut self, dir: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.directory
            .update(cx, |input, cx| input.set_value(dir.to_string(), window, cx));
        self.schedule_search(window, cx);
    }

    /// The replacer for the current query and replacement text, if the
    /// query is valid.
    fn replacer(&self, cx: &App) -> Option<Replacer> {
        let query = self.text_query(cx);
        if query.text.is_empty() {
            return None;
        }
        Replacer::new(&query, &self.replacement.read(cx).value()).ok()
    }

    /// Search after the debounce; a newer change restarts the wait.
    fn schedule_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        let query = self.text_query(cx);
        if query.text.is_empty() {
            self.search_task = None;
            self.status = Status::Idle;
            self.results.clear();
            self.rows.clear();
            self.selected = None;
            self.preview = Preview::None;
            cx.notify();
            return;
        }
        self.status = Status::Loading;
        let generation = self.generation;
        let search = self.services.search.clone();
        self.search_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let result = cx
                .background_spawn(search.find_text(query, RESULT_LIMIT))
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok(results) => this.set_results(results, window, cx),
                    Err(err) => {
                        this.status = Status::Failed(tr_args(
                            "search-error",
                            &[("error", err.to_string().into())],
                        ));
                        this.results.clear();
                        this.rows.clear();
                        this.selected = None;
                        this.preview = Preview::None;
                        cx.notify();
                    }
                }
            });
        }));
        cx.notify();
    }

    /// Show a new set of results (also used by tests to skip the service).
    pub fn set_results(
        &mut self,
        results: Vec<TextMatch>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.rows = build_rows(&results);
        self.results = results;
        self.status = Status::Ready;
        self.selected = self.rows.iter().position(|r| matches!(r, Row::Match(_)));
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.update_preview(window, cx);
        cx.notify();
    }

    fn select_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.rows.get(ix), Some(Row::Match(_))) && self.selected != Some(ix) {
            self.selected = Some(ix);
            self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
            self.update_preview(window, cx);
            cx.notify();
        }
    }

    /// Move to the next/previous match row, skipping file headings.
    pub fn move_selection(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(current) = self.selected else { return };
        let next = if forward {
            (current + 1..self.rows.len()).find(|&i| matches!(self.rows[i], Row::Match(_)))
        } else {
            (0..current)
                .rev()
                .find(|&i| matches!(self.rows[i], Row::Match(_)))
        };
        if let Some(next) = next {
            self.select_row(next, window, cx);
        }
    }

    fn update_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preview_task = None;
        let Some(m) = self.selected_match().cloned() else {
            self.preview = Preview::None;
            return;
        };
        // An open editor has the freshest text.
        let open_text = self.workspace.upgrade().and_then(|ws| {
            ws.read(cx)
                .editors
                .iter()
                .find(|e| e.read(cx).path() == m.path)
                .and_then(|e| e.read(cx).text(cx))
        });
        if let Some(text) = open_text {
            self.preview = context_preview(&text, &m);
            return;
        }
        self.preview = Preview::Loading;
        let read = self.services.project.read_file(&m.path);
        self.preview_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(read).await;
            let _ = this.update(cx, |this, cx| {
                this.preview = match result {
                    Ok(text) => context_preview(&text, &m),
                    Err(err) => Preview::Failed(tr_args(
                        "search-preview-failed",
                        &[("error", err.to_string().into())],
                    )),
                };
                cx.notify();
            });
        }));
    }

    /// Open the selected match in the editor and close the dialog.
    pub fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.selected_match().cloned() else {
            return;
        };
        let column = m
            .ranges
            .first()
            .map_or(0, |r| m.line_text[..r.start].chars().count() as u32);
        let location = OpenLocation::at(m.path, TextPos::new(m.line, column));
        window.close_dialog(cx);
        let _ = self
            .workspace
            .update(cx, |ws, cx| ws.open_location(&location, window, cx));
    }

    /// Replace every match: files open in editors are changed there (one
    /// undoable edit each), the others on disk in the background. Returns
    /// (occurrences replaced in open files, open files changed, files being
    /// rewritten on disk). The whole operation is recorded for
    /// `ferro::UndoReplaceInFiles`.
    pub fn replace_all(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (usize, usize, usize) {
        let query = self.text_query(cx);
        let replacement = self.replacement.read(cx).value().to_string();
        let replacer = match Replacer::new(&query, &replacement) {
            Ok(replacer) => replacer,
            Err(err) => {
                let message = tr_args("search-error", &[("error", format!("{err:#}").into())]);
                window.push_notification(Notification::error(message.to_string()), cx);
                return (0, 0, 0);
            }
        };
        let Some(workspace) = self.workspace.upgrade() else {
            return (0, 0, 0);
        };
        let workspace_id = workspace.entity_id();
        let mut paths: Vec<PathBuf> = self.results.iter().map(|m| m.path.clone()).collect();
        paths.dedup();
        let (mut count, mut files) = (0, 0);
        let mut changed: Vec<ReplacedFile> = Vec::new();
        let mut closed: Vec<PathBuf> = Vec::new();
        for path in paths {
            let editor = workspace
                .read(cx)
                .editors
                .iter()
                .find(|e| same_path(e.read(cx).path(), &path) && e.read(cx).is_loaded())
                .cloned();
            let Some((editor, text)) = editor.and_then(|e| {
                let text = e.read(cx).text(cx)?;
                Some((e, text))
            }) else {
                closed.push(path);
                continue;
            };
            let (new_text, n) = replacer.apply(&text);
            if n == 0 {
                continue;
            }
            editor.update(cx, |e, cx| e.replace_text(new_text.clone(), window, cx));
            changed.push(ReplacedFile {
                path,
                before: text,
                after: new_text,
            });
            count += n;
            files += 1;
        }
        let pending = closed.len();
        let project = self.services.project.clone();
        cx.spawn_in(window, async move |this, cx| {
            let (mut disk_count, mut disk_files, mut skipped) = (0usize, 0usize, 0usize);
            for path in closed {
                let result = async {
                    let bytes = project.read_bytes(&path).await?;
                    // Files in other encodings are left alone rather than
                    // re-encoded.
                    let text =
                        String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("not UTF-8"))?;
                    let (new_text, n) = replacer.apply(&text);
                    if n > 0 {
                        project
                            .write_file(&path, new_text.clone().into_bytes())
                            .await?;
                    }
                    anyhow::Ok((text, new_text, n))
                }
                .await;
                match result {
                    Ok((_, _, 0)) => {}
                    Ok((before, after, n)) => {
                        disk_count += n;
                        disk_files += 1;
                        changed.push(ReplacedFile {
                            path,
                            before,
                            after,
                        });
                    }
                    Err(err) => {
                        tracing::warn!("replace in {} failed: {err:#}", path.display());
                        skipped += 1;
                    }
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                let (count, files) = (count + disk_count, files + disk_files);
                crate::search::record_replace(workspace_id, changed, cx);
                let mut message = if count > 0 {
                    tr_args(
                        "search-fif-replaced",
                        &[("count", count.into()), ("files", files.into())],
                    )
                    .to_string()
                } else {
                    tr("search-fif-nothing-replaced").to_string()
                };
                if skipped > 0 {
                    message.push(' ');
                    message.push_str(&tr_args(
                        "search-fif-skipped",
                        &[("skipped", skipped.into())],
                    ));
                }
                if count > 0 {
                    message.push(' ');
                    message.push_str(&tr("search-fif-undo-hint"));
                }
                window.push_notification(Notification::info(message), cx);
                this.schedule_search(window, cx);
            });
        })
        .detach();
        (count, files, pending)
    }

    // -----------------------------------------------------------------------
    // Rendering
    // -----------------------------------------------------------------------

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let (muted, accent, selected_bg, hover_bg, success, danger) = (
            theme.muted_foreground,
            theme.primary,
            theme.list_active,
            theme.list_hover,
            theme.success,
            theme.danger,
        );
        let replacer = if self.replace_mode {
            self.replacer(cx)
        } else {
            None
        };
        let mark = HighlightStyle {
            background_color: Some(accent.opacity(0.3)),
            ..Default::default()
        };
        let removed = HighlightStyle {
            background_color: Some(danger.opacity(0.3)),
            strikethrough: Some(Default::default()),
            ..Default::default()
        };
        range
            .filter_map(|ix| self.rows.get(ix).cloned().map(|row| (ix, row)))
            .map(|(ix, row)| match row {
                Row::File {
                    relative, count, ..
                } => {
                    let name = relative.rsplit('/').next().unwrap_or_default().to_string();
                    h_flex()
                        .id(ix)
                        .role(Role::Heading)
                        .aria_label(format!("{relative}, {count}"))
                        .h(px(ROW_HEIGHT))
                        .px_2()
                        .gap_2()
                        .text_sm()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .child(
                            Icon::from(FerroIcon::for_file(&name))
                                .small()
                                .text_color(muted),
                        )
                        .child(div().child(name))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(muted)
                                .child(relative.clone()),
                        )
                        .child(div().text_xs().text_color(muted).child(count.to_string()))
                        .into_any_element()
                }
                Row::Match(match_ix) => {
                    let m = &self.results[match_ix];
                    let is_selected = self.selected == Some(ix);
                    let trimmed = m.line_text.trim_start();
                    let cut = m.line_text.len() - trimmed.len();
                    let shown: String = trimmed.chars().take(240).collect();
                    let ranges: Vec<Range<usize>> = m
                        .ranges
                        .iter()
                        .filter(|r| r.start >= cut && r.end - cut <= shown.len())
                        .map(|r| r.start - cut..r.end - cut)
                        .collect();
                    let label: SharedString =
                        format!("{}:{} {}", m.relative, m.line + 1, trimmed).into();
                    let line_element: AnyElement = match &replacer {
                        // Before → after: the match struck through, then the
                        // replacement in the success color.
                        Some(replacer) => {
                            let after = replacer.preview_line(&shown, &ranges);
                            h_flex()
                                .gap_2()
                                .min_w_0()
                                .child(
                                    StyledText::new(SharedString::from(shown.clone()))
                                        .with_highlights(
                                            ranges.iter().map(|r| (r.clone(), removed)),
                                        ),
                                )
                                .child(div().text_color(muted).child("→"))
                                .child(div().text_color(success).child(after))
                                .into_any_element()
                        }
                        None => StyledText::new(SharedString::from(shown.clone()))
                            .with_highlights(ranges.iter().map(|r| (r.clone(), mark)))
                            .into_any_element(),
                    };
                    h_flex()
                        .id(ix)
                        .role(Role::ListBoxOption)
                        .aria_label(label)
                        .aria_selected(is_selected)
                        .h(px(ROW_HEIGHT))
                        .pl(px(28.))
                        .pr_2()
                        .gap_2()
                        .text_sm()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .cursor_pointer()
                        .when(is_selected, |el| el.bg(selected_bg))
                        .hover(|el| el.bg(hover_bg))
                        .child(
                            div()
                                .w(px(40.))
                                .flex_none()
                                .text_xs()
                                .text_color(muted)
                                .child(format!("{}", m.line + 1)),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(line_element))
                        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                            this.select_row(ix, window, cx);
                            if event.click_count() >= 2 {
                                this.open_selected(window, cx);
                            }
                        }))
                        .into_any_element()
                }
            })
            .collect()
    }

    /// Scope buttons, and the directory field for the directory scope.
    fn render_scopes(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let mut choices: Vec<(SharedString, ScopeChoice)> = vec![
            (tr("search-scope-project"), ScopeChoice::Project),
            (tr("search-scope-directory"), ScopeChoice::Directory),
            (tr("search-scope-open-files"), ScopeChoice::OpenFiles),
        ];
        choices.extend(self.named_scopes.iter().map(|(name, _)| {
            (
                SharedString::from(name.clone()),
                ScopeChoice::Named(name.clone()),
            )
        }));
        let mut row = h_flex()
            .pl(px(28.))
            .gap_1()
            .flex_wrap()
            .text_xs()
            .child(div().text_color(muted).child(tr("search-scope")));
        for (ix, (label, choice)) in choices.into_iter().enumerate() {
            let selected = self.scope == choice;
            row = row.child(
                Button::new(("fif-scope", ix))
                    .ghost()
                    .xsmall()
                    .label(label)
                    .selected(selected)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_scope(choice.clone(), window, cx)
                    })),
            );
        }
        if self.scope == ScopeChoice::Directory {
            row = row.child(
                div().w(px(260.)).child(
                    Input::new(&self.directory)
                        .small()
                        .aria_label(tr("search-fif-directory"))
                        .prefix(Icon::from(FerroIcon::Folder).small().text_color(muted)),
                ),
            );
        }
        row.into_any_element()
    }

    fn render_preview(&self, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        match &self.preview {
            Preview::None => div().into_any_element(),
            Preview::Loading => h_flex()
                .p_2()
                .gap_2()
                .text_xs()
                .text_color(muted)
                .child(Spinner::new().xsmall())
                .child(tr("search-preview-loading"))
                .into_any_element(),
            Preview::Failed(message) => div()
                .p_2()
                .text_xs()
                .text_color(theme.danger)
                .child(message.clone())
                .into_any_element(),
            Preview::Lines {
                first_line,
                lines,
                focus,
                ranges,
            } => {
                let mark = HighlightStyle {
                    background_color: Some(theme.primary.opacity(0.35)),
                    ..Default::default()
                };
                v_flex()
                    .p_2()
                    .font_family(theme.mono_font_family.clone())
                    .text_xs()
                    .children(lines.iter().enumerate().map(|(i, line)| {
                        let number = first_line + i as u32;
                        let is_focus = number == *focus;
                        let text = if is_focus {
                            StyledText::new(line.clone()).with_highlights(
                                ranges
                                    .iter()
                                    .filter(|r| r.end <= line.len())
                                    .map(|r| (r.clone(), mark)),
                            )
                        } else {
                            StyledText::new(line.clone())
                        };
                        h_flex()
                            .gap_2()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .when(is_focus, |el| el.bg(theme.list_active))
                            .child(
                                div()
                                    .w(px(36.))
                                    .flex_none()
                                    .text_color(muted)
                                    .child(format!("{}", number + 1)),
                            )
                            .child(text)
                    }))
                    .into_any_element()
            }
        }
    }
}

fn context_preview(text: &str, m: &TextMatch) -> Preview {
    let first_line = m.line.saturating_sub(CONTEXT_LINES);
    let lines = text
        .lines()
        .skip(first_line as usize)
        .take((m.line - first_line + CONTEXT_LINES + 1) as usize)
        .map(|l| SharedString::from(l.chars().take(200).collect::<String>()))
        .collect();
    Preview::Lines {
        first_line,
        lines,
        focus: m.line,
        ranges: m.ranges.clone(),
    }
}

impl Focusable for FindInFiles {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.query.read(cx).focus_handle(cx)
    }
}

impl Render for FindInFiles {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let match_count: usize = self.results.iter().map(|m| m.ranges.len().max(1)).sum();
        let file_count = self
            .rows
            .iter()
            .filter(|r| matches!(r, Row::File { .. }))
            .count();
        let list_height = px(ROW_HEIGHT * 12.);

        let toggle = |id: &'static str, icon: FerroIcon, key: &str, on: bool| {
            Button::new(id)
                .ghost()
                .small()
                .icon(Icon::from(icon))
                .selected(on)
                .tooltip(tr(key))
        };

        let status: AnyElement = match &self.status {
            Status::Idle => div().child(tr("search-fif-hint")).into_any_element(),
            Status::Loading => h_flex()
                .gap_2()
                .child(Spinner::new().xsmall())
                .child(tr("search-loading"))
                .into_any_element(),
            Status::Failed(message) => div()
                .text_color(theme.danger)
                .child(message.clone())
                .into_any_element(),
            Status::Ready if self.results.is_empty() => {
                div().child(tr("search-fif-empty")).into_any_element()
            }
            Status::Ready => h_flex()
                .gap_3()
                .child(tr_args(
                    "search-fif-summary",
                    &[
                        ("matches", match_count.into()),
                        ("files", file_count.into()),
                    ],
                ))
                .when(self.results.len() >= RESULT_LIMIT, |el| {
                    el.child(tr_args(
                        "search-fif-truncated",
                        &[("limit", RESULT_LIMIT.into())],
                    ))
                })
                .child(div().flex_1())
                .child(tr("search-fif-open-hint"))
                .into_any_element(),
        };

        let list = div()
            .id("find-in-files-results")
            .role(Role::ListBox)
            .aria_label(tr("search-results-label"))
            .h(list_height)
            .w_full()
            .child(
                uniform_list(
                    "find-in-files-list",
                    self.rows.len(),
                    cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx)),
                )
                .track_scroll(&self.scroll)
                .h_full(),
            );

        v_flex()
            .id("find-in-files")
            .key_context(CONTEXT)
            .on_action(cx.listener(|this, _: &SelectPrevious, window, cx| {
                this.move_selection(false, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &SelectNext, window, cx| {
                    this.move_selection(true, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &OpenSelected, window, cx| this.open_selected(window, cx)),
            )
            .on_action(cx.listener(|_, _: &Close, window, cx| window.close_dialog(cx)))
            .on_action(cx.listener(|this, _: &ToggleCase, window, cx| this.toggle_case(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleWholeWord, window, cx| {
                this.toggle_whole_word(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ToggleRegex, window, cx| this.toggle_regex(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ReplaceAll, window, cx| {
                if this.replace_mode {
                    this.replace_all(window, cx);
                }
            }))
            .w_full()
            .gap_2()
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("fif-toggle-replace")
                            .ghost()
                            .small()
                            .icon(Icon::from(if self.replace_mode {
                                FerroIcon::ChevronDown
                            } else {
                                FerroIcon::ChevronRight
                            }))
                            .tooltip(tr("search-fif-toggle-replace"))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_replace(cx))),
                    )
                    .child(
                        div().flex_1().child(
                            Input::new(&self.query)
                                .aria_label(tr("search-fif-query"))
                                .prefix(Icon::from(FerroIcon::Search).small().text_color(muted)),
                        ),
                    )
                    .child(
                        toggle(
                            "fif-case",
                            FerroIcon::CaseSensitive,
                            "search-fif-case",
                            self.case_sensitive,
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_case(window, cx))),
                    )
                    .child(
                        toggle(
                            "fif-word",
                            FerroIcon::WholeWord,
                            "search-fif-word",
                            self.whole_word,
                        )
                        .on_click(
                            cx.listener(|this, _, window, cx| this.toggle_whole_word(window, cx)),
                        ),
                    )
                    .child(
                        toggle(
                            "fif-regex",
                            FerroIcon::Regex,
                            "search-fif-regex",
                            self.regex,
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_regex(window, cx))),
                    ),
            )
            .when(self.replace_mode, |el| {
                el.child(
                    h_flex()
                        .gap_1()
                        .pl(px(28.))
                        .child(
                            div().flex_1().child(
                                Input::new(&self.replacement)
                                    .aria_label(tr("search-fif-replacement"))
                                    .prefix(
                                        Icon::from(FerroIcon::Replace).small().text_color(muted),
                                    ),
                            ),
                        )
                        .child(
                            Button::new("fif-replace-all")
                                .small()
                                .primary()
                                .label(tr("search-fif-replace-all"))
                                .disabled(self.results.is_empty())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.replace_all(window, cx);
                                })),
                        ),
                )
            })
            .child(self.render_scopes(cx))
            .child(
                div().pl(px(28.)).child(
                    Input::new(&self.filter)
                        .aria_label(tr("search-fif-filter"))
                        .prefix(Icon::from(FerroIcon::Filter).small().text_color(muted)),
                ),
            )
            .child(div().px_1().text_xs().text_color(muted).child(status))
            .child(
                v_flex()
                    .border_1()
                    .border_color(border)
                    .rounded_md()
                    .overflow_hidden()
                    .child(list)
                    .child(
                        div()
                            .id("find-in-files-preview")
                            .aria_label(tr("search-preview-loading"))
                            .h(px(150.))
                            .border_t_1()
                            .border_color(border)
                            .overflow_hidden()
                            .child(self.render_preview(cx)),
                    ),
            )
    }
}

/// Open the Find in Files dialog; `replace` starts in Replace mode.
pub fn open(
    workspace: &mut Workspace,
    replace: bool,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let _ = open_view(workspace, replace, window, cx);
}

/// Open the dialog and return its view (tests drive it directly). The
/// query starts with the word at the caret of the active editor.
pub fn open_view(
    workspace: &mut Workspace,
    replace: bool,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Entity<FindInFiles> {
    bind_keys(cx);
    if window.has_active_dialog(cx) {
        window.close_dialog(cx);
    }
    let initial = workspace.active_editor_entity().and_then(|editor| {
        let editor = editor.read(cx);
        let text = editor.text(cx)?;
        let caret = editor.caret(cx)?;
        word_at(&text, caret)
    });
    let handle = cx.entity().downgrade();
    let view = cx.new(|cx| FindInFiles::new(workspace, handle, replace, initial, window, cx));
    let content = view.clone();
    let title = if replace {
        tr("search-fif-title-replace")
    } else {
        tr("search-fif-title-find")
    };
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title(title.clone())
            .w(px(900.))
            .margin_top(px(56.))
            .child(content.clone())
    });
    view.read(cx)
        .query
        .clone()
        .update(cx, |input, cx| input.focus(window, cx));
    view
}

/// The identifier around `pos`, if any.
fn word_at(text: &str, pos: TextPos) -> Option<String> {
    let line = text.lines().nth(pos.line as usize)?;
    let chars: Vec<char> = line.chars().collect();
    let col = (pos.column as usize).min(chars.len());
    let start = (0..col)
        .rev()
        .take_while(|&i| is_word_char(chars[i]))
        .last()
        .unwrap_or(col);
    let end = (col..chars.len())
        .take_while(|&i| is_word_char(chars[i]))
        .last()
        .map_or(col, |i| i + 1);
    (end > start).then(|| chars[start..end].iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_ranges_follow_case_and_word_rules() {
        let text = "greet greets Greet";
        assert_eq!(find_ranges(text, "greet", false, false).len(), 3);
        assert_eq!(find_ranges(text, "greet", true, false).len(), 2);
        assert_eq!(find_ranges(text, "greet", false, true), vec![0..5, 13..18]);
        assert!(find_ranges(text, "", false, false).is_empty());
        let pl = "żółć Żółć";
        assert_eq!(find_ranges(pl, "ŻÓŁĆ", false, false).len(), 2);
    }

    #[test]
    fn replacement_keeps_the_rest() {
        let text = "a greet b greet";
        let ranges = find_ranges(text, "greet", false, false);
        assert_eq!(replace_ranges(text, &ranges, "hi"), "a hi b hi");
    }

    #[test]
    fn rows_group_matches_by_file() {
        let m = |p: &str, line| TextMatch {
            path: PathBuf::from(p),
            relative: p.into(),
            line,
            line_text: "x".into(),
            ranges: std::iter::once(0..1).collect(),
        };
        let rows = build_rows(&[m("a", 1), m("a", 4), m("b", 2)]);
        assert_eq!(rows.len(), 5);
        assert!(matches!(&rows[0], Row::File { count: 2, .. }));
        assert_eq!(rows[1], Row::Match(0));
        assert!(matches!(&rows[3], Row::File { count: 1, .. }));
    }

    #[test]
    fn word_at_caret() {
        let text = "fn greet(name: &str)";
        assert_eq!(word_at(text, TextPos::new(0, 5)), Some("greet".into()));
        assert_eq!(word_at(text, TextPos::new(0, 8)), Some("greet".into()));
        assert_eq!(word_at(text, TextPos::new(0, 15)), None);
    }
}
