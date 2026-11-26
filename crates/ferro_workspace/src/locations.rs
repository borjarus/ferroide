//! A list of code locations with a preview: Find Usages, Go to
//! Implementation with several targets, call and type hierarchies.
//!
//! Results are grouped by file; the selected one shows ±3 lines of context
//! (from an open editor, else read through the project service). Enter or a
//! double click opens it; Escape closes the list.

use std::{ops::Range, path::PathBuf};

use ferro_services::{Location, Services};
use ferro_ui::{FerroIcon, tr, tr_args};
use gpui_kit::{
    AnyElement, App, AppContext as _, ClickEvent, Context, Entity, FocusHandle, Focusable, Global,
    HighlightStyle, InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _, Render,
    Role, ScrollStrategy, SharedString, StatefulInteractiveElement as _, Styled as _, StyledText,
    Task, UniformListScrollHandle, WeakEntity, Window,
    component::{ActiveTheme as _, Icon, Sizable as _, WindowExt as _, h_flex, v_flex},
    div,
    prelude::FluentBuilder as _,
    px, uniform_list,
};

use crate::{requests::OpenLocation, workspace::Workspace};

const ROW_HEIGHT: f32 = 24.;
const CONTEXT_LINES: u32 = 3;
const CONTEXT: &str = "Locations";

gpui_kit::actions!(locations, [SelectPrevious, SelectNext, OpenSelected, Close]);

struct KeysBound;
impl Global for KeysBound {}

fn bind_keys(cx: &mut App) {
    if cx.has_global::<KeysBound>() {
        return;
    }
    cx.set_global(KeysBound);
    let context = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", SelectPrevious, context),
        KeyBinding::new("down", SelectNext, context),
        KeyBinding::new("enter", OpenSelected, context),
        KeyBinding::new("escape", Close, context),
    ]);
}

/// One entry: a location, with a label for hierarchy items (`main`,
/// `↑ Shape`).
#[derive(Debug, Clone, PartialEq)]
pub struct LocationItem {
    pub location: Location,
    pub label: Option<String>,
}

impl From<Location> for LocationItem {
    fn from(location: Location) -> Self {
        Self {
            location,
            label: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    File {
        relative: SharedString,
        count: usize,
    },
    Item(usize),
}

/// Group items by file (keeping their order within a file, files in order
/// of first appearance).
pub fn build_rows(
    items: &[LocationItem],
    relative: impl Fn(&PathBuf) -> String,
) -> (Vec<LocationItem>, Vec<Row>) {
    let mut files: Vec<PathBuf> = Vec::new();
    for item in items {
        if !files.contains(&item.location.path) {
            files.push(item.location.path.clone());
        }
    }
    let mut ordered = Vec::with_capacity(items.len());
    let mut rows = Vec::with_capacity(items.len() + files.len());
    for file in files {
        let of_file: Vec<&LocationItem> =
            items.iter().filter(|i| i.location.path == file).collect();
        rows.push(Row::File {
            relative: relative(&file).into(),
            count: of_file.len(),
        });
        for item in of_file {
            rows.push(Row::Item(ordered.len()));
            ordered.push(item.clone());
        }
    }
    (ordered, rows)
}

/// The byte range of a location's columns within its line text.
fn byte_range(line: &str, location: &Location) -> Range<usize> {
    let byte = |column: u32| {
        line.char_indices()
            .nth(column as usize)
            .map_or(line.len(), |(b, _)| b)
    };
    let start = byte(location.range.start.column);
    let end = if location.range.end.line == location.range.start.line {
        byte(location.range.end.column).max(start)
    } else {
        line.len()
    };
    start..end
}

struct Preview {
    first_line: u32,
    lines: Vec<SharedString>,
    focus: u32,
    range: Range<usize>,
}

pub struct LocationsView {
    workspace: WeakEntity<Workspace>,
    services: Services,
    title: SharedString,
    items: Vec<LocationItem>,
    rows: Vec<Row>,
    selected: Option<usize>,
    preview: Option<Preview>,
    preview_task: Option<Task<()>>,
    scroll: UniformListScrollHandle,
    focus_handle: FocusHandle,
}

impl LocationsView {
    fn new(
        workspace: &Workspace,
        handle: WeakEntity<Workspace>,
        title: String,
        items: Vec<LocationItem>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let roots = workspace.services.project.roots();
        let (items, rows) = build_rows(&items, |path| {
            roots
                .iter()
                .find_map(|root| path.strip_prefix(root).ok())
                .map(|rel| rel.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|| path.display().to_string())
        });
        let mut this = Self {
            workspace: handle,
            services: workspace.services.clone(),
            title: title.into(),
            selected: rows.iter().position(|r| matches!(r, Row::Item(_))),
            items,
            rows,
            preview: None,
            preview_task: None,
            scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
        };
        // The workspace is being updated right now (it opens this list), so
        // the first preview reads its editors through the reference.
        let open_text = this.selected_item().and_then(|item| {
            workspace
                .editors
                .iter()
                .find(|e| e.read(cx).path() == item.location.path)
                .and_then(|e| e.read(cx).text(cx))
        });
        this.update_preview_with(open_text, window, cx);
        this
    }

    pub fn items(&self) -> &[LocationItem] {
        &self.items
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn selected_item(&self) -> Option<&LocationItem> {
        match self.rows.get(self.selected?)? {
            Row::Item(ix) => self.items.get(*ix),
            Row::File { .. } => None,
        }
    }

    fn select_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.rows.get(ix), Some(Row::Item(_))) && self.selected != Some(ix) {
            self.selected = Some(ix);
            self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
            self.update_preview(window, cx);
            cx.notify();
        }
    }

    pub fn move_selection(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(current) = self.selected else { return };
        let next = if forward {
            (current + 1..self.rows.len()).find(|&i| matches!(self.rows[i], Row::Item(_)))
        } else {
            (0..current)
                .rev()
                .find(|&i| matches!(self.rows[i], Row::Item(_)))
        };
        if let Some(next) = next {
            self.select_row(next, window, cx);
        }
    }

    fn update_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open_text = self.selected_item().cloned().and_then(|item| {
            self.workspace.upgrade().and_then(|ws| {
                ws.read(cx)
                    .editors
                    .iter()
                    .find(|e| e.read(cx).path() == item.location.path)
                    .and_then(|e| e.read(cx).text(cx))
            })
        });
        self.update_preview_with(open_text, window, cx);
    }

