//! The TODO tool window: TODO and FIXME comments of the project, grouped by
//! file.

use std::{collections::BTreeMap, ops::Range};

use ferro_services::{Services, TextPos, TodoItem, TodoKind};
use ferro_ui::{FerroIcon, components, tr, tr_args};
use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, ClickEvent, Context, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, ScrollStrategy,
    SharedString, StatefulInteractiveElement as _, Styled as _, UniformListScrollHandle, Window,
    component::{ActiveTheme as _, Icon, Sizable as _, h_flex, spinner::Spinner, v_flex},
    div,
    prelude::FluentBuilder as _,
    px, uniform_list,
};

use super::common::{self, ROW_HEIGHT};
use crate::requests::OpenLocation;

enum State {
    Loading,
    Loaded,
    Failed(SharedString),
}

#[derive(Clone)]
enum TodoRow {
    File(SharedString, usize),
    Item(usize),
}

pub struct TodoView {
    services: Services,
    state: State,
    items: Vec<TodoItem>,
    rows: Vec<TodoRow>,
    selected: Option<usize>,
    focus_handle: FocusHandle,
    scroll: UniformListScrollHandle,
}

pub fn build(services: Services, _window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| TodoView::new(services, cx)).into()
}

impl TodoView {
    pub fn new(services: Services, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            services,
            state: State::Loading,
            items: Vec::new(),
            rows: Vec::new(),
            selected: None,
            focus_handle: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
        };
        this.refresh(cx);
        this
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.state = State::Loading;
        let task = cx.background_spawn(self.services.search.todos());
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(items) => this.set_items(items),
                    Err(err) => this.state = State::Failed(err.to_string().into()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn set_items(&mut self, items: Vec<TodoItem>) {
        let mut by_file: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (ix, item) in items.iter().enumerate() {
            by_file.entry(item.relative.clone()).or_default().push(ix);
        }
        let mut rows = Vec::new();
        for (file, mut ixs) in by_file {
            ixs.sort_by_key(|&ix| items[ix].line);
            rows.push(TodoRow::File(file.into(), ixs.len()));
            rows.extend(ixs.into_iter().map(TodoRow::Item));
        }
        self.items = items;
        self.rows = rows;
        self.state = State::Loaded;
        self.selected = None;
    }

    pub fn counts(&self) -> (usize, usize) {
        let todo = self
            .items
            .iter()
            .filter(|i| i.kind == TodoKind::Todo)
            .count();
        (todo, self.items.len() - todo)
    }

    pub fn file_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| matches!(r, TodoRow::File(..)))
            .count()
    }

    fn open(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(TodoRow::Item(i)) = self.rows.get(ix) {
            let item = &self.items[*i];
            window.dispatch_action(
                Box::new(OpenLocation::at(
                    item.path.clone(),
                    TextPos::new(item.line, 0),
                )),
                cx,
            );
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if common::has_command_modifier(&event.keystroke) {
            return;
        }
        let key = event.keystroke.key.as_str();
        if let Some(ix) = common::nav(key, self.selected, self.rows.len()) {
            self.selected = Some(ix);
            self.scroll.scroll_to_item(ix, ScrollStrategy::Center);
            cx.notify();
        } else if key == "enter"
            && let Some(ix) = self.selected
        {
            self.open(ix, window, cx);
        } else {
            return;
        }
        cx.stop_propagation();
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let (muted, selected_bg, hover_bg, info, warning) = (
            theme.muted_foreground,
            theme.list_active,
            theme.list_hover,
            theme.info,
            theme.warning,
        );
        range
            .filter_map(|ix| self.rows.get(ix).cloned().map(|r| (ix, r)))
            .map(|(ix, row)| {
                let base = h_flex()
                    .id(ix)
                    .h(px(ROW_HEIGHT))
                    .px_2()
                    .gap_1()
                    .w_full()
                    .text_sm()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .cursor_pointer()
                    .when(Some(ix) == self.selected, |el| el.bg(selected_bg))
                    .hover(|el| el.bg(hover_bg))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.focus_handle.focus(window, cx);
                        this.selected = Some(ix);
                        this.open(ix, window, cx);
                        cx.notify();
                    }));
                match row {
                    TodoRow::File(file, count) => base
                        .aria_label(file.clone())
                        .child(
                            Icon::from(FerroIcon::for_file(&file))
                                .small()
                                .text_color(muted),
                        )
                        .child(div().truncate().child(file))
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(tr_args("panels-todo-count", &[("count", count.into())])),
                        )
                        .into_any_element(),
                    TodoRow::Item(i) => {
                        let item = &self.items[i];
                        let (marker, color) = match item.kind {
                            TodoKind::Todo => ("TODO", info),
                            TodoKind::Fixme => ("FIXME", warning),
                        };
                        base.pl(px(24.))
                            .aria_label(SharedString::from(item.text.clone()))
                            .child(Icon::from(FerroIcon::Todo).small().text_color(color))
                            .child(div().text_color(color).child(marker))
                            .child(div().flex_1().truncate().child(item.text.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(format!(":{}", item.line + 1)),
                            )
                            .into_any_element()
                    }
                }
            })
            .collect()
    }
}

impl Focusable for TodoView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TodoView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (todo, fixme) = self.counts();
        let toolbar = common::toolbar("todo-toolbar", cx)
            .child(tr_args(
                "panels-todo-summary",
                &[("todo", todo.into()), ("fixme", fixme.into())],
            ))
            .child(div().flex_1())
            .child(
                common::tool_button("todo-refresh", FerroIcon::Refresh, tr("panels-refresh"))
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            );
        let body = match &self.state {
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
            State::Loaded if self.rows.is_empty() => {
                components::empty_state(FerroIcon::Todo, tr("panels-todo-empty"), None, cx)
                    .into_any_element()
            }
            State::Loaded => uniform_list(
                "todo-rows",
                self.rows.len(),
                cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx)),
            )
            .track_scroll(&self.scroll)
            .size_full()
            .into_any_element(),
        };
        v_flex()
            .id("todo")
            .track_focus(&self.focus_handle)
            .aria_label(tr("tool-todo"))
            .size_full()
            .on_key_down(cx.listener(Self::on_key_down))
            .child(toolbar)
            .child(div().flex_1().min_h_0().child(body))
    }
}
