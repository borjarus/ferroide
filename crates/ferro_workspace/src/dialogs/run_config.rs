//! Run Configuration editor: list, name, command, arguments, working
//! directory, environment.
//!
//! Edits are made on a draft list; Apply / OK write it through
//! `RunService::save_configuration` / `delete_configuration` and refresh
//! the run selector in the title bar.

use ferro_services::{RunConfiguration, Services};
use ferro_ui::{FerroIcon, tr};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, WeakEntity, Window,
    component::{
        ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, WindowExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputState},
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};

use crate::workspace::Workspace;

/// The single-line fields of a configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunField {
    Name,
    Kind,
    Command,
    Arguments,
    WorkingDirectory,
}

impl RunField {
    const ALL: [RunField; 5] = [
        RunField::Name,
        RunField::Kind,
        RunField::Command,
        RunField::Arguments,
        RunField::WorkingDirectory,
    ];

    fn label_key(self) -> &'static str {
        match self {
            RunField::Name => "dialogs-run-name",
            RunField::Kind => "dialogs-run-kind",
            RunField::Command => "dialogs-run-command",
            RunField::Arguments => "dialogs-run-arguments",
            RunField::WorkingDirectory => "dialogs-run-working-directory",
        }
    }

    fn get(self, c: &RunConfiguration) -> &str {
        match self {
            RunField::Name => &c.name,
            RunField::Kind => &c.kind,
            RunField::Command => &c.command,
            RunField::Arguments => &c.arguments,
            RunField::WorkingDirectory => &c.working_directory,
        }
    }

    fn set(self, c: &mut RunConfiguration, value: String) {
        match self {
            RunField::Name => c.name = value,
            RunField::Kind => c.kind = value,
            RunField::Command => c.command = value,
            RunField::Arguments => c.arguments = value,
            RunField::WorkingDirectory => c.working_directory = value,
        }
    }
}

pub struct RunConfigDialog {
    workspace: WeakEntity<Workspace>,
    services: Services,
    configs: Vec<RunConfiguration>,
    deleted: Vec<String>,
    selected: Option<usize>,
    fields: Vec<(RunField, Entity<InputState>)>,
    /// Environment rows: key and value inputs.
    environment: Vec<(Entity<InputState>, Entity<InputState>)>,
    error: Option<SharedString>,
    focus_handle: FocusHandle,
}

