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

## Stage 3 — Editor core

The editor core is a new GUI-free crate, `ferro_text`. The editor tabs in
the window still draw text through GPUI Kit's editor (as in Stage 2), with
`ferro_text` underneath for files, saving and the text commands. Replacing
the view itself with our own `ferro_editor` element is left for later (see
"Deviations").

| Plan item | Delivered |
|---|---|
| Rope, transactions, undo/redo, anchors | ✔ `ferro_text::Buffer`: versioned edits, undo/redo grouped by typing pauses (300 ms) with selections restored, `Anchor`s with left/right bias that follow edits and undo |
| Multi-cursor: Ctrl+D, Ctrl+Shift+L, Alt+click, Ctrl+Alt+↑/↓ | ✔ `ops::select_next_occurrence` / `select_all_occurrences` / `add_cursor_vertical`, bound in every scheme; Alt+click comes from GPUI Kit |
| Column selection, clipboard with several cursors | ✔ `ops::column_selection`, `ops::copy/cut/paste` (one piece per cursor, whole-line copy); in the window GPUI Kit's Alt+drag and clipboard are used |
| Find / replace in file (regex) | ✔ `ferro_text::search`: plain or regex, case, whole word, `$1` / `${name}` replacements; the window keeps GPUI Kit's find bar |
| Duplicate / move / delete lines, line and block comments | ✔ `ops::*`, actions `ferro::DuplicateLineDown/Up`, `MoveLineUp/Down`, `DeleteLine`, `ToggleLineComment`, `ToggleBlockComment`; comment syntax per language in `ferro_text::comment` until Language Packs (Stage 5) |
| Unicode: graphemes, emoji, wide characters, basic bidi, IME, AltGr | ✔ grapheme-aware deletion and columns, display width of CJK/emoji/ZWJ sequences, base direction and visual runs (`unicode::visual_runs`); IME and AltGr input come from GPUI Kit's input handler |
| Soft wrap, indentation, visible whitespace, tabs | ✔ `display_map` (folds, word wrap, tabs); indent/outdent ops; *Show whitespace* setting and `ferro::ToggleWhitespace`, `ferro::ToggleSoftWrap` |
| Encoding detection, line separators | ✔ BOM → UTF-8 → UTF-16 without BOM → `chardetng`; LF / CRLF / CR detected (mixed reported); the status bar shows both and changes them (Reopen / Save with Encoding) |
| Atomic save, autosave on focus loss, external changes | ✔ temp file + fsync + rename (permissions and symlinks kept, read-only refused); autosave on blur / window deactivation / tab close, optionally after 1 s; a changed file reloads silently when unmodified, otherwise a bar offers Reload / Compare (diff view) / Keep Mine |
| Incremental display map, large-file mode | ✔ per-line layouts with a Fenwick tree over screen rows; files > 20 MB open without highlighting, analysis and soft wrap, > 500 MB as a read-only 4 MB preview |
| Vim mode prepared in the action system | Partial: every text command is an action with an id and a dedicated key context (`FerroEditor > Input`) a modal layer can bind to; the modal layer itself is P2 |

**Definition of Done:**

| Criterion | How it is checked | Result |
|---|---|---|
| Property-based tests for rope and undo | `crates/ferro_text/tests/properties.rs`: buffer vs a `String` model with undo/redo through every state, anchors, incremental display map vs a fresh one, encodings, line separators, search | ✔ 7 properties, 256 cases each |
| Fuzzing of editing operations | `ferro_text::fuzz::run` turns bytes into editor commands (typing, deletion, cursors, line ops, comments, clipboard, undo/redo, column selection) and checks invariants after each; run by proptest (96 sessions × up to 200 commands) | ✔ (see deviations for `cargo fuzz`) |
| Typing latency within budget on a 100k-line file, weak profile | `tests/typing_latency.rs` (asserts p99 < 8 ms debug / 2 ms optimized) and `cargo run -p ferro_text --release --example typing_latency` | Core per keystroke, release, this machine: p50 4.6 µs, p99 0.82 ms (a line break rebuilds the row index, O(lines)); initial layout of 100k lines 128 ms. Weak-profile and key-to-pixel runs pending |

