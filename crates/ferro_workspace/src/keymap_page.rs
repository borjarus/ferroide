//! Settings → Keymap: the scheme selector, Vim mode, and the keybinding
//! table with search by name or by pressed keys, editing, removing and
//! resetting a binding, and conflict markers.

use std::ops::Range;

use ferro_keymap::{ACTIONS, BindingSource, Conflict, KeymapPreset, ResolvedBinding};
use ferro_ui::{FerroIcon, tr, tr_args};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyContext, KeyDownEvent, Keystroke, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    UniformListScrollHandle, Window,
    component::{
        ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, WindowExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputEvent, InputState},
        switch::Switch,
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px, uniform_list,
};

use crate::{
    actions::keymap_action,
    app_state::{self, AppState},
    chrome::display_keystroke,
};

/// The most strokes a recorded binding has (a two-stroke chord).
const MAX_STROKES: usize = 2;

/// One row of the table.
#[derive(Debug, Clone)]
pub struct ActionRow {
    pub id: SharedString,
    pub title: SharedString,
    pub category: SharedString,
    /// Lowercased `title id category`, for matching.
    haystack: String,
}

impl ActionRow {
    pub fn new(id: impl Into<SharedString>, title: SharedString, category: SharedString) -> Self {
        let id = id.into();
        let haystack = format!("{title} {id} {category}").to_lowercase();
        Self {
            id,
            title,
            category,
            haystack,
        }
    }
}

/// Every searchable catalog action, localized.
pub fn catalog_rows() -> Vec<ActionRow> {
    ACTIONS
        .iter()
        .filter(|a| a.searchable)
        .map(|a| ActionRow::new(a.id, tr(a.title_key), tr(a.category.title_key())))
        .collect()
}

/// GPUI keystroke → `ctrl-shift-a`, or `None` for a lone modifier.
pub fn keystroke_string(keystroke: &Keystroke) -> Option<String> {
    let key = keystroke.key.to_lowercase();
    if matches!(
        key.as_str(),
        "" | "shift" | "control" | "ctrl" | "alt" | "platform" | "cmd" | "function" | "fn"
    ) {
        return None;
    }
    let m = &keystroke.modifiers;
    let mut out = String::new();
    for (on, name) in [
        (m.control, "ctrl"),
        (m.alt, "alt"),
        (m.shift, "shift"),
        (m.platform, "cmd"),
        (m.function, "fn"),
    ] {
        if on {
            out.push_str(name);
            out.push('-');
        }
    }
    out.push_str(&key);
    Some(ferro_keymap::normalize_keystrokes(&out))
}

/// Whether `binding` starts with the recorded strokes `prefix`.
fn starts_with_strokes(binding: &str, prefix: &str) -> bool {
    let binding = ferro_keymap::normalize_keystrokes(binding);
    let b: Vec<&str> = binding.split(' ').collect();
    let p: Vec<&str> = prefix.split(' ').collect();
    p.len() <= b.len() && b.iter().zip(&p).all(|(x, y)| x == y)
}

