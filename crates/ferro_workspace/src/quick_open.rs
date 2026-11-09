//! Quick Open and the Command Palette: one popup at the top of the window
//! (VS Code/Zed style). The first character of the query picks the mode:
//! none = files, `>` = commands, `@` = symbols in the file, `#` = workspace
//! symbols, `:` = go to line, `%` = text search.
//!
//! Queries run on the background executor; every query bumps a generation
//! counter, so results of a query the user has already typed past are
//! dropped. Recently used files and commands come first, per workspace.

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use ferro_keymap::{ACTIONS, ResolvedBinding};
use ferro_services::{
    FileMatch, Services, SymbolKind, TextMatch, TextPos, TextQuery,
    fuzzy::{FuzzyMatch, fuzzy_match},
};
use ferro_ui::{FerroIcon, tr, tr_args};
use gpui_kit::{
    AnyElement, App, AppContext as _, ClickEvent, Context, Entity, EntityId, FocusHandle,
    Focusable, FontWeight, Global, HighlightStyle, InteractiveElement as _, IntoElement,
    KeyBinding, KeyDownEvent, Modifiers, ModifiersChangedEvent, ParentElement as _, Render, Role,
    ScrollStrategy, SharedString, StatefulInteractiveElement as _, Styled as _, StyledText,
    Subscription, Task, UniformListScrollHandle, WeakEntity, Window,
    component::{
        ActiveTheme as _, Icon, Placement, Sizable as _, WindowExt as _, h_flex,
        input::{Input, InputEvent, InputState},
        spinner::Spinner,
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px, uniform_list,
};

use crate::{
    app_state::AppState, chrome::display_keystroke, editor::EditorPanel, requests::OpenLocation,
    workspace::Workspace,
};

/// What the popup searches, selected by the query prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuickOpenMode {
    Files,
    Commands,
    SymbolsInFile,
    SymbolsInWorkspace,
    GoToLine,
    Text,
}

impl QuickOpenMode {
    pub const ALL: [QuickOpenMode; 6] = [
        QuickOpenMode::Files,
        QuickOpenMode::Commands,
        QuickOpenMode::SymbolsInFile,
        QuickOpenMode::SymbolsInWorkspace,
        QuickOpenMode::GoToLine,
        QuickOpenMode::Text,
    ];

    /// The query prefix that selects the mode.
    pub fn prefix(self) -> &'static str {
        match self {
            QuickOpenMode::Files => "",
            QuickOpenMode::Commands => ">",
            QuickOpenMode::SymbolsInFile => "@",
            QuickOpenMode::SymbolsInWorkspace => "#",
            QuickOpenMode::GoToLine => ":",
            QuickOpenMode::Text => "%",
        }
    }

    /// Split a query into its mode and the text after the prefix.
    pub fn parse(query: &str) -> (QuickOpenMode, &str) {
        for mode in QuickOpenMode::ALL.into_iter().skip(1) {
            if let Some(rest) = query.strip_prefix(mode.prefix()) {
                return (mode, rest);
            }
        }
        (QuickOpenMode::Files, query)
    }

    fn title_key(self) -> &'static str {
        match self {
            QuickOpenMode::Files => "search-mode-files",
            QuickOpenMode::Commands => "search-mode-commands",
            QuickOpenMode::SymbolsInFile => "search-mode-symbols-file",
            QuickOpenMode::SymbolsInWorkspace => "search-mode-symbols-workspace",
            QuickOpenMode::GoToLine => "search-mode-line",
            QuickOpenMode::Text => "search-mode-text",
        }
    }

    fn placeholder_key(self) -> &'static str {
        match self {
            QuickOpenMode::Files => "search-placeholder-files",
            QuickOpenMode::Commands => "search-placeholder-commands",
            QuickOpenMode::SymbolsInFile => "search-placeholder-symbols-file",
            QuickOpenMode::SymbolsInWorkspace => "search-placeholder-symbols-workspace",
            QuickOpenMode::GoToLine => "search-placeholder-line",
            QuickOpenMode::Text => "search-placeholder-text",
        }
    }

    fn empty_key(self) -> &'static str {
        match self {
            QuickOpenMode::Files => "search-no-files",
            QuickOpenMode::Commands => "search-no-commands",
            QuickOpenMode::SymbolsInFile | QuickOpenMode::SymbolsInWorkspace => "search-no-symbols",
            QuickOpenMode::GoToLine | QuickOpenMode::Text => "search-no-text",
        }
    }

    /// Whether results show a preview of the file.
    fn has_preview(self) -> bool {
        !matches!(self, QuickOpenMode::Commands)
    }
}

/// Results per query; more is noise for a popup.
const RESULT_LIMIT: usize = 200;
const TEXT_RESULT_LIMIT: usize = 100;
const TEXT_DEBOUNCE: Duration = Duration::from_millis(150);
const ROW_HEIGHT: f32 = 28.;
const VISIBLE_ROWS: usize = 12;
const CONTEXT: &str = "QuickOpen";

