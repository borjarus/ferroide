# Implementation status

Stages follow [FerroIDE_plan.md](FerroIDE_plan.md) §8. This file records what
each finished stage delivered, how its Definition of Done was checked, and
what deviated from the plan.

## Stage 0 — Foundations and technology spike

| Plan item | Delivered |
|---|---|
| Repository, Cargo workspace, `rust-toolchain.toml`, `deny.toml` | ✔ Rust 1.99, edition 2024, resolver 3; licenses and RustSec audit in `deny.toml` |
| Issue/PR templates, commit convention | ✔ `.github/ISSUE_TEMPLATE`, PR template, Conventional Commits in `CONTRIBUTING.md` |
| CI: Linux x64, Windows x64, macOS arm64 + x64; clippy, rustfmt, nextest, cargo-deny | ✔ `.github/workflows/ci.yml` (macOS x64 and spikes run nightly); not yet run on GitHub |
| Spike: window + dock with three panels | ✔ `spike_dock` |
| Spike: list with 1 M items | ✔ `spike_list` |
| Spike: GPUI Kit code editor with a 100k-line file | ✔ `spike_editor` |
| Measurements: startup, RAM, FPS, typing latency | ✔ `cargo xtask spike` → `docs/reports/spike-<os>-<arch>.json`; results below |
| Rendering test matrix (old Intel HD, NVIDIA, X11/Wayland, RDP, VM) | ✘ needs that hardware; see "Open items" |
| ADR-001…005 | ✔ [docs/adr](adr/) |

**Definition of Done — spike report and go/no-go:** see
[reports/stage0-spike.md](reports/stage0-spike.md) and
[ADR-001](adr/ADR-001-gui-framework.md): **go**, conditional on the Linux and
weak-hardware runs listed there.

## Stage 1 — GUI skeleton in the style of IntelliJ New UI

Everything runs on the mock services in `ferro_services` (synthetic data,
simulated latency and failures).

| Plan item | Delivered |
|---|---|
| Window: own title bar, one window = one project, many windows | ✔ GPUI Kit `TitleBar` (client-side on Linux, native buttons on Windows/macOS); New Window / Open open more windows |
| Main toolbar: project widget, VCS branch widget, run configuration selector with Run/Debug/Stop, search, settings | ✔ all dropdowns live on mocks (branch checkout, run lifecycle) |
| Tool-window stripes left/right; dock with dragging, pinning, modes; layout memory | ✔ stripes, drag between docks, resize, zoom (maximize), hide all; layout saved per project and restored. Partial: see deviations |
| Editor area: tabs (pinning, limit, group closing), horizontal/vertical splits, dragging tabs between splits | ✔ |
| Status bar: cursor, encoding, line separator, indentation, language, branch, LSP status, memory, background progress | ✔ cursor and memory are real; the rest mock |
| Action system and keymap: registry with ids, IntelliJ default keymap, Find Action | ✔ 48 actions; IntelliJ and VS Code presets for Windows/Linux and macOS, switchable at runtime |
| Themes: Ferro Dark, Ferro Light, High Contrast; semantic tokens; switch without restart; UI and font scaling | ✔ |
| Welcome screen: recent projects, New / Open / Get from VCS | ✔ with filtering |
| Menus: native on macOS, in-window on Windows/Linux, hamburger in compact mode | ✔ |
| i18n: Fluent, Polish and English from the first screen | ✔ catalogs checked for parity by a test |
| `ferro_storybook` | ✔ `cargo run -p ferro_storybook`, incl. slow/failing services |

**Definition of Done:**

| Criterion | How it is checked | Result |
|---|---|---|
| Complete window layout on three systems | Built and run on Windows 11 here; Linux/macOS via CI matrix | Windows ✔; Linux/macOS pending first CI run |
| Theme switch < 50 ms | `every_theme_switch_fits_the_50ms_budget` test; logged at startup | ✔ (≈0.1 ms measured in the app) |
| No frame drops while dragging panels on the weak profile | `spike_dock` resizes a dock every frame | see spike report; weak-profile run pending |
| Headless UI tests for dock and tabs | `crates/ferro_workspace/src/tests.rs` (11 tests) | ✔ |

### Deviations from the plan