pub struct KeymapPage {
    rows: Vec<ActionRow>,
    /// Indices into `rows` that pass the filters.
    visible: Vec<usize>,
    query: Entity<InputState>,
    /// Search by pressed keys instead of by name.
    recording: bool,
    recorded: Vec<String>,
    conflicts_only: bool,
    conflicts: Vec<Conflict>,
    selected: Option<usize>,
    scroll: UniformListScrollHandle,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl KeymapPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_rows(catalog_rows(), window, cx)
    }

    /// A page over arbitrary rows (the performance test uses many more than
    /// the catalog has).
    pub fn with_rows(rows: Vec<ActionRow>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query =
            cx.new(|cx| InputState::new(window, cx).placeholder(tr("keymap-search-placeholder")));
        let subscriptions = vec![
            cx.subscribe(&query, |this, _, _: &InputEvent, cx| this.refilter(cx)),
            cx.observe_global::<AppState>(|this, cx| this.refilter(cx)),
        ];
        let mut this = Self {
            rows,
            visible: Vec::new(),
            query,
            recording: false,
            recorded: Vec::new(),
            conflicts_only: false,
            conflicts: Vec::new(),
            selected: None,
            scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.refilter(cx);
        this
    }

    /// Recompute conflicts and the visible rows.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.refilter(cx);
    }

    fn keymap(cx: &App) -> &[ResolvedBinding] {
        &AppState::global(cx).keymap
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let keymap = Self::keymap(cx);
        self.conflicts = ferro_keymap::conflicts(keymap);
        let query = self.query.read(cx).value().trim().to_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();
        let recorded = self.recorded.join(" ");
        let recording = self.recording && !recorded.is_empty();
        self.visible = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                if self.conflicts_only && !ferro_keymap::is_conflicting(&row.id, &self.conflicts) {
                    return false;
                }
                if recording {
                    return keymap.iter().any(|b| {
                        b.action == row.id.as_ref() && starts_with_strokes(&b.keystrokes, &recorded)
                    });
                }
                words.iter().all(|w| {
                    row.haystack.contains(w)
                        || keymap.iter().any(|b| {
                            b.action == row.id.as_ref()
                                && display_keystroke(&b.keystrokes).to_lowercase().contains(w)
                        })
                })
            })
            .map(|(ix, _)| ix)
            .collect();
        if self.selected.is_some_and(|s| s >= self.visible.len()) {
            self.selected = None;
        }
        cx.notify();
    }

    /// Ids of the visible actions, in order.
    pub fn visible_ids(&self) -> Vec<SharedString> {
        self.visible
            .iter()
            .map(|&ix| self.rows[ix].id.clone())
            .collect()
    }

    pub fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.query
            .update(cx, |q, cx| q.set_value(query.to_string(), window, cx));
        self.refilter(cx);
    }

    pub fn set_conflicts_only(&mut self, on: bool, cx: &mut Context<Self>) {
        self.conflicts_only = on;
        self.refilter(cx);
    }

    pub fn set_recording(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.recording = on;
        self.recorded.clear();
        if on {
            self.focus_handle.focus(window, cx);
        }
        self.refilter(cx);
    }

    /// Feed a keystroke (e.g. `ctrl-k`) to key search.
    pub fn record(&mut self, keystroke: &str, cx: &mut Context<Self>) {
        if self.recorded.len() >= MAX_STROKES {
            self.recorded.clear();
        }
        self.recorded
            .push(ferro_keymap::normalize_keystrokes(keystroke));
        self.refilter(cx);
    }

    pub fn conflicts(&self) -> &[Conflict] {
        &self.conflicts
    }

    pub fn is_conflicting(&self, action: &str) -> bool {
        ferro_keymap::is_conflicting(action, &self.conflicts)
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.recording {
            let m = &event.keystroke.modifiers;
            if m.control || m.alt || m.platform || m.shift {
                return;
            }
            match event.keystroke.key.as_str() {
                "down" => self.move_selection(1, cx),
                "up" => self.move_selection(-1, cx),
                "enter" => {
                    if let Some(row) = self.selected.and_then(|s| self.visible.get(s)) {
                        open_record_dialog(self.rows[*row].id.clone(), window, cx);
                    }
                }
                _ => return,
            }
            cx.stop_propagation();
            return;
        }
        if let Some(stroke) = keystroke_string(&event.keystroke) {
            self.record(&stroke, cx);
            cx.stop_propagation();
        }
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.visible.is_empty() {
            return;
        }
        let last = self.visible.len() as isize - 1;
        let next = match self.selected {
            None => 0,
            Some(s) => (s as isize + delta).clamp(0, last),
        } as usize;
        self.selected = Some(next);
        self.scroll
            .scroll_to_item(next, gpui_kit::ScrollStrategy::Center);
        cx.notify();
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let (muted, warning, accent, hover, selected_bg) = (
            theme.muted_foreground,
            theme.warning,
            theme.primary,
            theme.list_hover,
            theme.list_active,
        );
        let keymap = Self::keymap(cx).to_vec();
        let user = AppState::global(cx).user_keymap.clone();
        range
            .filter_map(|vix| {
                self.visible
                    .get(vix)
                    .map(|&ix| (vix, self.rows[ix].clone()))
            })
            .map(|(vix, row)| {
                let shortcuts: Vec<String> = ferro_keymap::shortcuts_in(&row.id, &keymap)
                    .into_iter()
                    .map(display_keystroke)
                    .collect();
                let overridden = user.overrides(&row.id)
                    || keymap
                        .iter()
                        .any(|b| b.action == row.id.as_ref() && b.source == BindingSource::User);
                let conflict = ferro_keymap::is_conflicting(&row.id, &self.conflicts);
                let action: SharedString = row.id.clone();
                let (edit_action, remove_action, reset_action) =
                    (action.clone(), action.clone(), action.clone());
                let title = row.title.clone();
                h_flex()
                    .id(("keymap-row", vix))
                    .h(px(30.))
                    .w_full()
                    .px_2()
                    .gap_2()
                    .text_sm()
                    .hover(|el| el.bg(hover))
                    .when(self.selected == Some(vix), |el| el.bg(selected_bg))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(vix);
                        cx.notify();
                    }))
                    .child(div().flex_1().min_w_0().truncate().child(row.title.clone()))
                    .child(
                        div()
                            .w(px(110.))
                            .truncate()
                            .text_xs()
                            .text_color(muted)
                            .child(row.category.clone()),
                    )
                    .child(
                        div()
                            .w(px(170.))
                            .truncate()
                            .when(shortcuts.is_empty(), |el| el.text_color(muted))
                            .child(if shortcuts.is_empty() {
                                tr("keymap-unbound")
                            } else {
                                shortcuts.join(", ").into()
                            }),
                    )
                    .child(
                        h_flex()
                            .w(px(56.))
                            .gap_1()
                            .when(overridden, |el| {
                                el.child(
                                    div().text_xs().text_color(accent).child(tr("keymap-user")),
                                )
                            })
                            .when(conflict, |el| {
                                el.child(
                                    Icon::from(FerroIcon::Warning).xsmall().text_color(warning),
                                )
                            }),
                    )
                    .child(
                        Button::new(("keymap-edit", vix))
                            .ghost()
                            .xsmall()
                            .icon(Icon::from(FerroIcon::Rename))
                            .tooltip(tr("keymap-edit"))
                            .accessibility_label(tr_args(
                                "keymap-edit-for",
                                &[("action", title.to_string().into())],
                            ))
                            .on_click(move |_, window, cx| {
                                open_record_dialog(edit_action.clone(), window, cx);
                            }),
                    )
                    .child(
                        Button::new(("keymap-remove", vix))
                            .ghost()
                            .xsmall()
                            .icon(Icon::from(FerroIcon::Remove))
                            .tooltip(tr("keymap-remove"))
                            .accessibility_label(tr("keymap-remove"))
                            .disabled(shortcuts.is_empty())
                            .on_click(move |_, _, cx| {
                                let action = remove_action.clone();
                                AppState::update_user_keymap(cx, |k| k.set(&action, &[]));
                            }),
                    )
                    .child(
                        Button::new(("keymap-reset", vix))
                            .ghost()
                            .xsmall()
                            .icon(Icon::from(FerroIcon::Undo))
                            .tooltip(tr("keymap-reset"))
                            .accessibility_label(tr("keymap-reset"))
                            .disabled(!overridden)
                            .on_click(move |_, _, cx| {
                                let action = reset_action.clone();
                                AppState::update_user_keymap(cx, |k| k.reset(&action));
                            }),
                    )
                    .into_any_element()
            })
            .collect()
    }
}

