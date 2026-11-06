//! The action catalog and the keymaps that bind keystrokes to it.
//!
//! Every user-facing feature of FerroIDE is an *action* with a stable id
//! (`ferro::FindAction`, `ferro::ToggleProject`, …). The id is what the
//! keymap, the menus, Find Action and — later — extensions refer to, so this
//! crate is plain data with no GUI dependency: the UI layer turns ids into
//! GPUI actions with `App::build_action`.
//!
//! Keystrokes use GPUI's syntax: `ctrl-shift-a`, `alt-f12`, chords separated
//! by spaces.

use serde::{Deserialize, Serialize};

mod catalog;

pub use catalog::{ACTIONS, ActionCategory, ActionDescriptor};

/// The platform a keymap is resolved for. macOS has its own variant of every
/// preset, following macOS conventions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Windows,
    Linux,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(target_os = "windows") {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }
}

/// A named keymap the user can choose in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeymapPreset {
    /// IntelliJ IDEA defaults — FerroIDE's default.
    #[default]
    IntelliJ,
    /// VS Code-like bindings for the actions that differ.
    VsCode,
}

impl KeymapPreset {
    pub const ALL: [KeymapPreset; 2] = [KeymapPreset::IntelliJ, KeymapPreset::VsCode];

    pub fn display_name(self) -> &'static str {
        match self {
            KeymapPreset::IntelliJ => "IntelliJ IDEA",
            KeymapPreset::VsCode => "VS Code",
        }
    }
}

/// One keystroke-to-action binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub keystrokes: &'static str,
    /// An id from [`ACTIONS`].
    pub action: &'static str,
    /// GPUI key context the binding is limited to; `None` is global.
    pub context: Option<&'static str>,
}

const fn b(keystrokes: &'static str, action: &'static str) -> Binding {
    Binding {
        keystrokes,
        action,
        context: None,
    }
}

/// Key context set by the workspace root element.
pub const WORKSPACE_CONTEXT: &str = "Workspace";

/// Resolve the bindings of `preset` for `platform`.
pub fn bindings(preset: KeymapPreset, platform: Platform) -> Vec<Binding> {
    let mac = platform == Platform::MacOs;
    let mut out: Vec<Binding> = if mac {
        INTELLIJ_MAC.to_vec()
    } else {
        INTELLIJ_PC.to_vec()
    };

    if preset == KeymapPreset::VsCode {
        let overrides: &[Binding] = if mac { VSCODE_MAC } else { VSCODE_PC };
        out.retain(|binding| !overrides.iter().any(|o| o.action == binding.action));
        out.extend_from_slice(overrides);
    }
    out
}

/// The first keystroke bound to `action` in `preset`, for display in menus
/// and Find Action.
pub fn shortcut_for(
    action: &str,
    preset: KeymapPreset,
    platform: Platform,
) -> Option<&'static str> {
    bindings(preset, platform)
        .into_iter()
        .find(|binding| binding.action == action)
        .map(|binding| binding.keystrokes)
}

const INTELLIJ_PC: &[Binding] = &[
    // Navigation and search
    b("ctrl-shift-a", "ferro::FindAction"),
    b("ctrl-alt-s", "ferro::OpenSettings"),
    // Tool windows
    b("alt-1", "ferro::ToggleProject"),
    b("alt-0", "ferro::ToggleCommit"),
    b("alt-7", "ferro::ToggleStructure"),
    b("alt-4", "ferro::ToggleRun"),
    b("alt-5", "ferro::ToggleDebug"),
    b("alt-6", "ferro::ToggleProblems"),
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
    // View
    b("ctrl-=", "ferro::ZoomInUi"),
    b("ctrl--", "ferro::ZoomOutUi"),
    b("ctrl-alt-0", "ferro::ResetUiZoom"),
    // Application
    b("ctrl-q", "ferro::Quit"),
];

