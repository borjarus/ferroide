//! A small line diff for the VCS mock: unified patches, gutter line changes
//! and diff3-style conflict markers.
//!
//! Common prefix and suffix are trimmed first; the remainder is diffed with
//! an LCS table. Oversized remainders (beyond a few million cells) are
//! reported as one replaced block, which is correct, if coarse.

use std::ops::Range;

use crate::{LineChange, LineChangeKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    /// Old line index, new line index.
    Equal(usize, usize),
    /// Old line index.
    Delete(usize),
    /// New line index.
    Insert(usize),
}

const MAX_TABLE: usize = 4_000_000;

pub(crate) fn diff_lines(old: &[&str], new: &[&str]) -> Vec<Op> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let (a, b) = (
        &old[prefix..old.len() - suffix],
        &new[prefix..new.len() - suffix],
    );

    let mut ops: Vec<Op> = (0..prefix).map(|i| Op::Equal(i, i)).collect();
    if a.len() * b.len() > MAX_TABLE {
        ops.extend((0..a.len()).map(|i| Op::Delete(prefix + i)));
        ops.extend((0..b.len()).map(|j| Op::Insert(prefix + j)));
    } else {
        // lcs[i][j] = LCS length of a[i..] and b[j..].
        let w = b.len() + 1;
        let mut lcs = vec![0u32; (a.len() + 1) * w];
        for i in (0..a.len()).rev() {
            for j in (0..b.len()).rev() {
                lcs[i * w + j] = if a[i] == b[j] {
                    lcs[(i + 1) * w + j + 1] + 1
                } else {
                    lcs[(i + 1) * w + j].max(lcs[i * w + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < a.len() || j < b.len() {
            if i < a.len() && j < b.len() && a[i] == b[j] {
                ops.push(Op::Equal(prefix + i, prefix + j));
                i += 1;
                j += 1;
            } else if i < a.len() && (j == b.len() || lcs[(i + 1) * w + j] >= lcs[i * w + j + 1]) {
                // Deletions first, as `git diff` prints them.
                ops.push(Op::Delete(prefix + i));
                i += 1;
            } else {
                ops.push(Op::Insert(prefix + j));
                j += 1;
            }
        }
    }
    let (old_tail, new_tail) = (old.len() - suffix, new.len() - suffix);
    ops.extend((0..suffix).map(|k| Op::Equal(old_tail + k, new_tail + k)));
    ops
}

/// A run of consecutive non-equal ops.
struct Block {
    old: Range<usize>,
    new: Range<usize>,
}

fn blocks(ops: &[Op]) -> Vec<Block> {
    let (mut old_pos, mut new_pos) = (0, 0);
    let mut out: Vec<Block> = Vec::new();
    let mut current: Option<Block> = None;
    for op in ops {
        match *op {
            Op::Equal(i, j) => {
                if let Some(b) = current.take() {
                    out.push(b);
                }
                old_pos = i + 1;
                new_pos = j + 1;
            }
            Op::Delete(i) => {
                let b = current.get_or_insert(Block {
                    old: i..i,
                    new: new_pos..new_pos,
                });
                b.old.end = i + 1;
                old_pos = i + 1;
            }
            Op::Insert(j) => {
                let b = current.get_or_insert(Block {
                    old: old_pos..old_pos,
                    new: j..j,
                });
                b.new.end = j + 1;
                new_pos = j + 1;
            }
        }
    }
    out.extend(current);
    out
}

/// Gutter markers for `new` against `old`.
pub(crate) fn line_changes(old: &str, new: &str) -> Vec<LineChange> {
    let (old, new): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    blocks(&diff_lines(&old, &new))
        .into_iter()
        .map(|b| {
            let lines = b.new.start as u32..b.new.end as u32;
            let kind = match (b.old.is_empty(), b.new.is_empty()) {
                (_, true) => LineChangeKind::Deleted,
                (true, false) => LineChangeKind::Added,
                (false, false) => LineChangeKind::Modified,
            };
            LineChange { lines, kind }
        })
        .collect()
}

/// A unified patch in `git diff` format with three lines of context. An
/// empty `old` is a new file.
pub(crate) fn unified_patch(relative: &str, old: &str, new: &str) -> String {
    const CONTEXT: usize = 3;
    let (old_lines, new_lines): (Vec<&str>, Vec<&str>) =
        (old.lines().collect(), new.lines().collect());
    let ops = diff_lines(&old_lines, &new_lines);

    let mut out = format!("diff --git a/{relative} b/{relative}\n");
    if old.is_empty() {
        out.push_str("new file mode 100644\n--- /dev/null\n");
    } else {
        out.push_str(&format!("--- a/{relative}\n"));
    }
    out.push_str(&format!("+++ b/{relative}\n"));

    // Positions (old, new) before each op.
    let mut before = Vec::with_capacity(ops.len() + 1);
    let (mut o, mut n) = (0usize, 0usize);
    for op in &ops {
        before.push((o, n));
        match op {
            Op::Equal(..) => {
                o += 1;
                n += 1;
            }
            Op::Delete(_) => o += 1,
            Op::Insert(_) => n += 1,
        }
    }

    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| !matches!(op, Op::Equal(..)))
        .map(|(ix, _)| ix)
        .collect();
    let mut groups: Vec<Range<usize>> = Vec::new();
    for ix in changed {
        let start = ix.saturating_sub(CONTEXT);
        let end = (ix + CONTEXT + 1).min(ops.len());
        match groups.last_mut() {
            Some(last) if start <= last.end => last.end = end,
            _ => groups.push(start..end),
        }
    }

    for group in groups {
        let (old_start, new_start) = before[group.start];
        let slice = &ops[group.clone()];
        let old_count = slice.iter().filter(|o| !matches!(o, Op::Insert(_))).count();
        let new_count = slice.iter().filter(|o| !matches!(o, Op::Delete(_))).count();
        let first = |start: usize, count: usize| if count == 0 { start } else { start + 1 };
        out.push_str(&format!(
            "@@ -{},{old_count} +{},{new_count} @@\n",
            first(old_start, old_count),
            first(new_start, new_count),
        ));
        for op in slice {
            match *op {
                Op::Equal(i, _) => out.push_str(&format!(" {}\n", old_lines[i])),
                Op::Delete(i) => out.push_str(&format!("-{}\n", old_lines[i])),
                Op::Insert(j) => out.push_str(&format!("+{}\n", new_lines[j])),
            }
        }
    }
    out
}

/// A three-way merge of `ours` and `theirs` against `base`. Regions only one
/// side changed are taken from it; regions both changed differently become
/// diff3-style conflict blocks.
pub(crate) fn merge3(
    base: &str,
    ours: &str,
    theirs: &str,
    ours_label: &str,
    theirs_label: &str,
) -> String {
    let base_lines: Vec<&str> = base.lines().collect();
    let ours_lines: Vec<&str> = ours.lines().collect();
    let theirs_lines: Vec<&str> = theirs.lines().collect();
    let ours_blocks = blocks(&diff_lines(&base_lines, &ours_lines));
    let theirs_blocks = blocks(&diff_lines(&base_lines, &theirs_lines));

    // Tag each block with its side and walk them in base order.
    let mut all: Vec<(bool, &Block)> = ours_blocks
        .iter()
        .map(|b| (true, b))
        .chain(theirs_blocks.iter().map(|b| (false, b)))
        .collect();
    all.sort_by_key(|(_, b)| (b.old.start, b.old.end));

    let mut out: Vec<String> = Vec::new();
    let mut base_pos = 0;
    let mut ix = 0;
    while ix < all.len() {
        // Gather every block overlapping or touching the first one.
        let mut region = all[ix].1.old.clone();
        let mut members = vec![all[ix]];
        ix += 1;
        while ix < all.len() {
            let next = &all[ix].1.old;
            let overlaps = next.start < region.end
                || (next.start == region.start)
                || (next.start == region.end && (next.is_empty() || region.is_empty()));
            if !overlaps {
                break;
            }
            region.end = region.end.max(next.end);
            members.push(all[ix]);
            ix += 1;
        }
        out.extend(
            base_lines[base_pos..region.start]
                .iter()
                .map(|s| s.to_string()),
        );

        // Each side's text for the region: its blocks applied over the base.
        let side_text = |side: bool, lines: &[&str]| -> Vec<String> {
            let mut text = Vec::new();
            let mut pos = region.start;
            for (_, b) in members.iter().filter(|(s, _)| *s == side) {
                text.extend(base_lines[pos..b.old.start].iter().map(|s| s.to_string()));
                text.extend(lines[b.new.clone()].iter().map(|s| s.to_string()));
                pos = b.old.end;
            }
            text.extend(base_lines[pos..region.end].iter().map(|s| s.to_string()));
            text
        };
        let has_ours = members.iter().any(|(s, _)| *s);
        let has_theirs = members.iter().any(|(s, _)| !*s);
        let ours_text = side_text(true, &ours_lines);
        let theirs_text = side_text(false, &theirs_lines);
        if has_ours && has_theirs && ours_text != theirs_text {
            out.push(format!("<<<<<<< {ours_label}"));
            out.extend(ours_text);
            out.push("||||||| base".to_string());
            out.extend(base_lines[region.clone()].iter().map(|s| s.to_string()));
            out.push("=======".to_string());
            out.extend(theirs_text);
            out.push(format!(">>>>>>> {theirs_label}"));
        } else if has_ours {
            out.extend(ours_text);
        } else {
            out.extend(theirs_text);
        }
        base_pos = region.end;
    }
    out.extend(base_lines[base_pos..].iter().map(|s| s.to_string()));
    let mut text = out.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_texts_have_no_changes() {
        assert!(line_changes("a\nb\nc\n", "a\nb\nc\n").is_empty());
    }

    #[test]
    fn line_changes_classify_added_modified_deleted() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nX\nc\nnew\nd\n";
        let changes = line_changes(old, new);
        assert_eq!(
            changes,
            vec![
                LineChange {
                    lines: 1..2,
                    kind: LineChangeKind::Modified
                },
                LineChange {
                    lines: 3..4,
                    kind: LineChangeKind::Added
                },
                LineChange {
                    lines: 5..5,
                    kind: LineChangeKind::Deleted
                },
            ]
        );
    }

    #[test]
    fn unified_patch_has_correct_hunk_headers() {
        let old = (1..=20).map(|i| format!("line {i}\n")).collect::<String>();
        let new = old.replace("line 10\n", "line ten\n");
        let patch = unified_patch("x.txt", &old, &new);
        assert!(patch.starts_with("diff --git a/x.txt b/x.txt\n--- a/x.txt\n+++ b/x.txt\n"));
        assert!(patch.contains("@@ -7,7 +7,7 @@\n"), "{patch}");
        assert!(patch.contains("-line 10\n+line ten\n"));

        // Apply the patch back: context and `+` lines rebuild the new text
        // within the hunk.
        let rebuilt: Vec<&str> = patch
            .lines()
            .skip_while(|l| !l.starts_with("@@"))
            .skip(1)
            .filter(|l| !l.starts_with('-'))
            .map(|l| &l[1..])
            .collect();
        assert_eq!(rebuilt, new.lines().skip(6).take(7).collect::<Vec<_>>());
    }

    #[test]
    fn new_files_diff_against_dev_null() {
        let patch = unified_patch("a.ts", "", "one\ntwo\n");
        assert!(patch.contains("--- /dev/null\n"));
        assert!(patch.contains("@@ -0,0 +1,2 @@\n+one\n+two\n"));
    }

    #[test]
    fn merge3_takes_one_sided_changes_and_marks_conflicts() {
        let base = "a\nb\nc\nd\n";
        let ours = "A\nb\nc\nd\n";
        let theirs = "a\nb\nc\nD\n";
        assert_eq!(merge3(base, ours, theirs, "ours", "theirs"), "A\nb\nc\nD\n");

        let ours = "a\nours\nc\nd\n";
        let theirs = "a\ntheirs\nc\nd\n";
        let merged = merge3(base, ours, theirs, "HEAD", "feature");
        assert_eq!(
            merged,
            "a\n<<<<<<< HEAD\nours\n||||||| base\nb\n=======\ntheirs\n>>>>>>> feature\nc\nd\n"
        );
    }
}
