//! Every action FerroIDE exposes, with the message id of its title.

/// Grouping shown in the Command Palette and the keymap settings, and used
/// to build menus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionCategory {
    File,
    View,
    Navigate,
    Code,
    ToolWindows,
    Editor,
    Run,
    Vcs,
    Appearance,
    Help,
}

impl ActionCategory {
    pub const ALL: [ActionCategory; 10] = [
        ActionCategory::File,
        ActionCategory::View,
        ActionCategory::Navigate,
        ActionCategory::Code,
        ActionCategory::ToolWindows,
        ActionCategory::Editor,
        ActionCategory::Run,
        ActionCategory::Vcs,
        ActionCategory::Appearance,
        ActionCategory::Help,
    ];

    /// Message id of the category's localized name.
    pub fn title_key(self) -> &'static str {
        match self {
            ActionCategory::File => "category-file",
            ActionCategory::View => "category-view",
            ActionCategory::Navigate => "category-navigate",
            ActionCategory::Code => "category-code",
            ActionCategory::ToolWindows => "category-tool-windows",
            ActionCategory::Editor => "category-editor",
            ActionCategory::Run => "category-run",
            ActionCategory::Vcs => "category-vcs",
            ActionCategory::Appearance => "category-appearance",
            ActionCategory::Help => "category-help",
        }
    }
}

/// Static description of one action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionDescriptor {
    /// GPUI action name, `namespace::Name`.
    pub id: &'static str,
    /// Fluent message id of the action's title.
    pub title_key: &'static str,
    pub category: ActionCategory,
    /// Whether the Command Palette lists it. Internal actions stay hidden.
    pub searchable: bool,
}

const fn a(
    id: &'static str,
    title_key: &'static str,
    category: ActionCategory,
) -> ActionDescriptor {
    ActionDescriptor {
        id,
        title_key,
        category,
        searchable: true,
    }
}

/// Look an action up by id.
pub fn action(id: &str) -> Option<&'static ActionDescriptor> {
    ACTIONS.iter().find(|a| a.id == id)
}

use ActionCategory::*;

