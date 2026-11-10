//! The text buffer: a rope with versioned edits, undo/redo grouped in
//! transactions, and anchors that survive edits.

use std::time::{Duration, Instant};

use ropey::Rope;

use crate::{
    Bias, LINES, SelectionSet,
    edit::{self, Edit, EditError},
};

/// Typing pauses shorter than this keep extending one undo step.
pub const DEFAULT_GROUP_INTERVAL: Duration = Duration::from_millis(300);

/// Undo steps kept; the oldest are dropped first.
pub const DEFAULT_UNDO_LIMIT: usize = 10_000;

/// What kind of edit a transaction is; consecutive transactions of the same
/// groupable kind made within the group interval undo together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EditKind {
    /// Inserting typed text.
    Typing,
    /// Backspace / Delete.
    Deleting,
    /// Everything else (paste, line moves, comments, replace…): never
    /// grouped with its neighbours.
    Other,
}

impl EditKind {
    fn groups(self) -> bool {
        self != EditKind::Other
    }
}

/// A position that follows the text it points at through later edits.
///
/// Created by [`Buffer::anchor`], read back with [`Buffer::resolve`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Anchor {
    offset: usize,
    version: u64,
    pub bias: Bias,
}

/// One applied set of edits: what it did and how to take it back.
#[derive(Debug, Clone)]
struct Step {
    /// In the coordinates of the text before the step.
    forward: Vec<Edit>,
    /// In the coordinates of the text after the step.
    inverse: Vec<Edit>,
}

/// One undo step: one or more applied edit sets.
#[derive(Debug, Clone)]
struct Transaction {
    steps: Vec<Step>,
    kind: EditKind,
    selections_before: SelectionSet,
    selections_after: SelectionSet,
    last_edit_at: Instant,
}

/// Summary of one version change, for resolving anchors.
type LogEntry = Vec<(usize, usize, usize)>;

/// A text buffer.
///
/// Text is stored with `\n` line breaks only; see [`crate::line_ending`]
/// for converting from and to the file's line separators.
#[derive(Debug, Clone)]
pub struct Buffer {
    rope: Rope,
    version: u64,
    /// `log[i]` turns version `log_base + i` into `log_base + i + 1`.
    log: Vec<LogEntry>,
    log_base: u64,
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    /// The next transaction starts a new undo step even if it could group.
    group_closed: bool,
    group_interval: Duration,
    undo_limit: usize,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new("")
    }
}

impl Buffer {
    pub fn new(text: &str) -> Self {
        Self::from_rope(Rope::from_str(text))
    }

    pub fn from_rope(rope: Rope) -> Self {
        Self {
            rope,
            version: 0,
            log: Vec::new(),
            log_base: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            group_closed: true,
            group_interval: DEFAULT_GROUP_INTERVAL,
            undo_limit: DEFAULT_UNDO_LIMIT,
        }
    }

    pub fn with_group_interval(mut self, interval: Duration) -> Self {
        self.group_interval = interval;
        self
    }

    pub fn with_undo_limit(mut self, limit: usize) -> Self {
        self.undo_limit = limit.max(1);
        self
    }

    /// The text. Cloning a rope is O(1), so this doubles as a snapshot for
    /// background work.
    pub fn text(&self) -> &Rope {
        &self.rope
    }

    pub fn snapshot(&self) -> Rope {
        self.rope.clone()
    }

    pub fn len(&self) -> usize {
        self.rope.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rope.len() == 0
    }

    /// Number of lines; an empty text and a text ending in `\n` count the
    /// empty last line.
    pub fn line_count(&self) -> usize {
        self.rope.len_lines(LINES)
    }