Tests: `ferro_text` has 64 unit tests, 7 property tests and the latency
test. `ferro_workspace/src/tests_files.rs` adds 7 headless tests (saving in
the original format, encodings and separators, changes on disk, project
files, closing modified tabs, line commands, Ctrl+D with typing, shortcuts).

### Deviations from the plan

- **The view is still GPUI Kit's editor.** It exposes only its active
  selection, so line commands (move, duplicate, comment) act on the first
  cursor when there are several; Select Next Occurrence keeps its own list.
  GPUI Kit keeps its own undo stack and display map; `ferro_text`'s are used
  by the core and its tests and are ready for `ferro_editor`.
- **`ropey` 2.0 beta instead of 1.x**: the exact version GPUI Kit's editor
  uses, so both share one rope type and snapshots pass without copying.
- **No `cargo fuzz` target**: it needs a nightly toolchain; the same driver
  runs under proptest on stable. A `cargo fuzz` target can call
  `ferro_text::fuzz::run` unchanged.
- **Closing a modified tab saves it** (as IntelliJ IDEA does), also with
  autosave set to "Only on Save and when a tab closes"; there is no
  "Save changes?" prompt.
- **External changes** are noticed when the window regains focus and through
  project-service events; a file watcher comes with `FsProjectService`
  (Stage 4).

### Open items

- ~~The `ferro_workspace` changes were not compiled on the development
  machine~~ (antivirus locks new build-script executables under `target/`).
  They are now built and tested by CI on all four platforms; the wrong
  import path of the multi-cursor actions that broke every CI job since
  51796da was fixed, and the debug typing-latency check got the full 16 ms
  budget (the macOS Intel runner measured p99 ≈ 8.7 ms).
- Weak-profile and key-to-pixel measurements on the 100k-line file.
- `ferro_editor`: our own view on top of `ferro_text` (true inline hints,
  all cursors visible to commands, one undo stack).

## Stage 4 — Project, files and state

FerroIDE opens folders from disk. A project opened with **Open…**, from the
welcome screen or as `ferroide <folder>` runs on `FsProjectService` and
`FsSearchService`; the other services (language, VCS, run, debug, terminal)
stay mocks until their stages. `--demo` still opens the mock project.

| Plan item | Delivered |
|---|---|
| `FsProjectService`: background scanning with `ignore`, `notify` watcher with a debouncer, file operations, delete to the system trash | ✔ new crate `ferro_project`: listings respect `.gitignore` / `.ignore` / `.git/info/exclude` and hide `.git`; a parallel scan builds a `FileIndex` that the watcher keeps current; the watcher coalesces a burst into one event per path (50 ms debounce) and drops ignored paths; create, rename / move, delete to the trash (`trash`), atomic saves (`ferro_text::file::write_atomic`) |
| Workspace state in SQLite: open files, layout, cursor positions, Back / Forward, recent files, Recent Locations | ✔ new crate `ferro_db` (`rusqlite`, bundled SQLite, versioned migrations): the dock layout (with the open tabs), carets per file, Back / Forward, recent files (seed Quick Open), visited locations, recent projects (welcome screen, with the branch from `.git/HEAD`). Stage 1–3 JSON layouts are imported once. The Recent Locations *popup* is not built yet (see deviations) |
| Settings: global and project-level, JSON Schema with suggestions, live reload, migrations | ✔ `.ferroide/settings.toml` overrides `[editor]` per project; `settings.schema.json` generated with `schemars` plus a `#:schema` directive; `version` key with `ferro_settings::migrate`; `settings.toml`, `keymap.toml` and project settings reload when edited outside the IDE |
| Keymap persistence (`[keymap] base`, `keymap.toml`, live reload) | ✔ (base and overrides were stored since Stage 2; live reload is new) |
| Multiple root directories | ✔ `ProjectService::add_root` / `remove_root`, **File ▸ Add Folder to Project…**, the tree shows each root; search prefixes paths with the root name; reopened with all roots |
| Trusted projects | ✔ a folder never trusted opens in safe mode with a bar (**Trust Project** / **Stay in Safe Mode**); Run / Debug are refused until it is trusted; trust is stored per folder and covers subfolders |

