//! Window chrome around the dock area: the title bar with the main toolbar,
//! the tool-window stripes and the status bar.

use ferro_keymap::{Platform, shortcut_for};
use ferro_services::{RunState, ServerHealth};
use ferro_ui::{FerroIcon, components, tr, tr_args};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    Styled as _, Window,
    component::{
        ActiveTheme as _, Disableable as _, Icon, Sizable as _, TitleBar,
        button::{Button, ButtonVariants as _},
        h_flex,
        menu::{DropdownMenu as _, PopupMenuItem},
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};

use crate::{
    actions::*,
    app_state::AppState,
    editor::language_display,
    tool_window::{StripeSlot, ToolWindowKind},
    workspace::Workspace,
};

/// A tooltip with the action's shortcut appended, e.g. "Project (Alt+1)".
fn with_shortcut(label: SharedString, action_id: &str, cx: &gpui_kit::App) -> SharedString {
    let preset = AppState::settings(cx).keymap;
    match shortcut_for(action_id, preset, Platform::current()) {
        Some(keys) => format!("{label}  ({})", display_keystroke(keys)).into(),
        None => label,
    }
}

/// `ctrl-shift-a` → `Ctrl+Shift+A`.
pub fn display_keystroke(keys: &str) -> String {
    fn part(p: &str) -> String {
        match p {
            "ctrl" => "Ctrl".into(),
            "shift" => "Shift".into(),
            "alt" => "Alt".into(),
            "cmd" => "⌘".into(),
            p if p.chars().count() == 1 => p.to_uppercase(),
            p => {
                let mut c = p.chars();
                c.next()
                    .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                    .unwrap_or_default()
            }
        }
    }
    keys.split(' ')
        .map(|stroke| {
            // The key itself may be `-`, as in `ctrl--`.
            let (mods, key) = match stroke.strip_suffix("--") {
                Some(mods) => (mods, "-"),
                None => stroke.rsplit_once('-').unwrap_or(("", stroke)),
            };
            mods.split('-')
                .filter(|m| !m.is_empty())
                .map(part)
                .chain(std::iter::once(part(key)))
                .collect::<Vec<_>>()
                .join("+")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl Workspace {
    pub(crate) fn render_title_bar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let compact = AppState::settings(cx).appearance.compact;
        let native_menu = cfg!(target_os = "macos");
        let project = self.project_name();
        let branch = self
            .status
            .branch
            .as_ref()
            .map(|b| SharedString::from(b.name.clone()))
            .unwrap_or_else(|| tr("vcs-no-branch"));
        let run_name: SharedString = self
            .run_configs
            .get(self.selected_run)
            .map(|c| c.name.clone().into())
            .unwrap_or_else(|| tr("run-no-config"));
        let running = matches!(self.run_state, RunState::Running { .. });

        // Project widget: recent projects plus project actions.
        let recent = self.services.recent.recent();
        let project_widget = components::toolbar_widget("project-widget", None, project)
            .tooltip(tr("toolbar-project"))
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu
                    .menu(tr("action-new-project"), Box::new(NewProject))
                    .menu(tr("action-open-project"), Box::new(OpenProject))
                    .menu(tr("action-get-from-vcs"), Box::new(GetFromVcs))
                    .separator()
                    .label(tr("welcome-recent"));
                for project in recent.iter().take(5) {
                    menu = menu.item(
                        PopupMenuItem::new(SharedString::from(project.name.clone())).on_click(
                            |_, window, cx| window.dispatch_action(Box::new(OpenProject), cx),
                        ),
                    );
                }
                menu.separator()
                    .menu(tr("action-close-project"), Box::new(CloseProject))
            });

        // Branch widget: local and remote branches, checkout on click.
        let branches = self.status.branches.clone();
        let current_branch = self.status.branch.as_ref().map(|b| b.name.clone());
        let this = cx.entity().downgrade();
        let branch_widget =
            components::toolbar_widget("branch-widget", Some(FerroIcon::Branch), branch)
                .tooltip(tr("toolbar-branch"))
                .dropdown_menu(move |mut menu, _, _| {
                    if branches.is_empty() {
                        return menu.label(tr("vcs-loading"));
                    }
                    for (remote, title) in
                        [(false, "vcs-local-branches"), (true, "vcs-remote-branches")]
                    {
                        menu = menu.label(tr(title));
                        for branch in branches.iter().filter(|b| b.is_remote == remote) {
                            let name = branch.name.clone();
                            let mut label = name.clone();
                            if branch.ahead > 0 {
                                label.push_str(&format!("  ↑{}", branch.ahead));
                            }
                            if branch.behind > 0 {
                                label.push_str(&format!("  ↓{}", branch.behind));
                            }
                            let this = this.clone();
                            let item = PopupMenuItem::new(label)
                                .checked(current_branch.as_deref() == Some(name.as_str()))
                                .disabled(remote)
                                .on_click(move |_, _, cx| {
                                    let name = name.clone();
                                    let _ =
                                        this.update(cx, |this, cx| this.checkout_branch(name, cx));
                                });
                            menu = menu.item(item);
                        }
                    }
                    menu
                });

        // Run configuration selector.
        let configs = self.run_configs.clone();
        let selected = self.selected_run;
        let this = cx.entity().downgrade();
        let run_selector = components::toolbar_widget("run-config", None, run_name)
            .tooltip(tr("toolbar-run-config"))
            .dropdown_menu(move |mut menu, _, _| {
                for (ix, config) in configs.iter().enumerate() {
                    let this = this.clone();
                    menu = menu.item(
                        PopupMenuItem::new(SharedString::from(config.name.clone()))
                            .checked(ix == selected)
                            .on_click(move |_, _, cx| {
                                let _ = this.update(cx, |this, cx| {
                                    this.selected_run = ix;
                                    this.run_state =
                                        this.services.run.state(&this.run_configs[ix].id);
                                    cx.notify();
                                });
                            }),
                    );
                }
                menu
            });

        // Settings gear: quick access to appearance options.
        let settings = AppState::settings(cx).clone();
        let settings_menu =
            components::icon_button("settings", FerroIcon::Settings, tr("toolbar-settings"))
                .dropdown_menu_with_anchor(gpui_kit::Anchor::TopRight, move |menu, window, cx| {
                    use ferro_settings::themes;
                    let theme = settings.appearance.theme.clone();
                    let locale = settings.locale;
                    let keymap = settings.keymap;
                    menu.menu(tr("action-open-settings"), Box::new(OpenSettings))
                        .separator()
                        .submenu(tr("menu-appearance"), window, cx, move |menu, _, _| {
                            menu.menu_with_check(
                                tr("action-theme-dark"),
                                theme == themes::FERRO_DARK,
                                Box::new(ThemeFerroDark),
                            )
                            .menu_with_check(
                                tr("action-theme-light"),
                                theme == themes::FERRO_LIGHT,
                                Box::new(ThemeFerroLight),
                            )
                            .menu_with_check(
                                tr("action-theme-high-contrast"),
                                theme == themes::HIGH_CONTRAST,
                                Box::new(ThemeHighContrast),
                            )
                            .separator()
                            .menu(tr("action-zoom-in-ui"), Box::new(ZoomInUi))
                            .menu(tr("action-zoom-out-ui"), Box::new(ZoomOutUi))
                            .menu(tr("action-reset-ui-zoom"), Box::new(ResetUiZoom))
                        })
                        .submenu(tr("menu-language"), window, cx, move |menu, _, _| {
                            menu.menu_with_check(
                                "English",
                                locale == ferro_i18n::Locale::En,
                                Box::new(LanguageEnglish),
                            )
                            .menu_with_check(
                                "Polski",
                                locale == ferro_i18n::Locale::Pl,
                                Box::new(LanguagePolish),
                            )
                        })
                        .submenu(tr("menu-keymap"), window, cx, move |menu, _, _| {
                            menu.menu_with_check(
                                "IntelliJ IDEA",
                                keymap == ferro_keymap::KeymapPreset::IntelliJ,
                                Box::new(KeymapIntelliJ),
                            )
                            .menu_with_check(
                                "VS Code",
                                keymap == ferro_keymap::KeymapPreset::VsCode,
                                Box::new(KeymapVsCode),
                            )
                        })
                        .menu_with_check(
                            tr("action-toggle-compact-mode"),
                            settings.appearance.compact,
                            Box::new(ToggleCompactMode),
                        )
                });

        // Main menu: in-window bar on Windows/Linux, hamburger in compact mode.
        let menu_element: Option<AnyElement> = if native_menu {
            None
        } else if compact {
            let menus = crate::menus::build(cx);
            Some(
                components::icon_button("main-menu", FerroIcon::Menu, tr("toolbar-main-menu"))
                    .dropdown_menu(move |mut popup, window, cx| {
                        for menu in &menus {
                            let items: Vec<_> = menu
                                .items
                                .iter()
                                .filter_map(|item| match item {
                                    gpui_kit::MenuItem::Action { name, action, .. } => {
                                        Some((name.clone(), action.boxed_clone()))
                                    }
                                    _ => None,
                                })
                                .collect();
                            popup = popup.submenu(
                                menu.name.clone(),
                                window,
                                cx,
                                move |mut sub, _, _| {
                                    for (name, action) in &items {
                                        sub = sub.menu(name.clone(), action.boxed_clone());
                                    }
                                    sub
                                },
                            );
                        }
                        popup
                    })
                    .into_any_element(),
            )
        } else {
            Some(self.app_menu_bar.clone().into_any_element())
        };

        let height = if compact { px(32.) } else { px(40.) };
        let _ = window;
        TitleBar::new().h(height).child(
            h_flex()
                .id("main-toolbar")
                .w_full()
                .h_full()
                .gap_1()
                .pr_2()
                .when_some(menu_element, |el, menu| el.child(menu))
                .child(project_widget)
                .child(branch_widget)
                .child(div().flex_1())
                .child(run_selector)
                .child(
                    components::icon_button(
                        "run",
                        FerroIcon::Run,
                        with_shortcut(tr("action-run"), "ferro::Run", cx),
                    )
                    .disabled(running)
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(Run), cx)),
                )
                .child(
                    components::icon_button(
                        "debug",
                        FerroIcon::Debug,
                        with_shortcut(tr("action-debug"), "ferro::Debug", cx),
                    )
                    .disabled(running)
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(Debug), cx)),
                )
                .child(
                    components::icon_button(
                        "stop",
                        FerroIcon::Stop,
                        with_shortcut(tr("action-stop"), "ferro::Stop", cx),
                    )
                    .disabled(!running)
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(Stop), cx)),
                )
                .child(div().w(px(8.)))
                .child(
                    components::icon_button(
                        "search",
                        FerroIcon::Search,
                        with_shortcut(tr("action-find-action"), "ferro::FindAction", cx),
                    )
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(FindAction), cx)),
                )
                .child(settings_menu),
        )
    }

    pub(crate) fn render_stripe(
        &mut self,
        side: StripeSlot,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, border) = (theme.title_bar, theme.title_bar_border);
        let button = |kind: ToolWindowKind, this: &Self, cx: &mut Context<Self>| {
            let active = this.is_tool_window_visible(kind, cx);
            components::stripe_button(
                SharedString::from(format!("stripe-{}", kind.id())),
                kind.icon(),
                with_shortcut(kind.title(), kind.action_id(), cx),
                active,
                cx.listener(move |this, _, window, cx| this.toggle_tool_window(kind, window, cx)),
            )
        };
        let group = |slot: StripeSlot, this: &Self, cx: &mut Context<Self>| {
            v_flex().gap_1().children(
                ToolWindowKind::ALL
                    .into_iter()
                    .filter(|k| k.stripe() == slot)
                    .map(|k| button(k, this, cx))
                    .collect::<Vec<_>>(),
            )
        };
        let content = match side {
            StripeSlot::Right => v_flex().child(group(StripeSlot::Right, self, cx)),
            _ => v_flex()
                .h_full()
                .justify_between()
                .child(group(StripeSlot::LeftTop, self, cx))
                .child(group(StripeSlot::LeftBottom, self, cx)),
        };
        v_flex()
            .id(match side {
                StripeSlot::Right => "stripe-right",
                _ => "stripe-left",
            })
            .w(px(40.))
            .h_full()
            .py_1()
            .items_center()
            .bg(bg)
            .map(|el| match side {
                StripeSlot::Right => el.border_l_1(),
                _ => el.border_r_1(),
            })
            .border_color(border)
            .child(content)
    }

    pub(crate) fn render_status_bar(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, border, muted, success, warning, danger) = (
            theme.status_bar,
            theme.status_bar_border,
            theme.muted_foreground,
            theme.success,
            theme.warning,
            theme.danger,
        );

        let editor = self.active_editor_entity();
        let (cursor, language) = match &editor {
            Some(editor) => {
                let editor = editor.read(cx);
                (
                    editor.cursor(cx).map(|(l, c)| format!("{l}:{c}")),
                    Some(language_display(editor.path())),
                )
            }
            None => (None, None),
        };

        let task = self.status.task.clone().map(|task| {
            h_flex()
                .gap_2()
                .child(tr(&task.title_key))
                .child(components::progress_bar(
                    task.fraction.unwrap_or(0.0),
                    px(100.),
                    cx,
                ))
                .child(format!("{:.0}%", task.fraction.unwrap_or(0.0) * 100.0))
        });

        let servers = self.services.status.language_servers();
        let lsp = h_flex().gap_2().children(servers.into_iter().map(|server| {
            let (color, state) = match server.health {
                ServerHealth::Ready => (success, tr("lsp-ready")),
                ServerHealth::Starting => (warning, tr("lsp-starting")),
                ServerHealth::Crashed => (danger, tr("lsp-crashed")),
            };
            h_flex()
                .gap_1()
                .child(div().size(px(7.)).rounded_full().bg(color))
                .child(format!("{}: {}", server.name, state))
        }));

        let branch = self.status.branch.as_ref().map(|b| b.name.clone());
        let memory = self
            .status
            .memory_mb
            .map(|mb| tr_args("status-memory", &[("mb", mb.into())]));

        h_flex()
            .id("status-bar")
            .h(px(26.))
            .px_2()
            .gap_3()
            .text_xs()
            .text_color(muted)
            .bg(bg)
            .border_t_1()
            .border_color(border)
            .child(
                h_flex()
                    .gap_3()
                    .flex_1()
                    .min_w_0()
                    .when_some(task, |el, task| el.child(task)),
            )
            .child(lsp)
            .when_some(cursor, |el, cursor| {
                el.child(
                    components::status_item("status-cursor", cursor).tooltip(tr("status-cursor")),
                )
            })
            .child(
                components::status_item("status-line-sep", "LF")
                    .tooltip(tr("status-line-separator")),
            )
            .child(
                components::status_item("status-encoding", "UTF-8").tooltip(tr("status-encoding")),
            )
            .child(
                components::status_item("status-indent", tr("status-indent-4"))
                    .tooltip(tr("status-indent")),
            )
            .when_some(language, |el, language| {
                el.child(
                    components::status_item("status-language", language)
                        .tooltip(tr("status-language")),
                )
            })
            .when_some(branch, |el, branch| {
                el.child(
                    components::status_item("status-branch", branch)
                        .icon(Icon::from(FerroIcon::Branch))
                        .tooltip(tr("toolbar-branch")),
                )
            })
            .when_some(memory, |el, memory| {
                el.child(
                    Button::new("status-memory")
                        .ghost()
                        .xsmall()
                        .icon(Icon::from(FerroIcon::Memory))
                        .label(memory)
                        .tooltip(tr("status-memory-tooltip")),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::display_keystroke;

    #[test]
    fn keystrokes_are_displayed_like_intellij() {
        assert_eq!(display_keystroke("ctrl-shift-a"), "Ctrl+Shift+A");
        assert_eq!(display_keystroke("alt-f12"), "Alt+F12");
        assert_eq!(display_keystroke("ctrl--"), "Ctrl+-");
        assert_eq!(display_keystroke("shift-f10"), "Shift+F10");
    }
}
