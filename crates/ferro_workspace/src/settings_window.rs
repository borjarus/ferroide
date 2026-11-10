//! The Settings window: a category list, search across every page, field
//! editors, a draft with Apply / Reset / OK / Cancel, and the Keymap page.
//!
//! Edits go to a draft copy of [`Settings`]; nothing changes in the IDE
//! until Apply (or OK). Reset reverts the draft to the applied settings; the
//! ↺ button beside a field restores that field's default.

use ferro_i18n::Locale;
use ferro_keymap::KeymapPreset;
use ferro_settings::{MAX_UI_FONT_SIZE, MIN_UI_FONT_SIZE, Settings, themes};
use ferro_ui::{FerroIcon, tr};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyContext, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    component::{
        ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, WindowExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputEvent, InputState},
        menu::{DropdownMenu as _, PopupMenuItem},
        switch::Switch,
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};

use crate::{
    app_state::{self, AppState},
    keymap_page::KeymapPage,
    requests::OpenLocation,
    workspace::Workspace,
};

/// The page the window opens on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsPage {
    Appearance,
    Editor,
    Keymap,
}

impl SettingsPage {
    pub const ALL: [SettingsPage; 3] = [
        SettingsPage::Appearance,
        SettingsPage::Editor,
        SettingsPage::Keymap,
    ];

    pub fn title_key(self) -> &'static str {
        match self {
            SettingsPage::Appearance => "settings-page-appearance",
            SettingsPage::Editor => "settings-page-editor",
            SettingsPage::Keymap => "settings-page-keymap",
        }
    }

    fn icon(self) -> FerroIcon {
        match self {
            SettingsPage::Appearance => FerroIcon::ThemeDark,
            SettingsPage::Editor => FerroIcon::FileCode,
            SettingsPage::Keymap => FerroIcon::Keyboard,
        }
    }
}

/// One editable setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingKey {
    Theme,
    UiFontSize,
    Compact,
    Animations,
    Language,
    EditorFontSize,
    TabSize,
    TabLimit,
    SoftWrap,
    LineNumbers,
    InlayHints,
    StickyLines,
    Breadcrumbs,
    VcsMarkers,
    PreviewTabs,
    Autosave,
    ShowWhitespace,
    KeymapScheme,
    VimMode,
}

/// How a setting is edited.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldKind {
    Switch,
    Number {
        min: f64,
        max: f64,
        step: f64,
    },
    /// (value id, label) pairs.
    Choice(Vec<(&'static str, SharedString)>),
}

/// A setting's value, as the field editors see it.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingValue {
    Bool(bool),
    Number(f64),
    Choice(String),
}

impl SettingKey {
    pub const ALL: [SettingKey; 19] = [
        SettingKey::Theme,
        SettingKey::UiFontSize,
        SettingKey::Compact,
        SettingKey::Animations,
        SettingKey::Language,
        SettingKey::EditorFontSize,
        SettingKey::TabSize,
        SettingKey::TabLimit,
        SettingKey::SoftWrap,
        SettingKey::LineNumbers,
        SettingKey::InlayHints,
        SettingKey::StickyLines,
        SettingKey::Breadcrumbs,
        SettingKey::VcsMarkers,
        SettingKey::PreviewTabs,
        SettingKey::Autosave,
        SettingKey::ShowWhitespace,
        SettingKey::KeymapScheme,
        SettingKey::VimMode,
    ];

    pub fn page(self) -> SettingsPage {
        use SettingKey::*;
        match self {
            Theme | UiFontSize | Compact | Animations | Language => SettingsPage::Appearance,
            KeymapScheme | VimMode => SettingsPage::Keymap,
            _ => SettingsPage::Editor,
        }
    }

