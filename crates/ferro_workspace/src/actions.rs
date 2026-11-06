//! GPUI action types for every entry in the `ferro_keymap` catalog, and the
//! code that installs a keymap preset.

use std::rc::Rc;

use ferro_keymap::{ACTIONS, KeymapPreset, Platform};
use gpui_kit::{Action, App, KeyBinding, KeyBindingContextPredicate, NoAction};

gpui_kit::actions!(
    ferro,
    [
        NewProject,
        OpenProject,
        GetFromVcs,
        CloseProject,
        NewWindow,
        OpenSettings,
        Quit,
        FindAction,
        SearchEverywhere,
        HideAllToolWindows,
        MaximizeToolWindow,
        ResetLayout,
        ToggleCompactMode,
        ZoomInUi,
        ZoomOutUi,
        ResetUiZoom,
        ToggleProject,
        ToggleCommit,
        ToggleStructure,
        ToggleRun,
        ToggleDebug,
        ToggleProblems,
        ToggleGit,
        ToggleTerminal,
        ToggleTodo,
        ToggleNotifications,
        ToggleDatabase,
        ToggleBuild,
        CloseTab,
        CloseOtherTabs,
        CloseAllTabs,
        PinTab,
        NextTab,
        PreviousTab,
        SplitRight,
        SplitDown,
        Run,
        Debug,
        Stop,
        ThemeFerroDark,
        ThemeFerroLight,
        ThemeHighContrast,
        LanguageEnglish,
        LanguagePolish,
        KeymapIntelliJ,
        KeymapVsCode,
        ShowWelcome,
        About,
    ]
);

/// Build the GPUI action for a catalog id.
pub fn build(id: &str, cx: &App) -> Option<Box<dyn Action>> {
    cx.build_action(id, None).ok()
}

/// Install `preset`, overriding any keystrokes `previous` bound that the new
/// preset does not, so switching keymaps takes effect without a restart.
///
/// GPUI has no "remove binding" call, and clearing every binding would also
/// drop GPUI Kit's own component bindings. Binding a stale keystroke to
/// [`NoAction`] disables it instead — later bindings win.
pub fn install_keymap(preset: KeymapPreset, previous: Option<KeymapPreset>, cx: &mut App) {
    let platform = Platform::current();
    let new = ferro_keymap::bindings(preset, platform);
    let mut bindings = Vec::new();

    if let Some(previous) = previous {
        for old in ferro_keymap::bindings(previous, platform) {
            let still_bound = new
                .iter()
                .any(|b| b.keystrokes == old.keystrokes && b.context == old.context);
            if !still_bound
                && let Some(binding) = load(old.keystrokes, Box::new(NoAction), old.context, cx)
            {
                bindings.push(binding);
            }
        }
    }

    for binding in &new {
        match build(binding.action, cx) {
            Some(action) => {
                if let Some(kb) = load(binding.keystrokes, action, binding.context, cx) {
                    bindings.push(kb);
                }
            }
            None => tracing::error!("keymap names unknown action {}", binding.action),
        }
    }
    cx.bind_keys(bindings);
}

fn load(
    keystrokes: &str,
    action: Box<dyn Action>,
    context: Option<&str>,
    cx: &App,
) -> Option<KeyBinding> {
    let predicate = match context.map(KeyBindingContextPredicate::parse) {
        Some(Ok(predicate)) => Some(Rc::new(predicate)),
        Some(Err(err)) => {
            tracing::error!("invalid key context {context:?}: {err}");
            return None;
        }
        None => None,
    };
    KeyBinding::load(keystrokes, action, predicate, false, None, cx)
        .inspect_err(|err| tracing::error!("invalid keystroke `{keystrokes}`: {err}"))
        .ok()
}

/// Whether every catalog id resolves to a registered GPUI action.
pub fn missing_actions(cx: &App) -> Vec<&'static str> {
    ACTIONS
        .iter()
        .filter(|a| build(a.id, cx).is_none())
        .map(|a| a.id)
        .collect()
}
