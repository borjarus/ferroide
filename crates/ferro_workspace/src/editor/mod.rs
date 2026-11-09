//! Editor tabs: the view layer over GPUI Kit's code editor.
//!
//! The text, caret, selections, highlighting, completion and hover popups
//! come from GPUI Kit's `EditorState`; FerroIDE adds, around it, everything
//! the plan's "Editor — view layer" asks for, fed by the mock services:
//! diagnostics, quick fixes, parameter info, VCS change markers, a glyph
//! margin with breakpoints and ▶ run icons, breadcrumbs, sticky lines,
//! inlay hints and the editor toolbar. The production editor
//! (`ferro_editor`, Stage 3) replaces the body without changing the tab,
//! split and persistence behavior here.

pub mod model;
pub mod providers;
mod view;

use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    time::Duration,
};

use ferro_services::{
    Breakpoint, DebugEvent, Diagnostic, InlayHint, LineChange, LineChangeKind, Runnable, Services,
    Severity, SignatureHelp, Symbol, TextPos, TextRange,
};
use ferro_settings::EditorSettings;
use ferro_ui::{FerroIcon, components, tr};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, SharedString,
    Styled as _, Subscription, Task, Window,
    component::{
        ActiveTheme as _, Icon, Sizable as _,
        button::Button,
        dock::{BasePanel, Panel, PanelControl, PanelEvent, PanelInfo, PanelState},
        highlighter::DiagnosticSeverity,
        input::{
            CodeActionItem, EditorState, GutterDecoration, GutterDecorationCollection, InputEvent,
            Position, RopeExt as _, TabSize,
        },
        spinner::Spinner,
    },
    div,
    prelude::FluentBuilder as _,
};
use serde::{Deserialize, Serialize};

use crate::app_state::AppState;
use providers::{CODE_ACTION_PROVIDER_ID, LanguageProviders, from_lsp_pos, to_lsp_pos};

pub const EDITOR_PANEL_NAME: &str = "ferro.editor";

/// How long typing must pause before diagnostics, hints and the outline are
/// recomputed.
pub const ANALYSIS_DEBOUNCE: Duration = Duration::from_millis(300);

/// What an editor tab persists in the layout file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EditorTabState {
    pub path: PathBuf,
    #[serde(default)]
    pub pinned: bool,
}

/// Emitted to the workspace.
#[derive(Debug, Clone)]
pub enum EditorEvent {
    /// The tab was shown or focused; it becomes the target of tab actions.
    Activated,
    /// The cursor moved.
    CursorMoved,
}

/// Editor commands the workspace routes to the active editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorCommand {
    TriggerCompletion,
    QuickFix,
    QuickDocumentation,
    ParameterInfo,
    ToggleBreakpoint,
    NextProblem,
    PreviousProblem,
}

enum Content {
    Loading,
    Loaded(Entity<EditorState>),
    Failed(SharedString),
}

/// Everything the language and VCS services say about the current text.
#[derive(Default, Clone)]
pub struct Analysis {
    pub diagnostics: Vec<Diagnostic>,
    pub symbols: Vec<Symbol>,
    pub inlay_hints: Vec<InlayHint>,
    pub runnables: Vec<Runnable>,
    pub line_changes: Vec<LineChange>,
    /// Inlay hints grouped per line, for the end-of-line overlay.
    pub line_hints: Vec<model::LineHint>,
    /// Byte ranges of the change markers.
    pub gutter: Vec<(std::ops::Range<usize>, LineChangeKind)>,
}

pub struct EditorPanel {
    path: PathBuf,
    pinned: bool,
    /// A preview tab (italic title) is replaced by the next preview.
    preview: bool,
    /// Where to put the caret once the file has loaded.
    pending_position: Option<TextPos>,
    content: Content,
    focus_handle: FocusHandle,
    services: Services,
    analysis: Analysis,
    /// Breakpoints of this file, from the debug service.
    breakpoints: Vec<Breakpoint>,
    gutter: GutterDecorationCollection,
    analysis_task: Task<()>,
    /// Open Parameter Info popup.
    signature: Option<SignatureHelp>,
    request_task: Task<()>,
    /// Settings the editor state was last configured with.
    applied: Option<EditorSettings>,
    /// Width of one monospace character, measured on first render.
    char_width: Option<Pixels>,
    /// Distance from the editor's left edge to column 0 of the text.
    text_left: Option<Pixels>,
    _subscriptions: Vec<Subscription>,
}