    /// Stable id: element ids and message ids derive from it.
    pub fn id(self) -> &'static str {
        use SettingKey::*;
        match self {
            Theme => "theme",
            UiFontSize => "ui-font-size",
            Compact => "compact",
            Animations => "animations",
            Language => "language",
            EditorFontSize => "editor-font-size",
            TabSize => "tab-size",
            TabLimit => "tab-limit",
            SoftWrap => "soft-wrap",
            LineNumbers => "line-numbers",
            InlayHints => "inlay-hints",
            StickyLines => "sticky-lines",
            Breadcrumbs => "breadcrumbs",
            VcsMarkers => "vcs-markers",
            PreviewTabs => "preview-tabs",
            Autosave => "autosave",
            ShowWhitespace => "show-whitespace",
            KeymapScheme => "keymap-scheme",
            VimMode => "vim-mode",
        }
    }

    pub fn label(self) -> SharedString {
        tr(&format!("settings-field-{}", self.id()))
    }

    pub fn description(self) -> SharedString {
        tr(&format!("settings-field-{}-desc", self.id()))
    }

    pub fn kind(self) -> FieldKind {
        use SettingKey::*;
        match self {
            Theme => FieldKind::Choice(vec![
                (themes::FERRO_DARK, tr("action-theme-dark")),
                (themes::FERRO_LIGHT, tr("action-theme-light")),
                (themes::HIGH_CONTRAST, tr("action-theme-high-contrast")),
            ]),
            Language => FieldKind::Choice(
                Locale::ALL
                    .iter()
                    .map(|l| (l.code(), SharedString::from(l.native_name())))
                    .collect(),
            ),
            Autosave => FieldKind::Choice(
                ferro_settings::Autosave::ALL
                    .iter()
                    .map(|a| {
                        (
                            a.id(),
                            tr(&format!("settings-autosave-{}", a.id().replace('_', "-"))),
                        )
                    })
                    .collect(),
            ),
            KeymapScheme => FieldKind::Choice(
                KeymapPreset::ALL
                    .iter()
                    .map(|p| (preset_id(*p), SharedString::from(p.display_name())))
                    .collect(),
            ),
            UiFontSize => FieldKind::Number {
                min: MIN_UI_FONT_SIZE as f64,
                max: MAX_UI_FONT_SIZE as f64,
                step: 1.0,
            },
            EditorFontSize => FieldKind::Number {
                min: 6.0,
                max: 72.0,
                step: 1.0,
            },
            TabSize => FieldKind::Number {
                min: 1.0,
                max: 16.0,
                step: 1.0,
            },
            TabLimit => FieldKind::Number {
                min: 1.0,
                max: 100.0,
                step: 1.0,
            },
            _ => FieldKind::Switch,
        }
    }

    pub fn get(self, s: &Settings) -> SettingValue {
        use SettingKey::*;
        use SettingValue::*;
        match self {
            Theme => Choice(s.appearance.theme.clone()),
            UiFontSize => Number(s.appearance.ui_font_size as f64),
            Compact => Bool(s.appearance.compact),
            Animations => Bool(s.appearance.animations),
            Language => Choice(s.locale.code().to_string()),
            EditorFontSize => Number(s.editor.font_size as f64),
            TabSize => Number(s.editor.tab_size as f64),
            TabLimit => Number(s.editor.tab_limit as f64),
            SoftWrap => Bool(s.editor.soft_wrap),
            LineNumbers => Bool(s.editor.line_numbers),
            InlayHints => Bool(s.editor.inlay_hints),
            StickyLines => Bool(s.editor.sticky_lines),
            Breadcrumbs => Bool(s.editor.breadcrumbs),
            VcsMarkers => Bool(s.editor.vcs_markers),
            PreviewTabs => Bool(s.editor.preview_tabs),
            Autosave => Choice(s.editor.autosave.id().to_string()),
            ShowWhitespace => Bool(s.editor.show_whitespace),
            KeymapScheme => Choice(preset_id(s.keymap.base).to_string()),
            VimMode => Bool(s.keymap.vim_mode),
        }
    }

    /// Write `value` into `s`, clamping numbers. Mismatched kinds are ignored.
    pub fn set(self, s: &mut Settings, value: SettingValue) {
        use SettingKey::*;
        let value = match (self.kind(), value) {
            (FieldKind::Number { min, max, .. }, SettingValue::Number(n)) => {
                SettingValue::Number(n.clamp(min, max).round())
            }
            (_, v) => v,
        };
        match (self, value) {
            (Theme, SettingValue::Choice(v)) if themes::ALL.contains(&v.as_str()) => {
                s.appearance.theme = v
            }
            (UiFontSize, SettingValue::Number(n)) => s.appearance.ui_font_size = n as f32,
            (Compact, SettingValue::Bool(b)) => s.appearance.compact = b,
            (Animations, SettingValue::Bool(b)) => s.appearance.animations = b,
            (Language, SettingValue::Choice(v)) => {
                if let Some(locale) = Locale::from_code(&v) {
                    s.locale = locale;
                }
            }
            (EditorFontSize, SettingValue::Number(n)) => s.editor.font_size = n as f32,
            (TabSize, SettingValue::Number(n)) => s.editor.tab_size = n as usize,
            (TabLimit, SettingValue::Number(n)) => s.editor.tab_limit = n as usize,
            (SoftWrap, SettingValue::Bool(b)) => s.editor.soft_wrap = b,
            (LineNumbers, SettingValue::Bool(b)) => s.editor.line_numbers = b,
            (InlayHints, SettingValue::Bool(b)) => s.editor.inlay_hints = b,
            (StickyLines, SettingValue::Bool(b)) => s.editor.sticky_lines = b,
            (Breadcrumbs, SettingValue::Bool(b)) => s.editor.breadcrumbs = b,
            (VcsMarkers, SettingValue::Bool(b)) => s.editor.vcs_markers = b,
            (PreviewTabs, SettingValue::Bool(b)) => s.editor.preview_tabs = b,
            (Autosave, SettingValue::Choice(v)) => {
                if let Some(mode) = ferro_settings::Autosave::from_id(&v) {
                    s.editor.autosave = mode;
                }
            }
            (ShowWhitespace, SettingValue::Bool(b)) => s.editor.show_whitespace = b,
            (KeymapScheme, SettingValue::Choice(v)) => {
                if let Some(preset) = KeymapPreset::ALL.into_iter().find(|p| preset_id(*p) == v) {
                    s.keymap.base = preset;
                }
            }
            (VimMode, SettingValue::Bool(b)) => s.keymap.vim_mode = b,
            _ => {}
        }
    }

    /// Whether `query` (lowercase) matches the label, description, id or
    /// page name. Matches in English too, so a Polish UI finds "theme".
    pub fn matches(self, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }
        let haystacks = [
            self.label().to_lowercase(),
            self.description().to_lowercase(),
            self.id().replace('-', " "),
            tr(self.page().title_key()).to_lowercase(),
        ];
        haystacks.iter().any(|h| h.contains(query))
    }
}

