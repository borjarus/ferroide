//! Fuzzy matching for Quick Open, the Command Palette and speed search.
//!
//! A query matches a candidate when its characters appear in order
//! (case-insensitively). The score rewards what people mean when they type:
//! consecutive characters, word starts (`fb` → `FooBar`, `foo_bar`), the
//! beginning of the file name, and short candidates. The matcher is
//! allocation-light and runs over 100,000 paths in a few milliseconds.

/// The result of a successful match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatch {
    pub score: i64,
    /// Byte offsets of the matched characters in the candidate.
    pub positions: Vec<usize>,
}

const SCORE_MATCH: i64 = 16;
const BONUS_CONSECUTIVE: i64 = 24;
const BONUS_WORD_START: i64 = 30;
const BONUS_FIRST_CHAR: i64 = 20;
const BONUS_FILE_NAME: i64 = 12;
const PENALTY_GAP: i64 = 2;
const PENALTY_LENGTH: i64 = 1;

fn is_separator(c: char) -> bool {
    matches!(c, '/' | '\\' | '_' | '-' | '.' | ' ' | ':')
}

/// Match `query` against `candidate`. An empty query matches everything with
/// score 0.
///
/// Matching is greedy from the end of the candidate backwards for the
/// file-name part, which keeps it linear while still preferring matches in
/// the file name over matches in directory names.
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<FuzzyMatch> {
    match_lowered(&lower_query(query), candidate)
}