**Definition of Done:**

| Criterion | How it is checked | Result |
|---|---|---|
| A 100k-file project opens in < 1 s, first tree level immediately | `ferro_project` tests `a_large_project_shows_its_first_level_immediately` (20k files, every CI run) and `hundred_thousand_files_open_within_a_second` (`cargo test -p ferro_project --release -- --ignored hundred_thousand`) | Linux (WSL2, ext4), release: first level **0.4 ms**, full index of 100k files **120 ms** |
| Changes on disk visible within < 200 ms | `outside_changes_arrive_within_the_budget` (create, modify, delete by another program; asserts < 1 s on CI) | Linux: **~52 ms** for each kind of change |

Tests: `ferro_project` 8 (+2 on request), `ferro_db` 8, `ferro_settings` 8
(3 new), and `ferro_workspace/src/tests_projects.rs` with 5 headless window
tests: a folder from disk (tree, README, Quick Open), safe mode and trust,
project settings with live reload, adding and removing root folders, and
layout / carets / Back / recent files surviving a reopen.

### Deviations from the plan

- **Recent Locations popup** (IntelliJ Ctrl+Shift+E): visits are recorded
  in the database, but the popup itself comes with the navigation work of
  Stage 7; adding a mode without a prefix to Quick Open needs its parser
  reworked.
- **LSP and tasks in safe mode**: language servers and tasks are still
  mocks, so only Run / Debug check trust today; the real ones (Stages 6 and
  8) must call `projects::is_trusted`.
- **Ignored files are hidden** from the project tree instead of being shown
  dimmed (`ProjectOptions::show_ignored` exists, without a UI switch yet).
- **The watcher is recursive** over the whole root, including ignored
  directories (one inotify watch per directory on Linux); the events are
  filtered afterwards. Large `node_modules` / `target` trees can reach the
  inotify limit; watching only non-ignored directories is a later
  optimization. Nested `.gitignore` files apply to listings and the scan,
  only the root's to watcher events.
- **"Suggestions" from the JSON Schema** come from TOML editors that read
  the `#:schema` directive (Taplo); FerroIDE's own completion for
  `settings.toml` needs the LSP layer (Stage 6).
- **VCS, run configurations and diagnostics** of a project from disk are
  still the demo mocks.

### Open items

- Measure the 100k-file DoD on Windows (NTFS + antivirus) and macOS.
- The Stage 4 window tests run in CI only (the development machine cannot
  build GPUI, see Stage 3).

## Stage 5 — Syntax and Language Packs v1 (milestone M2 "Fast editor")

Two new GUI-free crates and a `languages/` folder of built-in packs. GPUI
Kit's editor highlights with the packs' grammars and queries; a syntax tree
per editor tab, kept current on a background thread, serves everything
else. Projects opened from disk get a tree-sitter `LanguageService` instead
of the mock until language servers arrive (Stage 6).