/// The serialized name of a keymap scheme, as in `settings.toml`.
pub fn preset_id(preset: KeymapPreset) -> &'static str {
    match preset {
        KeymapPreset::VsCode => "vscode",
        KeymapPreset::JetBrains => "jetbrains",
        KeymapPreset::Zed => "zed",
        KeymapPreset::Sublime => "sublime",
        KeymapPreset::Emacs => "emacs",
    }
}

/// The Settings window's content.
pub struct SettingsWindow {
    /// Edited, not yet applied.
    draft: Settings,
    page: SettingsPage,
    search: Entity<InputState>,
    keymap: Entity<KeymapPage>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl SettingsWindow {
    pub fn new(page: SettingsPage, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder(tr("settings-search-placeholder")));
        let keymap = cx.new(|cx| KeymapPage::new(window, cx));
        let subscriptions = vec![cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify())];
        Self {
            draft: AppState::settings(cx).clone(),
            page,
            search,
            keymap,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    pub fn page(&self) -> SettingsPage {
        self.page
    }

    pub fn set_page(&mut self, page: SettingsPage, cx: &mut Context<Self>) {
        self.page = page;
        cx.notify();
    }

    pub fn draft(&self) -> &Settings {
        &self.draft
    }

    pub fn keymap_page(&self) -> &Entity<KeymapPage> {
        &self.keymap
    }

    pub fn set(&mut self, key: SettingKey, value: SettingValue, cx: &mut Context<Self>) {
        key.set(&mut self.draft, value);
        cx.notify();
    }

    /// Whether the draft differs from the applied settings.
    pub fn is_dirty(&self, cx: &App) -> bool {
        self.draft != *AppState::settings(cx)
    }

    fn is_default(&self, key: SettingKey) -> bool {
        key.get(&self.draft) == key.get(&Settings::default())
    }

    pub fn reset_to_default(&mut self, key: SettingKey, cx: &mut Context<Self>) {
        let value = key.get(&Settings::default());
        self.set(key, value, cx);
    }

    /// Revert the draft to the applied settings.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.draft = AppState::settings(cx).clone();
        cx.notify();
    }