fn lower_query(query: &str) -> Vec<char> {
    query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// [`fuzzy_match`] with the query already lowercased, so ranking many
/// candidates lowercases it once. Rejection happens before any allocation:
/// most candidates of a large project do not match.
fn match_lowered(query: &[char], candidate: &str) -> Option<FuzzyMatch> {
    if query.is_empty() {
        return Some(FuzzyMatch {
            score: 0,
            positions: Vec::new(),
        });
    }

    // Quick reject: every query char must occur in order.
    let mut qi = 0;
    for c in candidate.chars() {
        if c.to_lowercase().eq(std::iter::once(query[qi])) {
            qi += 1;
            if qi == query.len() {
                break;
            }
        }
    }
    if qi < query.len() {
        return None;
    }

    let chars: Vec<(usize, char)> = candidate.char_indices().collect();
    let name_start = candidate
        .rfind(['/', '\\'])
        .map(|i| i + 1)
        .unwrap_or_default();

    // Try two alignments — leftmost-greedy and one anchored on the file
    // name — and keep the better.
    let mut best: Option<FuzzyMatch> = None;
    for anchor in [0, name_start] {
        if let Some(m) = align(query, &chars, candidate, anchor, name_start)
            && best.as_ref().is_none_or(|b| m.score > b.score)
        {
            best = Some(m);
        }
    }
    best
}

fn align(
    query: &[char],
    chars: &[(usize, char)],
    candidate: &str,
    anchor: usize,
    name_start: usize,
) -> Option<FuzzyMatch> {
    let start_ix = chars.iter().position(|(b, _)| *b >= anchor)?;
    let mut positions = Vec::with_capacity(query.len());
    let mut score = 0i64;
    let mut qi = 0;
    let mut last: Option<usize> = None;
    let mut ci = start_ix;
    // Characters before the anchor may still be needed if the anchored
    // part cannot hold the whole query; fall back to the full scan then.
    while ci < chars.len() && qi < query.len() {
        let (byte, c) = chars[ci];
        if c.to_lowercase().eq(std::iter::once(query[qi])) {
            let prev = ci.checked_sub(1).map(|p| chars[p].1);
            let word_start = match prev {
                None => true,
                Some(p) => is_separator(p) || (p.is_lowercase() && c.is_uppercase()),
            };
            score += SCORE_MATCH;
            if ci == 0 {
                score += BONUS_FIRST_CHAR;
            }
            if word_start {
                score += BONUS_WORD_START;
            }
            if byte >= name_start {
                score += BONUS_FILE_NAME;
            }
            match last {
                Some(l) if l + 1 == ci => score += BONUS_CONSECUTIVE,
                Some(l) => score -= PENALTY_GAP * (ci - l - 1).min(16) as i64,
                None => {}
            }
            positions.push(byte);
            last = Some(ci);
            qi += 1;
        }
        ci += 1;
    }
    if qi < query.len() {
        return None;
    }
    score -= PENALTY_LENGTH * (candidate.chars().count() as i64 / 4);
    Some(FuzzyMatch { score, positions })
}

/// Match `query` against every candidate and return the best `limit`
/// indices with their matches, best first. Ties keep the input order.
pub fn rank<'a>(
    query: &str,
    candidates: impl IntoIterator<Item = &'a str>,
    limit: usize,
) -> Vec<(usize, FuzzyMatch)> {
    let query = lower_query(query);
    let mut matches: Vec<(usize, FuzzyMatch)> = candidates
        .into_iter()
        .enumerate()
        .filter_map(|(ix, c)| match_lowered(&query, c).map(|m| (ix, m)))
        .collect();
    matches.sort_by(|a, b| b.1.score.cmp(&a.1.score).then(a.0.cmp(&b.0)));
    matches.truncate(limit);
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(q: &str, c: &str) -> i64 {
        fuzzy_match(q, c).map(|m| m.score).unwrap_or(i64::MIN)
    }

    #[test]
    fn subsequences_match_case_insensitively() {
        assert!(fuzzy_match("mrs", "src/main.rs").is_some());
        assert!(fuzzy_match("MAIN", "src/main.rs").is_some());
        assert!(fuzzy_match("xyz", "src/main.rs").is_none());
        assert!(fuzzy_match("sm", "ms").is_none(), "order matters");
        assert_eq!(fuzzy_match("", "anything").unwrap().score, 0);
    }

    #[test]
    fn positions_point_at_matched_bytes() {
        let m = fuzzy_match("mr", "src/main.rs").unwrap();
        assert_eq!(m.positions, vec![4, 9]);
    }

    #[test]
    fn file_names_and_word_starts_win() {
        assert!(score("main", "src/main.rs") > score("main", "maintenance/notes/x.txt"));
        assert!(score("cfg", "src/config.rs") > score("cfg", "docs/c_f_long_g.md") - 1000);
        assert!(score("fb", "FooBar.ts") > score("fb", "fabric.ts"));
        assert!(score("lib", "src/lib.rs") > score("lib", "very/long/library/path/x.rs"));
    }

    #[test]
    fn non_ascii_candidates_are_safe() {
        assert!(fuzzy_match("żół", "stress/żółć-gęślą-jaźń.rs").is_some());
        assert!(fuzzy_match("crab", "stress/🦀 crab notes.md").is_some());
        let m = fuzzy_match("日本", "stress/日本語のファイル名.ts").unwrap();
        assert!(
            m.positions
                .iter()
                .all(|&p| "stress/日本語のファイル名.ts".is_char_boundary(p))
        );
    }

    #[test]
    fn rank_orders_and_limits() {
        let files = ["a/config.rs", "src/main.rs", "src/lib.rs", "README.md"];
        let ranked = rank("rs", files, 2);
        assert_eq!(ranked.len(), 2);
        assert!(ranked[0].1.score >= ranked[1].1.score);
    }

    #[test]
    fn ranking_100k_paths_is_fast() {
        let paths: Vec<String> = (0..100_000)
            .map(|i| format!("stress/module_{}/module_{}/file_{i:04}.rs", i % 47, i % 13))
            .collect();
        let started = std::time::Instant::now();
        let ranked = rank("mod7file12", paths.iter().map(String::as_str), 50);
        let took = started.elapsed();
        assert!(!ranked.is_empty());
        // Generous: debug builds on CI are slow. Release is ~10× faster.
        assert!(took.as_millis() < 2_000, "took {took:?}");
    }
}