impl RunConfigDialog {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        services: Services,
        selected: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let configs = services.run.configurations();
        let fields = RunField::ALL
            .into_iter()
            .map(|f| (f, cx.new(|cx| InputState::new(window, cx))))
            .collect();
        let mut this = Self {
            workspace,
            services,
            selected: None,
            configs,
            deleted: Vec::new(),
            fields,
            environment: Vec::new(),
            error: None,
            focus_handle: cx.focus_handle(),
        };
        if !this.configs.is_empty() {
            this.select(selected.min(this.configs.len() - 1), window, cx);
        }
        this
    }

    pub fn configs(&self) -> &[RunConfiguration] {
        &self.configs
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// Copy the inputs into the selected draft configuration.
    fn commit_inputs(&mut self, cx: &App) {
        let Some(ix) = self.selected else { return };
        let Some(config) = self.configs.get_mut(ix) else {
            return;
        };
        for (field, input) in &self.fields {
            field.set(config, input.read(cx).value().to_string());
        }
        config.environment = self
            .environment
            .iter()
            .map(|(k, v)| {
                (
                    k.read(cx).value().trim().to_string(),
                    v.read(cx).value().to_string(),
                )
            })
            .filter(|(k, _)| !k.is_empty())
            .collect();
    }

    /// Show configuration `ix` in the form.
    pub fn select(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_inputs(cx);
        let Some(config) = self.configs.get(ix).cloned() else {
            return;
        };
        self.selected = Some(ix);
        for (field, input) in &self.fields {
            let value = field.get(&config).to_string();
            input.update(cx, |i, cx| i.set_value(value, window, cx));
        }
        self.environment = config
            .environment
            .iter()
            .map(|(k, v)| {
                (
                    cx.new(|cx| InputState::new(window, cx).default_value(k.clone())),
                    cx.new(|cx| InputState::new(window, cx).default_value(v.clone())),
                )
            })
            .collect();
        self.error = None;
        cx.notify();
    }

    /// Set a field of the selected configuration (tests, storybook).
    pub fn set_field(
        &mut self,
        field: RunField,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((_, input)) = self.fields.iter().find(|(f, _)| *f == field) {
            input.update(cx, |i, cx| i.set_value(value.to_string(), window, cx));
        }
        self.commit_inputs(cx);
        cx.notify();
    }

    fn unique_id(&self, base: &str) -> String {
        (1..)
            .map(|n| format!("{base}-{n}"))
            .find(|id| !self.configs.iter().any(|c| c.id == *id))
            .unwrap_or_default()
    }

    pub fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_inputs(cx);
        let config = RunConfiguration {
            id: self.unique_id("config"),
            name: tr("dialogs-run-unnamed").to_string(),
            kind: "cargo".into(),
            command: "cargo run".into(),
            ..Default::default()
        };
        self.configs.push(config);
        self.select(self.configs.len() - 1, window, cx);
    }

    pub fn duplicate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_inputs(cx);
        let Some(ix) = self.selected else { return };
        let mut copy = self.configs[ix].clone();
        copy.id = self.unique_id(&copy.id);
        copy.name = format!("{} ({})", copy.name, tr("dialogs-run-copy"));
        self.configs.insert(ix + 1, copy);
        self.selected = None;
        self.select(ix + 1, window, cx);
    }

    pub fn remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.selected else { return };
        let removed = self.configs.remove(ix);
        if self
            .services
            .run
            .configurations()
            .iter()
            .any(|c| c.id == removed.id)
        {
            self.deleted.push(removed.id);
        }
        self.selected = None;
        if self.configs.is_empty() {
            self.environment.clear();
            for (_, input) in &self.fields {
                input.update(cx, |i, cx| i.set_value(String::new(), window, cx));
            }
        } else {
            self.select(ix.min(self.configs.len() - 1), window, cx);
        }
        cx.notify();
    }

    pub fn add_env_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.environment.push((
            cx.new(|cx| InputState::new(window, cx).placeholder("KEY")),
            cx.new(|cx| InputState::new(window, cx).placeholder(tr("dialogs-run-env-value"))),
        ));
        cx.notify();
    }

    /// Validate and write the draft through the run service. Returns
    /// whether it succeeded.
    pub fn apply(&mut self, cx: &mut Context<Self>) -> bool {
        self.commit_inputs(cx);
        if let Some((ix, config)) = self
            .configs
            .iter()
            .enumerate()
            .find(|(_, c)| c.name.trim().is_empty() || c.command.trim().is_empty())
        {
            let key = if config.name.trim().is_empty() {
                "dialogs-run-error-name"
            } else {
                "dialogs-run-error-command"
            };
            self.error = Some(tr(key));
            self.selected = Some(ix);
            cx.notify();
            return false;
        }
        for id in self.deleted.drain(..) {
            self.services.run.delete_configuration(&id);
        }
        for config in &self.configs {
            if let Err(err) = self.services.run.save_configuration(config.clone()) {
                self.error = Some(err.to_string().into());
                cx.notify();
                return false;
            }
        }
        let _ = self
            .workspace
            .update(cx, |ws, cx| ws.reload_run_configs(cx));
        self.error = None;
        cx.notify();
        true
    }
}

