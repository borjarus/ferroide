//! Rendering of the editor's view layer around GPUI Kit's editor: the
//! breadcrumbs and toolbar row, the glyph margin (breakpoints, ▶ run
//! icons), sticky lines, end-of-line inlay hints and the Parameter Info
//! popup.
//!
//! Overlays are positioned from the editor state's layout (line height,
//! scroll offset, caret bounds) of the previous frame; the state notifies
//! on scroll, which re-renders the panel.

use ferro_services::{RunnableKind, TextPos};
use ferro_ui::{FerroIcon, tr, tr_args};
use gpui_kit::{
    Anchor, AnyElement, Context, Entity, InteractiveElement as _, IntoElement, KeyDownEvent,
    ParentElement as _, Pixels, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    anchored,
    component::{
        ActiveTheme as _, Icon, Selectable as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Editor, EditorState, RopeExt as _},
        menu::DropdownMenu as _,
        v_flex,
    },
    deferred, div, font, point,
    prelude::FluentBuilder as _,
    px,
};

use super::{EditorPanel, model};
use crate::{actions, app_state::AppState};

/// Where the first text row starts below the editor's top edge (GPUI Kit's
/// medium input padding).
pub const TEXT_TOP_INSET: f32 = 8.0;
/// Width of the glyph margin (breakpoint slot + run icon slot).
pub const GLYPH_MARGIN_WIDTH: f32 = 34.0;
/// Deepest nesting of sticky headers shown.
pub const MAX_STICKY_LINES: usize = 3;