gpui_kit::actions!(
    quick_open,
    [
        SelectPrevious,
        SelectNext,
        SelectPageUp,
        SelectPageDown,
        Confirm,
        ConfirmToSide,
        Cancel
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
    let side = if cfg!(target_os = "macos") {
        "cmd-enter"
    } else {
        "ctrl-enter"
    };
    cx.bind_keys([
        KeyBinding::new("up", SelectPrevious, context),
        KeyBinding::new("down", SelectNext, context),
        KeyBinding::new("pageup", SelectPageUp, context),
        KeyBinding::new("pagedown", SelectPageDown, context),
        KeyBinding::new("enter", Confirm, context),
        KeyBinding::new(side, ConfirmToSide, context),
        KeyBinding::new("escape", Cancel, context),
    ]);
}

// ---------------------------------------------------------------------------
// Recently used, per workspace
// ---------------------------------------------------------------------------

const HISTORY_LIMIT: usize = 50;

#[derive(Debug, Clone, Default)]
pub struct Recent {
    /// Files picked in Quick Open, most recent first.
    pub files: Vec<PathBuf>,
    /// Commands run from the palette, most recent first.
    pub commands: Vec<&'static str>,
}

#[derive(Default)]
struct History(HashMap<EntityId, Recent>);
impl Global for History {}

fn history(workspace: EntityId, cx: &App) -> Recent {
    cx.try_global::<History>()
        .and_then(|h| h.0.get(&workspace).cloned())
        .unwrap_or_default()
}

fn remember(workspace: EntityId, cx: &mut App, edit: impl FnOnce(&mut Recent)) {
    let recent = cx
        .default_global::<History>()
        .0
        .entry(workspace)
        .or_default();
    edit(recent);
    recent.files.truncate(HISTORY_LIMIT);
    recent.commands.truncate(HISTORY_LIMIT);
}

fn push_front<T: PartialEq>(list: &mut Vec<T>, item: T) {
    list.retain(|x| *x != item);
    list.insert(0, item);
}

/// Recently used files for a workspace, most recent first: editors by last
/// activation (the active one last, VS Code style, so Enter on an empty
/// query switches to the previous file), then files picked earlier.
pub fn recent_files(workspace: &Workspace, cx: &App) -> Vec<PathBuf> {
    let active = workspace
        .active_editor_entity()
        .map(|e| e.read(cx).path().to_path_buf());
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    for editor in workspace.editors.iter().rev() {
        let path = editor.read(cx).path().to_path_buf();
        if Some(&path) != active.as_ref() {
            push(path);
        }
    }
    let workspace_id = workspace.dock_area().entity_id();
    for path in history(workspace_id, cx).files {
        if Some(&path) != active.as_ref() {
            push(path);
        }
    }
    if let Some(active) = active {
        push(active);
    }
    out
}

// ---------------------------------------------------------------------------
// Shift Shift
// ---------------------------------------------------------------------------

/// Detects a double tap of Shift alone (JetBrains' Search Everywhere).
#[derive(Debug, Default, Clone)]
pub struct DoubleShift {
    shift_alone: bool,
    last_tap: Option<Instant>,
}

/// The longest pause between the two taps.
pub const DOUBLE_SHIFT_WINDOW: Duration = Duration::from_millis(300);

impl DoubleShift {
    /// Feed a modifier change; returns true when the second tap completes.
    pub fn modifiers_changed(&mut self, modifiers: Modifiers, now: Instant) -> bool {
        let others = modifiers.control || modifiers.alt || modifiers.platform || modifiers.function;
        if others {
            *self = Self::default();
            return false;
        }
        if modifiers.shift {
            self.shift_alone = true;
            return false;
        }
        // Everything released.
        if !std::mem::take(&mut self.shift_alone) {
            return false;
        }
        match self.last_tap.take() {
            Some(first) if now.duration_since(first) <= DOUBLE_SHIFT_WINDOW => true,
            _ => {
                self.last_tap = Some(now);
                false
            }
        }
    }

    /// Any key press between the taps cancels the gesture.
    pub fn key_down(&mut self) {
        *self = Self::default();
    }
}

#[derive(Default)]
struct DoubleShiftState(HashMap<EntityId, DoubleShift>);
impl Global for DoubleShiftState {}

/// Hook for the workspace root: opens Search Everywhere on Shift Shift when
/// the keymap scheme wants it.
pub fn on_modifiers_changed(
    workspace: &mut Workspace,
    event: &ModifiersChangedEvent,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    if !AppState::settings(cx).keymap.base.double_shift_search() {
        return;
    }
    let id = workspace.dock_area().entity_id();
    let fired = cx
        .default_global::<DoubleShiftState>()
        .0
        .entry(id)
        .or_default()
        .modifiers_changed(event.modifiers, Instant::now());
    if fired && !window.has_active_dialog(cx) {
        open(workspace, QuickOpenMode::Files, window, cx);
    }
}

/// Hook for the workspace root (capture phase): a key press cancels Shift
/// Shift.
pub fn on_key_down(workspace: &mut Workspace, _: &KeyDownEvent, cx: &mut Context<Workspace>) {
    let id = workspace.dock_area().entity_id();
    if let Some(state) = cx
        .try_global::<DoubleShiftState>()
        .and_then(|s| s.0.get(&id))
        .cloned()
        && state.last_tap.is_some()
    {
        cx.default_global::<DoubleShiftState>()
            .0
            .entry(id)
            .or_default()
            .key_down();
    }
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// Why a `:` query does not name a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineError {
    Empty,
    Invalid(String),
    OutOfRange(u32),
}

/// Parse `line` or `line:column` (1-based) into a 0-based position.
pub fn parse_line_target(query: &str, line_count: u32) -> Result<TextPos, LineError> {
    let query = query.trim();
    if query.is_empty() {
        return Err(LineError::Empty);
    }
    let (line, column) = match query.split_once([':', ',']) {
        Some((l, c)) => (l.trim(), Some(c.trim())),
        None => (query, None),
    };
    let invalid = || LineError::Invalid(query.to_string());
    let line: u32 = line.parse().map_err(|_| invalid())?;
    let column: u32 = match column {
        Some("") | None => 1,
        Some(c) => c.parse().map_err(|_| invalid())?,
    };
    if line == 0 || column == 0 {
        return Err(invalid());
    }
    if line > line_count.max(1) {
        return Err(LineError::OutOfRange(line));
    }
    Ok(TextPos::new(line - 1, column - 1))
}

/// Byte offset of a 0-based line/column (columns in chars), clamped to the
/// end of the line and of the text.
pub fn byte_offset(text: &str, position: TextPos) -> usize {
    let mut start = 0;
    for _ in 0..position.line {
        match text[start..].find('\n') {
            Some(nl) => start += nl + 1,
            None => return text.len(),
        }
    }
    let line_end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    text[start..line_end]
        .char_indices()
        .nth(position.column as usize)
        .map_or(line_end, |(i, _)| start + i)
}

/// Merge matched byte positions into highlight ranges (one char each).
pub fn position_ranges(text: &str, positions: &[usize]) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    for &p in positions {
        if p >= text.len() || !text.is_char_boundary(p) {
            continue;
        }
        let end = p + text[p..].chars().next().map_or(1, char::len_utf8);
        match out.last_mut() {
            Some(last) if last.end == p => last.end = end,
            _ => out.push(p..end),
        }
    }
    out
}