    /// Increases by one with every applied edit set, including undo/redo.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Apply `edits` as one undo step (or as part of the previous one, see
    /// [`EditKind`]). `before` and `after` are the selections to restore on
    /// undo and redo. Returns the normalized edits that were applied.
    pub fn edit(
        &mut self,
        edits: Vec<Edit>,
        kind: EditKind,
        before: &SelectionSet,
        after: &SelectionSet,
        now: Instant,
    ) -> Result<Vec<Edit>, EditError> {
        let edits = edit::normalize(edits, &self.rope)?;
        if edits.is_empty() {
            return Ok(edits);
        }
        let inverse = self.apply_raw(&edits);
        let step = Step {
            forward: edits.clone(),
            inverse,
        };
        self.redo.clear();
        let groups = !self.group_closed
            && kind.groups()
            && self.undo.last().is_some_and(|last| {
                last.kind == kind
                    && now.saturating_duration_since(last.last_edit_at) < self.group_interval
            });
        if groups && let Some(last) = self.undo.last_mut() {
            last.steps.push(step);
            last.selections_after = after.clone();
            last.last_edit_at = now;
        } else {
            self.undo.push(Transaction {
                steps: vec![step],
                kind,
                selections_before: before.clone(),
                selections_after: after.clone(),
                last_edit_at: now,
            });
            if self.undo.len() > self.undo_limit {
                self.undo.remove(0);
            }
        }
        self.group_closed = !kind.groups();
        Ok(edits)
    }

    /// Apply edits without recording them for undo. The undo history is
    /// cleared, since its steps no longer match the text. Anchors still
    /// follow the edits.
    pub fn edit_untracked(&mut self, edits: Vec<Edit>) -> Result<Vec<Edit>, EditError> {
        let edits = edit::normalize(edits, &self.rope)?;
        if !edits.is_empty() {
            self.apply_raw(&edits);
            self.undo.clear();
            self.redo.clear();
        }
        Ok(edits)
    }

    /// Replace the whole text and forget the history (reload from disk).
    /// Anchors resolve to the start of the replaced text.
    pub fn reset(&mut self, text: &str) {
        let edits = vec![Edit::new(0..self.rope.len(), text)];
        self.apply_raw(&edits);
        self.undo.clear();
        self.redo.clear();
        self.group_closed = true;
    }

