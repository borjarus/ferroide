//! The Structure tool window: the outline of the active editor's file, from
//! the language service, with a filter.

use std::{collections::HashSet, ops::Range, path::PathBuf};

use ferro_services::{Services, Symbol};
use ferro_ui::{FerroIcon, components, tr};
use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, ClickEvent, Context, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, ScrollStrategy,
    SharedString, StatefulInteractiveElement as _, Styled as _, UniformListScrollHandle, Window,
    component::{
        ActiveTheme as _, Icon, Sizable as _, h_flex,
        input::{Input, InputEvent, InputState},
        spinner::Spinner,
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px, uniform_list,
};

use super::common::{self, NodePath, ROW_HEIGHT};
use crate::{requests::OpenLocation, tool_window::ToolWindowKind, workspace::Workspace};

enum State {
    NoFile,
    Loading,
    Loaded,
    Failed(SharedString),
}

pub struct StructureView {
    services: Services,
    path: Option<PathBuf>,
    state: State,
    symbols: Vec<Symbol>,
    filter: Entity<InputState>,
    collapsed: HashSet<NodePath>,
    rows: Vec<NodePath>,
    selected: Option<usize>,
    /// Bumped per request, so a slow answer for an old file is dropped.
    generation: u64,
    focus_handle: FocusHandle,
    scroll: UniformListScrollHandle,
}

pub fn build(services: Services, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| StructureView::new(services, window, cx)).into()
}

/// Point the Structure tool window (if it exists) at the workspace's active
/// editor. Called by the workspace when an editor is activated.
pub fn active_editor_changed(workspace: &Workspace, cx: &mut App) {
    let Some(view) = workspace
        .tool_windows
        .get(&ToolWindowKind::Structure)
        .and_then(|panel| panel.read(cx).body().downcast::<StructureView>())
    else {
        return;
    };
    let file = workspace.active_editor_entity().map(|editor| {
        let editor = editor.read(cx);
        (editor.path().to_path_buf(), editor.text(cx))
    });
    view.update(cx, |view, cx| match file {
        Some((path, text)) => view.set_file(path, text, cx),
        None => view.clear(cx),
    });
}

fn kids(symbol: &Symbol) -> &[Symbol] {
    &symbol.children
}