fn relative_to(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|rel| {
            rel.components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

fn symbol_icon(kind: SymbolKind) -> FerroIcon {
    match kind {
        SymbolKind::Module => FerroIcon::SymbolModule,
        SymbolKind::Struct => FerroIcon::SymbolStruct,
        SymbolKind::Enum => FerroIcon::SymbolEnum,
        SymbolKind::Trait => FerroIcon::SymbolTrait,
        SymbolKind::Function | SymbolKind::Method => FerroIcon::SymbolFunction,
        SymbolKind::Field => FerroIcon::SymbolField,
        SymbolKind::Variable => FerroIcon::Variable,
        SymbolKind::Constant => FerroIcon::SymbolConstant,
        SymbolKind::Keyword => FerroIcon::SymbolKeyword,
        SymbolKind::Snippet => FerroIcon::FileCode,
        SymbolKind::Macro => FerroIcon::SymbolMacro,
        SymbolKind::Test => FerroIcon::Test,
    }
}

// ---------------------------------------------------------------------------
// Items
// ---------------------------------------------------------------------------

/// One result row.
#[derive(Debug, Clone, PartialEq)]
pub enum QuickOpenItem {
    File {
        path: PathBuf,
        relative: SharedString,
        positions: Vec<usize>,
        recent: bool,
    },
    Command {
        id: &'static str,
        title: SharedString,
        category: SharedString,
        shortcut: Option<SharedString>,
        positions: Vec<usize>,
        recent: bool,
    },
    Symbol {
        name: SharedString,
        kind: SymbolKind,
        path: PathBuf,
        position: TextPos,
        detail: SharedString,
        positions: Vec<usize>,
    },
    Line {
        position: TextPos,
    },
    Text {
        path: PathBuf,
        relative: SharedString,
        line: u32,
        line_text: SharedString,
        ranges: Vec<Range<usize>>,
    },
    /// A hint or validation message; not selectable.
    Message(SharedString),
}

impl QuickOpenItem {
    fn is_selectable(&self) -> bool {
        !matches!(self, QuickOpenItem::Message(_))
    }

    /// The file and position the item opens.
    fn location(&self) -> Option<(PathBuf, Option<TextPos>)> {
        match self {
            QuickOpenItem::File { path, .. } => Some((path.clone(), None)),
            QuickOpenItem::Symbol { path, position, .. } => Some((path.clone(), Some(*position))),
            QuickOpenItem::Text {
                path,
                line,
                ranges,
                line_text,
                ..
            } => {
                let column = ranges
                    .first()
                    .map(|r| line_text[..r.start].chars().count() as u32)
                    .unwrap_or(0);
                Some((path.clone(), Some(TextPos::new(*line, column))))
            }
            _ => None,
        }
    }

    /// The text announced by screen readers.
    fn accessible_label(&self) -> SharedString {
        match self {
            QuickOpenItem::File { relative, .. } => relative.clone(),
            QuickOpenItem::Command {
                title, shortcut, ..
            } => match shortcut {
                Some(s) => format!("{title}, {s}").into(),
                None => title.clone(),
            },
            QuickOpenItem::Symbol { name, detail, .. } => format!("{name}, {detail}").into(),
            QuickOpenItem::Line { position } => tr_args(
                "search-line-target",
                &[
                    ("line", (position.line + 1).into()),
                    ("column", (position.column + 1).into()),
                ],
            ),
            QuickOpenItem::Text {
                relative,
                line,
                line_text,
                ..
            } => format!("{relative}:{}, {}", line + 1, line_text.trim()).into(),
            QuickOpenItem::Message(m) => m.clone(),
        }
    }
}

/// Commands matching `query`, recently used first; with an empty query,
/// recently used first and then the catalog order.
pub fn command_items(
    query: &str,
    keymap: &[ResolvedBinding],
    recent: &[&'static str],
) -> Vec<QuickOpenItem> {
    let mut scored: Vec<(i64, usize, QuickOpenItem)> = Vec::new();
    for (ix, action) in ACTIONS.iter().filter(|a| a.searchable).enumerate() {
        let title = tr(action.title_key);
        let category = tr(action.category.title_key());
        let matched: Option<FuzzyMatch> = fuzzy_match(query, &title).or_else(|| {
            fuzzy_match(query, action.id).map(|m| FuzzyMatch {
                score: m.score - 50,
                positions: Vec::new(),
            })
        });
        let Some(m) = matched else { continue };
        let shortcut = ferro_keymap::shortcuts_in(action.id, keymap)
            .first()
            .map(|k| SharedString::from(display_keystroke(k)));
        let recent_ix = recent.iter().position(|r| *r == action.id);
        scored.push((
            m.score,
            ix,
            QuickOpenItem::Command {
                id: action.id,
                title,
                category,
                shortcut,
                positions: m.positions,
                recent: recent_ix.is_some(),
            },
        ));
    }
    let empty = query.trim().is_empty();
    scored.sort_by(|a, b| {
        let rank = |item: &QuickOpenItem| match item {
            QuickOpenItem::Command { id, .. } => recent.iter().position(|r| r == id),
            _ => None,
        };
        // Recent first (in recency order), then by score (or catalog order
        // for an empty query).
        match (rank(&a.2), rank(&b.2)) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) if empty => a.1.cmp(&b.1),
            (None, None) => b.0.cmp(&a.0).then(a.1.cmp(&b.1)),
        }
    });
    scored.into_iter().map(|(_, _, item)| item).collect()
}

/// Turn service results into rows, recently used files first.
pub fn file_items(matches: Vec<FileMatch>, recent: &[PathBuf]) -> Vec<QuickOpenItem> {
    let recent: HashSet<&PathBuf> = recent.iter().collect();
    let (mut first, rest): (Vec<_>, Vec<_>) =
        matches.into_iter().partition(|m| recent.contains(&m.path));
    first.extend(rest);
    first
        .into_iter()
        .map(|m| QuickOpenItem::File {
            recent: recent.contains(&m.path),
            path: m.path,
            relative: m.relative.into(),
            positions: m.positions,
        })
        .collect()
}

fn text_items(matches: Vec<TextMatch>) -> Vec<QuickOpenItem> {
    matches
        .into_iter()
        .map(|m| QuickOpenItem::Text {
            path: m.path,
            relative: m.relative.into(),
            line: m.line,
            line_text: m.line_text.into(),
            ranges: m.ranges,
        })
        .collect()
}

/// A symbol flattened for the list, with its enclosing element.
#[derive(Debug, Clone)]
struct FlatSymbol {
    name: String,
    kind: SymbolKind,
    path: PathBuf,
    position: TextPos,
    detail: String,
}

fn flatten_symbols(
    symbols: &[ferro_services::Symbol],
    parent: Option<&str>,
    root: &Path,
    with_file: bool,
    out: &mut Vec<FlatSymbol>,
) {
    for symbol in symbols {
        let container = symbol.container.as_deref().or(parent);
        let line = format!(":{}", symbol.selection.line + 1);
        let detail = match (container, with_file) {
            (Some(c), true) => format!("{c} — {}{line}", relative_to(root, &symbol.path)),
            (None, true) => format!("{}{line}", relative_to(root, &symbol.path)),
            (Some(c), false) => format!("{c}{line}"),
            (None, false) => line,
        };
        out.push(FlatSymbol {
            name: symbol.name.clone(),
            kind: symbol.kind,
            path: symbol.path.clone(),
            position: symbol.selection,
            detail,
        });
        flatten_symbols(&symbol.children, Some(&symbol.name), root, with_file, out);
    }
}

fn symbol_items(symbols: &[FlatSymbol], query: &str) -> Vec<QuickOpenItem> {
    let mut matches: Vec<(i64, usize, QuickOpenItem)> = symbols
        .iter()
        .enumerate()
        .filter_map(|(ix, s)| {
            let m = fuzzy_match(query, &s.name)?;
            Some((
                m.score,
                ix,
                QuickOpenItem::Symbol {
                    name: s.name.clone().into(),
                    kind: s.kind,
                    path: s.path.clone(),
                    position: s.position,
                    detail: s.detail.clone().into(),
                    positions: m.positions,
                },
            ))
        })
        .collect();
    if !query.trim().is_empty() {
        matches.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    }
    matches.into_iter().map(|(_, _, i)| i).collect()
}

// ---------------------------------------------------------------------------
// The view
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Ready,
    Loading,
    Failed(SharedString),
}