impl Focusable for KeymapPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for KeymapPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = AppState::settings(cx).keymap;
        let muted = cx.theme().muted_foreground;
        let has_overrides = !AppState::global(cx).user_keymap.bindings.is_empty();
        let mut context = KeyContext::default();
        context.add("KeymapPage");

        let schemes = h_flex()
            .gap_1()
            .flex_wrap()
            .children(KeymapPreset::ALL.map(|preset| {
                Button::new(SharedString::from(format!("scheme-{}", preset.action_id())))
                    .small()
                    .label(preset.display_name())
                    .selected(settings.base == preset)
                    .on_click(move |_, window, cx| {
                        window.dispatch_action(keymap_action(preset), cx)
                    })
            }));

        let recorded: SharedString = if self.recorded.is_empty() {
            tr("keymap-press-keys")
        } else {
            self.recorded
                .iter()
                .map(|s| display_keystroke(s))
                .collect::<Vec<_>>()
                .join(" ")
                .into()
        };

        v_flex()
            .id("keymap-page")
            .key_context(context)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().text_sm().child(tr("keymap-scheme")))
                    .child(schemes),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Switch::new("keymap-vim")
                            .checked(settings.vim_mode)
                            .label(tr("action-toggle-vim-mode"))
                            .accessibility_label(tr("action-toggle-vim-mode"))
                            .on_click(|checked: &bool, _, cx| {
                                app_state::set_vim_mode(*checked, cx)
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(tr("keymap-vim-note")),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(if self.recording {
                        div()
                            .flex_1()
                            .px_2()
                            .py_1()
                            .rounded(px(4.))
                            .border_1()
                            .border_color(cx.theme().primary)
                            .text_sm()
                            .child(recorded)
                            .into_any_element()
                    } else {
                        Input::new(&self.query)
                            .small()
                            .flex_1()
                            .cleanable(true)
                            .aria_label(tr("keymap-search-placeholder"))
                            .into_any_element()
                    })
                    .child(
                        Button::new("keymap-record")
                            .small()
                            .icon(Icon::from(FerroIcon::Keyboard))
                            .selected(self.recording)
                            .tooltip(tr("keymap-search-by-keys"))
                            .accessibility_label(tr("keymap-search-by-keys"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                let on = !this.recording;
                                this.set_recording(on, window, cx)
                            })),
                    )
                    .child(
                        Button::new("keymap-conflicts")
                            .small()
                            .icon(Icon::from(FerroIcon::Warning))
                            .label(tr_args(
                                "keymap-conflicts",
                                &[("count", self.conflicts.len().into())],
                            ))
                            .selected(self.conflicts_only)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let on = !this.conflicts_only;
                                this.set_conflicts_only(on, cx)
                            })),
                    )
                    .child(
                        Button::new("keymap-reset-all")
                            .small()
                            .ghost()
                            .label(tr("keymap-reset-all"))
                            .disabled(!has_overrides)
                            .on_click(|_, _, cx| {
                                AppState::update_user_keymap(cx, |k| k.bindings.clear())
                            }),
                    ),
            )
            .child(if self.visible.is_empty() {
                ferro_ui::components::empty_state(
                    FerroIcon::Keyboard,
                    tr("keymap-no-results"),
                    None,
                    cx,
                )
                .into_any_element()
            } else {
                uniform_list(
                    "keymap-table",
                    self.visible.len(),
                    cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx)),
                )
                .track_scroll(&self.scroll)
                .flex_1()
                .min_h(px(200.))
                .into_any_element()
            })
    }
}