    /// Apply the draft: persist it and apply what changed (theme and fonts,
    /// language, keymap).
    pub fn apply(&mut self, cx: &mut Context<Self>) {
        let old = AppState::settings(cx).clone();
        let new = self.draft.clone();
        if old == new {
            return;
        }
        AppState::update_settings(cx, |s| *s = new.clone());
        if old.locale != new.locale {
            app_state::set_locale(new.locale, cx);
        }
        if old.keymap.base != new.keymap.base {
            app_state::reinstall_keymap(cx);
        } else if old.keymap.vim_mode != new.keymap.vim_mode {
            app_state::set_vim_mode(new.keymap.vim_mode, cx);
        }
        if old.appearance != new.appearance || old.editor.font_size != new.editor.font_size {
            app_state::apply_appearance(cx);
        }
        // Sanitizing may have clamped something.
        self.draft = AppState::settings(cx).clone();
        self.keymap.update(cx, |page, cx| page.refresh(cx));
        cx.notify();
    }

    /// Settings matching the search box, in page order.
    pub fn search_results(&self, cx: &App) -> Vec<SettingKey> {
        let query = self.search.read(cx).value().trim().to_lowercase();
        SettingKey::ALL
            .into_iter()
            .filter(|key| key.matches(&query))
            .collect()
    }

    /// Set the search text (tests and the storybook).
    pub fn set_search(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |input, cx| {
            input.set_value(query.to_string(), window, cx)
        });
        cx.notify();
    }

    fn render_field(&self, key: SettingKey, cx: &mut Context<Self>) -> AnyElement {
        let value = key.get(&self.draft);
        let id = SharedString::from(format!("setting-{}", key.id()));
        match (key.kind(), value) {
            (FieldKind::Switch, SettingValue::Bool(on)) => Switch::new(id)
                .checked(on)
                .accessibility_label(key.label())
                .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                    this.set(key, SettingValue::Bool(*checked), cx)
                }))
                .into_any_element(),
            (FieldKind::Number { min, max, step }, SettingValue::Number(n)) => h_flex()
                .gap_1()
                .child(
                    Button::new(SharedString::from(format!("{id}-dec")))
                        .xsmall()
                        .label("−")
                        .accessibility_label(tr("settings-decrease"))
                        .disabled(n <= min)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set(key, SettingValue::Number(n - step), cx)
                        })),
                )
                .child(div().w(px(44.)).text_center().child(format!("{n:.0}")))
                .child(
                    Button::new(SharedString::from(format!("{id}-inc")))
                        .xsmall()
                        .label("+")
                        .accessibility_label(tr("settings-increase"))
                        .disabled(n >= max)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set(key, SettingValue::Number(n + step), cx)
                        })),
                )
                .into_any_element(),
            (FieldKind::Choice(options), SettingValue::Choice(current)) => {
                let label = options
                    .iter()
                    .find(|(v, _)| *v == current)
                    .map(|(_, l)| l.clone())
                    .unwrap_or_else(|| current.clone().into());
                let this = cx.entity().downgrade();
                Button::new(id)
                    .small()
                    .outline()
                    .label(label)
                    .dropdown_caret(true)
                    .accessibility_label(key.label())
                    .dropdown_menu(move |mut menu, _, _| {
                        for (value, label) in &options {
                            let this = this.clone();
                            let value = value.to_string();
                            menu = menu.item(
                                PopupMenuItem::new(label.clone())
                                    .checked(value == current)
                                    .on_click(move |_, _, cx| {
                                        let value = value.clone();
                                        let _ = this.update(cx, |this, cx| {
                                            this.set(key, SettingValue::Choice(value), cx)
                                        });
                                    }),
                            );
                        }
                        menu
                    })
                    .into_any_element()
            }
            _ => div().into_any_element(),
        }
    }

    fn render_item(&self, key: SettingKey, query: &str, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, accent) = (theme.muted_foreground, theme.primary);
        let applied = key.get(AppState::settings(cx)) == key.get(&self.draft);
        let is_default = self.is_default(key);
        h_flex()
            .id(SharedString::from(format!("item-{}", key.id())))
            .w_full()
            .py_2()
            .gap_3()
            .child(
                div()
                    .w(px(4.))
                    .h(px(28.))
                    .rounded_full()
                    .when(!applied, |el| el.bg(accent)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(highlighted(key.label(), query, accent))
                    .child(div().text_xs().text_color(muted).child(key.description())),
            )
            .child(
                Button::new(SharedString::from(format!("reset-{}", key.id())))
                    .ghost()
                    .xsmall()
                    .icon(Icon::from(FerroIcon::Undo))
                    .tooltip(tr("settings-reset-to-default"))
                    .accessibility_label(tr("settings-reset-to-default"))
                    .disabled(is_default)
                    .on_click(cx.listener(move |this, _, _, cx| this.reset_to_default(key, cx))),
            )
            .child(self.render_field(key, cx))
            .into_any_element()
    }
}

