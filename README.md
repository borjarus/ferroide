# FerroIDE

One universal IDE for every popular language — IntelliJ IDEA's ergonomics
with Zed's speed — written entirely in Rust with a GPU-rendered interface,
for Linux, Windows and macOS, including weaker computers.

> **Status: Stage 4 of 14.** FerroIDE opens folders from disk: the
> project tree, Quick Open and Find in Files work on real files (respecting
> `.gitignore`), changes made by other programs appear immediately, and the
> window remembers its layout, carets and history. The editor core
> (`ferro_text`) reads and saves files in any common encoding. Language
> servers, Git, running and the debugger are still mocks until their stages
> — see the [plan](docs/FerroIDE_plan.md) and [status](docs/STATUS.md).

## Try it

```sh
cargo run                     # welcome screen
cargo run -- path/to/project  # open a folder (several folders: several roots)
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
  ferro_project/    real project files: listing, .gitignore, watcher, file
                    operations, file index and search (no GUI)
  ferro_db/         workspace state in SQLite (no GUI)
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