| Plan item | Delivered |
|---|---|
| `ferro_syntax`: tree-sitter 0.26, incremental parsing, highlight queries, injected languages | ✔ `SyntaxTree::reparse` finds the edit by comparing the old and new rope (common prefix / suffix) and reparses incrementally; `highlights(range)` for a visible range, innermost capture wins; injections parsed on first use, nested up to four levels (`#set! injection.language`, `@injection.language`, `injection.combined`): HTML + JavaScript + CSS, code blocks in Markdown (by fence info string or alias), SQL in Python / Rust / Go strings |
| File structure, code folding, bracket matching, auto-pairs, smart indentation, Expand / Shrink Selection | ✔ outline from `outline.scm` or `tags.scm` (Structure tool window, breadcrumbs); folds from `folds.scm` (bracketed multi-line nodes otherwise) applied to the editor's fold gutter; the bracket pair at the caret is framed; auto-closing pairs (never in strings or comments) and Enter indentation patterns come from the manifest; `indents.scm` drives the new **Auto-Indent Lines**; **Expand / Shrink Selection** (VS Code Shift+Alt+→/←, JetBrains Ctrl+W / Ctrl+Shift+W, Zed, Sublime, Emacs; macOS variants), also inside injected languages |
| `ferro_languages`: manifest, detection (extension, file name, shebang, modeline), registry of built-in packs, grammars on demand (also WASM) | ✔ the Appendix B manifest (servers, debugger, run templates and test frameworks are parsed for later stages); detection by modeline (Vim / Emacs), file name, longest extension, shebang (version suffixes ignored), `first_line` regex; 20 built-in packs + Markdown's inline grammar, embedded at build time; user packs from `<config>/languages/` replace built-in ones; a pack's grammar and queries are built when its first file opens. WebAssembly grammars: see deviations |
| Fallback navigation: symbol index from `tags.scm`, ▶ icons from `runnables.scm` | ✔ `SyntaxLanguageService`: Go to Symbol in Workspace over a project index of definitions (re-indexed per file when its modification time changes); ▶ for `main` and tests in Rust, Python, Go, Java, C#, C/C++ (`main`), JavaScript / TypeScript (`test` / `it` / `describe`), Ruby (RSpec, Minitest). Also: syntax errors as diagnostics (Problems view), completion of the grammar's keywords and the file's words |

Languages (20): Rust, Python, JavaScript, TypeScript, TSX, Go, Java, C, C++,
C#, HTML, CSS, JSON, TOML, YAML, Markdown, Shell (bash/sh/zsh), Ruby, Lua,
SQL.

**Definition of Done (M2):**

| Criterion | How it is checked | Result |
|---|---|---|
| 15+ languages with highlighting | `ferro_languages` tests `every_builtin_pack_loads_and_compiles`, `fifteen_plus_languages_highlight` (keywords, strings, comments per language); `ferro_workspace` `every_pack_compiles_in_the_editor_highlighter` (GPUI Kit compiles each pack and alias) | ✔ 20 languages |
| Incremental parsing < 2 ms per edit, 10k-line file, medium profile | `incremental_parse_of_a_ten_thousand_line_file_fits_the_budget` (asserts the median < 2 ms optimized, < 25 ms debug); `cargo run -p ferro_languages --release --example reparse` | Windows, release, this machine: code in modules and `impl` blocks — typing in a name **median 145 µs**, p90 0.19 ms; typing that makes a syntax error 3.6 ms. Worst case, ~1,700 functions at the top level: 3.9–5.4 ms per edit (tree-sitter alone, measured with a plain `&str` too). Full parse of the 10k-line file 60–70 ms (on a background thread) |
| Suitable for everyday editing as a fast editor | Highlighting, structure, folding, brackets, auto-pairs, indentation, selection and the ▶ icons work without any language server | ✔ |

Tests: `ferro_languages` 8 unit + 12 syntax + 4 service tests;
`ferro_workspace/src/tests_syntax.rs` 6 headless window tests (every pack in
GPUI Kit's highlighter, detection by shebang / modeline / extension, comment
syntax from the pack, Expand / Shrink Selection, Auto-Indent Lines, the
tree-sitter service on a project from disk).

### Deviations from the plan

- **WebAssembly grammars** are recognized in manifests
  (`source = { wasm = "…" }`) but report "needs the extension host": loading
  them needs tree-sitter's `wasm` feature, i.e. wasmtime, which arrives
  with the extension host (Stage 11). Grammars named by `git` must match a
  built-in grammar for now.
- **Two parses per open file**: GPUI Kit's highlighter keeps its own tree
  for drawing; `ferro_syntax` keeps another for structure, folds, brackets
  and selection. They share grammars and queries. One tree for both comes
  with `ferro_editor`.
- **Smart indentation on Enter** uses the manifest's regex patterns (GPUI
  Kit's editing rules); the tree-based indentation of `indents.scm` runs on
  demand (Auto-Indent Lines), not on every Enter.
- **Folds** from `folds.scm` replace GPUI Kit's generic ones after each
  parse; right after an edit, GPUI Kit's own candidates show until the
  background parse finishes.