/// `text` with the first occurrence of `query` (case-insensitive) colored.
fn highlighted(text: SharedString, query: &str, color: gpui_kit::Hsla) -> AnyElement {
    let lower = text.to_lowercase();
    // Lowercasing can change byte lengths (e.g. some non-ASCII letters); only
    // highlight when the byte offsets line up.
    let range = (!query.is_empty() && lower.len() == text.len())
        .then(|| lower.find(query).map(|start| start..start + query.len()))
        .flatten()
        .filter(|r| text.is_char_boundary(r.start) && text.is_char_boundary(r.end));
    match range {
        Some(r) => h_flex()
            .child(text[..r.start].to_string())
            .child(
                div()
                    .text_color(color)
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .child(text[r.clone()].to_string()),
            )
            .child(text[r.end..].to_string())
            .into_any_element(),
        None => div().child(text).into_any_element(),
    }
}

impl Focusable for SettingsWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, side_bg) = (theme.border, theme.muted_foreground, theme.muted);
        let query = self.search.read(cx).value().trim().to_lowercase();
        let searching = !query.is_empty();
        let results = self.search_results(cx);
        let dirty = self.is_dirty(cx);
        let mut context = KeyContext::default();
        context.add("SettingsWindow");

        // Left: search and the category list.
        let sidebar = v_flex()
            .w(px(200.))
            .h_full()
            .p_2()
            .gap_1()
            .bg(side_bg)
            .border_r_1()
            .border_color(border)
            .child(
                Input::new(&self.search)
                    .small()
                    .cleanable(true)
                    .aria_label(tr("settings-search-placeholder")),
            )
            .children(SettingsPage::ALL.into_iter().map(|page| {
                let count = results.iter().filter(|k| k.page() == page).count()
                    + (page == SettingsPage::Keymap) as usize;
                Button::new(SharedString::from(format!("page-{}", page.title_key())))
                    .ghost()
                    .small()
                    .w_full()
                    .justify_start()
                    .icon(Icon::from(page.icon()))
                    .label(tr(page.title_key()))
                    .selected(!searching && self.page == page)
                    .disabled(searching && count == 0)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.search
                            .update(cx, |s, cx| s.set_value(String::new(), window, cx));
                        this.set_page(page, cx);
                    }))
            }));

        // Right: the page, or the search results across pages.
        let mut body = v_flex()
            .id("settings-body")
            .flex_1()
            .min_w_0()
            .h_full()
            .p_4()
            .gap_1()
            .overflow_y_scroll();
        if searching {
            if results.is_empty() {
                body = body.child(ferro_ui::components::empty_state(
                    FerroIcon::Search,
                    tr("settings-no-results"),
                    None,
                    cx,
                ));
            }
            for page in SettingsPage::ALL {
                let keys: Vec<_> = results.iter().filter(|k| k.page() == page).collect();
                if keys.is_empty() {
                    continue;
                }
                body = body.child(
                    Button::new(SharedString::from(format!("jump-{}", page.title_key())))
                        .ghost()
                        .xsmall()
                        .label(tr(page.title_key()))
                        .tooltip(tr("settings-jump-to-page"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.search
                                .update(cx, |s, cx| s.set_value(String::new(), window, cx));
                            this.set_page(page, cx);
                        })),
                );
                for key in keys {
                    body = body.child(self.render_item(*key, &query, cx));
                }
            }
        } else {
            body = body.child(
                div()
                    .text_lg()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .pb_2()
                    .child(tr(self.page.title_key())),
            );
            for key in SettingKey::ALL
                .into_iter()
                .filter(|k| k.page() == self.page)
            {
                body = body.child(self.render_item(key, "", cx));
            }
            if self.page == SettingsPage::Keymap {
                body = body.child(div().h(px(420.)).child(self.keymap.clone()));
            }
        }

        let footer = h_flex()
            .gap_2()
            .px_4()
            .py_2()
            .border_t_1()
            .border_color(border)
            .child(
                Button::new("open-settings-file")
                    .ghost()
                    .small()
                    .icon(Icon::from(FerroIcon::FileConfig))
                    .label(tr("action-open-settings-file"))
                    .on_click(|_, window, cx| {
                        window.close_dialog(cx);
                        window.dispatch_action(Box::new(crate::actions::OpenSettingsFile), cx);
                    }),
            )
            .child(div().flex_1())
            .when(dirty, |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(tr("settings-unsaved")),
                )
            })
            .child(
                Button::new("settings-reset")
                    .small()
                    .label(tr("settings-reset"))
                    .disabled(!dirty)
                    .on_click(cx.listener(|this, _, _, cx| this.reset(cx))),
            )
            .child(
                Button::new("settings-cancel")
                    .small()
                    .label(tr("settings-cancel"))
                    .on_click(|_, window, cx| window.close_dialog(cx)),
            )
            .child(
                Button::new("settings-apply")
                    .small()
                    .label(tr("settings-apply"))
                    .disabled(!dirty)
                    .on_click(cx.listener(|this, _, _, cx| this.apply(cx))),
            )
            .child(
                Button::new("settings-ok")
                    .small()
                    .primary()
                    .label(tr("settings-ok"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.apply(cx);
                        window.close_dialog(cx);
                    })),
            );

        v_flex()
            .id("settings-window")
            .key_context(context)
            .track_focus(&self.focus_handle)
            .w_full()
            .h(px(620.))
            .child(h_flex().flex_1().min_h_0().child(sidebar).child(body))
            .child(footer)
    }
}

/// Open the Settings window on `page` and return its view.
pub fn show(
    page: SettingsPage,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Entity<SettingsWindow> {
    let view = cx.new(|cx| SettingsWindow::new(page, window, cx));
    let content = view.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title(tr("settings-title"))
            .w(px(940.))
            .child(content.clone())
    });
    let search = view.read(cx).search.clone();
    search.update(cx, |s, cx| s.focus(window, cx));
    view
}

pub fn open(
    _workspace: &mut Workspace,
    page: SettingsPage,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    show(page, window, cx);
}

/// Open the raw `settings.toml` in an editor tab, writing it first so it
/// exists.
pub fn open_settings_file(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let state = AppState::global(cx);
    let path = state.paths.settings_file();
    if state.persist
        && let Err(err) = state.paths.save_settings(&state.settings)
    {
        tracing::warn!("could not write {}: {err:#}", path.display());
    }
    workspace.open_location(&OpenLocation::file(path), window, cx);
}