impl EditorPanel {
    pub(super) fn render_loaded(
        &mut self,
        state: &Entity<EditorState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.measure(state, window, cx);
        let settings = AppState::settings(cx).editor.clone();
        let (line_height, scroll, visible) = {
            let s = state.read(cx);
            (
                s.line_height().map(f32::from).unwrap_or(20.0),
                s.scroll_offset(),
                s.visible_row_range(),
            )
        };
        let scroll_y = f32::from(scroll.y);
        let row_top = move |line: u32| px(TEXT_TOP_INSET + line as f32 * line_height + scroll_y);
        let visible = visible.unwrap_or(0..60);
        let first_visible = model::first_visible_line(-scroll_y, TEXT_TOP_INSET, line_height);

        let file_label: SharedString = self.file_name();
        let editor = Editor::new(state)
            .bordered(false)
            .h_full()
            .aria_label(file_label);

        let mut stack = div()
            .id("editor-stack")
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .child(editor);
        if settings.inlay_hints && !settings.soft_wrap {
            stack = stack.children(self.render_hints(state, &visible, row_top, scroll, cx));
        }
        if settings.sticky_lines && first_visible > 0 {
            stack = stack.child(self.render_sticky(state, first_visible, line_height, window, cx));
        }

        v_flex()
            .size_full()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.close_signature(cx);
                }
            }))
            .child(self.render_header(settings.breadcrumbs, settings.soft_wrap, cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_glyph_margin(&visible, row_top, line_height, cx))
                    .child(stack),
            )
            .when_some(self.render_signature(state, cx), |el, popup| {
                el.child(popup)
            })
    }

    /// Measure the monospace character width and where column 0 starts.
    fn measure(
        &mut self,
        state: &Entity<EditorState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.char_width.is_none() {
            let theme = cx.theme();
            let font_id = window
                .text_system()
                .resolve_font(&font(theme.mono_font_family.clone()));
            self.char_width = window
                .text_system()
                .advance(font_id, theme.mono_font_size, 'm')
                .ok()
                .map(|size| size.width);
        }
        if self.text_left.is_some() {
            return;
        }
        let (Some(char_width), Some((caret, _))) =
            (self.char_width, state.read(cx).cursor_layout())
        else {
            return;
        };
        let s = state.read(cx);
        let column = s.cursor_position().character as f32;
        let left =
            caret.left() - s.input_bounds().left() - char_width * column - s.scroll_offset().x;
        if left > px(0.) && left < px(240.) {
            self.text_left = Some(left);
        }
    }

    fn render_header(
        &self,
        breadcrumbs: bool,
        soft_wrap: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let inlay = AppState::settings(cx).editor.inlay_hints;
        let mut crumbs = h_flex()
            .id("editor-breadcrumbs")
            .flex_1()
            .min_w_0()
            .gap_1()
            .overflow_hidden()
            .text_xs()
            .text_color(muted);
        if breadcrumbs {
            let relative = self
                .services
                .project
                .roots()
                .iter()
                .find_map(|root| self.path.strip_prefix(root).ok().map(|p| p.to_path_buf()))
                .unwrap_or_else(|| self.path.clone());
            let segments: Vec<String> = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            for (ix, segment) in segments.into_iter().enumerate() {
                if ix > 0 {
                    crumbs = crumbs.child("›");
                }
                crumbs = crumbs.child(div().whitespace_nowrap().child(segment));
            }
            for (ix, symbol) in self.breadcrumb_symbols(cx).into_iter().enumerate() {
                let target = symbol.selection;
                crumbs = crumbs.child("›").child(
                    Button::new(("editor-crumb", ix))
                        .ghost()
                        .xsmall()
                        .label(symbol.name.clone())
                        .tooltip(tr("editor-breadcrumb-tooltip"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.go_to(target, window, cx);
                        })),
                );
            }
        }
        let (errors, warnings) = self.problem_counts();
        let toolbar = h_flex()
            .gap_1()
            .flex_none()
            .when(errors + warnings > 0, |el| {
                el.child(
                    Button::new("editor-problems")
                        .ghost()
                        .xsmall()
                        .label(tr_args(
                            "editor-problem-count",
                            &[("errors", errors.into()), ("warnings", warnings.into())],
                        ))
                        .tooltip(tr("action-next-problem"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            let services = this.services.clone();
                            this.run_command(
                                super::EditorCommand::NextProblem,
                                &services,
                                window,
                                cx,
                            )
                        })),
                )
            })
            .child(
                Button::new("editor-search")
                    .ghost()
                    .xsmall()
                    .icon(Icon::from(FerroIcon::Search))
                    .tooltip(tr("editor-toolbar-search"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        if let Some(state) = this.state().cloned() {
                            state.update(cx, |state, cx| {
                                state.focus(window, cx);
                                state.open_search(false, cx);
                            });
                        }
                    })),
            )
            .child(
                Button::new("editor-soft-wrap")
                    .ghost()
                    .xsmall()
                    .label(tr("editor-toolbar-soft-wrap"))
                    .selected(soft_wrap)
                    .tooltip(tr("editor-toolbar-soft-wrap"))
                    .on_click(|_, _, cx| {
                        AppState::update_settings(cx, |s| s.editor.soft_wrap = !s.editor.soft_wrap)
                    }),
            )
            .child(
                Button::new("editor-inlay-hints")
                    .ghost()
                    .xsmall()
                    .label(tr("editor-toolbar-inlay-hints"))
                    .selected(inlay)
                    .tooltip(tr("editor-toolbar-inlay-hints"))
                    .on_click(|_, _, cx| {
                        AppState::update_settings(cx, |s| {
                            s.editor.inlay_hints = !s.editor.inlay_hints
                        })
                    }),
            )
            .child(
                Button::new("editor-show-diff")
                    .ghost()
                    .xsmall()
                    .icon(Icon::from(FerroIcon::Diff))
                    .tooltip(tr("action-show-diff"))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(actions::ShowDiff), cx)
                    }),
            );
        h_flex()
            .id("editor-header")
            .h(px(28.))
            .px_2()
            .gap_2()
            .border_b_1()
            .border_color(border)
            .child(crumbs)
            .child(toolbar)
    }

    fn render_glyph_margin(
        &self,
        visible: &std::ops::Range<usize>,
        row_top: impl Fn(u32) -> Pixels,
        line_height: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (danger, success, hover) = (theme.danger, theme.success, theme.list_hover);
        let mut margin = div()
            .id("editor-glyph-margin")
            .relative()
            .w(px(GLYPH_MARGIN_WIDTH))
            .h_full()
            .flex_none()
            .overflow_hidden()
            .bg(theme.background);
        for line in visible.start as u32..visible.end as u32 {
            let breakpoint = self
                .breakpoints
                .iter()
                .find(|b| b.line == line)
                .map(|b| b.enabled);
            let runnable = self.analysis.runnables.iter().find(|r| r.line == line);
            let mut row = h_flex()
                .id(("glyph-row", line as usize))
                .absolute()
                .top(row_top(line))
                .left_0()
                .w_full()
                .h(px(line_height))
                .cursor_pointer()
                .hover(|el| el.bg(hover))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_breakpoint(line, cx)))
                .child(div().w(px(16.)).flex().justify_center().when_some(
                    breakpoint,
                    |el, enabled| {
                        el.child(
                            div()
                                .size(px(10.))
                                .rounded_full()
                                .when(enabled, |el| el.bg(danger))
                                .when(!enabled, |el| el.border_1().border_color(danger)),
                        )
                    },
                ));
            if let Some(runnable) = runnable {
                let tooltip = match runnable.kind {
                    RunnableKind::Main => tr_args("editor-run-main", &[]),
                    RunnableKind::Test | RunnableKind::TestModule => tr_args(
                        "editor-run-test",
                        &[("name", runnable.label.clone().into())],
                    ),
                };
                row = row.child(
                    Button::new(("glyph-run", line as usize))
                        .ghost()
                        .xsmall()
                        .icon(Icon::from(FerroIcon::Run).text_color(success))
                        .tooltip(tooltip)
                        .dropdown_menu(|menu, _, _| {
                            menu.menu(tr("action-run"), Box::new(actions::Run))
                                .menu(tr("action-debug"), Box::new(actions::Debug))
                        }),
                );
            }
            margin = margin.child(row);
        }
        margin
    }

    fn render_hints(
        &self,
        state: &Entity<EditorState>,
        visible: &std::ops::Range<usize>,
        row_top: impl Fn(u32) -> Pixels,
        scroll: gpui_kit::Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let (Some(char_width), Some(text_left)) = (self.char_width, self.text_left) else {
            return Vec::new();
        };
        let _ = state;
        let theme = cx.theme();
        let (muted, bg) = (theme.muted_foreground, theme.secondary);
        let font_family = theme.mono_font_family.clone();
        let font_size = theme.mono_font_size;
        let first = self
            .analysis
            .line_hints
            .partition_point(|h| (h.line as usize) < visible.start);
        self.analysis.line_hints[first..]
            .iter()
            .take_while(|h| (h.line as usize) < visible.end)
            .cloned()
            .map(|hint| {
                let left = text_left + char_width * (hint.line_len as f32 + 2.0) + scroll.x;
                div()
                    .absolute()
                    .top(row_top(hint.line))
                    .left(left)
                    .px_1()
                    .rounded_sm()
                    .bg(bg)
                    .text_color(muted)
                    .font_family(font_family.clone())
                    .text_size(font_size)
                    .whitespace_nowrap()
                    .child(hint.label)
                    .into_any_element()
            })
            .collect()
    }

    fn render_sticky(
        &self,
        state: &Entity<EditorState>,
        first_visible: u32,
        line_height: f32,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let lines = model::sticky_lines(&self.analysis.symbols, first_visible, MAX_STICKY_LINES);
        let rope = state.read(cx).text().clone();
        let theme = cx.theme();
        let (bg, border, hover, fg) = (
            theme.background,
            theme.border,
            theme.list_hover,
            theme.foreground,
        );
        let font_family = theme.mono_font_family.clone();
        let font_size = theme.mono_font_size;
        let left = self.text_left.unwrap_or(px(48.));
        v_flex()
            .id("editor-sticky-lines")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bg(bg)
            .when(!lines.is_empty(), |el| el.border_b_1().border_color(border))
            .children(lines.into_iter().map(|line| {
                let start =
                    rope.position_to_offset(&gpui_kit::component::input::Position::new(line, 0));
                let end = rope
                    .position_to_offset(&gpui_kit::component::input::Position::new(line + 1, 0));
                let text = rope
                    .slice(start..end.max(start))
                    .to_string()
                    .trim_end()
                    .to_string();
                div()
                    .id(("sticky-line", line as usize))
                    .h(px(line_height))
                    .pl(left)
                    .font_family(font_family.clone())
                    .text_size(font_size)
                    .text_color(fg)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .cursor_pointer()
                    .hover(|el| el.bg(hover))
                    .child(text)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.go_to(TextPos::new(line, 0), window, cx)
                    }))
            }))
    }

    fn render_signature(
        &self,
        state: &Entity<EditorState>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let help = self.signature.as_ref()?;
        let (caret, _) = state.read(cx).cursor_layout()?;
        let theme = cx.theme();
        let label = &help.label;
        let active = help
            .parameters
            .get(help.active_parameter)
            .filter(|r| {
                r.end <= label.len()
                    && label.is_char_boundary(r.start)
                    && label.is_char_boundary(r.end)
            })
            .cloned()
            .unwrap_or(0..0);
        let popup = v_flex()
            .id("editor-parameter-info")
            .max_w(px(520.))
            .p_2()
            .gap_1()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_md()
            .font_family(theme.mono_font_family.clone())
            .text_size(theme.mono_font_size)
            .child(
                h_flex()
                    .child(label[..active.start].to_string())
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(theme.primary)
                            .child(label[active.clone()].to_string()),
                    )
                    .child(label[active.end..].to_string()),
            )
            .when_some(help.documentation.clone(), |el, doc| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(doc),
                )
            });
        Some(
            deferred(
                anchored()
                    .position(point(caret.left(), caret.top()))
                    .anchor(Anchor::BottomLeft)
                    .snap_to_window()
                    .child(popup),
            )
            .into_any_element(),
        )
    }
}
