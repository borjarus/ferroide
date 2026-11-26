//! Language-server commands of an editor tab (Stage 6): Go to Definition /
//! Declaration / Type Definition / Implementation, Find Usages, call and
//! type hierarchies, Format Document and Optimize Imports.
//!
//! Requests run on the background executor and replace the tab's pending
//! request (dropping it cancels it on the server). Results with one target
//! open it; more go to the locations list ([`ShowLocations`]).

use ferro_services::{GotoKind, HierarchyDirection, HierarchyItem, Location, TextEdit, TextPos};
use ferro_ui::{tr, tr_args};
use gpui_kit::{
    Context, Window,
    component::{WindowExt as _, input::RopeExt as _, notification::Notification},
};

use super::{EditorPanel, providers::to_lsp_pos};
use crate::{locations::LocationItem, requests::ShowLocations};

/// The identifier at (or just before) `pos`.
pub fn word_at(text: &str, pos: TextPos) -> String {
    let line: Vec<char> = text
        .lines()
        .nth(pos.line as usize)
        .unwrap_or_default()
        .chars()
        .collect();
    let is_word = |c: &char| c.is_alphanumeric() || *c == '_';
    let caret = (pos.column as usize).min(line.len());
    let start = line[..caret]
        .iter()
        .rposition(|c| !is_word(c))
        .map_or(0, |i| i + 1);
    let end = line[caret..]
        .iter()
        .position(|c| !is_word(c))
        .map_or(line.len(), |i| caret + i);
    line[start..end].iter().collect()
}

fn hierarchy_location(item: HierarchyItem, arrow: &str) -> LocationItem {
    let line_text = std::fs::read_to_string(&item.path)
        .ok()
        .and_then(|t| {
            t.lines()
                .nth(item.selection.start.line as usize)
                .map(str::to_string)
        })
        .unwrap_or_default();
    LocationItem {
        label: Some(format!("{arrow}{}", item.name)),
        location: Location {
            path: item.path,
            range: item.selection,
            line_text,
        },
    }
}

/// Edits sorted from the end of the text to its start, so applying them
/// one by one keeps the positions of the rest valid.
pub fn sorted_from_end(edits: &[TextEdit]) -> Vec<TextEdit> {
    let mut edits = edits.to_vec();
    edits.sort_by(|a, b| {
        b.range
            .start
            .cmp(&a.range.start)
            .then(b.range.end.cmp(&a.range.end))
    });
    edits
}

impl EditorPanel {
    fn caret_and_text(&self, cx: &gpui_kit::App) -> Option<(TextPos, String)> {
        Some((self.caret(cx)?, self.text(cx)?))
    }

    /// Show results: none → a notification, one → open it, more → list.
    fn show_locations(
        title: String,
        name: &str,
        items: Vec<LocationItem>,
        window: &mut Window,
        cx: &mut gpui_kit::App,
    ) {
        if items.is_empty() {
            window.push_notification(
                Notification::info(tr_args(
                    "lsp-nothing-found",
                    &[("name", name.to_string().into())],
                )),
                cx,
            );
            return;
        }
        window.dispatch_action(Box::new(ShowLocations { title, items }), cx);
    }

    pub(super) fn goto(&mut self, kind: GotoKind, window: &mut Window, cx: &mut Context<Self>) {
        let Some((caret, text)) = self.caret_and_text(cx) else {
            return;
        };
        let name = word_at(&text, caret);
        let request = self.services.language.goto(&self.path, &text, caret, kind);
        let key = match kind {
            GotoKind::Implementation => "locations-implementations",
            _ => "locations-definitions",
        };
        self.request_task = cx.spawn_in(window, async move |this, cx| {
            let found = request.await.unwrap_or_else(|err| {
                tracing::debug!("go to: {err:#}");
                Vec::new()
            });
            let _ = this.update_in(cx, |_, window, cx| {
                let title = tr_args(key, &[("name", name.clone().into())]).to_string();
                let items = found.into_iter().map(LocationItem::from).collect();
                Self::show_locations(title, &name, items, window, cx);
            });
        });
    }