const INTELLIJ_MAC: &[Binding] = &[
    b("cmd-shift-a", "ferro::FindAction"),
    b("cmd-,", "ferro::OpenSettings"),
    b("cmd-1", "ferro::ToggleProject"),
    b("cmd-0", "ferro::ToggleCommit"),
    b("cmd-7", "ferro::ToggleStructure"),
    b("cmd-4", "ferro::ToggleRun"),
    b("cmd-5", "ferro::ToggleDebug"),
    b("cmd-6", "ferro::ToggleProblems"),
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
    b("cmd-=", "ferro::ZoomInUi"),
    b("cmd--", "ferro::ZoomOutUi"),
    b("cmd-alt-0", "ferro::ResetUiZoom"),
    b("cmd-q", "ferro::Quit"),
];

const VSCODE_PC: &[Binding] = &[
    b("ctrl-shift-p", "ferro::FindAction"),
    b("ctrl-,", "ferro::OpenSettings"),
    b("ctrl-b", "ferro::ToggleProject"),
    b("ctrl-`", "ferro::ToggleTerminal"),
    b("ctrl-w", "ferro::CloseTab"),
    b("ctrl-pagedown", "ferro::NextTab"),
    b("ctrl-pageup", "ferro::PreviousTab"),
    b("ctrl-\\", "ferro::SplitRight"),
    b("ctrl-f5", "ferro::Run"),
    b("f5", "ferro::Debug"),
    b("shift-f5", "ferro::Stop"),
];

const VSCODE_MAC: &[Binding] = &[
    b("cmd-shift-p", "ferro::FindAction"),
    b("cmd-b", "ferro::ToggleProject"),
    b("ctrl-`", "ferro::ToggleTerminal"),
    b("cmd-alt-right", "ferro::NextTab"),
    b("cmd-alt-left", "ferro::PreviousTab"),
    b("cmd-\\", "ferro::SplitRight"),
    b("ctrl-f5", "ferro::Run"),
    b("f5", "ferro::Debug"),
    b("shift-f5", "ferro::Stop"),
];

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    const PLATFORMS: [Platform; 3] = [Platform::MacOs, Platform::Windows, Platform::Linux];

    #[test]
    fn every_bound_action_is_in_the_catalog() {
        for preset in KeymapPreset::ALL {
            for platform in PLATFORMS {
                for binding in bindings(preset, platform) {
                    assert!(
                        ACTIONS.iter().any(|a| a.id == binding.action),
                        "{preset:?}/{platform:?}: `{}` is bound but not in the catalog",
                        binding.action
                    );
                }
            }
        }
    }

    #[test]
    fn no_keystroke_is_bound_twice_in_one_context() {
        for preset in KeymapPreset::ALL {
            for platform in PLATFORMS {
                let mut seen = HashSet::new();
                for binding in bindings(preset, platform) {
                    assert!(
                        seen.insert((binding.keystrokes, binding.context)),
                        "{preset:?}/{platform:?}: `{}` is bound twice",
                        binding.keystrokes
                    );
                }
            }
        }
    }

    #[test]
    fn action_ids_are_unique_and_namespaced() {
        let mut seen = HashSet::new();
        for action in ACTIONS {
            assert!(action.id.starts_with("ferro::"), "{}", action.id);
            assert!(seen.insert(action.id), "duplicate id {}", action.id);
            assert!(
                action.title_key.starts_with("action-"),
                "{}",
                action.title_key
            );
        }
    }

    #[test]
    fn vscode_preset_overrides_only_what_it_names() {
        let p = Platform::Windows;
        assert_eq!(
            shortcut_for("ferro::FindAction", KeymapPreset::IntelliJ, p),
            Some("ctrl-shift-a")
        );
        assert_eq!(
            shortcut_for("ferro::FindAction", KeymapPreset::VsCode, p),
            Some("ctrl-shift-p")
        );
        // Not overridden: keeps the IntelliJ binding.
        assert_eq!(
            shortcut_for("ferro::ToggleCommit", KeymapPreset::VsCode, p),
            Some("alt-0")
        );
    }
}