    /// The next edit starts a new undo step (e.g. after the caret was moved
    /// with the mouse).
    pub fn close_group(&mut self) {
        self.group_closed = true;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Undo the last step; returns the selections to restore.
    pub fn undo(&mut self) -> Option<SelectionSet> {
        let transaction = self.undo.pop()?;
        for step in transaction.steps.iter().rev() {
            self.apply_raw(&step.inverse);
        }
        let selections = transaction.selections_before.clone();
        self.redo.push(transaction);
        self.group_closed = true;
        Some(selections)
    }

    /// Redo the last undone step; returns the selections to restore.
    pub fn redo(&mut self) -> Option<SelectionSet> {
        let transaction = self.redo.pop()?;
        for step in &transaction.steps {
            self.apply_raw(&step.forward);
        }
        let selections = transaction.selections_after.clone();
        self.undo.push(transaction);
        self.group_closed = true;
        Some(selections)
    }

    fn apply_raw(&mut self, edits: &[Edit]) -> Vec<Edit> {
        let inverse = edit::apply(&mut self.rope, edits);
        self.log.push(
            edits
                .iter()
                .map(|e| (e.range.start, e.range.end, e.text.len()))
                .collect(),
        );
        self.version += 1;
        inverse
    }

    /// The edits of the latest version change as `(start, end, new_len)`
    /// in the coordinates before it, for updating derived state such as a
    /// [`crate::display_map::DisplayMap`].
    pub fn last_change(&self) -> Option<&[(usize, usize, usize)]> {
        self.log.last().map(Vec::as_slice)
    }

    /// An anchor at `offset` (clamped to the text and to a character
    /// boundary).
    pub fn anchor(&self, offset: usize, bias: Bias) -> Anchor {
        let offset = self.rope.floor_char_boundary(offset.min(self.rope.len()));
        Anchor {
            offset,
            version: self.version,
            bias,
        }
    }

    /// The anchor's current offset.
    pub fn resolve(&self, anchor: &Anchor) -> usize {
        let mut offset = anchor.offset;
        let start = anchor.version.max(self.log_base);
        if anchor.version < self.log_base {
            // Older than the retained log: the best we know is the offset,
            // clamped to the text.
            return self.rope.floor_char_boundary(offset.min(self.rope.len()));
        }
        for entry in &self.log[(start - self.log_base) as usize..] {
            offset = edit::map_offset_summaries(offset, entry.iter().copied(), anchor.bias);
        }
        offset.min(self.rope.len())
    }

    /// Move an anchor to the current version, so resolving it later is
    /// cheaper and it survives [`Buffer::compact_anchor_log`].
    pub fn refresh(&self, anchor: &Anchor) -> Anchor {
        Anchor {
            offset: self.resolve(anchor),
            version: self.version,
            bias: anchor.bias,
        }
    }

    /// Forget the edit log before `version`. Anchors created earlier and
    /// not refreshed lose track of edits made since.
    pub fn compact_anchor_log(&mut self, version: u64) {
        let version = version.min(self.version);
        if version <= self.log_base {
            return;
        }
        let drop = (version - self.log_base) as usize;
        self.log.drain(..drop);
        self.log_base = version;
    }

    /// 0-based line of a byte offset.
    pub fn line_of(&self, offset: usize) -> usize {
        self.rope
            .byte_to_line_idx(offset.min(self.rope.len()), LINES)
    }

    /// Byte offset of the start of `line` (clamped to the end of the text).
    pub fn line_start(&self, line: usize) -> usize {
        if line >= self.line_count() {
            return self.rope.len();
        }
        self.rope.line_to_byte_idx(line, LINES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Selection;

    fn sel(offset: usize) -> SelectionSet {
        SelectionSet::single(Selection::cursor(offset))
    }

    #[test]
    fn typing_groups_until_a_pause() {
        let mut b = Buffer::new("");
        let t0 = Instant::now();
        for (i, ch) in "abc".chars().enumerate() {
            b.edit(
                vec![Edit::insert(i, ch.to_string())],
                EditKind::Typing,
                &sel(i),
                &sel(i + 1),
                t0 + Duration::from_millis(i as u64 * 100),
            )
            .unwrap();
        }
        b.edit(
            vec![Edit::insert(3, "d")],
            EditKind::Typing,
            &sel(3),
            &sel(4),
            t0 + Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(b.text().to_string(), "abcd");
        assert_eq!(b.undo(), Some(sel(3)));
        assert_eq!(b.text().to_string(), "abc");
        assert_eq!(b.undo(), Some(sel(0)));
        assert_eq!(b.text().to_string(), "");
        assert!(b.undo().is_none());
        assert_eq!(b.redo(), Some(sel(3)));
        assert_eq!(b.redo(), Some(sel(4)));
        assert_eq!(b.text().to_string(), "abcd");
    }

    #[test]
    fn other_edits_never_group() {
        let mut b = Buffer::new("x");
        let now = Instant::now();
        b.edit(
            vec![Edit::insert(0, "a")],
            EditKind::Other,
            &sel(0),
            &sel(1),
            now,
        )
        .unwrap();
        b.edit(
            vec![Edit::insert(0, "b")],
            EditKind::Other,
            &sel(0),
            &sel(1),
            now,
        )
        .unwrap();
        b.undo();
        assert_eq!(b.text().to_string(), "ax");
    }

    #[test]
    fn a_new_edit_clears_redo() {
        let mut b = Buffer::new("");
        let now = Instant::now();
        b.edit(
            vec![Edit::insert(0, "a")],
            EditKind::Other,
            &sel(0),
            &sel(1),
            now,
        )
        .unwrap();
        b.undo();
        b.edit(
            vec![Edit::insert(0, "b")],
            EditKind::Other,
            &sel(0),
            &sel(1),
            now,
        )
        .unwrap();
        assert!(!b.can_redo());
    }

    #[test]
    fn anchors_survive_edits_and_undo() {
        let mut b = Buffer::new("fn main() {}\n");
        let a = b.anchor(3, Bias::Left); // before "main"
        let now = Instant::now();
        b.edit(
            vec![Edit::insert(0, "pub ")],
            EditKind::Other,
            &sel(0),
            &sel(4),
            now,
        )
        .unwrap();
        assert_eq!(b.resolve(&a), 7);
        b.undo();
        assert_eq!(b.resolve(&a), 3);
        let a = b.refresh(&a);
        b.compact_anchor_log(b.version());
        assert_eq!(b.resolve(&a), 3);
    }

    #[test]
    fn line_helpers() {
        let b = Buffer::new("a\nbc\n");
        assert_eq!(b.line_count(), 3);
        assert_eq!(b.line_of(3), 1);
        assert_eq!(b.line_start(1), 2);
        assert_eq!(b.line_start(9), 5);
    }
}
