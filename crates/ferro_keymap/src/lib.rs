//! The action catalog and the keymaps that bind keystrokes to it.
//!
//! Every user-facing feature of FerroIDE is an *action* with a stable id
//! (`ferro::QuickOpen`, `ferro::ToggleProject`, …). The id is what the
//! keymap, the menus, the Command Palette and — later — extensions refer to,
//! so this crate is plain data with no GUI dependency: the UI layer turns ids
//! into GPUI actions with `App::build_action`.
//!
//! A keymap is a *scheme* (VS Code — the default, JetBrains, Zed, Sublime
//! Text, Emacs), resolved for a platform, plus the user's overrides from
//! `keymap.toml`. A user entry for an action replaces the scheme's bindings
//! of that action; an entry with empty `keys` unbinds it.
//!
//! Keystrokes use GPUI's syntax: `ctrl-shift-a`, `alt-f12`, chords separated
//! by spaces (`ctrl-k ctrl-s`).

use serde::{Deserialize, Serialize};

mod catalog;
mod schemes;

pub use catalog::{ACTIONS, ActionCategory, ActionDescriptor, action};
use schemes::MacRule;

/// The platform a keymap is resolved for. macOS has its own variant of every
/// scheme, following macOS conventions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Windows,
    Linux,
}

impl Platform {
    pub const ALL: [Platform; 3] = [Platform::MacOs, Platform::Windows, Platform::Linux];

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

/// A keymap scheme the user can choose in Settings → Keymap, on the welcome
/// screen or with a "Keymap: …" command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeymapPreset {
    /// VS Code defaults — FerroIDE's default.
    #[default]
    #[serde(rename = "vscode", alias = "vs-code")]
    VsCode,
    /// IntelliJ IDEA defaults. Stage 1 called it `intellij`.
    #[serde(alias = "intellij", alias = "intelli-j")]
    JetBrains,
    Zed,
    Sublime,
    Emacs,
}

impl KeymapPreset {
    pub const ALL: [KeymapPreset; 5] = [
        KeymapPreset::VsCode,
        KeymapPreset::JetBrains,
        KeymapPreset::Zed,
        KeymapPreset::Sublime,
        KeymapPreset::Emacs,
    ];

    pub fn display_name(self) -> &'static str {
        match self {
            KeymapPreset::VsCode => "VS Code",
            KeymapPreset::JetBrains => "JetBrains (IntelliJ IDEA)",
            KeymapPreset::Zed => "Zed",
            KeymapPreset::Sublime => "Sublime Text",
            KeymapPreset::Emacs => "Emacs",
        }
    }

    /// The id of the action that selects this scheme.
    pub fn action_id(self) -> &'static str {
        match self {
            KeymapPreset::VsCode => "ferro::KeymapVsCode",
            KeymapPreset::JetBrains => "ferro::KeymapJetBrains",
            KeymapPreset::Zed => "ferro::KeymapZed",
            KeymapPreset::Sublime => "ferro::KeymapSublime",
            KeymapPreset::Emacs => "ferro::KeymapEmacs",
        }
    }

    /// Whether Shift Shift opens Search Everywhere (a JetBrains habit).
    pub fn double_shift_search(self) -> bool {
        self == KeymapPreset::JetBrains
    }

    fn tables(self) -> (&'static [Binding], MacRule) {
        match self {
            KeymapPreset::VsCode => (schemes::VSCODE, schemes::VSCODE_MAC),
            KeymapPreset::JetBrains => (schemes::JETBRAINS, schemes::JETBRAINS_MAC),
            KeymapPreset::Zed => (schemes::ZED, schemes::ZED_MAC),
            KeymapPreset::Sublime => (schemes::SUBLIME, schemes::SUBLIME_MAC),
            KeymapPreset::Emacs => (schemes::EMACS, schemes::EMACS_MAC),
        }
    }
}