- **Incremental parsing in flat files**: files with thousands of top-level
  items (generated code) take 4–5 ms per edit in tree-sitter itself; the
  editor reparses on a background thread, so typing is not blocked.
- **Completion** without a language server offers keywords and words of
  the file; rename, hover, signature help, quick fixes and inlay hints
  need a language server (Stage 6). `--demo` keeps the mock language
  service.

### Open items

- Weak-profile measurement of the reparse budget.
- More packs (Kotlin, PHP, Swift, Dockerfile, Makefile…) as grammars are
  added; a pack registry to download them (Stage 11).

## Stage 6 — Code intelligence via LSP

A new GUI-free crate, `ferro_lsp`, is the language server client. A project
opened from disk now runs on `LspLanguageService`: the Language Pack of a
file names its servers (`[[language_servers]]`), they start on the first
request for one of their files, and the tree-sitter service of Stage 5
answers whenever no server can (none installed, untrusted project, crashed
for good, a language without one). `--demo` keeps the mock, which learned
Go to Definition and Find Usages.

| Plan item | Delivered |
|---|---|
| `ferro_lsp`: JSON-RPC over stdio, queues, cancellation, timeouts | ✔ `rpc`: `Content-Length` framing; a writer thread drains the outgoing queue (a busy server never blocks the caller), a reader thread matches responses; every request has a timeout, and a request future dropped before its answer sends `$/cancelRequest` (a newer completion, a closed popup) |
| Restart after a crash | ✔ the next request restarts a crashed server (at most three times in three minutes, then the fallback takes over and the status bar shows it crashed); its diagnostics are cleared |
| Multiple servers per file | ✔ a feature goes to the servers whose manifest lists it, or to all of them when none does (Python: basedpyright for navigation, ruff for formatting and fixes, both for diagnostics); completion and diagnostics are merged |
| Push and pull diagnostics | ✔ `publishDiagnostics` updates the editor through a new `LanguageService::subscribe` event (no polling); `textDocument/diagnostic` for servers that pull; the Problems view refreshes after a burst |
| `workspace/configuration`, client-side file watching | ✔ answered from `[lsp.servers.<id>.settings]` (dotted sections); watchers registered with `client/registerCapability` (globs, relative patterns, kinds) get the project watcher's events as `didChangeWatchedFiles`; also `workspace/applyEdit`, `workspaceFolders`, refresh requests, progress, `showMessage` |
| Document synchronization | ✔ every request carries the editor's text; the server gets `didOpen` or one incremental `didChange` (common prefix and suffix, in the negotiated UTF-16 / UTF-8 / UTF-32 encoding); `didSave`; `didClose` when a tab closes |
| Completion with resolve, snippets and auto-import | ✔ the first ten items are resolved (documentation, `additionalTextEdits`); an import edit is merged with the insertion into one edit; snippets are inserted as text (see deviations); items replace the typed word (GPUI Kit appended `insert_text` after it) |
| Quick Documentation, Parameter Info | ✔ hover, signature help (parameter offsets in UTF-16) |
| Go to Declaration / Type / Implementation | ✔ new actions Go to Definition, Declaration, Type Definition, Implementation; Ctrl+click and the Ctrl-hover underline through GPUI Kit's definition provider; one target opens (with Back), several go to a list |
| Find Usages with preview | ✔ a locations list (`locations.rs`): grouped by file, ±3 lines of context, Enter / double click opens |
| Rename with preview (F2) | ✔ the Stage 2 dialog on `textDocument/rename`; files that are not open are now written to disk (they were skipped) |
| Quick Fix / code actions (Ctrl+.) | ✔ code actions resolved lazily (`codeAction/resolve`) when chosen; commands run with `workspace/executeCommand`, and the edits the server applies meanwhile are applied too; edits to other files go to their editors or to disk |
| Formatting (document / range / on typing), Optimize Imports | ✔ Format Document (the selection when there is one), Optimize Imports (`source.organizeImports`); on-typing formatting is in the service but not triggered (see deviations) |
| Inlay hints, semantic tokens | ✔ inlay hints from the server (drawn at the end of the line, as in Stage 2); semantic tokens layered over tree-sitter through GPUI Kit's semantic-token provider, mapped to the theme's styles |
| Document and workspace symbols | ✔ Structure, breadcrumbs and `@` from `documentSymbol`; `#` / Go to Symbol in Workspace merges `workspace/symbol` of the running servers with the tree-sitter index |
| Call and type hierarchies | ✔ Call Hierarchy (callers) and Type Hierarchy (supertypes ↑, subtypes ↓) in the locations list |
| Code lens, links in documents | Partial: requested and converted by the service (`code_lenses`, `document_links`), not drawn (see deviations) |
| Problems panel, F8 / Shift+F8, linters as additional servers | ✔ panel and navigation from Stage 2, now with real diagnostics; linters run as extra servers of a pack (ruff) |
| Live Templates and Postfix Completion | ✔ `templates.rs`: built-in templates for Rust, Python, JavaScript / TypeScript, Go, Java, C / C++ and C#, offered by completion; postfix templates wrap the expression before the dot (`x.if`, `list.for`, `value.ok`); bodies in the LSP snippet format |
| Server installer | ✔ Code ▸ Install Language Server installs the active file's missing (or failing) servers into `<data>/servers/<id>` with npm, pip (a virtual environment), `go install`, `cargo install`, gem, `dotnet tool` or rustup; commands are looked up there first, then on `PATH`. Release archives: see deviations |
| LSP memory manager, power save mode | ✔ governor: `[lsp] max_servers` (default 6; the least recently used server stops), `idle_minutes` (30; restarts on demand); the status bar lists the real servers (starting / ready / crashed); Power Save Mode (Code menu, `[lsp] power_save`) skips inlay hints, semantic tokens and code lens |
| Safe mode (Stage 4 open item) | ✔ no server starts in an untrusted project; trusting it starts them, withdrawing trust stops them |

