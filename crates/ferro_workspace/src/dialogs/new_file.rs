//! New File: name, kind and location, created through the project service.

use std::path::{Path, PathBuf};

use ferro_services::{EntryKind, Services};
use ferro_ui::{FerroIcon, tr};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription, Task,
    WeakEntity, Window,
    component::{
        ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, WindowExt as _,
        button::{Button, ButtonGroup, ButtonVariants as _},
        h_flex,
        input::{Input, InputEvent, InputState},
        spinner::Spinner,
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};

use crate::{requests::OpenLocation, workspace::Workspace};

/// Characters no file name may contain on any supported platform.
const INVALID_CHARS: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|', '\0'];

/// Check a new entry's name; returns the message id of the problem.
pub fn validate_name(name: &str) -> Result<(), &'static str> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("dialogs-new-file-error-empty");
    }
    if trimmed != name
        || name == "."
        || name == ".."
        || name.ends_with('.')
        || name
            .chars()
            .any(|c| INVALID_CHARS.contains(&c) || c.is_control())
    {
        return Err("dialogs-new-file-error-invalid");
    }
    Ok(())
}

pub struct NewFileDialog {
    workspace: WeakEntity<Workspace>,
    services: Services,
    root: PathBuf,
    name: Entity<InputState>,
    directory: Entity<InputState>,
    kind: EntryKind,
    error: Option<SharedString>,
    creating: Option<Task<()>>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl NewFileDialog {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        services: Services,
        directory: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let root = services.project.roots()[0].clone();
        let relative = directory
            .strip_prefix(&root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let name = cx.new(|cx| {
            InputState::new(window, cx).placeholder(tr("dialogs-new-file-name-placeholder"))
        });
        let directory = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(tr("dialogs-new-file-directory-placeholder"))
                .default_value(relative)
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &name,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.create(window, cx),
                    _ => {
                        this.error = None;
                        cx.notify();
                    }
                },
            ),
            cx.subscribe(&directory, |this, _, _: &InputEvent, cx| {
                this.error = None;
                cx.notify();
            }),
        ];
        Self {
            workspace,
            services,
            root,
            name,
            directory,
            kind: EntryKind::File,
            error: None,
            creating: None,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    pub fn set_kind(&mut self, kind: EntryKind, cx: &mut Context<Self>) {
        self.kind = kind;
        cx.notify();
    }

    pub fn set_name(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.name
            .update(cx, |i, cx| i.set_value(name.to_string(), window, cx));
        self.error = None;
        cx.notify();
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// The path the entry will get.
    pub fn target(&self, cx: &App) -> PathBuf {
        let dir = self.directory.read(cx).value().trim().replace('\\', "/");
        let mut path = self.root.clone();
        for part in dir.split('/').filter(|p| !p.is_empty() && *p != ".") {
            path.push(part);
        }
        path.join(self.name.read(cx).value().as_ref())
    }

    /// Validate, then create through the project service; on success open
    /// the file, reveal it in the Project tool window and close.
    pub fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.creating.is_some() {
            return;
        }
        let name = self.name.read(cx).value().to_string();
        if let Err(key) = validate_name(&name) {
            self.error = Some(tr(key));
            cx.notify();
            return;
        }
        let dir = self.directory.read(cx).value().to_string();
        if dir.split(['/', '\\']).any(|p| p == "..") {
            self.error = Some(tr("dialogs-new-file-error-outside"));
            cx.notify();
            return;
        }
        let path = self.target(cx);
        let kind = self.kind;
        let task = cx.background_spawn(self.services.project.create(&path, kind));
        self.creating = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.creating = None;
                match result {
                    Ok(()) => {
                        let _ = this.workspace.update(cx, |ws, cx| {
                            if kind == EntryKind::File {
                                ws.open_location(&OpenLocation::file(path.clone()), window, cx);
                            }
                            ws.reveal_in_project(path.clone(), window, cx);
                        });
                        window.close_dialog(cx);
                    }
                    Err(err) => this.error = Some(err.to_string().into()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}

impl Focusable for NewFileDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for NewFileDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        let busy = self.creating.is_some();
        v_flex()
            .id("new-file-dialog")
            .track_focus(&self.focus_handle)
            .gap_3()
            .child(
                ButtonGroup::new("new-file-kind")
                    .small()
                    .child(
                        Button::new("kind-file")
                            .icon(Icon::from(FerroIcon::NewFile))
                            .label(tr("dialogs-new-file-kind-file"))
                            .selected(self.kind == EntryKind::File)
                            .on_click(
                                cx.listener(|this, _, _, cx| this.set_kind(EntryKind::File, cx)),
                            ),
                    )
                    .child(
                        Button::new("kind-dir")
                            .icon(Icon::from(FerroIcon::Folder))
                            .label(tr("dialogs-new-file-kind-directory"))
                            .selected(self.kind == EntryKind::Directory)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_kind(EntryKind::Directory, cx)
                            })),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(tr("dialogs-new-file-name")),
                    )
                    .child(Input::new(&self.name).aria_label(tr("dialogs-new-file-name"))),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(tr("dialogs-new-file-directory")),
                    )
                    .child(
                        Input::new(&self.directory)
                            .small()
                            .aria_label(tr("dialogs-new-file-directory")),
                    ),
            )
            .when_some(self.error.clone(), |el, err| {
                el.child(
                    h_flex()
                        .gap_1()
                        .text_sm()
                        .text_color(danger)
                        .child(Icon::from(FerroIcon::Error).small())
                        .child(err),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .when(busy, |el| el.child(Spinner::new().small()))
                    .child(
                        Button::new("new-file-cancel")
                            .small()
                            .label(tr("settings-cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("new-file-create")
                            .small()
                            .primary()
                            .label(tr("dialogs-new-file-create"))
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| this.create(window, cx))),
                    ),
            )
    }
}

/// Open the dialog; the directory defaults to the active editor's.
pub fn show(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Entity<NewFileDialog> {
    let services = workspace.services.clone();
    let root = services.project.roots()[0].clone();
    let directory = workspace
        .active_editor_entity()
        .and_then(|e| e.read(cx).path().parent().map(Path::to_path_buf))
        .filter(|d| d.starts_with(&root))
        .unwrap_or(root);
    let handle = cx.entity().downgrade();
    let view = cx.new(|cx| NewFileDialog::new(handle, services, &directory, window, cx));
    let content = view.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title(tr("action-new-file"))
            .w(px(460.))
            .child(content.clone())
    });
    let name = view.read(cx).name.clone();
    name.update(cx, |n, cx| n.focus(window, cx));
    view
}

pub fn open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    show(workspace, window, cx);
}

#[cfg(test)]
mod tests {
    use super::validate_name;

    #[test]
    fn names_are_validated() {
        assert!(validate_name("main.rs").is_ok());
        assert!(validate_name("żółć 🦀.md").is_ok());
        assert_eq!(validate_name(""), Err("dialogs-new-file-error-empty"));
        assert_eq!(validate_name("  "), Err("dialogs-new-file-error-empty"));
        for bad in ["a/b", "a:b", "x?", "..", " lead", "trail."] {
            assert_eq!(
                validate_name(bad),
                Err("dialogs-new-file-error-invalid"),
                "{bad}"
            );
        }
    }
}