    /// The preview of the selected item: from `open_text` (an editor has
    /// the freshest text), else read from the project.
    fn update_preview_with(
        &mut self,
        open_text: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preview_task = None;
        let Some(item) = self.selected_item().cloned() else {
            self.preview = None;
            return;
        };
        if let Some(text) = open_text {
            self.preview = Some(preview(&text, &item.location));
            return;
        }
        let read = self.services.project.read_file(&item.location.path);
        self.preview_task = Some(cx.spawn_in(window, async move |this, cx| {
            let text = cx.background_spawn(read).await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.preview = Some(preview(&text, &item.location));
                cx.notify();
            });
        }));
    }

    pub fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.selected_item().cloned() else {
            return;
        };
        let location = OpenLocation::at(item.location.path, item.location.range.start);
        window.close_dialog(cx);
        let _ = self
            .workspace
            .update(cx, |ws, cx| ws.open_location(&location, window, cx));
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let (muted, accent, selected_bg, hover_bg) = (
            theme.muted_foreground,
            theme.primary,
            theme.list_active,
            theme.list_hover,
        );
        let mark = HighlightStyle {
            background_color: Some(accent.opacity(0.3)),
            ..Default::default()
        };
        range
            .filter_map(|ix| self.rows.get(ix).cloned().map(|row| (ix, row)))
            .map(|(ix, row)| match row {
                Row::File { relative, count } => {
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
                Row::Item(item_ix) => {
                    let item = &self.items[item_ix];
                    let location = &item.location;
                    let is_selected = self.selected == Some(ix);
                    let line = &location.line_text;
                    let trimmed = line.trim_start();
                    let cut = line.len() - trimmed.len();
                    let shown: String = trimmed.chars().take(240).collect();
                    let range = byte_range(line, location);
                    let highlight = (range.start >= cut && range.end - cut <= shown.len())
                        .then(|| range.start - cut..range.end - cut);
                    let text = StyledText::new(SharedString::from(shown.clone()))
                        .with_highlights(highlight.into_iter().map(|r| (r, mark)));
                    h_flex()
                        .id(ix)
                        .role(Role::ListBoxOption)
                        .aria_label(format!(
                            "{}:{} {}",
                            location.path.display(),
                            location.range.start.line + 1,
                            trimmed
                        ))
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
                                .child(format!("{}", location.range.start.line + 1)),
                        )
                        .when_some(item.label.clone().filter(|l| !l.is_empty()), |el, label| {
                            el.child(div().flex_none().child(label))
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(muted)
                                .child(text),
                        )
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

    fn render_preview(&self, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let Some(preview) = &self.preview else {
            return div().into_any_element();
        };
        let mark = HighlightStyle {
            background_color: Some(theme.primary.opacity(0.35)),
            ..Default::default()
        };
        v_flex()
            .p_2()
            .font_family(theme.mono_font_family.clone())
            .text_xs()
            .children(preview.lines.iter().enumerate().map(|(i, line)| {
                let number = preview.first_line + i as u32;
                let is_focus = number == preview.focus;
                let text = if is_focus && preview.range.end <= line.len() {
                    StyledText::new(line.clone()).with_highlights([(preview.range.clone(), mark)])
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
                            .text_color(theme.muted_foreground)
                            .child(format!("{}", number + 1)),
                    )
                    .child(text)
            }))
            .into_any_element()
    }
}

fn preview(text: &str, location: &Location) -> Preview {
    let focus = location.range.start.line;
    let first_line = focus.saturating_sub(CONTEXT_LINES);
    let lines: Vec<SharedString> = text
        .lines()
        .skip(first_line as usize)
        .take((focus - first_line + CONTEXT_LINES + 1) as usize)
        .map(|l| SharedString::from(l.chars().take(200).collect::<String>()))
        .collect();
    let line = text.lines().nth(focus as usize).unwrap_or_default();
    let range = byte_range(line, location);
    Preview {
        first_line,
        lines,
        focus,
        range,
    }
}

impl Focusable for LocationsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for LocationsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let files = self
            .rows
            .iter()
            .filter(|r| matches!(r, Row::File { .. }))
            .count();
        let list = div()
            .id("locations-results")
            .role(Role::ListBox)
            .aria_label(self.title.clone())
            .h(px(ROW_HEIGHT * 12.))
            .w_full()
            .child(
                uniform_list(
                    "locations-list",
                    self.rows.len(),
                    cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx)),
                )
                .track_scroll(&self.scroll)
                .h_full(),
            );
        v_flex()
            .id("locations")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
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
            .w_full()
            .gap_2()
            .child(
                h_flex()
                    .px_1()
                    .text_xs()
                    .text_color(muted)
                    .child(tr_args(
                        "locations-summary",
                        &[("count", self.items.len().into()), ("files", files.into())],
                    ))
                    .child(div().flex_1())
                    .child(tr("locations-open-hint")),
            )
            .child(
                v_flex()
                    .border_1()
                    .border_color(border)
                    .rounded_md()
                    .overflow_hidden()
                    .child(list)
                    .child(
                        div()
                            .id("locations-preview")
                            .h(px(150.))
                            .border_t_1()
                            .border_color(border)
                            .overflow_hidden()
                            .child(self.render_preview(cx)),
                    ),
            )
    }
}