// ---------------------------------------------------------------------------
// Recording a binding
// ---------------------------------------------------------------------------

/// The "press the new shortcut" dialog for one action.
pub struct RecordBinding {
    action: SharedString,
    strokes: Vec<String>,
    focus_handle: FocusHandle,
}

impl RecordBinding {
    pub fn new(action: SharedString, cx: &mut Context<Self>) -> Self {
        Self {
            action,
            strokes: Vec::new(),
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn keys(&self) -> String {
        self.strokes.join(" ")
    }

    pub fn record(&mut self, stroke: &str, cx: &mut Context<Self>) {
        if self.strokes.len() >= MAX_STROKES {
            self.strokes.clear();
        }
        self.strokes
            .push(ferro_keymap::normalize_keystrokes(stroke));
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.strokes.clear();
        cx.notify();
    }

    /// Other actions the recorded keys would conflict with.
    pub fn conflicting_actions(&self, cx: &App) -> Vec<String> {
        if self.strokes.is_empty() {
            return Vec::new();
        }
        let state = AppState::global(cx);
        let mut user = state.user_keymap.clone();
        user.set(&self.action, &[self.keys()]);
        let keymap = ferro_keymap::resolve(
            state.settings.keymap.base,
            ferro_keymap::Platform::current(),
            &user,
        );
        let mut out: Vec<String> = ferro_keymap::conflicts(&keymap)
            .into_iter()
            .filter(|c| c.actions.iter().any(|a| a == self.action.as_ref()))
            .flat_map(|c| c.actions)
            .filter(|a| a != self.action.as_ref())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Store the recorded keys as the user's binding of the action.
    pub fn save(&self, cx: &mut App) {
        if self.strokes.is_empty() {
            return;
        }
        let (action, keys) = (self.action.to_string(), self.keys());
        AppState::update_user_keymap(cx, |k| k.set(&action, &[keys]));
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        let plain = !k.modifiers.control && !k.modifiers.alt && !k.modifiers.platform;
        match k.key.as_str() {
            // Plain Enter saves, Escape cancels, Tab moves focus.
            "enter" if plain && !k.modifiers.shift => {
                self.save(cx);
                window.close_dialog(cx);
            }
            "escape" if plain => window.close_dialog(cx),
            "tab" if plain => return,
            _ => {
                if let Some(stroke) = keystroke_string(k) {
                    self.record(&stroke, cx);
                }
            }
        }
        cx.stop_propagation();
    }
}

impl Focusable for RecordBinding {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RecordBinding {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, warning, primary) = (theme.muted_foreground, theme.warning, theme.primary);
        let conflicts: Vec<SharedString> = self
            .conflicting_actions(cx)
            .into_iter()
            .map(|id| {
                ferro_keymap::action(&id)
                    .map(|a| tr(a.title_key))
                    .unwrap_or_else(|| id.into())
            })
            .collect();
        let shown: SharedString = if self.strokes.is_empty() {
            tr("keymap-press-keys")
        } else {
            self.strokes
                .iter()
                .map(|s| display_keystroke(s))
                .collect::<Vec<_>>()
                .join(" ")
                .into()
        };
        v_flex()
            .id("record-binding")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .gap_3()
            .child(
                div()
                    .p_3()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(primary)
                    .text_center()
                    .text_lg()
                    .child(shown),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(tr("keymap-record-hint")),
            )
            .when(!conflicts.is_empty(), |el| {
                el.child(
                    h_flex()
                        .gap_1()
                        .text_sm()
                        .text_color(warning)
                        .child(Icon::from(FerroIcon::Warning).small())
                        .child(tr_args(
                            "keymap-conflict-with",
                            &[("actions", conflicts.join(", ").into())],
                        )),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("record-clear")
                            .small()
                            .label(tr("keymap-clear"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.clear(cx);
                                this.focus_handle.focus(window, cx);
                            })),
                    )
                    .child(
                        Button::new("record-cancel")
                            .small()
                            .label(tr("settings-cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("record-save")
                            .small()
                            .primary()
                            .label(tr("keymap-save"))
                            .disabled(self.strokes.is_empty())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save(cx);
                                window.close_dialog(cx);
                            })),
                    ),
            )
    }
}

/// Open the record dialog for `action` on top of the Settings window.
pub fn open_record_dialog(
    action: SharedString,
    window: &mut Window,
    cx: &mut App,
) -> Entity<RecordBinding> {
    let title = ferro_keymap::action(&action)
        .map(|a| tr(a.title_key))
        .unwrap_or_else(|| action.clone());
    let view = cx.new(|cx| RecordBinding::new(action, cx));
    let content = view.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title(tr_args(
                "keymap-edit-for",
                &[("action", title.to_string().into())],
            ))
            .w(px(440.))
            .child(content.clone())
    });
    let focus = view.read(cx).focus_handle.clone();
    focus.focus(window, cx);
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stroke_prefixes_match_chords() {
        assert!(starts_with_strokes("ctrl-k ctrl-s", "ctrl-k"));
        assert!(starts_with_strokes("ctrl-k ctrl-s", "ctrl-k ctrl-s"));
        assert!(!starts_with_strokes("ctrl-k", "ctrl-k ctrl-s"));
        assert!(!starts_with_strokes("ctrl-p", "ctrl-k"));
        assert!(starts_with_strokes("Shift-Ctrl-P", "ctrl-shift-p"));
    }
}