/// One keystroke-to-action binding of a built-in scheme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub keystrokes: &'static str,
    /// An id from [`ACTIONS`].
    pub action: &'static str,
    /// GPUI key context the binding is limited to; `None` is global.
    pub context: Option<&'static str>,
}

/// Key context set by the workspace root element.
pub const WORKSPACE_CONTEXT: &str = "Workspace";

/// Key context set around the text of an editor tab.
pub const EDITOR_CONTEXT: &str = "FerroEditor";

/// Context of text-editing bindings: the text input inside an editor tab.
/// It is as deep as GPUI Kit's own `Input` bindings, so a scheme's binding
/// wins over the kit's for the same keystroke, and it does not fire in
/// other inputs (Quick Open, search fields).
pub const EDITOR_INPUT_CONTEXT: &str = "FerroEditor > Input";

/// Replace `ctrl` with `cmd` in every stroke of a keystroke sequence.
fn ctrl_to_cmd(keystrokes: &str) -> String {
    keystrokes
        .split(' ')
        .map(|stroke| {
            let (mods, key) = split_stroke(stroke);
            let mods: Vec<&str> = mods
                .iter()
                .map(|m| if *m == "ctrl" { "cmd" } else { m })
                .collect();
            join_stroke(&mods, key)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Split `ctrl-shift-a` into (["ctrl", "shift"], "a"); handles `ctrl--`.
fn split_stroke(stroke: &str) -> (Vec<&str>, &str) {
    let (mods, key) = match stroke.strip_suffix("--") {
        Some(mods) => (mods, "-"),
        None => match stroke.rsplit_once('-') {
            Some((mods, key)) if !key.is_empty() => (mods, key),
            _ => ("", stroke),
        },
    };
    (mods.split('-').filter(|m| !m.is_empty()).collect(), key)
}

fn join_stroke(mods: &[&str], key: &str) -> String {
    let mut out = String::new();
    for m in mods {
        out.push_str(m);
        out.push('-');
    }
    out.push_str(key);
    out
}

/// Put modifiers in GPUI's canonical order (`ctrl alt shift cmd fn`) and
/// lowercase everything, so `Shift+Ctrl+A` and `ctrl-shift-a` compare equal.
pub fn normalize_keystrokes(keystrokes: &str) -> String {
    const ORDER: [&str; 5] = ["ctrl", "alt", "shift", "cmd", "fn"];
    keystrokes
        .split_whitespace()
        .map(|stroke| {
            let lower = stroke.to_lowercase();
            let (mods, key) = split_stroke(&lower);
            let mut mods: Vec<&str> = mods
                .into_iter()
                .map(|m| match m {
                    "control" => "ctrl",
                    "option" => "alt",
                    "super" | "win" | "platform" | "meta" => "cmd",
                    other => other,
                })
                .collect();
            mods.sort_by_key(|m| ORDER.iter().position(|o| o == m).unwrap_or(ORDER.len()));
            mods.dedup();
            join_stroke(&mods, key)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The built-in bindings of `preset` for `platform`.
pub fn bindings(preset: KeymapPreset, platform: Platform) -> Vec<Binding> {
    let (pc, mac) = preset.tables();
    if platform != Platform::MacOs {
        return pc.to_vec();
    }
    match mac {
        MacRule::Same => pc.to_vec(),
        MacRule::Explicit(table) => table.to_vec(),
        MacRule::CtrlToCmd { keep, overrides } => {
            let mut out: Vec<Binding> = pc
                .iter()
                .filter(|b| !overrides.iter().any(|o| o.action == b.action))
                .map(|b| {
                    let keystrokes = if keep.contains(&b.keystrokes) {
                        b.keystrokes
                    } else {
                        // Bindings are static data; the converted strings
                        // are interned once per process.
                        intern(ctrl_to_cmd(b.keystrokes))
                    };
                    Binding { keystrokes, ..*b }
                })
                .collect();
            out.extend_from_slice(overrides);
            out
        }
    }
}

/// Leak each distinct converted keystroke string once, so derived macOS
/// bindings can be `&'static str` like the tables they come from.
fn intern(s: String) -> &'static str {
    use std::{
        collections::HashSet,
        sync::{Mutex, OnceLock},
    };
    static POOL: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut pool = POOL
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(existing) = pool.get(s.as_str()) {
        return existing;
    }
    let leaked: &'static str = Box::leak(s.into_boxed_str());
    pool.insert(leaked);
    leaked
}

/// The first keystroke bound to `action` in the built-in `preset`, for
/// display where user overrides do not matter (e.g. the welcome screen).
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

// ---------------------------------------------------------------------------
// User overrides and the resolved keymap
// ---------------------------------------------------------------------------

/// One entry of `keymap.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserBinding {
    /// Keystrokes; empty unbinds the action.
    #[serde(default)]
    pub keys: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// The user's keymap overrides (`keymap.toml`), applied on top of the
/// selected scheme.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UserKeymap {
    #[serde(default, rename = "binding")]
    pub bindings: Vec<UserBinding>,
}

impl UserKeymap {
    /// Whether the user overrides `action`.
    pub fn overrides(&self, action: &str) -> bool {
        self.bindings.iter().any(|b| b.action == action)
    }

    /// Replace the user's bindings of `action` with `keys` (normalized).
    /// An empty list unbinds the action.
    pub fn set(&mut self, action: &str, keys: &[String]) {
        self.bindings.retain(|b| b.action != action);
        let keys: Vec<String> = keys
            .iter()
            .map(|k| normalize_keystrokes(k))
            .filter(|k| !k.is_empty())
            .collect();
        if keys.is_empty() {
            self.bindings.push(UserBinding {
                keys: String::new(),
                action: action.to_string(),
                context: None,
            });
        } else {
            for keys in keys {
                self.bindings.push(UserBinding {
                    keys,
                    action: action.to_string(),
                    context: None,
                });
            }
        }
    }

    /// Drop the user's bindings of `action`: the scheme's apply again.
    pub fn reset(&mut self, action: &str) {
        self.bindings.retain(|b| b.action != action);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingSource {
    Scheme,
    User,
}

/// A binding of the effective keymap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBinding {
    pub keystrokes: String,
    pub action: String,
    pub context: Option<String>,
    pub source: BindingSource,
}

/// The effective keymap: `preset` for `platform` with `user` applied.
/// User bindings come last, so they win where GPUI resolves overlaps.
pub fn resolve(
    preset: KeymapPreset,
    platform: Platform,
    user: &UserKeymap,
) -> Vec<ResolvedBinding> {
    let mut out: Vec<ResolvedBinding> = bindings(preset, platform)
        .into_iter()
        .filter(|b| !user.overrides(b.action))
        .map(|b| ResolvedBinding {
            keystrokes: b.keystrokes.to_string(),
            action: b.action.to_string(),
            context: b.context.map(str::to_string),
            source: BindingSource::Scheme,
        })
        .collect();
    out.extend(
        user.bindings
            .iter()
            .filter(|b| !b.keys.trim().is_empty())
            .map(|b| ResolvedBinding {
                keystrokes: normalize_keystrokes(&b.keys),
                action: b.action.clone(),
                context: b.context.clone(),
                source: BindingSource::User,
            }),
    );
    out
}

/// Every keystroke bound to `action` in a resolved keymap.
pub fn shortcuts_in<'a>(action: &str, keymap: &'a [ResolvedBinding]) -> Vec<&'a str> {
    keymap
        .iter()
        .filter(|b| b.action == action)
        .map(|b| b.keystrokes.as_str())
        .collect()
}

/// Two or more actions bound to the same keystrokes in the same context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub keystrokes: String,
    pub context: Option<String>,
    pub actions: Vec<String>,
}

/// Find keystrokes bound to more than one action in the same context, and
/// chords whose prefix is itself bound (the longer chord could never fire).
pub fn conflicts(keymap: &[ResolvedBinding]) -> Vec<Conflict> {
    use std::collections::BTreeMap;
    let mut by_keys: BTreeMap<(String, Option<String>), Vec<String>> = BTreeMap::new();
    for b in keymap {
        let actions = by_keys
            .entry((normalize_keystrokes(&b.keystrokes), b.context.clone()))
            .or_default();
        if !actions.contains(&b.action) {
            actions.push(b.action.clone());
        }
    }
    let mut out: Vec<Conflict> = by_keys
        .iter()
        .filter(|(_, actions)| actions.len() > 1)
        .map(|((keystrokes, context), actions)| Conflict {
            keystrokes: keystrokes.clone(),
            context: context.clone(),
            actions: actions.clone(),
        })
        .collect();
    // A bound prefix shadows a chord: `ctrl-k` bound alone breaks `ctrl-k ctrl-s`.
    for ((keys, context), actions) in &by_keys {
        let strokes: Vec<&str> = keys.split(' ').collect();
        for n in 1..strokes.len() {
            let prefix = strokes[..n].join(" ");
            if let Some(prefix_actions) = by_keys.get(&(prefix.clone(), context.clone())) {
                let mut all = prefix_actions.clone();
                all.extend(actions.iter().cloned());
                out.push(Conflict {
                    keystrokes: format!("{prefix} | {keys}"),
                    context: context.clone(),
                    actions: all,
                });
            }
        }
    }
    out
}

/// Whether `action` takes part in any of `conflicts`.
pub fn is_conflicting(action: &str, conflicts: &[Conflict]) -> bool {
    conflicts
        .iter()
        .any(|c| c.actions.iter().any(|a| a == action))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn every_bound_action_is_in_the_catalog() {
        for preset in KeymapPreset::ALL {
            for platform in Platform::ALL {
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
    fn no_scheme_has_conflicts() {
        for preset in KeymapPreset::ALL {
            for platform in Platform::ALL {
                let mut seen = HashSet::new();
                for binding in bindings(preset, platform) {
                    assert!(
                        seen.insert((normalize_keystrokes(binding.keystrokes), binding.context)),
                        "{preset:?}/{platform:?}: `{}` is bound twice",
                        binding.keystrokes
                    );
                }
                let resolved = resolve(preset, platform, &UserKeymap::default());
                assert_eq!(conflicts(&resolved), vec![], "{preset:?}/{platform:?}");
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
    fn vscode_is_the_default_and_follows_appendix_c() {
        assert_eq!(KeymapPreset::default(), KeymapPreset::VsCode);
        let p = Platform::Windows;
        let s = |a| shortcut_for(a, KeymapPreset::VsCode, p);
        assert_eq!(s("ferro::QuickOpen"), Some("ctrl-p"));
        assert_eq!(s("ferro::CommandPalette"), Some("ctrl-shift-p"));
        assert_eq!(s("ferro::QuickFix"), Some("ctrl-."));
        assert_eq!(s("ferro::GoToLine"), Some("ctrl-g"));
        assert_eq!(s("ferro::OpenKeymapSettings"), Some("ctrl-k ctrl-s"));
        assert_eq!(s("ferro::ToggleBottomDock"), Some("ctrl-j"));
    }

    #[test]
    fn macos_variants_use_cmd() {
        let m = Platform::MacOs;
        assert_eq!(
            shortcut_for("ferro::QuickOpen", KeymapPreset::VsCode, m),
            Some("cmd-p")
        );
        assert_eq!(
            shortcut_for("ferro::CommandPalette", KeymapPreset::VsCode, m),
            Some("cmd-shift-p")
        );
        assert_eq!(
            shortcut_for("ferro::OpenKeymapSettings", KeymapPreset::VsCode, m),
            Some("cmd-k cmd-s")
        );
        // Kept as ctrl on macOS, as in VS Code.
        assert_eq!(
            shortcut_for("ferro::ToggleTerminal", KeymapPreset::VsCode, m),
            Some("ctrl-`")
        );
        assert_eq!(
            shortcut_for("ferro::ZoomOutUi", KeymapPreset::Zed, m),
            Some("cmd--")
        );
        // Emacs is the same everywhere.
        assert_eq!(
            shortcut_for("ferro::QuickOpen", KeymapPreset::Emacs, m),
            Some("ctrl-x ctrl-f")
        );
        assert_eq!(
            shortcut_for("ferro::FindAction", KeymapPreset::JetBrains, m),
            Some("cmd-shift-a")
        );
    }

    #[test]
    fn stage1_settings_names_still_parse() {
        #[derive(Deserialize)]
        struct S {
            keymap: KeymapPreset,
        }
        let s: S = serde_json::from_str(r#"{"keymap":"intellij"}"#).unwrap();
        assert_eq!(s.keymap, KeymapPreset::JetBrains);
        let s: S = serde_json::from_str(r#"{"keymap":"vs-code"}"#).unwrap();
        assert_eq!(s.keymap, KeymapPreset::VsCode);
        let s: S = serde_json::from_str(r#"{"keymap":"vscode"}"#).unwrap();
        assert_eq!(s.keymap, KeymapPreset::VsCode);
    }

    #[test]
    fn normalization_orders_modifiers() {
        assert_eq!(normalize_keystrokes("shift-ctrl-A"), "ctrl-shift-a");
        assert_eq!(normalize_keystrokes("Ctrl-K  Ctrl-S"), "ctrl-k ctrl-s");
        assert_eq!(normalize_keystrokes("ctrl--"), "ctrl--");
        assert_eq!(normalize_keystrokes("super-alt-x"), "alt-cmd-x");
    }

    #[test]
    fn user_overrides_replace_unbind_and_reset() {
        let p = Platform::Windows;
        let mut user = UserKeymap::default();
        user.set("ferro::QuickOpen", &["Ctrl-E".to_string()]);
        user.set("ferro::GoToLine", &[]);
        let keymap = resolve(KeymapPreset::VsCode, p, &user);
        assert_eq!(shortcuts_in("ferro::QuickOpen", &keymap), vec!["ctrl-e"]);
        assert!(shortcuts_in("ferro::GoToLine", &keymap).is_empty());
        let user_bound = keymap
            .iter()
            .find(|b| b.action == "ferro::QuickOpen")
            .unwrap();
        assert_eq!(user_bound.source, BindingSource::User);

        // Switching the scheme keeps the overrides.
        let keymap = resolve(KeymapPreset::JetBrains, p, &user);
        assert_eq!(shortcuts_in("ferro::QuickOpen", &keymap), vec!["ctrl-e"]);

        user.reset("ferro::QuickOpen");
        let keymap = resolve(KeymapPreset::VsCode, p, &user);
        assert_eq!(shortcuts_in("ferro::QuickOpen", &keymap), vec!["ctrl-p"]);
    }

    #[test]
    fn conflicts_are_detected() {
        let p = Platform::Windows;
        let mut user = UserKeymap::default();
        // Ctrl+G is Go to Line in VS Code.
        user.set("ferro::QuickOpen", &["ctrl-g".to_string()]);
        let found = conflicts(&resolve(KeymapPreset::VsCode, p, &user));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].keystrokes, "ctrl-g");
        assert!(is_conflicting("ferro::GoToLine", &found));
        assert!(!is_conflicting("ferro::CloseTab", &found));

        // A bound prefix shadows a chord.
        let mut user = UserKeymap::default();
        user.set("ferro::Run", &["ctrl-k".to_string()]);
        let found = conflicts(&resolve(KeymapPreset::VsCode, p, &user));
        assert!(found.iter().any(|c| c.keystrokes.starts_with("ctrl-k |")));
    }

    #[test]
    fn user_keymap_round_trips_through_serde() {
        let mut user = UserKeymap::default();
        user.set("ferro::NewTerminal", &["ctrl-shift-t".into()]);
        let json = serde_json::to_string(&user).unwrap();
        let back: UserKeymap = serde_json::from_str(&json).unwrap();
        assert_eq!(back, user);
    }
}
