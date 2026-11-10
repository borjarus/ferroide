//! FerroIDE's editor core, independent of any GUI (plan §5.5, Stage 3).
//!
//! - [`Buffer`]: a rope ([`ropey`], the same rope GPUI Kit's editor uses, so
//!   snapshots pass between them without copying), versioned edits,
//!   undo/redo grouped by time into transactions, and [`Anchor`]s that
//!   follow the text through edits.
//! - [`SelectionSet`]: multiple cursors and selections.
//! - [`ops`]: editing commands over all selections at once — typing,
//!   grapheme-aware deletion, duplicating / moving / deleting lines,
//!   indentation, line and block comments, next / all occurrences (Ctrl+D,
//!   Ctrl+Shift+L), cursors above / below, column selection, clipboard with
//!   several cursors.
//! - [`Editor`]: the buffer, its selections and the commands together.
//! - [`search`]: find / replace in a file, plain or regex.
//! - [`display_map`]: folds, soft wrap and tabs, updated incrementally.
//! - [`unicode`]: grapheme clusters, wide characters, bidirectional text.
//! - [`encoding`], [`line_ending`], [`file`]: reading files in any common
//!   encoding and line separator, atomic saving, large-file thresholds and
//!   change detection.
//!
//! Offsets are UTF-8 byte offsets into text whose lines end with `\n`.

pub mod comment;
pub mod display_map;
pub mod edit;
pub mod encoding;
pub mod file;
#[doc(hidden)]
pub mod fuzz;
pub mod line_ending;
pub mod ops;
pub mod search;
pub mod unicode;

mod buffer;
mod editor;
mod selection;

pub use buffer::{Anchor, Buffer, DEFAULT_GROUP_INTERVAL, DEFAULT_UNDO_LIMIT, EditKind};
pub use edit::{Edit, EditError};
pub use editor::{Editor, Indent};
pub use ropey::{LineType, Rope, RopeSlice};
pub use selection::{Selection, SelectionSet};

/// Lines are separated by `\n` only (text is normalized on load).
pub const LINES: LineType = LineType::LF;

/// Which side of an insertion at its position an offset or anchor sticks
/// to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Bias {
    /// Stays before text inserted at its position.
    #[default]
    Left,
    /// Moves after text inserted at its position.
    Right,
}