#[derive(Debug, Clone)]
enum Preview {
    None,
    Loading,
    Lines {
        title: SharedString,
        first_line: u32,
        lines: Vec<SharedString>,
        focus: Option<u32>,
        ranges: Vec<Range<usize>>,
    },
    Failed(SharedString),
}

/// What the popup knows about the active editor when it opens.
struct ActiveEditor {
    editor: WeakEntity<EditorPanel>,
    path: PathBuf,
    text: String,
    caret: Option<TextPos>,
}

pub struct QuickOpen {
    workspace: WeakEntity<Workspace>,
    workspace_id: EntityId,
    services: Services,
    root: PathBuf,
    input: Entity<InputState>,
    mode: QuickOpenMode,
    query: String,
    items: Vec<QuickOpenItem>,
    selected: usize,
    status: Status,
    generation: u64,
    search_task: Option<Task<()>>,
    preview: Preview,
    preview_task: Option<Task<()>>,
    scroll: UniformListScrollHandle,
    active: Option<ActiveEditor>,
    /// Whether a `:` query moved the caret, to restore it on cancel.
    previewed_line: bool,
    recent_files: Vec<PathBuf>,
    recent_commands: Vec<&'static str>,
    symbols: Option<Vec<FlatSymbol>>,
    _subscriptions: Vec<Subscription>,
}