    pub(super) fn find_usages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((caret, text)) = self.caret_and_text(cx) else {
            return;
        };
        let name = word_at(&text, caret);
        let request = self
            .services
            .language
            .references(&self.path, &text, caret, true);
        self.request_task = cx.spawn_in(window, async move |this, cx| {
            let result = request.await;
            let _ = this.update_in(cx, |_, window, cx| match result {
                Ok(found) => {
                    let title =
                        tr_args("locations-usages", &[("name", name.clone().into())]).to_string();
                    // Usages always show as a list, also a single one.
                    let items: Vec<LocationItem> = found
                        .into_iter()
                        .map(|location| LocationItem {
                            label: Some(String::new()),
                            location,
                        })
                        .collect();
                    Self::show_locations(title, &name, items, window, cx);
                }
                Err(err) => window.push_notification(Notification::warning(err.to_string()), cx),
            });
        });
    }

    pub(super) fn hierarchy(&mut self, types: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((caret, text)) = self.caret_and_text(cx) else {
            return;
        };
        let name = word_at(&text, caret);
        let language = self.services.language.clone();
        let path = self.path.clone();
        self.request_task = cx.spawn_in(window, async move |this, cx| {
            let mut items = Vec::new();
            if types {
                let up = language.hierarchy(&path, &text, caret, HierarchyDirection::Supertypes);
                let down = language.hierarchy(&path, &text, caret, HierarchyDirection::Subtypes);
                items.extend(
                    up.await
                        .unwrap_or_default()
                        .into_iter()
                        .map(|i| hierarchy_location(i, "↑ ")),
                );
                items.extend(
                    down.await
                        .unwrap_or_default()
                        .into_iter()
                        .map(|i| hierarchy_location(i, "↓ ")),
                );
            } else {
                let callers = language.hierarchy(&path, &text, caret, HierarchyDirection::Incoming);
                items.extend(
                    callers
                        .await
                        .unwrap_or_default()
                        .into_iter()
                        .map(|i| hierarchy_location(i, "")),
                );
            }
            let _ = this.update_in(cx, |_, window, cx| {
                let key = if types {
                    "locations-type-hierarchy"
                } else {
                    "locations-callers"
                };
                let title = tr_args(key, &[("name", name.clone().into())]).to_string();
                if items.is_empty() {
                    Self::show_locations(title, &name, items, window, cx);
                } else {
                    window.dispatch_action(Box::new(ShowLocations { title, items }), cx);
                }
            });
        });
    }

    /// Apply edits to this tab's text, as one change per edit.
    pub fn apply_edits(&mut self, edits: &[TextEdit], window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.state().cloned() else {
            return;
        };
        if self.disk.is_read_only() || edits.is_empty() {
            return;
        }
        let edits: Vec<lsp_types::TextEdit> = sorted_from_end(edits)
            .iter()
            .map(|e| {
                lsp_types::TextEdit::new(
                    lsp_types::Range::new(to_lsp_pos(e.range.start), to_lsp_pos(e.range.end)),
                    e.new_text.clone(),
                )
            })
            .collect();
        state.update(cx, |state, cx| state.apply_lsp_edits(&edits, window, cx));
    }

    pub(super) fn format_document(
        &mut self,
        organize: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = self.text(cx) else { return };
        if self.disk.is_read_only() {
            return;
        }
        let selection = self.state().map(|s| {
            let state = s.read(cx);
            let rope = state.text();
            let range = state.selected_range();
            (range.start != range.end).then(|| {
                ferro_services::TextRange::new(
                    super::providers::from_lsp_pos(rope.offset_to_position(range.start)),
                    super::providers::from_lsp_pos(rope.offset_to_position(range.end)),
                )
            })
        });
        let request = if organize {
            self.services.language.organize_imports(&self.path, &text)
        } else {
            self.services
                .language
                .format(&self.path, &text, selection.flatten())
        };
        self.request_task = cx.spawn_in(window, async move |this, cx| {
            let result = request.await;
            let _ = this.update_in(cx, |this, window, cx| {
                // The text changed meanwhile: the edits no longer fit.
                if this.text(cx).as_deref() != Some(text.as_str()) {
                    return;
                }
                match result {
                    Ok(edits) if edits.is_empty() => {
                        let key = if organize {
                            "lsp-imports-nothing"
                        } else {
                            "lsp-format-nothing"
                        };
                        window.push_notification(Notification::info(tr(key)), cx);
                    }
                    Ok(edits) => this.apply_edits(&edits, window, cx),
                    Err(err) => {
                        window.push_notification(Notification::warning(err.to_string()), cx)
                    }
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use ferro_services::TextRange;

    use super::*;

    #[test]
    fn edits_apply_from_the_end() {
        let edit = |line, start| TextEdit {
            range: TextRange::on_line(line, start, start + 1),
            new_text: String::new(),
        };
        let sorted = sorted_from_end(&[edit(0, 1), edit(2, 0), edit(0, 5)]);
        let starts: Vec<(u32, u32)> = sorted
            .iter()
            .map(|e| (e.range.start.line, e.range.start.column))
            .collect();
        assert_eq!(starts, [(2, 0), (0, 5), (0, 1)]);
    }

    #[test]
    fn words_at_the_caret() {
        assert_eq!(
            word_at("let x = greet(name);", TextPos::new(0, 10)),
            "greet"
        );
        assert_eq!(word_at("greet", TextPos::new(0, 5)), "greet");
        assert_eq!(word_at("a + b", TextPos::new(0, 2)), "");
    }
}
