//! `ferro_terminal`: terminal sessions on real pseudoterminals (plan §8,
//! Stage 8), behind the [`TerminalService`](ferro_services::TerminalService)
//! contract.
//!
//! - [`PtyTerminalService`]: sessions running a shell on a PTY (ConPTY on
//!   Windows) through `alacritty_terminal`'s `tty` module; one I/O thread
//!   per session ([`pty`] module docs), events coalesced so a flood of
//!   output sends few `Changed` events.
//! - [`TerminalCore`]: the emulator without a PTY: `alacritty_terminal`'s
//!   grid, scrollback and escape-sequence parser, OSC 133 command blocks
//!   with absolute line numbers, and snapshots in the contract's terms.
//!   Tested by feeding it bytes.
//! - [`detect_shells`] and shell integration: scripts that make bash, zsh,
//!   fish, PowerShell and cmd mark prompts, commands and exit codes, while
//!   loading the user's own configuration.
//!
//! GUI-free: the view renders [`TerminalSnapshot`](ferro_services::TerminalSnapshot)s
//! and sends keys through `write`.

pub mod core;
mod osc;
mod pty;
mod service;
mod shell;

pub use crate::core::{CoreEvent, CoreOutput, TerminalCore};
pub use service::{PtyTerminalService, TerminalOptions};
pub use shell::detect_shells;
