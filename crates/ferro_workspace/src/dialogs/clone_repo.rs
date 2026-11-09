//! Clone Repository: URL, directory, progress, then open the project.

use std::path::{Path, PathBuf};

use ferro_services::Services;
use ferro_ui::{FerroIcon, tr};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window,
    component::{
        ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputEvent, InputState},
        spinner::Spinner,
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};

use crate::workspace::Workspace;

/// Check a repository URL; returns the message id of the problem.
pub fn validate_url(url: &str) -> Result<(), &'static str> {
    let url = url.trim();
    if url.is_empty() {
        return Err("dialogs-clone-error-url-empty");
    }
    let known_scheme = ["https://", "http://", "ssh://", "git://", "file://"]
        .iter()
        .any(|s| url.starts_with(s) && url.len() > s.len());
    let scp_like = url.starts_with("git@") && url.contains(':');
    if (known_scheme || scp_like || url.ends_with(".git")) && !url.contains(char::is_whitespace) {
        Ok(())
    } else {
        Err("dialogs-clone-error-url-invalid")
    }
}

/// The directory name a clone of `url` gets, e.g. `ferroide` for
/// `https://github.com/ferroide/ferroide.git`.
pub fn repository_name(url: &str) -> String {
    url.trim()
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .unwrap_or_default()
        .trim_end_matches(".git")
        .to_string()
}

enum CloneState {
    Idle,
    Cloning(#[allow(dead_code)] Task<()>),
}

pub struct CloneDialog {
    services: Services,
    url: Entity<InputState>,
    directory: Entity<InputState>,
    state: CloneState,
    error: Option<SharedString>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl CloneDialog {
    pub fn new(
        services: Services,
        parent_dir: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let url = cx.new(|cx| {
            InputState::new(window, cx).placeholder("https://github.com/user/repository.git")
        });
        let directory = cx.new(|cx| {
            InputState::new(window, cx).default_value(parent_dir.to_string_lossy().into_owned())
        });
        let subscriptions = vec![
            cx.subscribe_in(&url, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.start(window, cx);
                } else {
                    this.error = None;
                    cx.notify();
                }
            }),
            cx.subscribe(&directory, |this, _, _: &InputEvent, cx| {
                this.error = None;
                cx.notify();
            }),
        ];
        Self {
            services,
            url,
            directory,
            state: CloneState::Idle,
            error: None,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    pub fn is_cloning(&self) -> bool {
        matches!(self.state, CloneState::Cloning(_))
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    pub fn set_url(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.url
            .update(cx, |i, cx| i.set_value(url.to_string(), window, cx));
        cx.notify();
    }

    /// Validate and start cloning.
    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_cloning() {
            return;
        }
        let url = self.url.read(cx).value().trim().to_string();
        let directory = self.directory.read(cx).value().trim().to_string();
        if let Err(key) = validate_url(&url) {
            self.error = Some(tr(key));
            cx.notify();
            return;
        }
        if directory.is_empty() {
            self.error = Some(tr("dialogs-clone-error-directory"));
            cx.notify();
            return;
        }
        self.error = None;
        let task = cx.background_spawn(
            self.services
                .vcs
                .clone_repository(&url, &PathBuf::from(directory)),
        );
        self.state = CloneState::Cloning(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.state = CloneState::Idle;
                match result {
                    Ok(root) => {
                        tracing::info!("cloned into {}", root.display());
                        window.close_dialog(cx);
                        if let Err(err) = crate::open_workspace(crate::mock_services(), cx) {
                            tracing::error!("{err:#}");
                        }
                    }
                    Err(err) => this.error = Some(err.to_string().into()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Stop a running clone (drops the task).
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        self.state = CloneState::Idle;
        cx.notify();
    }
}

impl Focusable for CloneDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for CloneDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        let cloning = self.is_cloning();
        let name = repository_name(&self.url.read(cx).value());
        v_flex()
            .id("clone-dialog")
            .track_focus(&self.focus_handle)
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(tr("dialogs-clone-url")),
                    )
                    .child(
                        Input::new(&self.url)
                            .disabled(cloning)
                            .aria_label(tr("dialogs-clone-url")),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(tr("dialogs-clone-directory")),
                    )
                    .child(
                        Input::new(&self.directory)
                            .small()
                            .disabled(cloning)
                            .aria_label(tr("dialogs-clone-directory")),
                    )
                    .when(!name.is_empty(), |el| {
                        let target =
                            Path::new(self.directory.read(cx).value().as_ref()).join(&name);
                        el.child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(target.to_string_lossy().into_owned()),
                        )
                    }),
            )
            .when(cloning, |el| {
                el.child(
                    h_flex()
                        .gap_2()
                        .text_sm()
                        .child(Spinner::new().small())
                        .child(tr("dialogs-clone-progress")),
                )
            })
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
                    .child(if cloning {
                        Button::new("clone-stop")
                            .small()
                            .label(tr("dialogs-clone-stop"))
                            .on_click(cx.listener(|this, _, _, cx| this.cancel(cx)))
                    } else {
                        Button::new("clone-cancel")
                            .small()
                            .label(tr("settings-cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx))
                    })
                    .child(
                        Button::new("clone-start")
                            .small()
                            .primary()
                            .icon(Icon::from(FerroIcon::GetFromVcs))
                            .label(tr("dialogs-clone-button"))
                            .disabled(cloning)
                            .on_click(cx.listener(|this, _, window, cx| this.start(window, cx))),
                    ),
            )
    }
}

pub fn show(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Entity<CloneDialog> {
    let services = workspace.services.clone();
    let parent = services
        .project
        .roots()
        .first()
        .and_then(|r| r.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    let view = cx.new(|cx| CloneDialog::new(services, &parent, window, cx));
    let content = view.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title(tr("dialogs-clone-title"))
            .w(px(520.))
            .child(content.clone())
    });
    let url = view.read(cx).url.clone();
    url.update(cx, |u, cx| u.focus(window, cx));
    view
}

pub fn open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    show(workspace, window, cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_validated() {
        for ok in [
            "https://github.com/ferroide/ferroide.git",
            "https://gitlab.com/a/b",
            "git@github.com:ferroide/ferroide.git",
            "ssh://git@host/repo",
            "/srv/repos/project.git",
        ] {
            assert_eq!(validate_url(ok), Ok(()), "{ok}");
        }
        assert_eq!(validate_url(" "), Err("dialogs-clone-error-url-empty"));
        for bad in ["not a url", "https://", "ftp:/x", "github.com/a/b"] {
            assert_eq!(
                validate_url(bad),
                Err("dialogs-clone-error-url-invalid"),
                "{bad}"
            );
        }
    }

    #[test]
    fn repository_names_are_derived() {
        assert_eq!(
            repository_name("https://github.com/ferroide/ferroide.git"),
            "ferroide"
        );
        assert_eq!(
            repository_name("git@github.com:me/dotfiles.git"),
            "dotfiles"
        );
        assert_eq!(repository_name("https://gitlab.com/a/b/"), "b");
    }
}
