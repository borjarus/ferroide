//! Default bindings of every keymap scheme, for Windows/Linux. The macOS
//! variant of each scheme is derived from these (see [`MacRule`]).
//!
//! Sources: the published default keymaps of VS Code, IntelliJ IDEA, Zed,
//! Sublime Text and GNU Emacs, restricted to the actions FerroIDE has.

use crate::{Binding, EDITOR_INPUT_CONTEXT};

const fn b(keystrokes: &'static str, action: &'static str) -> Binding {
    Binding {
        keystrokes,
        action,
        context: None,
    }
}

/// A text-editing binding, active only in an editor tab's text.
const fn e(keystrokes: &'static str, action: &'static str) -> Binding {
    Binding {
        keystrokes,
        action,
        context: Some(EDITOR_INPUT_CONTEXT),
    }
}

/// How a scheme's macOS bindings are obtained.
pub(crate) enum MacRule {
    /// `ctrl` becomes `cmd`, except for the listed keystrokes, which stay as
    /// they are; then `overrides` replace the bindings of their actions.
    CtrlToCmd {
        keep: &'static [&'static str],
        overrides: &'static [Binding],
    },
    /// A hand-written table (JetBrains' macOS keymap differs too much).
    Explicit(&'static [Binding]),
    /// Identical on every platform (Emacs).
    Same,
}

// ---------------------------------------------------------------------------
// VS Code — FerroIDE's default (plan, Appendix C)
// ---------------------------------------------------------------------------

pub(crate) const VSCODE: &[Binding] = &[
    b("ctrl-p", "ferro::QuickOpen"),
    b("ctrl-shift-p", "ferro::CommandPalette"),
    b("f1", "ferro::CommandPalette"),
    b("ctrl-.", "ferro::QuickFix"),
    b("ctrl-shift-o", "ferro::GoToSymbolInFile"),
    b("ctrl-t", "ferro::GoToSymbolInWorkspace"),
    b("ctrl-g", "ferro::GoToLine"),
    b("alt-left", "ferro::GoBack"),
    b("alt-right", "ferro::GoForward"),
    b("f2", "ferro::RenameSymbol"),
    b("ctrl-space", "ferro::TriggerCompletion"),
    b("ctrl-shift-space", "ferro::ParameterInfo"),
    b("ctrl-k ctrl-i", "ferro::QuickDocumentation"),
    b("ctrl-shift-f", "ferro::FindInFiles"),
    b("ctrl-shift-h", "ferro::ReplaceInFiles"),
    b("f8", "ferro::NextProblem"),
    b("shift-f8", "ferro::PreviousProblem"),
    b("ctrl-shift-m", "ferro::ToggleProblems"),
    b("ctrl-f5", "ferro::Run"),
    b("f5", "ferro::Debug"),
    b("shift-f5", "ferro::Stop"),
    b("f9", "ferro::ToggleBreakpoint"),
    b("ctrl-shift-g", "ferro::ToggleCommit"),
    b("ctrl-shift-e", "ferro::ToggleProject"),
    b("ctrl-shift-d", "ferro::ToggleDebug"),
    b("ctrl-b", "ferro::ToggleLeftDock"),
    b("ctrl-j", "ferro::ToggleBottomDock"),
    b("ctrl-alt-b", "ferro::ToggleRightDock"),
    b("ctrl-`", "ferro::ToggleTerminal"),
    b("ctrl-shift-`", "ferro::NewTerminal"),
    b("ctrl-\\", "ferro::SplitRight"),
    b("ctrl-w", "ferro::CloseTab"),
    b("ctrl-k ctrl-w", "ferro::CloseAllTabs"),
    b("ctrl-shift-t", "ferro::ReopenClosedTab"),
    b("ctrl-pagedown", "ferro::NextTab"),
    b("ctrl-pageup", "ferro::PreviousTab"),
    b("ctrl-,", "ferro::OpenSettings"),
    b("ctrl-k ctrl-s", "ferro::OpenKeymapSettings"),
    b("ctrl-n", "ferro::NewFile"),
    b("ctrl-shift-n", "ferro::NewWindow"),
    b("ctrl-=", "ferro::ZoomInUi"),
    b("ctrl--", "ferro::ZoomOutUi"),
    b("ctrl-q", "ferro::Quit"),
    b("ctrl-s", "ferro::Save"),
    b("ctrl-k s", "ferro::SaveAll"),
    b("alt-z", "ferro::ToggleSoftWrap"),
    e("shift-alt-down", "ferro::DuplicateLineDown"),
    e("shift-alt-up", "ferro::DuplicateLineUp"),
    e("alt-up", "ferro::MoveLineUp"),
    e("alt-down", "ferro::MoveLineDown"),
    e("ctrl-shift-k", "ferro::DeleteLine"),
    e("ctrl-/", "ferro::ToggleLineComment"),
    e("shift-alt-a", "ferro::ToggleBlockComment"),
    e("ctrl-d", "ferro::SelectNextOccurrence"),
    e("ctrl-shift-l", "ferro::SelectAllOccurrences"),
    e("ctrl-alt-up", "ferro::AddCursorAbove"),
    e("ctrl-alt-down", "ferro::AddCursorBelow"),
];

