# FerroIDE

One universal IDE for every popular language — IntelliJ IDEA's ergonomics
with Zed's speed — written entirely in Rust with a GPU-rendered interface,
for Linux, Windows and macOS, including weaker computers.

> **Status: Stage 3 of 14.** The complete IDE window runs on mock services,
> and the editor core (`ferro_text`) reads and saves real files in any
> common encoding and line separator, with multi-cursor and line commands.
> Real project scanning, language servers, Git and the debugger arrive in
> later stages — see the [plan](docs/FerroIDE_plan.md) and
> [status](docs/STATUS.md).

## Try it

```sh
cargo run                     # welcome screen
cargo run -- --demo           # open the demo project directly
cargo run -- --demo --lang pl # in Polish
cargo run -p ferro_storybook  # component gallery, incl. slow/failing mocks
```

On Linux, install GPUI's system libraries first: `./script/linux-deps.sh`.

Useful shortcuts (IntelliJ keymap, Windows/Linux): **Ctrl+Shift+A** Find
Action · **Alt+1** Project · **Alt+0** Commit · **Alt+F12** Terminal ·
**Ctrl+Shift+F12** hide all tool windows · **Alt+←/→** switch tabs ·
**Ctrl+F4** close tab · **Shift+F10** Run · **Ctrl+=/−** zoom the UI.

## Repository layout

```text
crates/
  ferro/            the `ferroide` binary
  ferro_workspace/  window, dock, tool windows, editor tabs, toolbar, status bar
  ferro_ui/         themes, icons, shared components
  ferro_services/   service traits + mock implementations
  ferro_text/       editor core: rope, undo, anchors, selections, edits,
                    search, display map, encodings, atomic saving (no GUI)
  ferro_keymap/     action catalog and keymaps (no GUI)
  ferro_settings/   settings and persisted UI state (no GUI)
  ferro_i18n/       Fluent localization (no GUI)
  ferro_storybook/  component gallery
  ferro_perf/       performance spikes and generators
assets/             themes and translations
docs/               plan, ADRs, spike reports
xtask/              `cargo xtask ci`, `cargo xtask spike`
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Architecture decisions are recorded in
[docs/adr](docs/adr/).

## License

Dual-licensed under [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at
your option.
