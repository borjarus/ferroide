# ADR-005: Extension strategy — WebAssembly components with capabilities

- **Status:** Accepted (implementation in Stage 11)
- **Date:** 2026-11-06
- **Stage:** 0

## Context

FerroIDE must be extensible (languages, themes, tools) without letting an
extension crash the IDE, slow its startup, or access the machine
unchecked. The architecture has to anticipate this from Stage 1, even though
the host is built in Stage 11.

## Decision

1. **Extensions are WebAssembly components** run by `wasmtime` (LTS
   release), with an API described in WIT and an SDK crate
   (`ferro_extension_api`) generated with `wit-bindgen`.
2. **Capabilities, not ambient access.** Network, process spawning and file
   system access are separate permissions declared in the manifest and
   approved by the user. Memory and fuel/time limits apply per call.
3. **Data, not UI code.** Extensions register Language Packs, LSP/DAP
   adapter configurations, actions, themes, snippets, run templates and
   *data-driven* panels. They cannot run arbitrary GPUI code; the IDE renders
   what they describe. This keeps the UI thread safe and the look consistent.
4. **Everything is an action** (`ferro_keymap` catalog): extension commands
   are actions with namespaced ids, so they appear in Find Action, menus and
   keymaps with no special casing. Stage 1 already routes every feature
   through this catalog.
5. **Lazy activation.** An extension loads when its trigger fires (file
   type, action, project marker), never at startup.

## Consequences

- Built-in Language Packs can move to the same format (Stage 11 DoD: at
  least five).
- An extension fault becomes a notification, not a crash.
- Rich custom UI from extensions is out of scope for 1.0.

## Alternatives considered

- **Native dynamic libraries:** fastest, but no isolation and an unstable
  Rust ABI; rejected.
- **JavaScript runtime (VS Code style):** large runtime, memory cost on weak
  machines; rejected.
- **Out-of-process extensions over RPC:** good isolation but higher memory
  per extension; may be used later for heavy tools only.