impl QuickOpen {
    fn new(
        workspace: &Workspace,
        workspace_handle: WeakEntity<Workspace>,
        mode: QuickOpenMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let services = workspace.services.clone();
        let root = services
            .project
            .roots()
            .first()
            .cloned()
            .unwrap_or_default();
        let workspace_id = workspace.dock_area().entity_id();
        let active = workspace.active_editor_entity().and_then(|editor| {
            let e = editor.read(cx);
            Some(ActiveEditor {
                path: e.path().to_path_buf(),
                text: e.text(cx)?,
                caret: e.caret(cx),
                editor: editor.downgrade(),
            })
        });
        let recent_files = recent_files(workspace, cx);
        let recent_commands = history(workspace_id, cx).commands;

        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(tr(mode.placeholder_key()))
                .default_value(mode.prefix())
        });
        let subscription =
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::Change = event {
                    this.on_query_changed(window, cx);
                }
            });

        let mut this = Self {
            workspace: workspace_handle,
            workspace_id,
            services,
            root,
            input,
            mode,
            query: String::new(),
            items: Vec::new(),
            selected: 0,
            status: Status::Ready,
            generation: 0,
            search_task: None,
            preview: Preview::None,
            preview_task: None,
            scroll: UniformListScrollHandle::new(),
            active,
            previewed_line: false,
            recent_files,
            recent_commands,
            symbols: None,
            _subscriptions: vec![subscription],
        };
        this.on_query_changed(window, cx);
        this
    }

    pub fn mode(&self) -> QuickOpenMode {
        self.mode
    }

    pub fn items(&self) -> &[QuickOpenItem] {
        &self.items
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// Replace the whole query (prefix included), as typing would.
    pub fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.set_value(query.to_string(), window, cx)
        });
        self.on_query_changed(window, cx);
    }

    fn on_query_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let full = self.input.read(cx).value().to_string();
        let (mode, rest) = QuickOpenMode::parse(&full);
        if mode != self.mode {
            self.mode = mode;
            self.input.update(cx, |input, cx| {
                input.set_placeholder(tr(mode.placeholder_key()), window, cx)
            });
        }
        if mode != QuickOpenMode::GoToLine {
            self.restore_caret(cx);
        }
        self.query = rest.to_string();
        self.refresh(window, cx);
    }

    /// Run the current query.
    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        self.search_task = None;
        let query = self.query.clone();
        match self.mode {
            QuickOpenMode::Commands => {
                let keymap = AppState::global(cx).keymap.clone();
                let items = command_items(&query, &keymap, &self.recent_commands);
                self.set_items(items, window, cx);
            }
            QuickOpenMode::Files if query.trim().is_empty() => {
                let items = self
                    .recent_files
                    .iter()
                    .map(|path| QuickOpenItem::File {
                        relative: relative_to(&self.root, path).into(),
                        path: path.clone(),
                        positions: Vec::new(),
                        recent: true,
                    })
                    .collect();
                self.set_items(items, window, cx);
            }
            QuickOpenMode::Files => {
                let search = self.services.search.find_files(&query, RESULT_LIMIT);
                let recent = self.recent_files.clone();
                self.spawn_search(Duration::ZERO, window, cx, async move {
                    search.await.map(|matches| file_items(matches, &recent))
                });
            }
            QuickOpenMode::SymbolsInFile => {
                let Some(active) = &self.active else {
                    let items = vec![QuickOpenItem::Message(tr("search-no-editor"))];
                    return self.set_items(items, window, cx);
                };
                if let Some(symbols) = &self.symbols {
                    let items = symbol_items(symbols, &query);
                    return self.set_items(items, window, cx);
                }
                let task = self
                    .services
                    .language
                    .document_symbols(&active.path, &active.text);
                self.status = Status::Loading;
                let generation = self.generation;
                let root = self.root.clone();
                self.search_task = Some(cx.spawn_in(window, async move |this, cx| {
                    let result = task.await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        if this.generation != generation {
                            return;
                        }
                        match result {
                            Ok(symbols) => {
                                let mut flat = Vec::new();
                                flatten_symbols(&symbols, None, &root, false, &mut flat);
                                this.symbols = Some(flat);
                                this.refresh(window, cx);
                            }
                            Err(err) => this.fail(err, cx),
                        }
                    });
                }));
                cx.notify();
            }
            QuickOpenMode::SymbolsInWorkspace => {
                let task = self.services.language.workspace_symbols(&query);
                let root = self.root.clone();
                let query = query.clone();
                self.spawn_search(Duration::ZERO, window, cx, async move {
                    task.await.map(|symbols| {
                        let mut flat = Vec::new();
                        flatten_symbols(&symbols, None, &root, true, &mut flat);
                        // The service ranks; keep its order, add highlights.
                        flat.into_iter()
                            .map(|s| QuickOpenItem::Symbol {
                                positions: fuzzy_match(&query, &s.name)
                                    .map(|m| m.positions)
                                    .unwrap_or_default(),
                                name: s.name.into(),
                                kind: s.kind,
                                path: s.path,
                                position: s.position,
                                detail: s.detail.into(),
                            })
                            .take(RESULT_LIMIT)
                            .collect()
                    })
                });
            }
            QuickOpenMode::GoToLine => {
                let Some(active) = &self.active else {
                    let items = vec![QuickOpenItem::Message(tr("search-no-editor"))];
                    return self.set_items(items, window, cx);
                };
                let lines = active.text.lines().count().max(1) as u32;
                let item = match parse_line_target(&query, lines) {
                    Ok(position) => {
                        if let Some(editor) = active.editor.upgrade() {
                            editor.update(cx, |e, cx| e.preview_position(position, cx));
                            self.previewed_line = true;
                        }
                        QuickOpenItem::Line { position }
                    }
                    Err(LineError::Empty) => QuickOpenItem::Message(tr_args(
                        "search-line-hint",
                        &[("lines", lines.into())],
                    )),
                    Err(LineError::Invalid(input)) => QuickOpenItem::Message(tr_args(
                        "search-line-invalid",
                        &[("input", input.into())],
                    )),
                    Err(LineError::OutOfRange(line)) => QuickOpenItem::Message(tr_args(
                        "search-line-out-of-range",
                        &[("line", line.into()), ("lines", lines.into())],
                    )),
                };
                self.set_items(vec![item], window, cx);
            }
            QuickOpenMode::Text if query.trim().is_empty() => {
                let items = vec![QuickOpenItem::Message(tr("search-placeholder-text"))];
                self.set_items(items, window, cx);
            }
            QuickOpenMode::Text => {
                let task = self.services.search.find_text(
                    TextQuery {
                        text: query,
                        ..TextQuery::default()
                    },
                    TEXT_RESULT_LIMIT,
                );
                self.spawn_search(TEXT_DEBOUNCE, window, cx, async move {
                    task.await.map(text_items)
                });
            }
        }
    }

    /// Run `search` (after `debounce`) and show its rows unless a newer
    /// query has started meanwhile.
    fn spawn_search(
        &mut self,
        debounce: Duration,
        window: &mut Window,
        cx: &mut Context<Self>,
        search: impl Future<Output = ferro_services::Result<Vec<QuickOpenItem>>> + Send + 'static,
    ) {
        self.status = Status::Loading;
        let generation = self.generation;
        self.search_task = Some(cx.spawn_in(window, async move |this, cx| {
            if !debounce.is_zero() {
                cx.background_executor().timer(debounce).await;
            }
            let result = cx.background_spawn(search).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok(items) => this.set_items(items, window, cx),
                    Err(err) => this.fail(err, cx),
                }
            });
        }));
        cx.notify();
    }

    fn fail(&mut self, err: anyhow::Error, cx: &mut Context<Self>) {
        self.status = Status::Failed(tr_args(
            "search-error",
            &[("error", err.to_string().into())],
        ));
        self.items.clear();
        self.preview = Preview::None;
        cx.notify();
    }

    fn set_items(
        &mut self,
        items: Vec<QuickOpenItem>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.items = items;
        self.status = Status::Ready;
        self.selected = self
            .items
            .iter()
            .position(QuickOpenItem::is_selectable)
            .unwrap_or(0);
        self.scroll
            .scroll_to_item(self.selected, ScrollStrategy::Top);
        self.update_preview(window, cx);
        cx.notify();
    }

    fn select(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.items.is_empty() {
            return;
        }
        let ix = ix.min(self.items.len() - 1);
        if ix == self.selected || !self.items[ix].is_selectable() {
            return;
        }
        self.selected = ix;
        self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
        self.update_preview(window, cx);
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.items.is_empty() {
            return;
        }
        let last = self.items.len() as isize - 1;
        let target = (self.selected as isize + delta).clamp(0, last) as usize;
        self.select(target, window, cx);
    }

    fn update_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preview_task = None;
        let Some(item) = self.items.get(self.selected).cloned() else {
            self.preview = Preview::None;
            return;
        };
        if !self.mode.has_preview() {
            self.preview = Preview::None;
            return;
        }
        let (path, focus, ranges) = match &item {
            QuickOpenItem::File { path, .. } => (path.clone(), None, Vec::new()),
            QuickOpenItem::Symbol { path, position, .. } => {
                (path.clone(), Some(position.line), Vec::new())
            }
            QuickOpenItem::Text {
                path, line, ranges, ..
            } => (path.clone(), Some(*line), ranges.clone()),
            QuickOpenItem::Line { position } => match &self.active {
                Some(active) => (active.path.clone(), Some(position.line), Vec::new()),
                None => return,
            },
            _ => {
                self.preview = Preview::None;
                return;
            }
        };
        let title: SharedString = relative_to(&self.root, &path).into();
        // The active file's text is at hand; others are read asynchronously.
        if let Some(active) = self.active.as_ref().filter(|a| a.path == path) {
            self.preview = preview_lines(title, &active.text, focus, ranges);
            return;
        }
        self.preview = Preview::Loading;
        let read = self.services.project.read_file(&path);
        self.preview_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(read).await;
            let _ = this.update(cx, |this, cx| {
                this.preview = match result {
                    Ok(text) => preview_lines(title, &text, focus, ranges),
                    Err(err) => Preview::Failed(tr_args(
                        "search-preview-failed",
                        &[("error", err.to_string().into())],
                    )),
                };
                cx.notify();
            });
        }));
    }

    fn restore_caret(&mut self, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.previewed_line) {
            return;
        }
        if let Some(active) = &self.active
            && let (Some(editor), Some(caret)) = (active.editor.upgrade(), active.caret)
        {
            editor.update(cx, |e, cx| e.preview_position(caret, cx));
        }
    }

    /// Open the selected item (or run the command).
    pub fn confirm(&mut self, to_side: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.items.get(self.selected).cloned() else {
            return;
        };
        if !item.is_selectable() {
            return;
        }
        // The caret moved by a `:` preview is the intended result now.
        self.previewed_line = false;
        let workspace = self.workspace.clone();
        let workspace_id = self.workspace_id;
        window.close_dialog(cx);
        match item {
            QuickOpenItem::Command { id, .. } => {
                remember(workspace_id, cx, |r| push_front(&mut r.commands, id));
                let Some(workspace) = workspace.upgrade() else {
                    return;
                };
                // Dispatch from the workspace, where the handlers live.
                workspace.read(cx).focus_handle(cx).focus(window, cx);
                if let Some(action) = crate::actions::build(id, cx) {
                    window.dispatch_action(action, cx);
                }
            }
            QuickOpenItem::Line { position } => {
                if let Some(editor) = self.active.as_ref().and_then(|a| a.editor.upgrade()) {
                    editor.update(cx, |e, cx| e.go_to(position, window, cx));
                    editor.focus_handle(cx).focus(window, cx);
                }
            }
            item => {
                let Some((path, position)) = item.location() else {
                    return;
                };
                if matches!(item, QuickOpenItem::File { .. }) {
                    remember(workspace_id, cx, |r| push_front(&mut r.files, path.clone()));
                }
                let mut location = OpenLocation::file(path);
                location.position = position;
                let _ = workspace.update(cx, |ws, cx| {
                    ws.open_location(&location, window, cx);
                    if to_side {
                        ws.split(Placement::Right, window, cx);
                    }
                });
            }
        }
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.restore_caret(cx);
        window.close_dialog(cx);
    }

    // -----------------------------------------------------------------------
    // Rendering
    // -----------------------------------------------------------------------

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let (muted, accent, selected_bg, hover_bg) = (
            theme.muted_foreground,
            theme.primary,
            theme.list_active,
            theme.list_hover,
        );
        let strong = HighlightStyle {
            color: Some(accent),
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        let highlighted = |text: SharedString, ranges: Vec<Range<usize>>| {
            StyledText::new(text).with_highlights(ranges.into_iter().map(|r| (r, strong)))
        };
        range
            .filter_map(|ix| self.items.get(ix).cloned().map(|item| (ix, item)))
            .map(|(ix, item)| {
                let is_selected = ix == self.selected && item.is_selectable();
                let label = item.accessible_label();
                let (icon, primary, secondary, trailing): (
                    Option<FerroIcon>,
                    AnyElement,
                    Option<SharedString>,
                    Option<SharedString>,
                ) = match item {
                    QuickOpenItem::File {
                        relative,
                        positions,
                        recent,
                        ..
                    } => {
                        let name_start = relative.rfind('/').map_or(0, |i| i + 1);
                        let name: SharedString = relative[name_start..].to_string().into();
                        let shifted: Vec<usize> = positions
                            .iter()
                            .filter(|p| **p >= name_start)
                            .map(|p| p - name_start)
                            .collect();
                        let dir = relative[..name_start].trim_end_matches('/').to_string();
                        (
                            Some(FerroIcon::for_file(&name)),
                            highlighted(name.clone(), position_ranges(&name, &shifted))
                                .into_any_element(),
                            (!dir.is_empty()).then(|| dir.into()),
                            recent.then(|| tr("search-recent")),
                        )
                    }
                    QuickOpenItem::Command {
                        title,
                        category,
                        shortcut,
                        positions,
                        recent,
                        ..
                    } => (
                        None,
                        highlighted(title.clone(), position_ranges(&title, &positions))
                            .into_any_element(),
                        Some(if recent {
                            format!("{category} · {}", tr("search-recent-command")).into()
                        } else {
                            category
                        }),
                        shortcut,
                    ),
                    QuickOpenItem::Symbol {
                        name,
                        kind,
                        detail,
                        positions,
                        ..
                    } => (
                        Some(symbol_icon(kind)),
                        highlighted(name.clone(), position_ranges(&name, &positions))
                            .into_any_element(),
                        Some(detail),
                        None,
                    ),
                    QuickOpenItem::Line { .. } => (
                        Some(FerroIcon::ChevronRight),
                        div().child(label.clone()).into_any_element(),
                        None,
                        None,
                    ),
                    QuickOpenItem::Text {
                        relative,
                        line,
                        line_text,
                        ranges,
                        ..
                    } => {
                        let trimmed = line_text.trim_start();
                        let cut = line_text.len() - trimmed.len();
                        let shown: SharedString =
                            trimmed.chars().take(200).collect::<String>().into();
                        let ranges = ranges
                            .iter()
                            .filter(|r| r.start >= cut && r.end - cut <= shown.len())
                            .map(|r| r.start - cut..r.end - cut)
                            .collect();
                        let name = relative.rsplit('/').next().unwrap_or_default().to_string();
                        (
                            Some(FerroIcon::for_file(&name)),
                            highlighted(shown, ranges).into_any_element(),
                            Some(format!("{relative}:{}", line + 1).into()),
                            None,
                        )
                    }
                    QuickOpenItem::Message(message) => (
                        Some(FerroIcon::Info),
                        div().text_color(muted).child(message).into_any_element(),
                        None,
                        None,
                    ),
                };
                h_flex()
                    .id(ix)
                    .role(Role::ListBoxOption)
                    .aria_label(label)
                    .aria_selected(is_selected)
                    .aria_position_in_set(ix + 1)
                    .aria_size_of_set(self.items.len())
                    .h(px(ROW_HEIGHT))
                    .w_full()
                    .px_3()
                    .gap_2()
                    .text_sm()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .cursor_pointer()
                    .when(is_selected, |el| el.bg(selected_bg))
                    .hover(|el| el.bg(hover_bg))
                    .when_some(icon, |el, icon| {
                        el.child(Icon::from(icon).small().text_color(muted))
                    })
                    .child(div().flex_none().max_w(px(360.)).truncate().child(primary))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(muted)
                            .when_some(secondary, |el, s| el.child(s)),
                    )
                    .when_some(trailing, |el, t| {
                        el.child(div().flex_none().text_xs().text_color(muted).child(t))
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.selected = ix;
                        this.confirm(false, window, cx);
                    }))
                    .into_any_element()
            })
            .collect()
    }

    fn render_preview(&self, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let (muted, border, accent) = (theme.muted_foreground, theme.border, theme.primary);
        let body: AnyElement = match &self.preview {
            Preview::None => div().into_any_element(),
            Preview::Loading => h_flex()
                .gap_2()
                .p_2()
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
                title,
                first_line,
                lines,
                focus,
                ranges,
            } => {
                let highlight = HighlightStyle {
                    background_color: Some(accent.opacity(0.3)),
                    ..Default::default()
                };
                v_flex()
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .text_xs()
                            .text_color(muted)
                            .truncate()
                            .child(title.clone()),
                    )
                    .child(
                        v_flex()
                            .px_2()
                            .font_family(theme.mono_font_family.clone())
                            .text_xs()
                            .children(lines.iter().enumerate().map(|(i, line)| {
                                let number = first_line + i as u32;
                                let is_focus = Some(number) == *focus;
                                let text = if is_focus {
                                    StyledText::new(line.clone()).with_highlights(
                                        ranges
                                            .iter()
                                            .filter(|r| r.end <= line.len())
                                            .map(|r| (r.clone(), highlight)),
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
                                            .w(px(32.))
                                            .flex_none()
                                            .text_color(muted)
                                            .child(format!("{}", number + 1)),
                                    )
                                    .child(text)
                            })),
                    )
                    .into_any_element()
            }
        };
        div()
            .id("quick-open-preview")
            .aria_label(tr("search-preview-loading"))
            .w(px(300.))
            .flex_none()
            .h_full()
            .border_l_1()
            .border_color(border)
            .overflow_hidden()
            .child(body)
            .into_any_element()
    }
}

