//! GPUI action types for every entry in the `ferro_keymap` catalog, and the
//! code that installs a resolved keymap.

use std::rc::Rc;

use ferro_keymap::{ACTIONS, ResolvedBinding};
use gpui_kit::{Action, App, KeyBinding, KeyBindingContextPredicate, NoAction};

gpui_kit::actions!(
    ferro,
    [
        NewFile,
        NewProject,
        OpenProject,
        GetFromVcs,
        CloseProject,
        NewWindow,
        Save,
        SaveAll,
        ReloadFromDisk,
        OpenSettings,
        OpenSettingsFile,
        OpenKeymapSettings,
        Quit,
        FindAction,
        SearchEverywhere,
        HideAllToolWindows,
        MaximizeToolWindow,
        ToggleLeftDock,
        ToggleBottomDock,
        ToggleRightDock,
        ResetLayout,
        ToggleCompactMode,
        ZoomInUi,
        ZoomOutUi,
        ResetUiZoom,
        QuickOpen,
        CommandPalette,
        GoToSymbolInFile,
        GoToSymbolInWorkspace,
        GoToLine,
        GoBack,
        GoForward,
        FindInFiles,
        ReplaceInFiles,
        SelectOpenedFile,
        NextProblem,
        PreviousProblem,
        TriggerCompletion,
        QuickFix,
        QuickDocumentation,
        ParameterInfo,
        RenameSymbol,
        ToggleBreakpoint,
        ToggleProject,
        ToggleCommit,
        ToggleStructure,
        ToggleRun,
        ToggleDebug,
        ToggleProblems,
        ToggleGit,
        ToggleTerminal,
        NewTerminal,
        ToggleTodo,
        ToggleNotifications,
        ToggleDatabase,
        ToggleBuild,
        ToggleServices,
        CloseTab,
        CloseOtherTabs,
        CloseAllTabs,
        ReopenClosedTab,
        PinTab,
        NextTab,
        PreviousTab,
        SplitRight,
        SplitDown,
        DuplicateLineDown,
        DuplicateLineUp,
        MoveLineUp,
        MoveLineDown,
        DeleteLine,
        ToggleLineComment,
        ToggleBlockComment,
        SelectNextOccurrence,
        SelectAllOccurrences,
        AddCursorAbove,
        AddCursorBelow,
        ToggleSoftWrap,
        ToggleWhitespace,
        Run,
        Debug,
        Stop,
        EditRunConfigurations,
        ShowDiff,
        ResolveConflicts,
        ThemeFerroDark,
        ThemeFerroLight,
        ThemeHighContrast,
        LanguageEnglish,
        LanguagePolish,
        KeymapVsCode,
        KeymapJetBrains,
        KeymapZed,
        KeymapSublime,
        KeymapEmacs,
        ToggleVimMode,
        ShowWelcome,
        About,
    ]
);

/// The action that selects a keymap scheme.
pub fn keymap_action(preset: ferro_keymap::KeymapPreset) -> Box<dyn Action> {
    use ferro_keymap::KeymapPreset::*;
    match preset {
        VsCode => Box::new(KeymapVsCode),
        JetBrains => Box::new(KeymapJetBrains),
        Zed => Box::new(KeymapZed),
        Sublime => Box::new(KeymapSublime),
        Emacs => Box::new(KeymapEmacs),
    }
}

/// Build the GPUI action for a catalog id.
pub fn build(id: &str, cx: &App) -> Option<Box<dyn Action>> {
    cx.build_action(id, None).ok()
}

/// Install a resolved keymap, disabling the keystrokes of `previous` the
/// new one no longer binds, so switching schemes or editing a binding takes
/// effect without a restart.
///
/// GPUI has no "remove binding" call, and clearing every binding would also
/// drop GPUI Kit's own component bindings. Binding a stale keystroke to
/// [`NoAction`] disables it instead — later bindings win.
pub fn install_keymap(new: &[ResolvedBinding], previous: Option<&[ResolvedBinding]>, cx: &mut App) {
    let mut bindings = Vec::new();

    if let Some(previous) = previous {
        for old in previous {
            let still_bound = new
                .iter()
                .any(|b| b.keystrokes == old.keystrokes && b.context == old.context);
            if !still_bound
                && let Some(binding) = load(
                    &old.keystrokes,
                    Box::new(NoAction),
                    old.context.as_deref(),
                    cx,
                )
            {
                bindings.push(binding);
            }
        }
    }

    for binding in new {
        match build(&binding.action, cx) {
            Some(action) => {
                if let Some(kb) = load(&binding.keystrokes, action, binding.context.as_deref(), cx)
                {
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
