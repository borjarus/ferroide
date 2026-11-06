//! Every action FerroIDE exposes, with the message id of its title.

/// Grouping shown in Find Action and used to build menus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionCategory {
    File,
    View,
    ToolWindows,
    Editor,
    Run,
    Appearance,
    Help,
}

impl ActionCategory {
    /// Message id of the category's localized name.
    pub fn title_key(self) -> &'static str {
        match self {
            ActionCategory::File => "category-file",
            ActionCategory::View => "category-view",
            ActionCategory::ToolWindows => "category-tool-windows",
            ActionCategory::Editor => "category-editor",
            ActionCategory::Run => "category-run",
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
    /// Whether Find Action lists it. Internal actions stay hidden.
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

use ActionCategory::*;

pub const ACTIONS: &[ActionDescriptor] = &[
    // File
    a("ferro::NewProject", "action-new-project", File),
    a("ferro::OpenProject", "action-open-project", File),
    a("ferro::GetFromVcs", "action-get-from-vcs", File),
    a("ferro::CloseProject", "action-close-project", File),
    a("ferro::NewWindow", "action-new-window", File),
    a("ferro::OpenSettings", "action-open-settings", File),
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
    a("ferro::ResetLayout", "action-reset-layout", View),
    a(
        "ferro::ToggleCompactMode",
        "action-toggle-compact-mode",
        View,
    ),
    a("ferro::ZoomInUi", "action-zoom-in-ui", View),
    a("ferro::ZoomOutUi", "action-zoom-out-ui", View),
    a("ferro::ResetUiZoom", "action-reset-ui-zoom", View),
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
    // Editor tabs and splits
    a("ferro::CloseTab", "action-close-tab", Editor),
    a("ferro::CloseOtherTabs", "action-close-other-tabs", Editor),
    a("ferro::CloseAllTabs", "action-close-all-tabs", Editor),
    a("ferro::PinTab", "action-pin-tab", Editor),
    a("ferro::NextTab", "action-next-tab", Editor),
    a("ferro::PreviousTab", "action-previous-tab", Editor),
    a("ferro::SplitRight", "action-split-right", Editor),
    a("ferro::SplitDown", "action-split-down", Editor),
    // Run
    a("ferro::Run", "action-run", Run),
    a("ferro::Debug", "action-debug", Run),
    a("ferro::Stop", "action-stop", Run),
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
    a(
        "ferro::KeymapIntelliJ",
        "action-keymap-intellij",
        Appearance,
    ),
    a("ferro::KeymapVsCode", "action-keymap-vscode", Appearance),
    // Help
    a("ferro::ShowWelcome", "action-show-welcome", Help),
    a("ferro::About", "action-about", Help),
];