- **Dock modes.** GPUI Kit's dock supports docked panels, dragging between
  docks and zoom. *Floating* and *separate window* tool-window modes are not
  available in GPUI Kit 0.9 and are deferred; "overlay" is approximated by
  zoom (Maximize Tool Window).
- **Collapsed bottom dock** keeps a 29 px strip of tab names
  (`gpui_base::dock::CLOSED_BOTTOM_STRIP`, structural in GPUI Kit). It is
  usable but duplicates the stripe buttons; a custom dock renderer can remove
  it later.
- **Search Everywhere (Shift Shift)** is Stage 2; in Stage 1 the toolbar
  search button and the `SearchEverywhere` action open Find Action. GPUI key
  bindings do not support a modifier-only double tap, so Shift Shift needs a
  custom key-event handler in Stage 2.
- **Settings** opens a quick-settings dialog (theme, language, keymap, UI
  size, compact mode); the full Settings window is Stage 2.
- **Window state storage** is a JSON file per project instead of SQLite
  (moves to SQLite in Stage 4, per ADR-003).
- **Extra crates** not named in the plan: `ferro_i18n` (Fluent, GUI-free)
  and `ferro_keymap` also hosts the action catalog.

### Open items

- Run the spikes on Linux (X11 and Wayland), macOS, an RDP session, a VM and
  the reference weak laptop; fill in [stage0-spike.md](reports/stage0-spike.md).
- First CI run on GitHub (no remote is configured yet).
- `conduct@ferroide.dev` in `CODE_OF_CONDUCT.md` is a placeholder address.

## Stage 2 — IDE components on mocks (milestone M1 "UI Preview")

Every view runs on the mock services in `ferro_services`. Stage 2 added the
`LanguageService`, `SearchService`, `DebugService` and `TerminalService`
contracts and extended the project, VCS, run and status services. The mocks
are deterministic and support simulated latency, failures and a synthetic
1,000,000-file project tree.

| Plan item | Delivered |
|---|---|
| Project tool window: virtual tree (1M nodes), file icons, context menu, drag & drop, Select Opened File, speed search | ✔ `panels/project.rs`; incremental row model, keyboard navigation, VCS status colors, inline rename / new directory |
| Editor view layer: highlighting, gutter (VCS markers, breakpoints, ▶ at `main` and tests), breadcrumbs, sticky lines, inlay hints, diagnostics, completion, Quick Documentation, Parameter Info, Quick Fix | ✔ `editor/` on GPUI Kit's editor; inlay hints partial (see deviations) |
| Quick Open / Command Palette with prefixes (none, `>`, `@`, `#`, `:`, `%`), recent first, preview | ✔ `quick_open.rs`; Shift Shift opens it in the JetBrains scheme |
| Settings window: categories, search, field editors, Apply / Reset, raw `settings.toml` | ✔ `settings_window.rs` (draft with Apply / Reset / OK / Cancel) |
| Keymap page: scheme selector (VS Code default, JetBrains, Zed, Sublime Text, Emacs), Vim mode toggle, binding table, search by name or keys, edit / reset, conflicts, user overrides, macOS variants | ✔ `keymap_page.rs`, `ferro_keymap` (schemes, `keymap.toml` overrides, conflict detection); Stage 1 `keymap = "intellij"` settings still load |
| Tool windows: Problems, Terminal (static), Run and Debug (console, test tree, variables, call stack), Commit, Git Log (graph), Structure, TODO, Notifications, Services | ✔ `panels/`, `vcs/` |
| Diff viewer (side-by-side and unified) and 3-way merge | ✔ `vcs/diff_view.rs` (GPUI Kit diff), `vcs/merge_view.rs` + `vcs/merge_model.rs` |
| Dialogs: Rename with preview, Find in Files with preview, New File, Run Configuration editor, Clone Repository | ✔ `dialogs/`, `find_in_files.rs` |
| Accessibility: keyboard navigation, labels, contrast | Partial: every new view is keyboard-navigable with accessible labels; no AccessKit audit or contrast check yet |

**Definition of Done (M1):**