/// The lines shown in the preview: around `focus`, or the start of the file.
fn preview_lines(
    title: SharedString,
    text: &str,
    focus: Option<u32>,
    ranges: Vec<Range<usize>>,
) -> Preview {
    const BEFORE: u32 = 3;
    const SHOWN: usize = 13;
    let first = focus.map_or(0, |f| f.saturating_sub(BEFORE));
    let lines = text
        .lines()
        .skip(first as usize)
        .take(SHOWN)
        .map(|l| SharedString::from(l.chars().take(160).collect::<String>()))
        .collect();
    Preview::Lines {
        title,
        first_line: first,
        lines,
        focus,
        ranges,
    }
}

impl Focusable for QuickOpen {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl Render for QuickOpen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let count = self.items.len();
        let list_height = px(ROW_HEIGHT * VISIBLE_ROWS as f32);

        let body: AnyElement = match &self.status {
            Status::Loading if self.items.is_empty() => h_flex()
                .h(list_height)
                .justify_center()
                .gap_2()
                .text_sm()
                .text_color(muted)
                .child(Spinner::new().small())
                .child(tr("search-loading"))
                .into_any_element(),
            Status::Failed(message) => div()
                .h(list_height)
                .p_3()
                .text_sm()
                .text_color(theme.danger)
                .child(message.clone())
                .into_any_element(),
            _ if count == 0 => div()
                .h(list_height)
                .p_3()
                .text_sm()
                .text_color(muted)
                .child(tr(self.mode.empty_key()))
                .into_any_element(),
            _ => h_flex()
                .h(list_height)
                .w_full()
                .child(
                    div()
                        .id("quick-open-results")
                        .role(Role::ListBox)
                        .aria_label(tr("search-results-label"))
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(
                            uniform_list(
                                "quick-open-list",
                                count,
                                cx.processor(|this, range: Range<usize>, _, cx| {
                                    this.render_rows(range, cx)
                                }),
                            )
                            .track_scroll(&self.scroll)
                            .h_full(),
                        ),
                )
                .when(self.mode.has_preview(), |el| {
                    el.child(self.render_preview(cx))
                })
                .into_any_element(),
        };