pub(crate) const VSCODE_MAC: MacRule = MacRule::CtrlToCmd {
    keep: &[
        "ctrl-`",
        "ctrl-shift-`",
        "ctrl-g",
        "ctrl-f5",
        "ctrl-space",
        "ctrl-shift-space",
    ],
    overrides: &[
        b("cmd-alt-right", "ferro::NextTab"),
        b("cmd-alt-left", "ferro::PreviousTab"),
        b("ctrl--", "ferro::GoBack"),
        b("ctrl-shift--", "ferro::GoForward"),
    ],
};

// ---------------------------------------------------------------------------
// JetBrains (IntelliJ IDEA)
// ---------------------------------------------------------------------------

pub(crate) const JETBRAINS: &[Binding] = &[
    // Search; Shift Shift (Search Everywhere) is a modifier double tap the
    // workspace detects itself.
    b("ctrl-shift-a", "ferro::FindAction"),
    b("ctrl-shift-n", "ferro::QuickOpen"),
    b("ctrl-alt-shift-n", "ferro::GoToSymbolInWorkspace"),
    b("ctrl-f12", "ferro::GoToSymbolInFile"),
    b("ctrl-g", "ferro::GoToLine"),
    b("ctrl-alt-left", "ferro::GoBack"),
    b("ctrl-alt-right", "ferro::GoForward"),
    b("ctrl-shift-f", "ferro::FindInFiles"),
    b("ctrl-shift-r", "ferro::ReplaceInFiles"),
    b("alt-f1", "ferro::SelectOpenedFile"),
    b("f2", "ferro::NextProblem"),
    b("shift-f2", "ferro::PreviousProblem"),
    // Code
    b("alt-enter", "ferro::QuickFix"),
    b("ctrl-space", "ferro::TriggerCompletion"),
    b("ctrl-q", "ferro::QuickDocumentation"),
    b("ctrl-p", "ferro::ParameterInfo"),
    b("shift-f6", "ferro::RenameSymbol"),
    b("ctrl-f8", "ferro::ToggleBreakpoint"),
    // Settings
    b("ctrl-alt-s", "ferro::OpenSettings"),
    b("alt-insert", "ferro::NewFile"),
    // Tool windows
    b("alt-1", "ferro::ToggleProject"),
    b("alt-0", "ferro::ToggleCommit"),
    b("alt-7", "ferro::ToggleStructure"),
    b("alt-4", "ferro::ToggleRun"),
    b("alt-5", "ferro::ToggleDebug"),
    b("alt-6", "ferro::ToggleProblems"),
    b("alt-8", "ferro::ToggleServices"),
    b("alt-9", "ferro::ToggleGit"),
    b("alt-f12", "ferro::ToggleTerminal"),
    b("ctrl-shift-f12", "ferro::HideAllToolWindows"),
    b("ctrl-shift-'", "ferro::MaximizeToolWindow"),
    // Editor tabs and splits
    b("ctrl-f4", "ferro::CloseTab"),
    b("alt-right", "ferro::NextTab"),
    b("alt-left", "ferro::PreviousTab"),
    // Run
    b("shift-f10", "ferro::Run"),
    b("shift-f9", "ferro::Debug"),
    b("ctrl-f2", "ferro::Stop"),
    b("ctrl-d", "ferro::ShowDiff"),
    // View
    b("ctrl-=", "ferro::ZoomInUi"),
    b("ctrl--", "ferro::ZoomOutUi"),
    b("ctrl-alt-0", "ferro::ResetUiZoom"),
    // Files and text editing
    b("ctrl-s", "ferro::SaveAll"),
    b("ctrl-alt-y", "ferro::ReloadFromDisk"),
    e("ctrl-d", "ferro::DuplicateLineDown"),
    e("alt-shift-up", "ferro::MoveLineUp"),
    e("alt-shift-down", "ferro::MoveLineDown"),
    e("ctrl-y", "ferro::DeleteLine"),
    e("ctrl-/", "ferro::ToggleLineComment"),
    e("ctrl-shift-/", "ferro::ToggleBlockComment"),
    e("alt-j", "ferro::SelectNextOccurrence"),
    e("ctrl-alt-shift-j", "ferro::SelectAllOccurrences"),
];