New actions, bound in every keymap scheme where the editor has one (VS
Code: F12, Ctrl+F12, Shift+F12 / Shift+Alt+F12, Shift+Alt+H, Shift+Alt+F,
Shift+Alt+O; JetBrains: Ctrl+B, Ctrl+Alt+B, Ctrl+Shift+B, Alt+F7,
Ctrl+Alt+H, Ctrl+H, Ctrl+Alt+L, Ctrl+Alt+O; Zed, Sublime Text, Emacs;
macOS variants), in the Navigate and Code menus and the Command Palette:
Go to Definition / Declaration / Type Definition / Implementation, Find
Usages, Call Hierarchy, Type Hierarchy, Format Document, Optimize Imports,
Install Language Server, Restart Language Servers, Power Save Mode.
Settings: a new `[lsp]` section (`max_servers`, `idle_minutes`,
`power_save`, `[lsp.servers.<id>]` with `enabled`, `command`, `args`,
`settings`, `initialization_options`), in the JSON Schema and applied when
`settings.toml` changes.

**Definition of Done:**

| Criterion | How it is checked | Result |
|---|---|---|
| Complete inner loop in Rust, Java, Python, TypeScript and Go | `crates/ferro_lsp/tests/real_servers.rs`: per language, hover, Go to Definition, completion, references, an error diagnostic and rename against the real server | TypeScript (vtsls) ✔ on Windows 11 here; rust-analyzer, basedpyright and gopls in the new CI job (not run on GitHub yet); Java (jdtls) has a test but no CI job yet (see open items) |
| Integration tests with real servers in CI | CI job `lsp` installs rust-analyzer (rustup), basedpyright (pip), vtsls (npm) and gopls (`go install`) in the installer's layout and runs those tests with `FERRO_LSP_REQUIRE` | Job added; a GitHub runner with the servers installed instead of container images |

Tests: `ferro_lsp` has unit tests (framing, URIs, UTF-16 / UTF-8 columns,
incremental changes, snippets, templates, conversions, install plans) and
`tests/service.rs` against an in-process fake server (`ferro_lsp::testing`,
feature `test-support`): synchronization after edits around an emoji,
pushed diagnostics as events, auto-import completion, lazy quick fixes and
commands with `applyEdit`, configuration, watchers, save and close, crash
and restart limits, safe mode, the governor, cancelled requests, power
save. `ferro_workspace/src/tests_lsp.rs` drives the window on the fake
server: diagnostics in the editor and the status bar, Go to Definition, the
Find Usages list, Format Document, `didSave`, edits to a closed file
written to disk, and an untrusted project starting its server once
trusted.