/// The language name GPUI Kit's highlighter knows the file by.
pub fn language_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
    {
        "rs" => "rust",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" => "javascript",
        "json" => "json",
        "toml" => "toml",
        "md" => "markdown",
        "py" => "python",
        "go" => "go",
        "java" => "java",
        _ => "plaintext",
    }
}

/// The human-readable language name for the status bar.
pub fn language_display(path: &Path) -> &'static str {
    match language_for(path) {
        "rust" => "Rust",
        "typescript" => "TypeScript",
        "javascript" => "JavaScript",
        "json" => "JSON",
        "toml" => "TOML",
        "markdown" => "Markdown",
        "python" => "Python",
        "go" => "Go",
        "java" => "Java",
        _ => "Plain text",
    }
}

type ReadFuture = Pin<Box<dyn Future<Output = anyhow::Result<String>> + Send>>;

/// Files inside the project come from the project service; anything else
/// (e.g. the raw `settings.toml`) is read from disk.
fn read_source(path: &Path, services: &Services) -> ReadFuture {
    let in_project = services
        .project
        .roots()
        .iter()
        .any(|root| path.starts_with(root));
    if in_project {
        services.project.read_file(path)
    } else {
        let path = path.to_path_buf();
        Box::pin(async move {
            std::fs::read_to_string(&path)
                .map_err(|err| anyhow::anyhow!("{}: {err}", path.display()))
        })
    }
}

fn severity(severity: Severity) -> DiagnosticSeverity {
    match severity {
        Severity::Error => DiagnosticSeverity::Error,
        Severity::Warning => DiagnosticSeverity::Warning,
        Severity::Info => DiagnosticSeverity::Info,
        Severity::Hint => DiagnosticSeverity::Hint,
    }
}

impl EditorPanel {
    pub fn open(
        path: PathBuf,
        pinned: bool,
        services: &Services,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let read = read_source(&path, services);
        let services = services.clone();
        cx.new(|cx| {
            let read = cx.background_spawn(read);
            cx.spawn_in(window, async move |this, cx| {
                let result = read.await;
                let _ = this.update_in(cx, |this: &mut Self, window, cx| {
                    this.content = match result {
                        Ok(text) => {
                            let state = this.create_state(text, window, cx);
                            // Focus given to the tab while loading moves
                            // into the text.
                            if this.focus_handle.contains_focused(window, cx) {
                                state.update(cx, |state, cx| state.focus(window, cx));
                            }
                            Content::Loaded(state)
                        }
                        Err(err) => Content::Failed(err.to_string().into()),
                    };
                    if let Some(position) = this.pending_position.take() {
                        this.go_to(position, window, cx);
                    }
                    this.schedule_analysis(Duration::ZERO, cx);
                    cx.notify();
                });
            })
            .detach();

            // Breakpoints change from the gutter, the Debug tool window or
            // another split of the same file.
            let events = services.debug.subscribe();
            cx.spawn(async move |this, cx| {
                while let Ok(event) = events.recv().await {
                    if event != DebugEvent::BreakpointsChanged {
                        continue;
                    }
                    if this
                        .update(cx, |this, cx| this.refresh_breakpoints(cx))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();

            let focus_handle = cx.focus_handle();
            cx.on_focus_in(&focus_handle, window, |_, _, cx| {
                cx.emit(EditorEvent::Activated)
            })
            .detach();
            let breakpoints = services.debug.breakpoints_in(&path);
            Self {
                path,
                pinned,
                preview: false,
                pending_position: None,
                content: Content::Loading,
                focus_handle,
                services,
                analysis: Analysis::default(),
                breakpoints,
                gutter: GutterDecorationCollection::default(),
                analysis_task: Task::ready(()),
                signature: None,
                request_task: Task::ready(()),
                applied: None,
                char_width: None,
                text_left: None,
                _subscriptions: Vec::new(),
            }
        })
    }

    fn create_state(
        &mut self,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<EditorState> {
        let settings = AppState::settings(cx).editor.clone();
        let providers = LanguageProviders::new(self.path.clone(), self.services.language.clone());
        let language = language_for(&self.path);
        let state = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .language(language)
                .line_number(settings.line_numbers)
                .soft_wrap(settings.soft_wrap)
                .tab_size(TabSize {
                    tab_size: settings.tab_size,
                    hard_tabs: false,
                })
                .searchable(true)
                .default_value(text);
            let lsp = state.lsp_mut();
            lsp.completion_provider = Some(providers.clone());
            lsp.hover_provider = Some(providers.clone());
            lsp.code_action_providers.push(providers);
            state
        });
        self.applied = Some(settings);
        self.gutter = state.update(cx, |state, cx| {
            state.create_gutter_decorations_collection(Vec::new(), cx)
        });
        self._subscriptions.push(cx.subscribe_in(
            &state,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::Change = event {
                    this.on_text_changed(window, cx);
                }
                cx.emit(EditorEvent::CursorMoved);
            },
        ));
        // Scrolling repaints the glyph margin, sticky lines and hints.
        self._subscriptions
            .push(cx.observe(&state, |_, _, cx| cx.notify()));
        state
    }

    fn state(&self) -> Option<&Entity<EditorState>> {
        match &self.content {
            Content::Loaded(state) => Some(state),
            _ => None,
        }
    }

    fn on_text_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Typing in a preview tab keeps it.
        self.set_preview(false, cx);
        self.schedule_analysis(ANALYSIS_DEBOUNCE, cx);
        if self.signature.is_some() {
            self.request_signature(window, cx);
        }
    }