pub(crate) const JETBRAINS_MAC: MacRule = MacRule::Explicit(&[
    b("cmd-shift-a", "ferro::FindAction"),
    b("cmd-shift-o", "ferro::QuickOpen"),
    b("cmd-alt-o", "ferro::GoToSymbolInWorkspace"),
    b("cmd-f12", "ferro::GoToSymbolInFile"),
    b("cmd-l", "ferro::GoToLine"),
    b("cmd-[", "ferro::GoBack"),
    b("cmd-]", "ferro::GoForward"),
    b("cmd-shift-f", "ferro::FindInFiles"),
    b("cmd-shift-r", "ferro::ReplaceInFiles"),
    b("alt-f1", "ferro::SelectOpenedFile"),
    b("f2", "ferro::NextProblem"),
    b("shift-f2", "ferro::PreviousProblem"),
    b("alt-enter", "ferro::QuickFix"),
    b("ctrl-space", "ferro::TriggerCompletion"),
    b("f1", "ferro::QuickDocumentation"),
    b("cmd-p", "ferro::ParameterInfo"),
    b("shift-f6", "ferro::RenameSymbol"),
    b("cmd-f8", "ferro::ToggleBreakpoint"),
    b("cmd-,", "ferro::OpenSettings"),
    b("cmd-n", "ferro::NewFile"),
    b("cmd-1", "ferro::ToggleProject"),
    b("cmd-0", "ferro::ToggleCommit"),
    b("cmd-7", "ferro::ToggleStructure"),
    b("cmd-4", "ferro::ToggleRun"),
    b("cmd-5", "ferro::ToggleDebug"),
    b("cmd-6", "ferro::ToggleProblems"),
    b("cmd-8", "ferro::ToggleServices"),
    b("cmd-9", "ferro::ToggleGit"),
    b("alt-f12", "ferro::ToggleTerminal"),
    b("cmd-shift-f12", "ferro::HideAllToolWindows"),
    b("cmd-shift-'", "ferro::MaximizeToolWindow"),
    b("cmd-w", "ferro::CloseTab"),
    b("ctrl-right", "ferro::NextTab"),
    b("ctrl-left", "ferro::PreviousTab"),
    b("ctrl-r", "ferro::Run"),
    b("ctrl-d", "ferro::Debug"),
    b("cmd-f2", "ferro::Stop"),
    b("cmd-d", "ferro::ShowDiff"),
    b("cmd-=", "ferro::ZoomInUi"),
    b("cmd--", "ferro::ZoomOutUi"),
    b("cmd-alt-0", "ferro::ResetUiZoom"),
    b("cmd-q", "ferro::Quit"),
    b("cmd-s", "ferro::SaveAll"),
    b("cmd-alt-y", "ferro::ReloadFromDisk"),
    e("cmd-d", "ferro::DuplicateLineDown"),
    e("alt-shift-up", "ferro::MoveLineUp"),
    e("alt-shift-down", "ferro::MoveLineDown"),
    e("cmd-backspace", "ferro::DeleteLine"),
    e("cmd-/", "ferro::ToggleLineComment"),
    e("cmd-alt-/", "ferro::ToggleBlockComment"),
    e("ctrl-g", "ferro::SelectNextOccurrence"),
    e("ctrl-cmd-g", "ferro::SelectAllOccurrences"),
]);

// ---------------------------------------------------------------------------
// Zed
// ---------------------------------------------------------------------------