### Deviations from the plan

- **Snippets have no tab stops**: GPUI Kit's editor has no snippet session,
  so a snippet (from a server or a template) is inserted with its
  placeholders' default text and the caret ends after it.
- **Completion resolve is eager and limited**: GPUI Kit never asks to
  resolve an item, so the service resolves the first ten items of each
  answer (documentation, auto-import edits) within 800 ms.
- **Code lens and document links are not drawn**, and **on-typing
  formatting is not triggered**: GPUI Kit's editor cannot insert virtual
  lines or clickable ranges and does not report which character was typed.
  The service methods exist (`code_lenses`, `document_links`,
  `format_on_type`, `format_on_type_triggers`) for `ferro_editor`.
- **Hierarchies are one level deep**, in the locations list rather than an
  expandable tree tool window.
- **The installer does not download release archives** (clangd, jdtls,
  lua-language-server, marksman, taplo, standalone rust-analyzer): it says
  where to get them. Package managers verify their own downloads; there is
  no extra checksum. A deep install folder can hit Windows' path limit with
  pip (basedpyright bundles Node.js); `<data>/servers` is short enough.
- **Per-server memory** is read on Linux only (`/proc`); elsewhere the
  status bar shows the server without it.
- **Changed server settings** apply to servers started afterwards; Restart
  Language Servers applies them to running ones (no
  `didChangeConfiguration` on edit).
- **A rustup proxy without the component** looks installed: the server
  fails to start, the failure is kept with its last stderr line, and
  Install Language Server offers `rustup component add rust-analyzer`.

### Open items

- First run of the `lsp` CI job; a Java job (JDK and a jdtls download).
- `ferro_editor`: snippet sessions, code lens, links, on-typing
  formatting, inline hints.

## Stage 7 — Navigation and search

