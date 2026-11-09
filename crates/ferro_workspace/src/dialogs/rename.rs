//! Rename Symbol with a preview of every affected line.
//!
//! The dialog asks the language service for the edits of a rename as the
//! name is typed, lists them grouped by file (before → after) with a
//! checkbox each, and applies the selected ones to the open editors.

use std::path::{Path, PathBuf};

use ferro_services::{Services, TextEdit, TextPos, WorkspaceEdit};
use ferro_ui::{FerroIcon, tr, tr_args};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, WeakEntity, Window,
    component::{
        ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _,
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
        h_flex,
        input::{Input, InputEvent, InputState},
        notification::Notification,
        spinner::Spinner,
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};

use crate::workspace::Workspace;

const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while",
];

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Check a new identifier; returns the message id of the problem.
pub fn validate_identifier(name: &str) -> Result<(), &'static str> {
    let mut chars = name.chars();
    match chars.next() {
        None => Err("dialogs-rename-error-empty"),
        Some(first) if !(first.is_alphabetic() || first == '_') => {
            Err("dialogs-rename-error-invalid")
        }
        _ if !chars.all(is_ident_char) => Err("dialogs-rename-error-invalid"),
        _ if KEYWORDS.contains(&name) => Err("dialogs-rename-error-keyword"),
        _ => Ok(()),
    }
}

/// The identifier at `pos` (0-based line, column in chars) and where it
/// starts.
pub fn identifier_at(text: &str, pos: TextPos) -> Option<(String, TextPos)> {
    let line = text.lines().nth(pos.line as usize)?;
    let chars: Vec<char> = line.chars().collect();
    let col = (pos.column as usize).min(chars.len());
    // The caret may sit just after the identifier.
    let at = if col < chars.len() && is_ident_char(chars[col]) {
        col
    } else if col > 0 && is_ident_char(chars[col - 1]) {
        col - 1
    } else {
        return None;
    };
    let start = (0..=at)
        .rev()
        .take_while(|&i| is_ident_char(chars[i]))
        .last()?;
    let end = (at..chars.len())
        .take_while(|&i| is_ident_char(chars[i]))
        .last()?
        + 1;
    let word: String = chars[start..end].iter().collect();
    (!word.chars().next()?.is_ascii_digit()).then_some((word, TextPos::new(pos.line, start as u32)))
}

/// Apply edits to `text`. Positions are 0-based lines and char columns;
/// edits must not overlap.
pub fn apply_edits(text: &str, edits: &[TextEdit]) -> String {
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let offset = |pos: TextPos| -> usize {
        let Some(&start) = line_starts.get(pos.line as usize) else {
            return text.len();
        };
        let line = &text[start..];
        let line = &line[..line.find('\n').unwrap_or(line.len())];
        start
            + line
                .char_indices()
                .nth(pos.column as usize)
                .map(|(i, _)| i)
                .unwrap_or(line.len())
    };
    let mut ranges: Vec<(usize, usize, &str)> = edits
        .iter()
        .map(|e| {
            (
                offset(e.range.start),
                offset(e.range.end),
                e.new_text.as_str(),
            )
        })
        .collect();
    ranges.sort_by_key(|r| std::cmp::Reverse(r.0));
    let mut out = text.to_string();
    for (start, end, new) in ranges {
        if start <= end && end <= out.len() {
            out.replace_range(start..end, new);
        }
    }
    out
}