        let side = if cfg!(target_os = "macos") {
            "⌘+Enter"
        } else {
            "Ctrl+Enter"
        };
        v_flex()
            .id("quick-open")
            .key_context(CONTEXT)
            .aria_label(tr("search-query-label"))
            .on_action(cx.listener(|this, _: &SelectPrevious, window, cx| {
                this.move_selection(-1, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &SelectNext, window, cx| this.move_selection(1, window, cx)),
            )
            .on_action(cx.listener(|this, _: &SelectPageUp, window, cx| {
                this.move_selection(-(VISIBLE_ROWS as isize), window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectPageDown, window, cx| {
                this.move_selection(VISIBLE_ROWS as isize, window, cx)
            }))
            .on_action(cx.listener(|this, _: &Confirm, window, cx| this.confirm(false, window, cx)))
            .on_action(
                cx.listener(|this, _: &ConfirmToSide, window, cx| this.confirm(true, window, cx)),
            )
            .on_action(cx.listener(|this, _: &Cancel, window, cx| this.cancel(window, cx)))
            .w_full()
            .gap_1()
            .child(
                Input::new(&self.input)
                    .aria_label(tr("search-query-label"))
                    .prefix(Icon::from(FerroIcon::Search).small().text_color(muted)),
            )
            .child(
                h_flex()
                    .px_1()
                    .gap_2()
                    .text_xs()
                    .text_color(muted)
                    .child(div().child(tr(self.mode.title_key())))
                    .when(matches!(self.status, Status::Loading), |el| {
                        el.child(Spinner::new().xsmall())
                    })
                    .child(div().flex_1())
                    .child(tr("search-hint")),
            )
            .child(
                div()
                    .border_1()
                    .border_color(border)
                    .rounded_md()
                    .overflow_hidden()
                    .child(body),
            )
            .child(
                div()
                    .px_1()
                    .text_xs()
                    .text_color(muted)
                    .child(tr_args("search-footer", &[("split", side.into())])),
            )
    }
}

// ---------------------------------------------------------------------------
// Opening
// ---------------------------------------------------------------------------

/// Open the popup in `mode` (its prefix is pre-filled).
pub fn open(
    workspace: &mut Workspace,
    mode: QuickOpenMode,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let _ = open_view(workspace, mode, window, cx);
}

/// Open the popup and return its view (tests drive it directly).
pub fn open_view(
    workspace: &mut Workspace,
    mode: QuickOpenMode,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Entity<QuickOpen> {
    bind_keys(cx);
    if window.has_active_dialog(cx) {
        window.close_dialog(cx);
    }
    let handle = cx.entity().downgrade();
    let view = cx.new(|cx| QuickOpen::new(workspace, handle, mode, window, cx));
    let content = view.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(820.))
            .margin_top(px(56.))
            .close_button(false)
            .child(content.clone())
    });
    view.read(cx)
        .input
        .clone()
        .update(cx, |input, cx| input.focus(window, cx));
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_select_modes() {
        assert_eq!(QuickOpenMode::parse("main"), (QuickOpenMode::Files, "main"));
        assert_eq!(
            QuickOpenMode::parse(">theme"),
            (QuickOpenMode::Commands, "theme")
        );
        assert_eq!(
            QuickOpenMode::parse("@greet"),
            (QuickOpenMode::SymbolsInFile, "greet")
        );
        assert_eq!(
            QuickOpenMode::parse("#Conf"),
            (QuickOpenMode::SymbolsInWorkspace, "Conf")
        );
        assert_eq!(QuickOpenMode::parse(":12"), (QuickOpenMode::GoToLine, "12"));
        assert_eq!(QuickOpenMode::parse("%TODO"), (QuickOpenMode::Text, "TODO"));
    }

    #[test]
    fn line_targets_parse_and_validate() {
        assert_eq!(parse_line_target("12", 40), Ok(TextPos::new(11, 0)));
        assert_eq!(parse_line_target(" 3:5 ", 40), Ok(TextPos::new(2, 4)));
        assert_eq!(parse_line_target("3:", 40), Ok(TextPos::new(2, 0)));
        assert_eq!(parse_line_target("", 40), Err(LineError::Empty));
        assert_eq!(
            parse_line_target("x", 40),
            Err(LineError::Invalid("x".into()))
        );
        assert_eq!(
            parse_line_target("0", 40),
            Err(LineError::Invalid("0".into()))
        );
        assert_eq!(parse_line_target("41", 40), Err(LineError::OutOfRange(41)));
    }

    #[test]
    fn byte_offsets_clamp_to_lines() {
        let text = "ab\nżółw\nx";
        assert_eq!(byte_offset(text, TextPos::new(0, 1)), 1);
        assert_eq!(byte_offset(text, TextPos::new(1, 2)), 3 + "żó".len());
        assert_eq!(byte_offset(text, TextPos::new(1, 99)), 3 + "żółw".len());
        assert_eq!(byte_offset(text, TextPos::new(9, 0)), text.len());
    }

    #[test]
    fn positions_merge_into_char_ranges() {
        assert_eq!(position_ranges("main.rs", &[0, 1, 2, 5]), vec![0..3, 5..6]);
        let s = "żółw";
        assert_eq!(position_ranges(s, &[0, 2]), vec![0..4]);
        assert!(position_ranges("ab", &[7]).is_empty());
    }

    fn t(ms: u64, base: Instant) -> Instant {
        base + Duration::from_millis(ms)
    }

    fn shift(down: bool) -> Modifiers {
        Modifiers {
            shift: down,
            ..Default::default()
        }
    }

    #[test]
    fn double_shift_fires_on_two_quick_taps() {
        let base = Instant::now();
        let mut d = DoubleShift::default();
        assert!(!d.modifiers_changed(shift(true), t(0, base)));
        assert!(!d.modifiers_changed(shift(false), t(50, base)));
        assert!(!d.modifiers_changed(shift(true), t(120, base)));
        assert!(d.modifiers_changed(shift(false), t(200, base)));
        // A third tap starts over.
        assert!(!d.modifiers_changed(shift(true), t(250, base)));
        assert!(!d.modifiers_changed(shift(false), t(260, base)));
    }

    #[test]
    fn double_shift_ignores_slow_taps_keys_and_chords() {
        let base = Instant::now();
        let mut d = DoubleShift::default();
        d.modifiers_changed(shift(true), t(0, base));
        d.modifiers_changed(shift(false), t(10, base));
        d.modifiers_changed(shift(true), t(500, base));
        assert!(!d.modifiers_changed(shift(false), t(510, base)), "too slow");

        let mut d = DoubleShift::default();
        d.modifiers_changed(shift(true), t(0, base));
        d.modifiers_changed(shift(false), t(10, base));
        d.key_down();
        d.modifiers_changed(shift(true), t(50, base));
        assert!(
            !d.modifiers_changed(shift(false), t(60, base)),
            "a key between"
        );

        let mut d = DoubleShift::default();
        d.modifiers_changed(shift(true), t(0, base));
        d.modifiers_changed(
            Modifiers {
                shift: true,
                control: true,
                ..Default::default()
            },
            t(5, base),
        );
        d.modifiers_changed(Modifiers::default(), t(10, base));
        d.modifiers_changed(shift(true), t(50, base));
        assert!(
            !d.modifiers_changed(shift(false), t(60, base)),
            "ctrl+shift is not a tap"
        );
    }

    #[test]
    fn recent_files_come_first_in_file_results() {
        let m = |p: &str| FileMatch {
            path: PathBuf::from(p),
            relative: p.into(),
            score: 0,
            positions: Vec::new(),
        };
        let items = file_items(vec![m("a"), m("b"), m("c")], &[PathBuf::from("c")]);
        let order: Vec<_> = items
            .iter()
            .map(|i| match i {
                QuickOpenItem::File {
                    relative, recent, ..
                } => (relative.to_string(), *recent),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            order,
            vec![("c".into(), true), ("a".into(), false), ("b".into(), false)]
        );
    }
}