pub(crate) const ZED: &[Binding] = &[
    b("ctrl-p", "ferro::QuickOpen"),
    b("ctrl-shift-p", "ferro::CommandPalette"),
    b("ctrl-shift-o", "ferro::GoToSymbolInFile"),
    b("ctrl-t", "ferro::GoToSymbolInWorkspace"),
    b("ctrl-g", "ferro::GoToLine"),
    b("alt-left", "ferro::GoBack"),
    b("alt-right", "ferro::GoForward"),
    b("ctrl-shift-f", "ferro::FindInFiles"),
    b("ctrl-shift-h", "ferro::ReplaceInFiles"),
    b("ctrl-.", "ferro::QuickFix"),
    b("ctrl-space", "ferro::TriggerCompletion"),
    b("ctrl-k ctrl-i", "ferro::QuickDocumentation"),
    b("ctrl-i", "ferro::ParameterInfo"),
    b("f2", "ferro::RenameSymbol"),
    b("f8", "ferro::NextProblem"),
    b("shift-f8", "ferro::PreviousProblem"),
    b("f9", "ferro::ToggleBreakpoint"),
    b("ctrl-shift-m", "ferro::ToggleProblems"),
    b("ctrl-shift-e", "ferro::ToggleProject"),
    b("ctrl-shift-g", "ferro::ToggleGit"),
    b("ctrl-shift-b", "ferro::ToggleStructure"),
    b("ctrl-`", "ferro::ToggleTerminal"),
    b("ctrl-~", "ferro::NewTerminal"),
    b("ctrl-b", "ferro::ToggleLeftDock"),
    b("ctrl-j", "ferro::ToggleBottomDock"),
    b("ctrl-alt-b", "ferro::ToggleRightDock"),
    b("ctrl-shift-escape", "ferro::MaximizeToolWindow"),
    b("ctrl-w", "ferro::CloseTab"),
    b("ctrl-k w", "ferro::CloseAllTabs"),
    b("ctrl-shift-t", "ferro::ReopenClosedTab"),
    b("ctrl-pagedown", "ferro::NextTab"),
    b("ctrl-pageup", "ferro::PreviousTab"),
    b("ctrl-k right", "ferro::SplitRight"),
    b("ctrl-k down", "ferro::SplitDown"),
    b("ctrl-,", "ferro::OpenSettings"),
    b("ctrl-k ctrl-s", "ferro::OpenKeymapSettings"),
    b("ctrl-n", "ferro::NewFile"),
    b("ctrl-shift-n", "ferro::NewWindow"),
    b("ctrl-=", "ferro::ZoomInUi"),
    b("ctrl--", "ferro::ZoomOutUi"),
    b("ctrl-0", "ferro::ResetUiZoom"),
    b("ctrl-q", "ferro::Quit"),
    b("ctrl-s", "ferro::Save"),
    b("ctrl-alt-s", "ferro::SaveAll"),
    e("alt-shift-down", "ferro::DuplicateLineDown"),
    e("alt-shift-up", "ferro::DuplicateLineUp"),
    e("alt-up", "ferro::MoveLineUp"),
    e("alt-down", "ferro::MoveLineDown"),
    e("ctrl-shift-k", "ferro::DeleteLine"),
    e("ctrl-/", "ferro::ToggleLineComment"),
    e("ctrl-d", "ferro::SelectNextOccurrence"),
    e("ctrl-shift-l", "ferro::SelectAllOccurrences"),
    e("ctrl-alt-up", "ferro::AddCursorAbove"),
    e("ctrl-alt-down", "ferro::AddCursorBelow"),
];

pub(crate) const ZED_MAC: MacRule = MacRule::CtrlToCmd {
    keep: &["ctrl-`", "ctrl-~", "ctrl-g", "ctrl-space"],
    overrides: &[
        b("cmd-alt-right", "ferro::NextTab"),
        b("cmd-alt-left", "ferro::PreviousTab"),
        b("ctrl--", "ferro::GoBack"),
        b("ctrl-shift--", "ferro::GoForward"),
    ],
};

// ---------------------------------------------------------------------------
// Sublime Text
// ---------------------------------------------------------------------------

