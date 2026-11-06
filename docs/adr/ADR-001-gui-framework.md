# ADR-001: GUI framework — GPUI through GPUI Kit 0.9

- **Status:** Accepted — **go**, with the conditions below
- **Date:** 2026-11-06
- **Stage:** 0 (go/no-go gate of the plan, §4.1 and Stage 0 DoD)

## Context

FerroIDE needs Zed-class rendering performance (plan §6.2: key → pixel p99
< 8 ms, 120 FPS scrolling, cold start < 400 ms on a medium machine) and a
large set of IDE components (dock, tabs, trees, virtual lists, tables, a code
editor). The plan compared GPUI + GPUI Kit, Iced, Slint, egui and Floem and
proposed GPUI + GPUI Kit, subject to a Stage 0 spike, with Iced 0.14 (and its
tiny-skia software renderer) as plan B.

## What the spike checked

The `ferro_perf` crate holds three spike binaries, run by
`cargo xtask spike` with the `spike` profile (release code generation, thin
LTO):

- `spike_dock` — dock with left, center and bottom panels; the left dock is
  resized every frame (the "dragging panels" case);
- `spike_list` — `uniform_list` with 1,000,000 rows, scrolled ~40 rows per
  frame;
- `spike_editor` — GPUI Kit's code editor with a generated 100,000-line Rust
  file and tree-sitter highlighting: 200 typed characters measured key →
  next frame, then page-down scrolling.

The application itself (`ferroide --demo`) is measured cold start → first
frame, median of five runs. Results for each machine are in
[docs/reports](../reports/); the summary is in
[stage0-spike.md](../reports/stage0-spike.md).

Integration findings, independent of hardware:

- `gpui-kit` 0.9 is a single dependency that re-exports GPUI, `gpui-base`
  and `gpui-component`. Its dock (layout tree, drag and drop, zoom,
  persistence through `DockAreaState`), title bar, menus, command palette,
  virtual lists and code editor covered the whole of Stage 1 without forking.
- Headless UI tests (`#[gpui_kit::test]` + `test-support`) work: Stage 1 has
  11 workspace tests on the real views.
- **Dependency resolution needs care.** GPUI Kit's optional `gpui-fast`
  backend pins exact versions of shared dependencies (e.g. `taffy =0.15.0`).
  A fresh `cargo` resolution picked newer patch releases and later failed to
  resolve once `test-support` was enabled. The fix used: seed `Cargo.lock`
  from the one GPUI Kit ships, then let Cargo add FerroIDE's own crates. Keep
  `Cargo.lock` committed and upgrade GPUI Kit only as a unit (quarterly,
  plan §4.3).
- Windows renders through DirectX 11 (Direct3D 11.1 feature level on an
  Intel Iris Xe); the DXGI debug-layer warning in debug builds is harmless.

## Decision

**Go: GPUI via GPUI Kit 0.9 is FerroIDE's UI framework.** On the machine
measured so far (Windows 11, Iris Xe, 60 Hz display) every scenario holds the
display's refresh rate, memory is well inside budget, and cold start is at
the 400 ms budget (405 ms). The 120 FPS / 8 ms medium-profile budgets could
not be verified because the display caps frames at 60 Hz (see the spike
report). GPUI Kit provides every component Stages 1–2 need.

Conditions — any of these turning out false reopens the decision for plan B:

0. On a ≥ 120 Hz display (or with a vsync-independent CPU-time metric),
   the medium-profile frame and latency budgets are met.
1. The spikes meet the **weak-profile** budgets (2 cores / 4 GB / Intel UHD
   620: start < 1 s, key → frame p99 < 16 ms, 60 FPS) on the reference
   laptop.
2. Linux renders correctly on X11 and Wayland, including an older Intel GPU
   and NVIDIA, through wgpu/Vulkan.
3. Windows over RDP and in a VM without a GPU either renders (WARP) or fails
   with a clear message rather than a blank window.

Owners of these checks run `cargo xtask spike` on each target and commit the
JSON report; the nightly CI job repeats it on Windows and macOS runners.

## Consequences

- Application code imports only `gpui_kit::*`; no direct `gpui` dependency.
- Logic crates stay GUI-free (plan §2.3), which keeps the cost of a framework
  switch bounded to the view layer.
- GPUI is pre-1.0: expect breaking changes at each quarterly GPUI Kit
  upgrade; the upgrade branch must pass the full test and spike suite.

## Alternatives considered

See plan §4.1. Iced 0.14 remains plan B because of its software renderer.