    /// Recompute diagnostics, outline, hints, runnables and change markers
    /// after `delay`; a newer call replaces (cancels) a pending one.
    fn schedule_analysis(&mut self, delay: Duration, cx: &mut Context<Self>) {
        let Some(text) = self.text(cx) else { return };
        let path = self.path.clone();
        let services = self.services.clone();
        self.analysis_task = cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let analysis = cx
                .background_spawn(async move {
                    let language = &services.language;
                    let diagnostics = language.diagnostics(&path, &text);
                    let symbols = language.document_symbols(&path, &text);
                    let hints = language.inlay_hints(&path, &text);
                    let inlay_hints = hints.await.unwrap_or_default();
                    let line_changes = services.vcs.line_changes(&path, &text);
                    Analysis {
                        diagnostics: diagnostics.await.unwrap_or_default(),
                        symbols: symbols.await.unwrap_or_default(),
                        line_hints: model::end_of_line_hints(&inlay_hints, &text),
                        gutter: model::gutter_ranges(&line_changes, &text),
                        inlay_hints,
                        runnables: language.runnables(&path, &text),
                        line_changes,
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| this.apply_analysis(analysis, cx));
        });
    }

    fn apply_analysis(&mut self, analysis: Analysis, cx: &mut Context<Self>) {
        self.analysis = analysis;
        if let Some(state) = self.state().cloned() {
            let diagnostics = self.analysis.diagnostics.clone();
            state.update(cx, |state, cx| {
                let text = state.text().clone();
                if let Some(set) = state.diagnostics_mut() {
                    set.reset(&text);
                    set.extend(diagnostics.iter().map(|d| {
                        gpui_kit::component::highlighter::Diagnostic::new(
                            to_lsp_pos(d.range.start)..to_lsp_pos(d.range.end),
                            d.message.clone(),
                        )
                        .with_severity(severity(d.severity))
                        .with_source(d.source.clone())
                    }));
                }
                cx.notify();
            });
            self.update_gutter(cx);
        }
        cx.notify();
    }