/// Show `items` in a dialog titled `title`; returns the view (tests drive
/// it directly).
pub fn show(
    workspace: &mut Workspace,
    title: String,
    items: Vec<LocationItem>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Entity<LocationsView> {
    bind_keys(cx);
    if window.has_active_dialog(cx) {
        window.close_dialog(cx);
    }
    let handle = cx.entity().downgrade();
    let dialog_title: SharedString = title.clone().into();
    let view = cx.new(|cx| LocationsView::new(workspace, handle, title, items, window, cx));
    let content = view.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title(dialog_title.clone())
            .w(px(900.))
            .margin_top(px(56.))
            .child(content.clone())
    });
    view.read(cx).focus_handle.clone().focus(window, cx);
    view
}

/// Where a list of results goes: one result opens directly.
pub fn open_or_show(
    workspace: &mut Workspace,
    title: String,
    items: Vec<LocationItem>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Option<Entity<LocationsView>> {
    match items.as_slice() {
        [] => None,
        [only] if only.label.is_none() => {
            let location = OpenLocation::at(only.location.path.clone(), only.location.range.start);
            workspace.open_location(&location, window, cx);
            None
        }
        _ => Some(show(workspace, title, items, window, cx)),
    }
}

#[cfg(test)]
mod tests {
    use ferro_services::{TextPos, TextRange};

    use super::*;

    fn item(path: &str, line: u32, text: &str, start: u32, end: u32) -> LocationItem {
        Location {
            path: PathBuf::from(path),
            range: TextRange::on_line(line, start, end),
            line_text: text.into(),
        }
        .into()
    }

    #[test]
    fn rows_group_by_file_in_order_of_appearance() {
        let items = [
            item("b.rs", 3, "x", 0, 1),
            item("a.rs", 1, "y", 0, 1),
            item("b.rs", 9, "z", 0, 1),
        ];
        let (ordered, rows) = build_rows(&items, |p| p.display().to_string());
        assert_eq!(
            rows,
            [
                Row::File {
                    relative: "b.rs".into(),
                    count: 2
                },
                Row::Item(0),
                Row::Item(1),
                Row::File {
                    relative: "a.rs".into(),
                    count: 1
                },
                Row::Item(2),
            ]
        );
        assert_eq!(ordered[1].location.range.start, TextPos::new(9, 0));
    }

    #[test]
    fn highlight_ranges_count_chars() {
        let location = item("a.rs", 0, "let ż = greet();", 8, 13).location;
        let range = byte_range(&location.line_text, &location);
        assert_eq!(&location.line_text[range], "greet");
    }
}