enum Preview {
    Idle,
    Loading(#[allow(dead_code)] Task<()>),
    Ready(WorkspaceEdit),
    Failed(SharedString),
}

pub struct RenameDialog {
    workspace: WeakEntity<Workspace>,
    services: Services,
    path: PathBuf,
    text: String,
    position: TextPos,
    original: String,
    input: Entity<InputState>,
    preview: Preview,
    /// Per file, per edit: whether it will be applied.
    selected: Vec<Vec<bool>>,
    invalid: Option<&'static str>,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

/// What is being renamed: a document, its text and the identifier's start.
#[derive(Debug, Clone)]
pub struct RenameTarget {
    pub path: PathBuf,
    pub text: String,
    pub position: TextPos,
    pub original: String,
}

impl RenameDialog {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        services: Services,
        target: RenameTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let RenameTarget {
            path,
            text,
            position,
            original,
        } = target;
        let input = cx.new(|cx| InputState::new(window, cx).default_value(original.clone()));
        let subscription =
            cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    if this.apply(window, cx) > 0 {
                        window.close_dialog(cx);
                    }
                }
                InputEvent::Change => this.update_preview(cx),
                _ => {}
            });
        Self {
            workspace,
            services,
            path,
            text,
            position,
            original,
            input,
            preview: Preview::Idle,
            selected: Vec::new(),
            invalid: None,
            focus_handle: cx.focus_handle(),
            _subscription: subscription,
        }
    }

    pub fn new_name(&self, cx: &App) -> String {
        self.input.read(cx).value().trim().to_string()
    }

    pub fn set_name(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |i, cx| i.set_value(name.to_string(), window, cx));
        self.update_preview(cx);
    }

    pub fn invalid(&self) -> Option<&'static str> {
        self.invalid
    }

    /// The preview, once the language service answered.
    pub fn edits(&self) -> Option<&WorkspaceEdit> {
        match &self.preview {
            Preview::Ready(edit) => Some(edit),
            _ => None,
        }
    }

    pub fn set_selected(&mut self, file: usize, edit: usize, on: bool, cx: &mut Context<Self>) {
        if let Some(slot) = self.selected.get_mut(file).and_then(|f| f.get_mut(edit)) {
            *slot = on;
            cx.notify();
        }
    }

    pub fn update_preview(&mut self, cx: &mut Context<Self>) {
        let name = self.new_name(cx);
        self.invalid = validate_identifier(&name).err();
        if self.invalid.is_some() || name == self.original {
            self.preview = Preview::Idle;
            self.selected.clear();
            cx.notify();
            return;
        }
        let task = cx.background_spawn(self.services.language.rename(
            &self.path,
            &self.text,
            self.position,
            &name,
        ));
        self.preview = Preview::Loading(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                // A newer request replaced this one.
                if this.new_name(cx) != name {
                    return;
                }
                match result {
                    Ok(edit) => {
                        this.selected = edit
                            .files
                            .iter()
                            .map(|f| vec![true; f.edits.len()])
                            .collect();
                        this.preview = Preview::Ready(edit);
                    }
                    Err(err) => this.preview = Preview::Failed(err.to_string().into()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Apply the selected edits to the editors that have the files open.
    /// Returns the number of edits applied.
    pub fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) -> usize {
        let Preview::Ready(edit) = &self.preview else {
            return 0;
        };
        let mut per_file: Vec<(PathBuf, Vec<TextEdit>)> = Vec::new();
        for (fi, file) in edit.files.iter().enumerate() {
            let chosen: Vec<TextEdit> = file
                .edits
                .iter()
                .enumerate()
                .filter(|(ei, _)| self.selected.get(fi).and_then(|f| f.get(*ei)) == Some(&true))
                .map(|(_, e)| e.edit.clone())
                .collect();
            if !chosen.is_empty() {
                per_file.push((file.path.clone(), chosen));
            }
        }
        self.workspace
            .update(cx, |ws, cx| {
                let mut count = (0, 0, 0);
                for (path, edits) in &per_file {
                    let editor = ws
                        .editors
                        .iter()
                        .find(|e| e.read(cx).path() == path.as_path())
                        .cloned();
                    match editor.and_then(|e| e.read(cx).text(cx).map(|t| (e, t))) {
                        Some((editor, text)) => {
                            let new = apply_edits(&text, edits);
                            editor.update(cx, |e, cx| e.set_text(new, window, cx));
                            count.0 += edits.len();
                            count.1 += 1;
                        }
                        None => count.2 += 1,
                    }
                }
                let mut message = tr_args(
                    "dialogs-rename-done",
                    &[("count", count.0.into()), ("files", count.1.into())],
                )
                .to_string();
                if count.2 > 0 {
                    message.push_str(&format!(
                        " {}",
                        tr_args("dialogs-rename-skipped", &[("files", count.2.into())])
                    ));
                }
                window.push_notification(Notification::info(message), cx);
                count.0
            })
            .unwrap_or(0)
    }
}

impl Focusable for RenameDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

impl Render for RenameDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, danger, success, border) = (
            theme.muted_foreground,
            theme.danger,
            theme.success,
            theme.border,
        );
        let root = self.services.project.roots()[0].clone();
        let can_apply =
            matches!(self.preview, Preview::Ready(_)) && self.selected.iter().flatten().any(|s| *s);

        let preview = match &self.preview {
            Preview::Idle => div()
                .text_sm()
                .text_color(muted)
                .child(tr("dialogs-rename-type-name"))
                .into_any_element(),
            Preview::Loading(_) => h_flex()
                .gap_2()
                .text_sm()
                .child(Spinner::new().small())
                .child(tr("dialogs-rename-loading"))
                .into_any_element(),
            Preview::Failed(err) => div()
                .text_sm()
                .text_color(danger)
                .child(err.clone())
                .into_any_element(),
            Preview::Ready(edit) if edit.files.is_empty() => div()
                .text_sm()
                .text_color(muted)
                .child(tr("dialogs-rename-no-usages"))
                .into_any_element(),
            Preview::Ready(edit) => v_flex()
                .gap_2()
                .child(div().text_xs().text_color(muted).child(tr_args(
                    "dialogs-rename-summary",
                    &[
                        ("count", edit.edit_count().into()),
                        ("files", edit.files.len().into()),
                    ],
                )))
                .children(edit.files.iter().enumerate().map(|(fi, file)| {
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_1()
                                .text_sm()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(
                                    Icon::from(FerroIcon::for_file(&relative(&file.path, &root)))
                                        .small(),
                                )
                                .child(relative(&file.path, &root)),
                        )
                        .children(file.edits.iter().enumerate().map(|(ei, e)| {
                            let checked = self
                                .selected
                                .get(fi)
                                .and_then(|f| f.get(ei))
                                .copied()
                                .unwrap_or(false);
                            h_flex()
                                .gap_2()
                                .pl_4()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_xs()
                                .child(
                                    Checkbox::new(("rename-edit", fi * 10_000 + ei))
                                        .checked(checked)
                                        .accessibility_label(e.line_after.clone())
                                        .on_click(cx.listener(move |this, on: &bool, _, cx| {
                                            this.set_selected(fi, ei, *on, cx)
                                        })),
                                )
                                .child(
                                    div()
                                        .w(px(36.))
                                        .text_color(muted)
                                        .child(format!("{}", e.edit.range.start.line + 1)),
                                )
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .truncate()
                                                .text_color(danger)
                                                .child(format!("- {}", e.line_before.trim())),
                                        )
                                        .child(
                                            div()
                                                .truncate()
                                                .text_color(success)
                                                .child(format!("+ {}", e.line_after.trim())),
                                        ),
                                )
                        }))
                }))
                .into_any_element(),
        };

        v_flex()
            .id("rename-dialog")
            .track_focus(&self.focus_handle)
            .gap_3()
            .child(div().text_xs().text_color(muted).child(tr_args(
                "dialogs-rename-symbol",
                &[("name", self.original.clone().into())],
            )))
            .child(Input::new(&self.input).aria_label(tr("dialogs-rename-new-name")))
            .when_some(self.invalid, |el, key| {
                el.child(div().text_sm().text_color(danger).child(tr(key)))
            })
            .child(
                div()
                    .id("rename-preview")
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .p_2()
                    .border_1()
                    .border_color(border)
                    .rounded(px(4.))
                    .child(preview),
            )
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("rename-cancel")
                            .small()
                            .label(tr("settings-cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("rename-apply")
                            .small()
                            .primary()
                            .label(tr("dialogs-rename-apply"))
                            .disabled(!can_apply)
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.apply(window, cx) > 0 {
                                    window.close_dialog(cx);
                                }
                            })),
                    ),
            )
    }
}

