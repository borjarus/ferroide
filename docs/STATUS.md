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
