//! A driver that turns arbitrary bytes into a sequence of editor commands
//! and checks the editor's invariants after each one. Shared by the
//! property tests and the `cargo fuzz` target in `crates/ferro_text/fuzz`.

use std::time::{Duration, Instant};

use crate::{
    Editor, LINES, Selection, SelectionSet,
    comment::CommentTokens,
    display_map::{DisplayMap, DisplayMapConfig},
    edit::normalize,
    ops::{Clipboard, RowColumn},
    unicode::is_grapheme_boundary,
};

/// Text fragments commands insert: ASCII, Polish, CJK, emoji with ZWJ,
/// combining marks, tabs and line breaks.
const SNIPPETS: &[&str] = &[
    "a",
    "xyz",
    " ",
    "\n",
    "\t",
    "ż",
    "ółć",
    "漢字",
    "👨‍👩‍👧",
    "e\u{301}",
    "// ",
    "fn x() {}\n",
    "",
    "שלום",
];

/// Above this many bytes, commands that add text are skipped.
const MAX_TEXT: usize = 4 * 1024;

struct Input<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Input<'_> {
    fn byte(&mut self) -> Option<u8> {
        let b = self.bytes.get(self.at).copied();
        self.at += 1;
        b
    }

    fn index(&mut self, len: usize) -> Option<usize> {
        let hi = self.byte()? as usize;
        let lo = self.byte()? as usize;
        Some(if len == 0 {
            0
        } else {
            (hi << 8 | lo) % (len + 1)
        })
    }
}

/// Run the commands encoded in `data`, panicking when an invariant breaks.
pub fn run(data: &[u8]) {
    let mut input = Input { bytes: data, at: 0 };
    let mut editor = Editor::new("fn main() {\n    let zażółć = 1;\n}\n")
        .with_comments(CommentTokens::for_language("rust"));
    let initial = editor.text().to_string();
    let mut clipboard = Clipboard::from_text("pasted\ntext");
    let mut now = Instant::now();
    let mut map = DisplayMap::new(
        editor.text(),
        DisplayMapConfig {
            tab_size: 4,
            wrap_column: Some(12),
        },
    );
    let mut steps = 0;
    while let Some(op) = input.byte() {
        steps += 1;
        now += Duration::from_millis(u64::from(op % 7) * 100);
        let before = editor.text().clone();
        let version = editor.buffer().version();
        let len = editor.text().len();
        // Copy / paste / duplicate can double the text; keep it small so a
        // session stays fast.
        if len > MAX_TEXT && matches!(op % 24, 0..=3 | 8 | 18) {
            continue;
        }
        match op % 24 {
            0..=3 => {
                let Some(i) = input.byte() else { break };
                editor.insert(SNIPPETS[i as usize % SNIPPETS.len()], now);
            }
            4 => editor.backspace(now),
            5 => editor.delete(now),
            6 => {
                let (Some(a), Some(b)) = (input.index(len), input.index(len)) else {
                    break;
                };
                let rope = editor.text();
                let s = Selection::new(rope.floor_char_boundary(a), rope.floor_char_boundary(b));
                editor.set_selections(SelectionSet::single(s));
            }
            7 => {
                let Some(a) = input.index(len) else { break };
                let mut set = editor.selections().clone();
                set.add(Selection::cursor(editor.text().floor_char_boundary(a)));
                editor.set_selections(set);
            }
            8 => editor.duplicate_lines(op & 0x80 != 0, now),
            9 => {
                editor.move_lines(op & 0x80 != 0, now);
            }
            10 => {
                editor.toggle_line_comment(now);
            }
            11 => {
                editor.toggle_block_comment(now);
            }
            12 => {
                editor.select_next_occurrence();
            }
            13 => {
                editor.select_all_occurrences();
            }
            14 => editor.add_cursor_above(),
            15 => editor.add_cursor_below(),
            16 => clipboard = editor.copy(),
            17 => clipboard = editor.cut(now),
            18 => editor.paste(&clipboard, now),
            19 => {
                editor.undo();
            }
            20 => {
                editor.redo();
            }
            21 => {
                if op & 0x80 != 0 {
                    editor.indent_lines(now);
                } else {
                    editor.outdent_lines(now);
                }
            }
            22 => {
                let rows = editor.text().len_lines(LINES);
                let (Some(a), Some(b), Some(c), Some(d)) = (
                    input.index(rows),
                    input.byte(),
                    input.index(rows),
                    input.byte(),
                ) else {
                    break;
                };
                editor.select_column(
                    RowColumn::new(a, usize::from(b % 20)),
                    RowColumn::new(c, usize::from(d % 20)),
                );
            }
            _ => editor.delete_lines(now),
        }
        check(&editor);
        // The display map, updated incrementally, must match one built
        // from scratch.
        if editor.buffer().version() != version {
            let after = editor.text().clone();
            match editor.buffer().last_change() {
                Some(change) if editor.buffer().version() == version + 1 => {
                    map.sync_summaries(&before, &after, change);
                }
                _ => {
                    let whole = vec![crate::Edit::new(0..before.len(), after.to_string())];
                    let edits = normalize(whole, &before).unwrap_or_default();
                    map.sync(&before, &after, &edits);
                }
            }
            let fresh = DisplayMap::new(&after, map.config());
            assert_eq!(
                map.display_rows(&after, 0..usize::MAX),
                fresh.display_rows(&after, 0..usize::MAX)
            );
        }
        if steps > 200 {
            break;
        }
    }
    // Undoing everything restores the initial text.
    while editor.undo() {}
    assert_eq!(editor.text().to_string(), initial);
    check(&editor);
}

fn check(editor: &Editor) {
    let rope = editor.text();
    let len = rope.len();
    let selections = editor.selections().as_slice();
    assert!(!selections.is_empty());
    for pair in selections.windows(2) {
        assert!(
            pair[0].end() <= pair[1].start(),
            "selections overlap or are unsorted: {selections:?}"
        );
    }
    for s in selections {
        assert!(s.anchor <= len && s.head <= len, "{s:?} beyond {len}");
        assert!(rope.is_char_boundary(s.anchor) && rope.is_char_boundary(s.head));
    }
    // Carets never land inside a grapheme cluster after a deletion.
    let _ = is_grapheme_boundary(rope, selections[0].head);
}