impl StructureView {
    pub fn new(services: Services, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter =
            cx.new(|cx| InputState::new(window, cx).placeholder(tr("panels-structure-filter")));
        cx.subscribe(&filter, |this: &mut Self, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                this.rebuild(cx);
                cx.notify();
            }
        })
        .detach();
        Self {
            services,
            path: None,
            state: State::NoFile,
            symbols: Vec::new(),
            filter,
            collapsed: HashSet::new(),
            rows: Vec::new(),
            selected: None,
            generation: 0,
            focus_handle: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
        }
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.path = None;
        self.state = State::NoFile;
        self.symbols.clear();
        self.rows.clear();
        cx.notify();
    }

    /// Show the outline of `path`. `text` is the editor's current text;
    /// `None` reads the file from the project.
    pub fn set_file(&mut self, path: PathBuf, text: Option<String>, cx: &mut Context<Self>) {
        if self.path.as_ref() == Some(&path)
            && matches!(self.state, State::Loaded)
            && text.is_none()
        {
            return;
        }
        self.generation += 1;
        let generation = self.generation;
        self.path = Some(path.clone());
        self.state = State::Loading;
        self.selected = None;
        self.collapsed.clear();
        let language = self.services.language.clone();
        let read = text
            .is_none()
            .then(|| self.services.project.read_file(&path));
        let task = cx.background_spawn(async move {
            let text = match (text, read) {
                (Some(text), _) => text,
                (None, Some(read)) => read.await?,
                (None, None) => String::new(),
            };
            language.document_symbols(&path, &text).await
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok(symbols) => {
                        this.symbols = symbols;
                        this.state = State::Loaded;
                    }
                    Err(err) => this.state = State::Failed(err.to_string().into()),
                }
                this.rebuild(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn file(&self) -> Option<&PathBuf> {
        self.path.as_ref()
    }

    fn rebuild(&mut self, cx: &App) {
        let query = self.filter.read(cx).value().to_lowercase();
        let collapsed = &self.collapsed;
        self.rows = common::flatten(
            &self.symbols,
            &kids,
            // Filtering shows every match, even inside collapsed nodes.
            &|p| query.is_empty() && collapsed.contains(p),
            &|s| query.is_empty() || s.name.to_lowercase().contains(&query),
        );
        if self.selected.is_some_and(|ix| ix >= self.rows.len()) {
            self.selected = None;
        }
    }

    /// Names of the visible outline rows (tests).
    pub fn row_names(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter_map(|p| common::node_at(&self.symbols, p, &kids))
            .map(|s| s.name.clone())
            .collect()
    }

    pub fn is_loaded(&self) -> bool {
        matches!(self.state, State::Loaded)
    }

    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.selected = Some(ix);
        self.scroll.scroll_to_item(ix, ScrollStrategy::Center);
        cx.notify();
    }

    fn open(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let target = self
            .rows
            .get(ix)
            .and_then(|p| common::node_at(&self.symbols, p, &kids))
            .map(|s| (s.path.clone(), s.selection));
        if let Some((path, pos)) = target {
            window.dispatch_action(Box::new(OpenLocation::at(path, pos)), cx);
        }
    }

    fn toggle(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(path) = self.rows.get(ix).cloned() {
            if !self.collapsed.remove(&path) {
                self.collapsed.insert(path);
            }
            self.rebuild(cx);
            cx.notify();
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if common::has_command_modifier(&event.keystroke) {
            return;
        }
        let key = event.keystroke.key.as_str();
        if let Some(ix) = common::nav(key, self.selected, self.rows.len()) {
            self.select(ix, cx);
        } else if let Some(ix) = self.selected {
            let collapsed = self.collapsed.contains(&self.rows[ix]);
            match key {
                "enter" => self.open(ix, window, cx),
                "left" if !collapsed => self.toggle(ix, cx),
                "right" if collapsed => self.toggle(ix, cx),
                _ => return,
            }
        } else {
            return;
        }
        cx.stop_propagation();
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let (muted, selected_bg, hover_bg, accent) = (
            theme.muted_foreground,
            theme.list_active,
            theme.list_hover,
            theme.primary,
        );
        let query = self.filter.read(cx).value().to_string();
        range
            .filter_map(|ix| {
                let path = self.rows.get(ix)?;
                let symbol = common::node_at(&self.symbols, path, &kids)?;
                Some((ix, path.len() - 1, symbol))
            })
            .map(|(ix, depth, symbol)| {
                let has_children = !symbol.children.is_empty();
                let open = query.is_empty() && !self.collapsed.contains(&self.rows[ix])
                    || !query.is_empty();
                let name: SharedString = symbol.name.clone().into();
                let ranges = common::substring_ranges(&symbol.name, &query);
                h_flex()
                    .id(ix)
                    .h(px(ROW_HEIGHT))
                    .pl(px(4. + depth as f32 * 14.))
                    .pr_2()
                    .gap_1()
                    .w_full()
                    .text_sm()
                    .cursor_pointer()
                    .aria_label(name.clone())
                    .when(Some(ix) == self.selected, |el| el.bg(selected_bg))
                    .hover(|el| el.bg(hover_bg))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.focus_handle.focus(window, cx);
                        this.select(ix, cx);
                        this.open(ix, window, cx);
                    }))
                    .child(div().w(px(14.)).flex_none().when(has_children, |el| {
                        el.child(
                            Icon::from(if open {
                                FerroIcon::ChevronDown
                            } else {
                                FerroIcon::ChevronRight
                            })
                            .xsmall()
                            .text_color(muted),
                        )
                    }))
                    .child(
                        Icon::from(common::symbol_icon(symbol.kind))
                            .small()
                            .text_color(muted),
                    )
                    .child(
                        div()
                            .truncate()
                            .child(common::highlighted(name, ranges, accent)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(format!(":{}", symbol.selection.line + 1)),
                    )
                    .into_any_element()
            })
            .collect()
    }
}

impl Focusable for StructureView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for StructureView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.state {
            State::NoFile => components::empty_state(
                FerroIcon::Structure,
                tr("panels-structure-no-file"),
                None,
                cx,
            )
            .into_any_element(),
            State::Loading => h_flex()
                .size_full()
                .justify_center()
                .gap_2()
                .text_color(cx.theme().muted_foreground)
                .child(Spinner::new().small())
                .child(tr("panels-loading"))
                .into_any_element(),
            State::Failed(err) => components::empty_state(
                FerroIcon::Error,
                tr("panels-load-failed"),
                Some(err.clone()),
                cx,
            )
            .into_any_element(),
            State::Loaded if self.rows.is_empty() => components::empty_state(
                FerroIcon::Structure,
                tr("panels-structure-empty"),
                None,
                cx,
            )
            .into_any_element(),
            State::Loaded => uniform_list(
                "structure-rows",
                self.rows.len(),
                cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx)),
            )
            .track_scroll(&self.scroll)
            .size_full()
            .into_any_element(),
        };
        v_flex()
            .id("structure")
            .track_focus(&self.focus_handle)
            .aria_label(tr("tool-structure"))
            .size_full()
            .on_key_down(cx.listener(Self::on_key_down))
            .child(
                div()
                    .p_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        Input::new(&self.filter)
                            .small()
                            .aria_label(tr("panels-structure-filter")),
                    ),
            )
            .child(div().flex_1().min_h_0().child(body))
    }
}