| Criterion | How it is checked | Result |
|---|---|---|
| Complete, clickable IDE on mocks | Every view above is reachable from menus, the Command Palette and shortcuts | ✔ (Windows 11) |
| Headless UI tests for key interactions | `ferro_workspace` has 114 tests (`tests*.rs`); 188 across the workspace | ✔ |
| Every view fits the frame budget with 10× data | Performance tests per area, debug build: project tree with 1.14M rows (render of a visible range 0.55 ms), 460-commit log (graph layout 0.9 ms), 50k-line file (per-frame editor overlays ~9 µs), 880-row keymap table (3.5 ms per 6 filter passes), 20k diagnostics, 1,000 Quick Open results (3–6 ms) | ✔ for UI-thread work; release-build and weak-hardware runs pending |

### Deviations from the plan

- **Inlay hints** are drawn at the end of the line: GPUI Kit's editor cannot
  insert virtual text. True inline hints need `ferro_editor` (Stage 3).
  Overlays (glyph margin, sticky lines, hints) assume one display row per
  line and can drift with soft wrap or folds.
- **Writes to files** that are not open in an editor (Rename, Replace in
  Files) are previewed only; the mock project service has no file writing
  until Stage 4. Merge results are stored in the VCS mock.
- **Terminal** is display-only (Stage 8), **debugger stepping** only reports
  "not available" (Stage 10), **Vim mode** is a stored setting whose modal
  layer arrives with the editor core (Stage 3).
- **Search Everywhere over 1M files** takes ~12 s in a debug build in the
  fuzzy matcher; the 10× test is `#[ignore]` and meant for
  `cargo test --release -- --ignored million`. Real indexing is Stage 7.
- **Clone** shows a spinner, not progress (the mock sends no progress
  events); the welcome onboarding card is always shown (no first-run flag).
- **Translations** are split per area: `assets/locales/<lang>/{ferro,editor,search,panels,vcs,settings,dialogs}.ftl`.

### Open items

- Release-build and weak-profile measurements of the Stage 2 views.
- AccessKit roles and a contrast check of the three themes.
- Real implementations behind the same contracts from Stage 3 onward.

## Zed window layout (plan §8, Stage 1 v1.1)

The window now follows the Zed layout the plan describes for Stage 1,
replacing the IntelliJ New UI chrome built in Stage 1. The Stage 1 section
above describes the earlier layout.

| Plan item | Status |
|---|---|
| Slim single-row title bar: app menu (hamburger on Windows/Linux, native on macOS), project picker, branch picker; run configuration with Run/Debug/Stop, notification bell (unread dot), settings menu | ✔ `chrome.rs`; the in-window menu bar and the main-toolbar search icon are gone |
| Three docks, no tool-window stripes: left — Project, Commit, Git, Structure, Database; bottom — Terminal, Run, Debug, Problems, TODO, Build, Services; right — Notifications | ✔ `ToolWindowKind::default_placement` |
| Status bar = panel switcher: left-dock toggles, diagnostics counter (opens Problems), background task; cursor, indent, encoding, line separator, language, LSP, memory; bottom- and right-dock toggles | ✔ highlighted while the panel is visible |
| Moving a panel to another dock (right-click a toggle → Dock Left / Bottom / Right), remembered per project | ✔ `Workspace::move_tool_window`; layout version bumped to 2, so layouts saved by the old UI are discarded once |
| Pane tab bar: Back / Forward, `+` new file, split | ✔ new `ferro::GoBack` / `ferro::GoForward` actions with navigation history; bound in every scheme (VS Code Alt+←/→, JetBrains Ctrl+Alt+←/→, Sublime Alt+-; macOS variants) |
| Dock toggle buttons inside tab bars | Removed (`set_toggle_button_visible(false)`); the status bar owns them |

Deviations:

- **Back / Forward sit on the right** of the pane's tab bar, next to `+` and
  split: GPUI Kit's tab bar exposes only a trailing toolbar slot to panels.
  Moving them to the left needs a custom tab-bar renderer.
- **Navigation history** records file switches and jumps that move focus
  (Quick Open, Go to Line, search results, tab clicks), not every caret move.
- **No AI Agent panel yet**, so the right dock holds only Notifications by
  default.

Tests: `default_layout_follows_zed`, `status_bar_toggles_hide_show_and_switch_tool_windows`,
`panels_sit_in_the_zed_docks_and_can_move`, `back_and_forward_retrace_navigation`.