A new GUI-free crate, `ferro_index`, is the project index and the search
engine. A project opened from disk now runs on `ProjectIndex` as its
`SearchService` (it replaces Stage 4's `FsSearchService`), and the
tree-sitter fallback answers Go to Symbol from the index instead of parsing
the project on every query. `--demo` keeps the mocks.

| Plan item | Delivered |
|---|---|
| Quick Open on real data: files (nucleo) | ✔ `ferro_index::fuzzy` on `nucleo-matcher`: nucleo's query syntax (words, `^`, `$`, `'`, `!`), path bonuses, a file-name bonus for the last word; the next keystroke rescores only what the previous query matched; above 20k candidates the scoring runs on several threads |
| Classes and symbols (index + LSP `workspace/symbol`) | ✔ symbols of every file from the Language Packs' `tags.scm`/`outline.scm`, stored in the index; `#` / Go to Symbol in Workspace merges them with `workspace/symbol` of the running servers (Stage 6) |
| Actions, settings, text | ✔ `>` lists commands and, for a non-empty query, settings (Enter opens the Settings window searched for it); `%` is text search over the index |
| Find / Replace in Files: ripgrep libraries, regex, file masks | ✔ `grep-searcher` + `grep-regex` on whole files read in parallel (a byte-regex prefilter rejects non-matching files first); masks `*.rs, src/**, !target/**` (`globset`; plain text still means "path contains") |
| Scopes (project, directory, scope) | ✔ Project, Directory (a field, starting at the active file's folder), Open files, and named scopes from `[search.scopes]` (`tests = "tests/**, *_test.go"`) |
| Preview and replace with undo in one operation | ✔ the preview shows each match before → after with regex groups (`$1`, `${name}`); Replace All changes open files in their editors and the others on disk, and records the operation: **Navigate ▸ Undo Replace in Files** (`ferro::UndoReplaceInFiles`) restores every file still as the replacement left it and reports those changed since |
| `ferro_index`: persistent file and symbol index in SQLite | ✔ `<data>/index/<name>-<hash>.sqlite` per set of roots (files, symbols, TODO comments, trigrams); loaded at startup, so Quick Open (the stored file list), Go to Symbol and TODO answer before the scan ends; a different schema version is rebuilt, never migrated |
| Updated incrementally in the background, paused while typing | ✔ after the scan, size and modification time of every file are compared with the database, changed files are re-indexed and missing ones forgotten; then the watcher's events keep it current. Every keystroke in an editor pauses indexing for 500 ms (`SearchService::user_activity`). Progress is the status bar's background task |
| Optional trigram index for repositories above a few GB | ✔ `[search] trigram_index = "auto"` (on above 2 GB of files), `"on"`, `"off"`; literal queries search only files holding all their trigrams, plus files changed or not indexed yet. Regex queries scan everything |
| TODO panel | ✔ from the index; refreshes itself when indexed data changes |
| Bookmarks (Ctrl+Alt+K) | ✔ Toggle / Next / Previous Bookmark and **Bookmarks…** (a Quick Open list); stored per project in the state database (migration 2), drawn as a bar in the glyph margin. VS Code: Ctrl+Alt+K / L / J (the Bookmarks extension's keys); JetBrains: F11, Shift+F11 (macOS F3, ⌘F3); Sublime Text: Ctrl+F2, F2, Shift+F2 |
| Recent Locations | ✔ **Navigate ▸ Recent Locations…** (JetBrains Ctrl+Shift+E / ⌘⇧E): a Quick Open list of visited places, newest first, with the line's text; Quick Open gained modes without a prefix for this and Bookmarks |

**Definition of Done — the §6.2 budgets for Quick Open and Find in Files:**

| Budget | How it is checked | Result (Windows 11, 12 threads, release, antivirus on) |
|---|---|---|
| Quick Open, 100k files, < 30 ms (medium) / < 60 ms (weak) per keystroke | `cargo run -p ferro_index --release --example search_bench` types five queries character by character over 100k synthetic monorepo paths with `PathMatcher` (the code behind Quick Open) | p50 4.1 ms, p99 14.9 ms, worst keystroke 17.7 ms ✔ |
| Find in Files, 1 GB repository, < 1 s (medium) / < 3 s (weak) | the same example with `--mb 1024`: 20,971 files of 50 kB, warm OS cache, median of three runs; it also measures reading every file without searching | full scan (no match) 0.82–1.48 s between runs, reading alone 0.6–1.0 s on this machine; a query that fills the 2,000-line result limit 0.64–0.85 s. Weak profile ✔; medium profile at the budget, bounded by file I/O here (see open items) |

Tests: `ferro_index` 19 unit and headless tests (masks, replacement with
groups, parallel search in path order with the limit, nucleo ranking and
narrowing, trigrams; on real folders: symbols and TODO, ignored files,
surviving a restart and catching up with changes made while closed, edits
and removals followed live, scopes and masks, the trigram index, the typing
pause). `ferro_db` bookmarks; `ferro_workspace/src/search.rs` bookmark
order; `tests_search.rs` drives the window: Replace All on open and closed
files and its undo, regex groups, scopes, bookmarks, Recent Locations,
settings in the Command Palette.

### Deviations from the plan

- **Find in Files searches files on disk**, not unsaved editor text
  (autosave writes it when focus leaves the editor).
- **Files that are not UTF-8 are skipped by Replace All** rather than
  re-encoded; the notification counts them.
- **Bookmarks keep their line number**: they do not follow lines inserted
  above them until `ferro_editor` brings anchors to the view.
- **No Bookmarks tool window**: the list is a Quick Open mode.
- **The trigram index narrows literal queries only**; required literals of
  a regular expression are not extracted.
- **Stage 4's `FsSearchService` stays** in `ferro_project` (tests, tools)
  but windows no longer use it.

### Open items

- Medium-profile Find in Files on a machine without real-time antivirus
  scanning, and both budgets on the weak profile and on Linux/macOS.
- Release builds on this machine run out of memory with full parallelism
  (`cargo build -j 2` works); the benchmark was built without LTO.