pub(crate) const SUBLIME: &[Binding] = &[
    b("ctrl-p", "ferro::QuickOpen"),
    b("ctrl-shift-p", "ferro::CommandPalette"),
    b("ctrl-r", "ferro::GoToSymbolInFile"),
    b("ctrl-shift-r", "ferro::GoToSymbolInWorkspace"),
    b("ctrl-g", "ferro::GoToLine"),
    b("alt--", "ferro::GoBack"),
    b("alt-shift--", "ferro::GoForward"),
    b("ctrl-shift-f", "ferro::FindInFiles"),
    b("ctrl-space", "ferro::TriggerCompletion"),
    b("f4", "ferro::NextProblem"),
    b("shift-f4", "ferro::PreviousProblem"),
    b("ctrl-k ctrl-b", "ferro::ToggleLeftDock"),
    b("ctrl-`", "ferro::ToggleTerminal"),
    b("alt-shift-2", "ferro::SplitRight"),
    b("alt-shift-8", "ferro::SplitDown"),
    b("ctrl-w", "ferro::CloseTab"),
    b("ctrl-shift-w", "ferro::CloseAllTabs"),
    b("ctrl-shift-t", "ferro::ReopenClosedTab"),
    b("ctrl-pagedown", "ferro::NextTab"),
    b("ctrl-pageup", "ferro::PreviousTab"),
    b("ctrl-b", "ferro::Run"),
    b("ctrl-break", "ferro::Stop"),
    b("ctrl-n", "ferro::NewFile"),
    b("ctrl-shift-n", "ferro::NewWindow"),
    b("ctrl-=", "ferro::ZoomInUi"),
    b("ctrl--", "ferro::ZoomOutUi"),
    b("ctrl-q", "ferro::Quit"),
    b("ctrl-s", "ferro::Save"),
    e("ctrl-shift-d", "ferro::DuplicateLineDown"),
    e("ctrl-shift-up", "ferro::MoveLineUp"),
    e("ctrl-shift-down", "ferro::MoveLineDown"),
    e("ctrl-shift-k", "ferro::DeleteLine"),
    e("ctrl-/", "ferro::ToggleLineComment"),
    e("ctrl-shift-/", "ferro::ToggleBlockComment"),
    e("ctrl-d", "ferro::SelectNextOccurrence"),
    e("alt-f3", "ferro::SelectAllOccurrences"),
    e("ctrl-alt-up", "ferro::AddCursorAbove"),
    e("ctrl-alt-down", "ferro::AddCursorBelow"),
];

pub(crate) const SUBLIME_MAC: MacRule = MacRule::CtrlToCmd {
    keep: &["ctrl-`", "ctrl-g", "ctrl-space"],
    overrides: &[
        // Sublime Text's macOS keymap differs here; plain ctrl to cmd would
        // take the system's select-to-start and select-to-end keys.
        e("ctrl-cmd-up", "ferro::MoveLineUp"),
        e("ctrl-cmd-down", "ferro::MoveLineDown"),
        e("ctrl-shift-up", "ferro::AddCursorAbove"),
        e("ctrl-shift-down", "ferro::AddCursorBelow"),
        e("ctrl-cmd-g", "ferro::SelectAllOccurrences"),
        b("cmd-alt-right", "ferro::NextTab"),
        b("cmd-alt-left", "ferro::PreviousTab"),
        b("cmd-alt-2", "ferro::SplitRight"),
        b("cmd-alt-shift-2", "ferro::SplitDown"),
        b("ctrl--", "ferro::GoBack"),
        b("ctrl-shift--", "ferro::GoForward"),
    ],
};

// ---------------------------------------------------------------------------
// Emacs
// ---------------------------------------------------------------------------

pub(crate) const EMACS: &[Binding] = &[
    b("alt-x", "ferro::CommandPalette"),
    b("ctrl-x ctrl-f", "ferro::QuickOpen"),
    b("ctrl-x b", "ferro::QuickOpen"),
    b("alt-g g", "ferro::GoToLine"),
    b("alt-g n", "ferro::NextProblem"),
    b("alt-g p", "ferro::PreviousProblem"),
    b("ctrl-x ctrl-i", "ferro::GoToSymbolInFile"),
    b("alt-/", "ferro::TriggerCompletion"),
    b("ctrl-x k", "ferro::CloseTab"),
    b("ctrl-x 2", "ferro::SplitDown"),
    b("ctrl-x 3", "ferro::SplitRight"),
    b("ctrl-x o", "ferro::NextTab"),
    b("ctrl-x d", "ferro::ToggleProject"),
    b("ctrl-x g", "ferro::ToggleCommit"),
    b("ctrl-x ctrl-d", "ferro::ToggleTerminal"),
    b("ctrl-c ctrl-c", "ferro::Run"),
    b("ctrl-c ctrl-d", "ferro::Debug"),
    b("ctrl-c ctrl-k", "ferro::Stop"),
    b("ctrl-c ctrl-b", "ferro::ToggleBreakpoint"),
    b("ctrl-x ctrl-c", "ferro::Quit"),
    b("ctrl-x ctrl-s", "ferro::Save"),
    b("ctrl-x s", "ferro::SaveAll"),
    e("alt-;", "ferro::ToggleLineComment"),
];

pub(crate) const EMACS_MAC: MacRule = MacRule::Same;