impl Focusable for RunConfigDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RunConfigDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, danger, border, side_bg) = (
            theme.muted_foreground,
            theme.danger,
            theme.border,
            theme.muted,
        );
        let has_selection = self.selected.is_some();

        let list = v_flex()
            .id("run-config-list")
            .w(px(200.))
            .h_full()
            .gap_1()
            .p_1()
            .bg(side_bg)
            .border_r_1()
            .border_color(border)
            .overflow_y_scroll()
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("run-add")
                            .ghost()
                            .xsmall()
                            .icon(Icon::from(FerroIcon::Add))
                            .tooltip(tr("dialogs-run-add"))
                            .accessibility_label(tr("dialogs-run-add"))
                            .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
                    )
                    .child(
                        Button::new("run-duplicate")
                            .ghost()
                            .xsmall()
                            .icon(Icon::from(FerroIcon::Copy))
                            .tooltip(tr("dialogs-run-duplicate"))
                            .accessibility_label(tr("dialogs-run-duplicate"))
                            .disabled(!has_selection)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.duplicate(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("run-remove")
                            .ghost()
                            .xsmall()
                            .icon(Icon::from(FerroIcon::Delete))
                            .tooltip(tr("dialogs-run-remove"))
                            .accessibility_label(tr("dialogs-run-remove"))
                            .disabled(!has_selection)
                            .on_click(cx.listener(|this, _, window, cx| this.remove(window, cx))),
                    ),
            )
            .when(self.configs.is_empty(), |el| {
                el.child(
                    div()
                        .p_2()
                        .text_xs()
                        .text_color(muted)
                        .child(tr("dialogs-run-empty")),
                )
            })
            .children(self.configs.iter().enumerate().map(|(ix, config)| {
                Button::new(("run-config", ix))
                    .ghost()
                    .small()
                    .w_full()
                    .justify_start()
                    .icon(Icon::from(FerroIcon::Run))
                    .label(SharedString::from(config.name.clone()))
                    .selected(self.selected == Some(ix))
                    .on_click(cx.listener(move |this, _, window, cx| this.select(ix, window, cx)))
            }));

        let form = v_flex()
            .id("run-config-form")
            .flex_1()
            .p_3()
            .gap_2()
            .overflow_y_scroll()
            .when(has_selection, |el| {
                el.children(self.fields.iter().map(|(field, input)| {
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .w(px(140.))
                                .text_sm()
                                .text_color(muted)
                                .child(tr(field.label_key())),
                        )
                        .child(
                            Input::new(input)
                                .small()
                                .flex_1()
                                .aria_label(tr(field.label_key())),
                        )
                }))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child(tr("dialogs-run-environment")),
                        )
                        .child(
                            Button::new("env-add")
                                .ghost()
                                .xsmall()
                                .icon(Icon::from(FerroIcon::Add))
                                .tooltip(tr("dialogs-run-env-add"))
                                .accessibility_label(tr("dialogs-run-env-add"))
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.add_env_row(window, cx)),
                                ),
                        ),
                )
                .children(self.environment.iter().enumerate().map(|(ix, (k, v))| {
                    h_flex()
                        .gap_1()
                        .child(Input::new(k).small().w(px(160.)).aria_label("KEY"))
                        .child(div().child("="))
                        .child(
                            Input::new(v)
                                .small()
                                .flex_1()
                                .aria_label(tr("dialogs-run-env-value")),
                        )
                        .child(
                            Button::new(("env-remove", ix))
                                .ghost()
                                .xsmall()
                                .icon(Icon::from(FerroIcon::Remove))
                                .accessibility_label(tr("dialogs-run-remove"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.environment.remove(ix);
                                    cx.notify();
                                })),
                        )
                }))
            });

        v_flex()
            .id("run-config-dialog")
            .track_focus(&self.focus_handle)
            .gap_2()
            .child(
                h_flex()
                    .h(px(380.))
                    .border_1()
                    .border_color(border)
                    .rounded(px(4.))
                    .child(list)
                    .child(form),
            )
            .when_some(self.error.clone(), |el, err| {
                el.child(div().text_sm().text_color(danger).child(err))
            })
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("run-cancel")
                            .small()
                            .label(tr("settings-cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("run-apply")
                            .small()
                            .label(tr("settings-apply"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.apply(cx);
                            })),
                    )
                    .child(
                        Button::new("run-ok")
                            .small()
                            .primary()
                            .label(tr("settings-ok"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.apply(cx) {
                                    window.close_dialog(cx);
                                }
                            })),
                    ),
            )
    }
}

pub fn show(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Entity<RunConfigDialog> {
    let services = workspace.services.clone();
    let selected = workspace.selected_run;
    let handle = cx.entity().downgrade();
    let view = cx.new(|cx| RunConfigDialog::new(handle, services, selected, window, cx));
    let content = view.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title(tr("dialogs-run-title"))
            .w(px(760.))
            .child(content.clone())
    });
    view
}

pub fn open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    show(workspace, window, cx);
}
