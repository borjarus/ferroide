//! Window chrome around the dock area, in the Zed layout (plan §8, Stage 1):
//! a slim single-row title bar (application menu, project and branch
//! pickers; run configuration, notifications and settings on the right) and
//! the status bar, which doubles as the panel switcher — toggles for the
//! left dock on its left end, for the bottom and right docks on its right
//! end.

use ferro_services::{RunState, ServerHealth};
use ferro_ui::{FerroIcon, components, tr, tr_args};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    Styled as _, Window,
    component::{
        ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, TitleBar,
        button::{Button, ButtonVariants as _},
        h_flex,
        menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem},
    },
    div,
    prelude::FluentBuilder as _,
    px,
};

use gpui_kit::component::dock::DockPlacement;

use crate::{
    actions::*, app_state::AppState, editor::language_display, tool_window::ToolWindowKind,
    workspace::Workspace,
};

/// A tooltip with the action's shortcut appended, e.g. "Project (Alt+1)".
pub(crate) fn with_shortcut(
    label: SharedString,
    action_id: &str,
    cx: &gpui_kit::App,
) -> SharedString {
    match AppState::shortcut(action_id, cx) {
        Some(keys) => format!("{label}  ({})", display_keystroke(&keys)).into(),
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
                        .submenu(tr("menu-keymap"), window, cx, move |mut menu, _, _| {
                            for preset in ferro_keymap::KeymapPreset::ALL {
                                menu = menu.menu_with_check(
                                    preset.display_name(),
                                    keymap.base == preset,
                                    keymap_action(preset),
                                );
                            }
                            menu.separator()
                                .menu_with_check(
                                    tr("action-toggle-vim-mode"),
                                    keymap.vim_mode,
                                    Box::new(ToggleVimMode),
                                )
                                .menu(
                                    tr("action-open-keymap-settings"),
                                    Box::new(OpenKeymapSettings),
                                )
                        })
                        .menu_with_check(
                            tr("action-toggle-compact-mode"),
                            settings.appearance.compact,
                            Box::new(ToggleCompactMode),
                        )
                });

        // Application menu: native on macOS, a hamburger button in the title
        // bar on Windows and Linux (as in Zed).
        let menu_element: Option<AnyElement> = (!native_menu).then(|| {
            let menus = crate::menus::build(cx);
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
                        popup =
                            popup.submenu(menu.name.clone(), window, cx, move |mut sub, _, _| {
                                for (name, action) in &items {
                                    sub = sub.menu(name.clone(), action.boxed_clone());
                                }
                                sub
                            });
                    }
                    popup
                })
                .into_any_element()
        });

        // Notification bell: toggles the Notifications panel; a dot marks
        // unread ones.
        let unread = self
            .services
            .status
            .notifications()
            .iter()
            .filter(|n| !n.read)
            .count();
        let accent = cx.theme().primary;
        let bell = div()
            .relative()
            .child(
                components::icon_button(
                    "notifications",
                    FerroIcon::Notifications,
                    with_shortcut(
                        ToolWindowKind::Notifications.title(),
                        ToolWindowKind::Notifications.action_id(),
                        cx,
                    ),
                )
                .selected(self.is_tool_window_visible(ToolWindowKind::Notifications, cx))
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(ToggleNotifications), cx)
                }),
            )
            .when(unread > 0, |el| {
                el.child(
                    div()
                        .absolute()
                        .top(px(4.))
                        .right(px(4.))
                        .size(px(6.))
                        .rounded_full()
                        .bg(accent),
                )
            });

        let height = if compact { px(30.) } else { px(34.) };
        let _ = window;
        TitleBar::new().h(height).child(
            h_flex()
                .id("title-bar")
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
                .child(bell)
                .child(settings_menu),
        )
    }

    /// One status-bar toggle: the tool window's icon, highlighted while it
    /// is on screen. Right-click moves it to another dock.
    fn render_panel_toggle(&self, kind: ToolWindowKind, cx: &mut Context<Self>) -> AnyElement {
        let active = self.is_tool_window_visible(kind, cx);
        let current = self.tool_window_placement(kind, cx);
        let this = cx.entity().downgrade();
        div()
            .id(SharedString::from(format!("toggle-{}", kind.id())))
            .child(components::panel_toggle(
                SharedString::from(format!("status-{}", kind.id())),
                kind.icon(),
                with_shortcut(kind.title(), kind.action_id(), cx),
                active,
                cx.listener(move |this, _, window, cx| this.toggle_tool_window(kind, window, cx)),
            ))
            .context_menu(move |menu: PopupMenu, _, _| {
                let mut menu = menu.label(kind.title());
                for (placement, key) in [
                    (DockPlacement::Left, "dock-move-left"),
                    (DockPlacement::Bottom, "dock-move-bottom"),
                    (DockPlacement::Right, "dock-move-right"),
                ] {
                    let this = this.clone();
                    menu = menu.item(
                        PopupMenuItem::new(tr(key))
                            .checked(placement == current)
                            .on_click(move |_, window, cx| {
                                let _ = this.update(cx, |this, cx| {
                                    this.move_tool_window(kind, placement, window, cx)
                                });
                            }),
                    );
                }
                menu
            })
            .into_any_element()
    }

    /// The toggles of every tool window currently in `docks`.
    fn render_panel_toggles(
        &self,
        docks: &[DockPlacement],
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut toggles = Vec::new();
        for dock in docks {
            for kind in ToolWindowKind::ALL {
                if self.tool_window_placement(kind, cx) == *dock {
                    toggles.push(self.render_panel_toggle(kind, cx));
                }
            }
        }
        toggles
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
        let problems = editor.as_ref().map(|e| e.read(cx).problem_counts());
        let indent = tr_args(
            "editor-status-indent",
            &[("size", AppState::settings(cx).editor.tab_size.into())],
        );

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

        let memory = self
            .status
            .memory_mb
            .map(|mb| tr_args("status-memory", &[("mb", mb.into())]));

        let left_toggles = self.render_panel_toggles(&[DockPlacement::Left], cx);
        let right_toggles =
            self.render_panel_toggles(&[DockPlacement::Bottom, DockPlacement::Right], cx);
        let (errors, warnings) = problems.unwrap_or((0, 0));

        h_flex()
            .id("status-bar")
            .h(px(26.))
            .px_1()
            .gap_3()
            .text_xs()
            .text_color(muted)
            .bg(bg)
            .border_t_1()
            .border_color(border)
            // Left: left-dock toggles, diagnostics, background activity.
            .child(h_flex().id("status-left-docks").children(left_toggles))
            .child(
                Button::new("status-problems")
                    .ghost()
                    .xsmall()
                    .icon(Icon::from(FerroIcon::Problems))
                    .label(tr_args(
                        "editor-problem-count",
                        &[("errors", errors.into()), ("warnings", warnings.into())],
                    ))
                    .tooltip(tr("editor-status-problems"))
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleProblems), cx)),
            )
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
            .child(components::status_item("status-indent", indent).tooltip(tr("status-indent")))
            .when_some(language, |el, language| {
                el.child(
                    components::status_item("status-language", language)
                        .tooltip(tr("status-language")),
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
            // Right: bottom- and right-dock toggles.
            .child(h_flex().id("status-right-docks").children(right_toggles))
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