pub const ACTIONS: &[ActionDescriptor] = &[
    // File
    a("ferro::NewFile", "action-new-file", File),
    a("ferro::NewProject", "action-new-project", File),
    a("ferro::OpenProject", "action-open-project", File),
    a("ferro::GetFromVcs", "action-get-from-vcs", File),
    a("ferro::CloseProject", "action-close-project", File),
    a("ferro::NewWindow", "action-new-window", File),
    a("ferro::OpenSettings", "action-open-settings", File),
    a("ferro::OpenSettingsFile", "action-open-settings-file", File),
    a(
        "ferro::OpenKeymapSettings",
        "action-open-keymap-settings",
        File,
    ),
    a("ferro::Quit", "action-quit", File),
    // View
    a("ferro::FindAction", "action-find-action", View),
    a("ferro::SearchEverywhere", "action-search-everywhere", View),
    a(
        "ferro::HideAllToolWindows",
        "action-hide-all-tool-windows",
        View,
    ),
    a(
        "ferro::MaximizeToolWindow",
        "action-maximize-tool-window",
        View,
    ),
    a("ferro::ToggleLeftDock", "action-toggle-left-dock", View),
    a("ferro::ToggleBottomDock", "action-toggle-bottom-dock", View),
    a("ferro::ToggleRightDock", "action-toggle-right-dock", View),
    a("ferro::ResetLayout", "action-reset-layout", View),
    a(
        "ferro::ToggleCompactMode",
        "action-toggle-compact-mode",
        View,
    ),
    a("ferro::ZoomInUi", "action-zoom-in-ui", View),
    a("ferro::ZoomOutUi", "action-zoom-out-ui", View),
    a("ferro::ResetUiZoom", "action-reset-ui-zoom", View),
    // Navigate
    a("ferro::QuickOpen", "action-quick-open", Navigate),
    a("ferro::CommandPalette", "action-command-palette", Navigate),
    a(
        "ferro::GoToSymbolInFile",
        "action-go-to-symbol-in-file",
        Navigate,
    ),
    a(
        "ferro::GoToSymbolInWorkspace",
        "action-go-to-symbol-in-workspace",
        Navigate,
    ),
    a("ferro::GoToLine", "action-go-to-line", Navigate),
    a("ferro::GoBack", "action-go-back", Navigate),
    a("ferro::GoForward", "action-go-forward", Navigate),
    a("ferro::FindInFiles", "action-find-in-files", Navigate),
    a("ferro::ReplaceInFiles", "action-replace-in-files", Navigate),
    a(
        "ferro::SelectOpenedFile",
        "action-select-opened-file",
        Navigate,
    ),
    a("ferro::NextProblem", "action-next-problem", Navigate),
    a(
        "ferro::PreviousProblem",
        "action-previous-problem",
        Navigate,
    ),
    // Code
    a(
        "ferro::TriggerCompletion",
        "action-trigger-completion",
        Code,
    ),
    a("ferro::QuickFix", "action-quick-fix", Code),
    a(
        "ferro::QuickDocumentation",
        "action-quick-documentation",
        Code,
    ),
    a("ferro::ParameterInfo", "action-parameter-info", Code),
    a("ferro::RenameSymbol", "action-rename-symbol", Code),
    a("ferro::ToggleBreakpoint", "action-toggle-breakpoint", Code),
    // Tool windows
    a("ferro::ToggleProject", "action-toggle-project", ToolWindows),
    a("ferro::ToggleCommit", "action-toggle-commit", ToolWindows),
    a(
        "ferro::ToggleStructure",
        "action-toggle-structure",
        ToolWindows,
    ),
    a("ferro::ToggleRun", "action-toggle-run", ToolWindows),
    a("ferro::ToggleDebug", "action-toggle-debug", ToolWindows),
    a(
        "ferro::ToggleProblems",
        "action-toggle-problems",
        ToolWindows,
    ),
    a("ferro::ToggleGit", "action-toggle-git", ToolWindows),
    a(
        "ferro::ToggleTerminal",
        "action-toggle-terminal",
        ToolWindows,
    ),
    a("ferro::NewTerminal", "action-new-terminal", ToolWindows),
    a("ferro::ToggleTodo", "action-toggle-todo", ToolWindows),
    a(
        "ferro::ToggleNotifications",
        "action-toggle-notifications",
        ToolWindows,
    ),
    a(
        "ferro::ToggleDatabase",
        "action-toggle-database",
        ToolWindows,
    ),
    a("ferro::ToggleBuild", "action-toggle-build", ToolWindows),
    a(
        "ferro::ToggleServices",
        "action-toggle-services",
        ToolWindows,
    ),
    // Editor tabs and splits
    a("ferro::CloseTab", "action-close-tab", Editor),
    a("ferro::CloseOtherTabs", "action-close-other-tabs", Editor),
    a("ferro::CloseAllTabs", "action-close-all-tabs", Editor),
    a("ferro::ReopenClosedTab", "action-reopen-closed-tab", Editor),
    a("ferro::PinTab", "action-pin-tab", Editor),
    a("ferro::NextTab", "action-next-tab", Editor),
    a("ferro::PreviousTab", "action-previous-tab", Editor),
    a("ferro::SplitRight", "action-split-right", Editor),
    a("ferro::SplitDown", "action-split-down", Editor),
    // Run
    a("ferro::Run", "action-run", Run),
    a("ferro::Debug", "action-debug", Run),
    a("ferro::Stop", "action-stop", Run),
    a(
        "ferro::EditRunConfigurations",
        "action-edit-run-configurations",
        Run,
    ),
    // Version control
    a("ferro::ShowDiff", "action-show-diff", Vcs),
    a("ferro::ResolveConflicts", "action-resolve-conflicts", Vcs),
    // Appearance
    a("ferro::ThemeFerroDark", "action-theme-dark", Appearance),
    a("ferro::ThemeFerroLight", "action-theme-light", Appearance),
    a(
        "ferro::ThemeHighContrast",
        "action-theme-high-contrast",
        Appearance,
    ),
    a(
        "ferro::LanguageEnglish",
        "action-language-english",
        Appearance,
    ),
    a(
        "ferro::LanguagePolish",
        "action-language-polish",
        Appearance,
    ),
    a("ferro::KeymapVsCode", "action-keymap-vscode", Appearance),
    a(
        "ferro::KeymapJetBrains",
        "action-keymap-jetbrains",
        Appearance,
    ),
    a("ferro::KeymapZed", "action-keymap-zed", Appearance),
    a("ferro::KeymapSublime", "action-keymap-sublime", Appearance),
    a("ferro::KeymapEmacs", "action-keymap-emacs", Appearance),
    a("ferro::ToggleVimMode", "action-toggle-vim-mode", Appearance),
    // Help
    a("ferro::ShowWelcome", "action-show-welcome", Help),
    a("ferro::About", "action-about", Help),
];