    /// Push the VCS change markers to the editor's gutter strip.
    fn update_gutter(&mut self, cx: &mut Context<Self>) {
        if !self.is_loaded() || !AppState::settings(cx).editor.vcs_markers {
            self.gutter.clear(cx);
            return;
        }
        let theme = cx.theme();
        let (added, modified, deleted) = (theme.success, theme.info, theme.danger);
        let marks = self
            .analysis
            .gutter
            .iter()
            .map(|(range, kind)| {
                GutterDecoration::new(range.clone()).with_color(match kind {
                    LineChangeKind::Added => added,
                    LineChangeKind::Modified => modified,
                    LineChangeKind::Deleted => deleted,
                })
            })
            .collect();
        self.gutter.set(marks, cx);
    }

    fn refresh_breakpoints(&mut self, cx: &mut Context<Self>) {
        self.breakpoints = self.services.debug.breakpoints_in(&self.path);
        cx.notify();
    }

    /// Apply the editor settings when they change (Settings window, toolbar).
    fn apply_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = AppState::settings(cx).editor.clone();
        if self.applied.as_ref() == Some(&settings) {
            return;
        }
        let vcs_changed = self
            .applied
            .as_ref()
            .is_some_and(|a| a.vcs_markers != settings.vcs_markers);
        if let Some(state) = self.state().cloned() {
            state.update(cx, |state, cx| {
                state.set_line_number(settings.line_numbers, window, cx);
                state.set_soft_wrap(settings.soft_wrap, window, cx);
                state.set_tab_size(
                    TabSize {
                        tab_size: settings.tab_size,
                        hard_tabs: false,
                    },
                    cx,
                );
            });
            self.applied = Some(settings);
            self.text_left = None;
            if vcs_changed {
                self.update_gutter(cx);
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn file_name(&self) -> SharedString {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
            .into()
    }

    pub fn is_preview(&self) -> bool {
        self.preview
    }

    pub fn set_preview(&mut self, preview: bool, cx: &mut Context<Self>) {
        if self.preview != preview {
            self.preview = preview;
            cx.notify();
        }
    }

    /// Move the caret to `position` (0-based), now or once loaded.
    pub fn go_to(&mut self, position: TextPos, window: &mut Window, cx: &mut Context<Self>) {
        match &self.content {
            Content::Loaded(state) => state.update(cx, |state, cx| {
                state.set_cursor_position(Position::new(position.line, position.column), window, cx)
            }),
            _ => self.pending_position = Some(position),
        }
    }

    /// The current text, when loaded.
    pub fn text(&self, cx: &App) -> Option<String> {
        self.state().map(|state| state.read(cx).value().to_string())
    }

    /// 0-based caret position, when loaded.
    pub fn caret(&self, cx: &App) -> Option<TextPos> {
        self.cursor(cx).map(|(l, c)| TextPos::new(l - 1, c - 1))
    }

    /// The latest analysis of the text (diagnostics, outline, hints, …).
    pub fn analysis(&self) -> &Analysis {
        &self.analysis
    }

    /// Breakpoints of this file, as the glyph margin shows them.
    pub fn breakpoints(&self) -> &[Breakpoint] {
        &self.breakpoints
    }

    /// Errors and warnings of this file, for the status bar.
    pub fn problem_counts(&self) -> (usize, usize) {
        model::problem_counts(&self.analysis.diagnostics)
    }

    /// The open Parameter Info popup.
    pub fn signature_help(&self) -> Option<&SignatureHelp> {
        self.signature.as_ref()
    }

    /// The breadcrumb symbols at the caret, outermost first.
    pub fn breadcrumb_symbols(&self, cx: &App) -> Vec<Symbol> {
        let caret = self.caret(cx).unwrap_or_default();
        model::symbol_chain(&self.analysis.symbols, caret)
            .into_iter()
            .cloned()
            .collect()
    }

    /// Run an editor command (completion, quick fix, breakpoints, …).
    pub fn run_command(
        &mut self,
        command: EditorCommand,
        _services: &Services,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.state().cloned() else {
            return;
        };
        match command {
            EditorCommand::TriggerCompletion => {
                state.update(cx, |state, cx| {
                    state.focus(window, cx);
                    state.show_completions(window, cx);
                });
            }
            EditorCommand::QuickFix => self.request_quick_fixes(&state, window, cx),
            EditorCommand::QuickDocumentation => self.request_documentation(&state, window, cx),
            EditorCommand::ParameterInfo => self.request_signature(window, cx),
            EditorCommand::ToggleBreakpoint => {
                if let Some(caret) = self.caret(cx) {
                    self.toggle_breakpoint(caret.line, cx);
                }
            }
            EditorCommand::NextProblem | EditorCommand::PreviousProblem => {
                let caret = self.caret(cx).unwrap_or_default();
                let forward = command == EditorCommand::NextProblem;
                if let Some(target) =
                    model::next_problem(&self.analysis.diagnostics, caret, forward)
                {
                    self.go_to(target, window, cx);
                }
            }
        }
    }

    pub fn toggle_breakpoint(&mut self, line: u32, cx: &mut Context<Self>) {
        self.services.debug.toggle_breakpoint(&self.path, line);
        self.refresh_breakpoints(cx);
    }

    fn request_quick_fixes(
        &mut self,
        state: &Entity<EditorState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (text, range) = {
            let state = state.read(cx);
            let rope = state.text();
            let selected = state.selected_range();
            (
                rope.to_string(),
                TextRange::new(
                    from_lsp_pos(rope.offset_to_position(selected.start)),
                    from_lsp_pos(rope.offset_to_position(selected.end)),
                ),
            )
        };
        let request = self
            .services
            .language
            .code_actions(&self.path, &text, range);
        let state = state.clone();
        state.update(cx, |state, cx| state.focus(window, cx));
        self.request_task = cx.spawn(async move |_, cx| {
            let Ok(fixes) = request.await else { return };
            let items: Vec<CodeActionItem> = providers::code_actions(&fixes)
                .into_iter()
                .map(|action| CodeActionItem {
                    provider_id: CODE_ACTION_PROVIDER_ID.into(),
                    action,
                })
                .collect();
            state.update(cx, |state, cx| state.present_code_actions(items, cx));
        });
    }

    fn request_documentation(
        &mut self,
        state: &Entity<EditorState>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(caret) = self.caret(cx) else { return };
        let Some(text) = self.text(cx) else { return };
        let request = self.services.language.hover(&self.path, &text, caret);
        let state = state.clone();
        self.request_task = cx.spawn(async move |_, cx| {
            let Ok(Some(info)) = request.await else {
                return;
            };
            state.update(cx, |state, cx| {
                let rope = state.text();
                let range = rope.position_to_offset(&to_lsp_pos(info.range.start))
                    ..rope.position_to_offset(&to_lsp_pos(info.range.end));
                state.present_hover(range, providers::hover(&info), cx);
            });
        });
    }

    fn request_signature(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let (Some(caret), Some(text)) = (self.caret(cx), self.text(cx)) else {
            return;
        };
        let request = self
            .services
            .language
            .signature_help(&self.path, &text, caret);
        self.request_task = cx.spawn(async move |this, cx| {
            let help = request.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.signature = help;
                cx.notify();
            });
        });
    }

