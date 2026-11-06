# Architecture Decision Records

Each ADR records one decision: its context, the decision, and its
consequences. ADRs are immutable once accepted; a changed decision gets a new
ADR that supersedes the old one.

| ADR | Title | Status |
|---|---|---|
| [ADR-001](ADR-001-gui-framework.md) | GUI framework: GPUI through GPUI Kit | Accepted (go, with conditions) |
| [ADR-002](ADR-002-async-model.md) | Async model: GPUI executor + background Tokio | Accepted |
| [ADR-003](ADR-003-settings-format.md) | Settings and state formats | Accepted |
| [ADR-004](ADR-004-project-license.md) | Project license: Apache-2.0 OR MIT | Accepted |
| [ADR-005](ADR-005-extension-strategy.md) | Extension strategy: WebAssembly components | Accepted (implementation in Stage 11) |

Template: copy an existing ADR; sections are *Context*, *Decision*,
*Consequences*, and *Alternatives considered*.