/// Open the dialog for the identifier at the caret of the active editor.
/// Returns `None` (after a notification) when there is none.
pub fn show(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Option<Entity<RenameDialog>> {
    let found = workspace.active_editor_entity().and_then(|editor| {
        let editor = editor.read(cx);
        let text = editor.text(cx)?;
        let caret = editor.caret(cx)?;
        let (name, start) = identifier_at(&text, caret)?;
        Some(RenameTarget {
            path: editor.path().to_path_buf(),
            text,
            position: start,
            original: name,
        })
    });
    let Some(target) = found else {
        workspace.notify_info("dialogs-rename-no-symbol", window, cx);
        return None;
    };
    let services = workspace.services.clone();
    let handle = cx.entity().downgrade();
    let view = cx.new(|cx| RenameDialog::new(handle, services, target, window, cx));
    let content = view.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title(tr("action-rename-symbol"))
            .w(px(620.))
            .child(content.clone())
    });
    let input = view.read(cx).input.clone();
    input.update(cx, |i, cx| {
        i.focus(window, cx);
        i.select_all(window, cx);
    });
    Some(view)
}

pub fn open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    show(workspace, window, cx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferro_services::TextRange;

    #[test]
    fn identifiers_are_validated() {
        assert!(validate_identifier("greet_all").is_ok());
        assert!(validate_identifier("_x1").is_ok());
        assert_eq!(validate_identifier(""), Err("dialogs-rename-error-empty"));
        assert_eq!(
            validate_identifier("1x"),
            Err("dialogs-rename-error-invalid")
        );
        assert_eq!(
            validate_identifier("a-b"),
            Err("dialogs-rename-error-invalid")
        );
        assert_eq!(
            validate_identifier("fn"),
            Err("dialogs-rename-error-keyword")
        );
    }

    #[test]
    fn identifier_at_caret() {
        let text = "fn main() {\n    greet(name);\n}";
        assert_eq!(
            identifier_at(text, TextPos::new(1, 6)),
            Some(("greet".into(), TextPos::new(1, 4)))
        );
        // Right after the identifier.
        assert_eq!(
            identifier_at(text, TextPos::new(1, 9)).map(|(w, _)| w),
            Some("greet".into())
        );
        assert_eq!(identifier_at(text, TextPos::new(1, 2)), None);
        assert_eq!(identifier_at(text, TextPos::new(9, 0)), None);
    }

    #[test]
    fn edits_apply_in_any_order() {
        let text = "let a = greet(x);\ngreet(y);\nżółć greet";
        let edit = |l, s, e| TextEdit {
            range: TextRange::on_line(l, s, e),
            new_text: "hello".into(),
        };
        let out = apply_edits(text, &[edit(1, 0, 5), edit(0, 8, 13), edit(2, 5, 10)]);
        assert_eq!(out, "let a = hello(x);\nhello(y);\nżółć hello");
    }
}