    pub fn close_signature(&mut self, cx: &mut Context<Self>) {
        if self.signature.take().is_some() {
            cx.notify();
        }
    }

    /// Move the caret without focusing the editor (live preview of Go to
    /// Line from Quick Open).
    pub fn preview_position(&mut self, position: TextPos, cx: &mut Context<Self>) {
        if let Content::Loaded(state) = &self.content {
            state.update(cx, |state, cx| {
                let offset = crate::quick_open::byte_offset(&state.value(), position);
                state.set_selected_range(offset..offset, cx);
            });
        }
    }

    /// Replace the whole text as one undoable edit (Replace in Files).
    pub fn replace_text(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Content::Loaded(state) = &self.content {
            state.update(cx, |state, cx| state.replace_all(text, window, cx));
        }
    }

    /// Replace the whole text (Rename applies its edits this way).
    pub fn set_text(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Content::Loaded(state) = &self.content {
            state.update(cx, |state, cx| state.set_value(text, window, cx));
        }
    }

    pub fn is_pinned(&self) -> bool {
        self.pinned
    }

    pub fn set_pinned(&mut self, pinned: bool, cx: &mut Context<Self>) {
        self.pinned = pinned;
        cx.notify();
    }

    /// 1-based line and column of the cursor, when the file is loaded.
    pub fn cursor(&self, cx: &App) -> Option<(u32, u32)> {
        self.state().map(|state| {
            let Position { line, character } = state.read(cx).cursor_position();
            (line + 1, character + 1)
        })
    }

