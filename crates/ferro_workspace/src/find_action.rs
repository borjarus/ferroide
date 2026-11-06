//! Find Action (Ctrl+Shift+A): search every action by its localized title,
//! category or id, see its shortcut, and run it.

use ferro_keymap::{ACTIONS, Platform, shortcut_for};
use ferro_ui::tr;
use gpui_kit::{
    AppContext as _, Context, Focusable as _, IntoElement as _, ParentElement as _, SharedString,
    Styled as _, Window,
    component::{
        ActiveTheme as _, WindowExt as _,
        command::{Command, CommandGroup, CommandItem, CommandState},
        h_flex,
    },
    div, px,
};

use crate::{actions, app_state::AppState, chrome::display_keystroke, workspace::Workspace};

pub fn open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let state = cx.new(|cx| CommandState::new(window, cx));
    let preset = AppState::settings(cx).keymap;
    let workspace_handle = cx.entity().downgrade();
    let workspace_focus = workspace.focus_handle(cx);
    let _ = workspace;

    // Group by category, keeping catalog order.
    let mut categories = Vec::new();
    for action in ACTIONS.iter().filter(|a| a.searchable) {
        if !categories.contains(&action.category) {
            categories.push(action.category);
        }
    }
    let groups: Vec<(SharedString, Vec<&'static ferro_keymap::ActionDescriptor>)> = categories
        .into_iter()
        .map(|category| {
            (
                tr(category.title_key()),
                ACTIONS
                    .iter()
                    .filter(|a| a.searchable && a.category == category)
                    .collect(),
            )
        })
        .collect();

    let state_for_dialog = state.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        let muted = cx.theme().muted_foreground;
        let mut command =
            Command::new(&state_for_dialog).placeholder(tr("find-action-placeholder"));
        for (title, descriptors) in &groups {
            let items = descriptors.iter().map(|descriptor| {
                let label = tr(descriptor.title_key);
                let shortcut = shortcut_for(descriptor.id, preset, Platform::current())
                    .map(display_keystroke)
                    .unwrap_or_default();
                let row_label = label.clone();
                CommandItem::new()
                    .label(label)
                    .keywords([SharedString::from(descriptor.id), title.clone()])
                    .child(move |_, _| {
                        h_flex()
                            .w_full()
                            .gap_2()
                            .child(div().flex_1().truncate().child(row_label.clone()))
                            .child(div().text_xs().text_color(muted).child(shortcut.clone()))
                    })
            });
            command = command.group(CommandGroup::new().label(title.clone()).items(items));
        }

        let groups_for_confirm: Vec<Vec<&'static str>> = groups
            .iter()
            .map(|(_, descriptors)| descriptors.iter().map(|d| d.id).collect())
            .collect();
        let workspace_handle = workspace_handle.clone();
        let workspace_focus = workspace_focus.clone();
        let command = command.on_confirm(move |index, window, cx| {
            let Some(id) = groups_for_confirm
                .get(index.section)
                .and_then(|group| group.get(index.row))
                .copied()
            else {
                return;
            };
            window.close_dialog(cx);
            // Dispatch from the workspace, where the handlers live.
            workspace_focus.focus(window, cx);
            if let Some(action) = actions::build(id, cx) {
                window.dispatch_action(action, cx);
            }
            let _ = workspace_handle.update(cx, |_, cx| cx.notify());
        });

        dialog
            .title(tr("action-find-action"))
            .w(px(560.))
            .child(command.into_any_element())
    });
    state.update(cx, |state, cx| state.focus(window, cx));
}
