//! Editor tabs. Stage 1 shows files from the mock project in GPUI Kit's code
//! editor; the production editor (`ferro_editor`) replaces the body in
//! Stage 2–3 without changing the tab, split and persistence behavior here.

use std::path::{Path, PathBuf};

use ferro_services::Services;
use ferro_ui::{FerroIcon, components, tr};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Window,
    component::{
        ActiveTheme as _, Icon, Sizable as _,
        dock::{BasePanel, Panel, PanelControl, PanelEvent, PanelInfo, PanelState},
        input::{Editor, EditorState, Position},
        spinner::Spinner,
    },
    div,
    prelude::FluentBuilder as _,
};
use serde::{Deserialize, Serialize};

pub const EDITOR_PANEL_NAME: &str = "ferro.editor";

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

enum Content {
    Loading,
    Loaded(Entity<EditorState>),
    Failed(SharedString),
}

pub struct EditorPanel {
    path: PathBuf,
    pinned: bool,
    content: Content,
    focus_handle: FocusHandle,
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

impl EditorPanel {
    pub fn open(
        path: PathBuf,
        pinned: bool,
        services: &Services,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let read = services.project.read_file(&path);
        cx.new(|cx| {
            let language = language_for(&path);
            cx.spawn_in(window, async move |this, cx| {
                let result = read.await;
                let _ = this.update_in(cx, |this: &mut Self, window, cx| {
                    this.content = match result {
                        Ok(text) => {
                            let state = cx.new(|cx| {
                                EditorState::new(window, cx)
                                    .language(language)
                                    .line_number(true)
                                    .default_value(text)
                            });
                            cx.subscribe(
                                &state,
                                |_, _, _: &gpui_kit::component::input::InputEvent, cx| {
                                    cx.emit(EditorEvent::CursorMoved);
                                },
                            )
                            .detach();
                            Content::Loaded(state)
                        }
                        Err(err) => Content::Failed(err.to_string().into()),
                    };
                    cx.notify();
                });
            })
            .detach();
            let focus_handle = cx.focus_handle();
            cx.on_focus_in(&focus_handle, window, |_, _, cx| {
                cx.emit(EditorEvent::Activated)
            })
            .detach();
            Self {
                path,
                pinned,
                content: Content::Loading,
                focus_handle,
            }
        })
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

    pub fn is_pinned(&self) -> bool {
        self.pinned
    }

    pub fn set_pinned(&mut self, pinned: bool, cx: &mut Context<Self>) {
        self.pinned = pinned;
        cx.notify();
    }

    /// 1-based line and column of the cursor, when the file is loaded.
    pub fn cursor(&self, cx: &App) -> Option<(u32, u32)> {
        match &self.content {
            Content::Loaded(state) => {
                let Position { line, character } = state.read(cx).cursor_position();
                Some((line + 1, character + 1))
            }
            _ => None,
        }
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
            .child(name)
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            Content::Loaded(state) => Editor::new(state)
                .bordered(false)
                .h_full()
                .into_any_element(),
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
