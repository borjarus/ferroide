# Contributing to FerroIDE

Thanks for helping build FerroIDE. This guide covers the workflow and the
rules every change follows. The architecture and roadmap are in
[docs/FerroIDE_plan.md](docs/FerroIDE_plan.md); recorded decisions are in
[docs/adr/](docs/adr/).

## Getting started

```sh
rustup show                  # installs the toolchain pinned in rust-toolchain.toml
./script/linux-deps.sh       # Linux only: system libraries GPUI needs
cargo run                    # the IDE (welcome screen)
cargo run -- --demo          # straight into the demo project
cargo run -p ferro_storybook # component gallery on mock data
cargo xtask ci               # what CI runs: fmt, clippy, tests, cargo-deny
cargo xtask spike            # Stage 0 performance spikes and budget table
```

The first build compiles GPUI and takes a while; later builds are incremental.
Dependencies are built with `opt-level = 2` even in debug, because GPUI in a
plain debug build is too slow to judge.

## Commits and pull requests

- Commit messages and PR titles follow
  [Conventional Commits](https://www.conventionalcommits.org):
  `type(scope): summary`, where `type` is one of `feat`, `fix`, `perf`,
  `refactor`, `docs`, `test`, `build`, `ci`, `chore`, and `scope` is usually a
  crate without the `ferro_` prefix (`feat(workspace): pin editor tabs`).
  The changelog is generated from them.
- Keep pull requests focused. Fill in the template, including how you tested.
- Every PR runs the benchmarks; a performance regression above 5% blocks the
  merge until it is explained or fixed.

## Engineering rules

These come from the plan (§2.3) and reviews enforce them:

1. **GUI-first on contracts.** Views depend on the traits in
   `ferro_services`, never on concrete implementations. New data a view needs
   goes into a trait, with a mock first.
2. **Logic without GUI.** Crates that are not UI (`ferro_services`,
   `ferro_settings`, `ferro_keymap`, `ferro_i18n`, and later `ferro_text`,
   `ferro_lsp`, …) must not depend on `gpui-kit`.
3. **The UI thread is untouchable.** No file or network I/O, no blocking and
   no heavy computation on it. Use `cx.background_spawn`.
4. **Everything lazy, everything virtualized.** Large lists use
   `uniform_list` / virtual lists; panels load data on first use.
5. **Every feature is an action.** Add it to the catalog in
   `crates/ferro_keymap/src/catalog.rs`, define the GPUI action in
   `crates/ferro_workspace/src/actions.rs`, and (if it has a default
   shortcut) bind it in the keymaps. Tests fail if these drift apart.
6. **Strings are translated.** UI text goes through `ferro_ui::tr` with a
   message in both `assets/locales/en/ferro.ftl` and
   `assets/locales/pl/ferro.ftl`; a test checks the two catalogs match.
7. **`unsafe`** is denied by default. Where it is truly needed, allow it
   locally with a `// SAFETY:` comment, and get a second reviewer.

## Dependencies

- Only permissive licenses (see `deny.toml`). Never copy code from GPL/AGPL
  crates, including Zed's editor and workspace crates.
- Add dependencies with `cargo add` (latest stable) and commit `Cargo.lock`.
- GPUI Kit is upgraded once a quarter in its own branch with the full
  performance suite; Dependabot ignores it.

## Code of conduct

Participation is governed by our [Code of Conduct](CODE_OF_CONDUCT.md).

## License

By contributing you agree that your work is dual-licensed under
[Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at the user's option.
