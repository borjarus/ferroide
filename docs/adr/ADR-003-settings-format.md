# ADR-003: Settings and state formats

- **Status:** Accepted
- **Date:** 2026-11-06
- **Stage:** 0 (implemented in part in Stage 1, completed in Stage 4)

## Context

FerroIDE stores three kinds of data with different owners and lifetimes:

1. **User settings** — written by people, read by the IDE, diffed in dotfile
   repositories and shared within teams.
2. **Window and workspace state** — written by the IDE (layout, open tabs,
   cursor positions); people never edit it.
3. **Indexes and history** — large, written incrementally (symbol index,
   Local History).

## Decision

| Data | Format | Location |
|---|---|---|
| Global settings | TOML | `<config>/FerroIDE/settings.toml` |
| Project settings (Stage 4) | TOML | `<project>/.ferroide/settings.toml` |
| Run configurations (Stage 8) | TOML, committable | `<project>/.ferroide/run/*.toml` |
| Window layout (Stage 1) | JSON (GPUI Kit `DockAreaState`) | `<data>/FerroIDE/layouts/<project>.json` |
| Workspace state, index, Local History (Stage 4+) | SQLite (`rusqlite`, bundled) | `<data>/FerroIDE/` |

- Settings are deserialized with `#[serde(default)]` at every level, so
  unknown keys and missing sections never fail a load; values are clamped by
  `Settings::sanitized`. A broken file is logged and replaced by defaults —
  it must never prevent startup.
- Writes are atomic (temporary file + rename).
- JSON Schema for settings (via `schemars`) and versioned migrations arrive
  in Stage 4. The layout file carries a version (`LAYOUT_VERSION`); a layout
  from another version is discarded rather than mis-restored.
- `<config>` and `<data>` are the platform directories from the `dirs`
  crate (`%APPDATA%`/`%LOCALAPPDATA%` on Windows, `~/Library/Application
  Support` on macOS, XDG on Linux).

## Consequences

- People can read, diff and hand-edit everything they own; machine state
  stays out of their way.
- Stage 1 stores layouts as JSON files instead of SQLite; Stage 4 moves them
  into the workspace database with the rest of the state.

## Alternatives considered

- **JSON for settings** (VS Code style): no comments and noisy diffs.
- **One SQLite database for everything:** not hand-editable, not shareable.