    pub fn is_loaded(&self) -> bool {
        matches!(self.content, Content::Loaded(_))
    }

    pub fn tab_state(&self) -> EditorTabState {
        EditorTabState {
            path: self.path.clone(),
            pinned: self.pinned,
        }
    }
}

impl BasePanel for EditorPanel {
    fn panel_name(&self) -> &'static str {
        EDITOR_PANEL_NAME
    }

    /// A pinned tab cannot be closed by the close button or close actions.
    fn closable(&self, _: &App) -> bool {
        !self.pinned
    }

    fn zoomable(&self, _: &App) -> bool {
        true
    }

    fn set_active(&mut self, active: bool, _: &mut Window, cx: &mut Context<Self>) {
        if active {
            cx.emit(EditorEvent::Activated);
        }
    }

    fn dump(&self, _: &App) -> PanelState {
        PanelState {
            panel_name: EDITOR_PANEL_NAME.to_string(),
            children: Vec::new(),
            info: PanelInfo::panel(serde_json::to_value(self.tab_state()).unwrap_or_default()),
        }
    }
}

impl Panel for EditorPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(self.file_name())
    }

    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let name = self.file_name();
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                Icon::from(FerroIcon::for_file(&name))
                    .small()
                    .text_color(muted),
            )
            .child(div().when(self.preview, |el| el.italic()).child(name))
            .when(self.pinned, |el| {
                el.child(Icon::from(FerroIcon::Pin).xsmall().text_color(muted))
            })
    }

    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        Some(PanelControl::Menu)
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }

    /// The pane's tab-bar icons (Zed): Back / Forward, new file, split.
    fn toolbar_buttons(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<Vec<Button>> {
        let tip = |key: &str, action: &str, cx: &App| {
            crate::chrome::with_shortcut(tr(&format!("action-{key}")), action, cx)
        };
        Some(vec![
            Button::new("pane-back")
                .icon(Icon::from(FerroIcon::Back))
                .tooltip(tip("go-back", "ferro::GoBack", cx))
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(crate::actions::GoBack), cx)
                }),
            Button::new("pane-forward")
                .icon(Icon::from(FerroIcon::Forward))
                .tooltip(tip("go-forward", "ferro::GoForward", cx))
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(crate::actions::GoForward), cx)
                }),
            Button::new("pane-new-file")
                .icon(Icon::from(FerroIcon::Add))
                .tooltip(tip("new-file", "ferro::NewFile", cx))
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(crate::actions::NewFile), cx)
                }),
            Button::new("pane-split")
                .icon(Icon::from(FerroIcon::SplitRight))
                .tooltip(tip("split-right", "ferro::SplitRight", cx))
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(crate::actions::SplitRight), cx)
                }),
        ])
    }
}

impl EventEmitter<PanelEvent> for EditorPanel {}
impl EventEmitter<EditorEvent> for EditorPanel {}

impl Focusable for EditorPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.content {
            Content::Loaded(state) => state.read(cx).focus_handle(cx),
            _ => self.focus_handle.clone(),
        }
    }
}

impl Render for EditorPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.apply_settings(window, cx);
        let body = match &self.content {
            Content::Loading => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(cx.theme().muted_foreground)
                .child(Spinner::new().small())
                .child(tr("editor-loading"))
                .into_any_element(),
            Content::Loaded(state) => {
                let state = state.clone();
                self.render_loaded(&state, window, cx).into_any_element()
            }
            Content::Failed(err) => components::empty_state(
                FerroIcon::Error,
                tr("editor-load-failed"),
                Some(err.clone()),
                cx,
            )
            .into_any_element(),
        };
        div()
            .id("editor-panel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().background)
            .child(body)
    }
}
