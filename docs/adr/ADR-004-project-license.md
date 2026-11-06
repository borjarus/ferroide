# ADR-004: Project license — Apache-2.0 OR MIT

- **Status:** Accepted (reinstated; the interim decision in [ADR-005](ADR-005-apache-only-license.md) was reverted)
- **Date:** 2026-11-06
- **Stage:** 0

## Context

FerroIDE builds on GPUI and GPUI Kit (Apache-2.0) and on many crates under
MIT, Apache-2.0, BSD, ISC, Zlib and MPL-2.0. Zed's editor and workspace
crates are GPL/AGPL and must not be used or copied (plan §2.2). Extension
authors and downstream packagers need a license that is easy to combine.

## Decision

- FerroIDE is dual-licensed **Apache-2.0 OR MIT**, the Rust ecosystem
  default (`LICENSE-APACHE`, `LICENSE-MIT`, `license` in `Cargo.toml`).
- Contributions are accepted under the same terms (inbound = outbound,
  stated in `CONTRIBUTING.md`); no CLA.
- Dependencies are limited to the permissive list in `deny.toml`, enforced
  by `cargo deny` in CI. GPL/AGPL are blocked. MPL-2.0 (e.g. `nucleo`) is
  allowed as a file-level copyleft that does not affect FerroIDE's code.
- Every Language Pack's third-party tools get a license review (e.g. the
  Microsoft `cpptools` debug adapter is not used; plan §7.2).

## Consequences

- Compatible with GPUI's Apache-2.0, including its patent grant.
- Commercial redistribution and extensions under any license are possible.

## Alternatives considered

- **Apache-2.0 only:** incompatible with GPLv2-only downstream projects.
- **GPL/AGPL:** would forbid proprietary extensions and some packaging; also
  inconsistent with the decision not to depend on Zed's GPL crates.
