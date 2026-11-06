# ADR-005: Project license — Apache-2.0 only (reverted)

- **Status:** Rejected (reverted the same day; ADR-004 stands)
- **Date:** 2026-11-06
- **Stage:** 0

## Context

ADR-004 chose the dual license Apache-2.0 OR MIT. On the same day the project
owner asked for Apache-2.0 alone, and a single-license change was made.

## Decision

Reverted. FerroIDE stays **Apache-2.0 OR MIT** as decided in
[ADR-004](ADR-004-project-license.md), so the project follows the Rust
ecosystem convention. `LICENSE-APACHE` and `LICENSE-MIT` are both present,
and `license = "Apache-2.0 OR MIT"` is set in the workspace `Cargo.toml`.

## Consequences

- No change to dependency policy (`deny.toml`, `cargo deny` in CI).
- The original `LICENSE-MIT` file was removed and then recreated from the
  standard MIT text. Its copyright line uses the git author name
  (Mirosław Puczyński, 2026). Verify it against the original before release.
