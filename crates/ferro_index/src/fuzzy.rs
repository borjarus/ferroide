//! Fuzzy matching of paths and symbol names with `nucleo-matcher` (the
//! matcher of Helix and Zed's file finder).
//!
//! Queries use nucleo's pattern syntax: words separated by spaces must all
//! match (`src main`), `^` anchors at the start, `$` at the end, `'` asks
//! for a substring and `!` excludes. A path whose file name alone matches
//! the last word ranks above one where the match is spread over
//! directories. Large lists are scored on several threads.

use std::thread;

use nucleo_matcher::{
    Config, Matcher, Utf32String,
    pattern::{CaseMatching, Normalization, Pattern},
};

/// Candidates above this count are split across threads.
const PARALLEL_FROM: usize = 20_000;

/// A candidate prepared for matching (converted to UTF-32 once).
#[derive(Debug, Clone)]
pub struct Haystack {
    text: Utf32String,
    /// Where the file name starts (in chars), for the file-name bonus.
    name_start: usize,
}

impl Haystack {
    pub fn new(text: &str) -> Self {
        let name_start = text
            .rfind(['/', '\\'])
            .map_or(0, |i| text[..=i].chars().count());
        // Code points, not graphemes, so match indices map back to bytes.
        let text = if text.is_ascii() {
            Utf32String::Ascii(text.into())
        } else {
            Utf32String::Unicode(text.chars().collect())
        };
        Self { text, name_start }
    }
}

/// One match: the candidate's index, its score and the byte offsets of the
/// matched characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ranked {
    pub index: usize,
    pub score: u32,
    pub positions: Vec<usize>,
}

/// A parsed query.
pub struct Query {
    pattern: Pattern,
    /// The last word, scored against file names alone.
    last: Option<Pattern>,
    paths: bool,
}

impl Query {
    /// A query over paths (path-aware bonuses, file-name preference).
    pub fn paths(query: &str) -> Self {
        Self::new(query, true)
    }

    /// A query over plain names (symbols).
    pub fn names(query: &str) -> Self {
        Self::new(query, false)
    }

    fn new(query: &str, paths: bool) -> Self {
        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
        let last = query
            .split_whitespace()
            .last()
            .filter(|w| !w.starts_with('!'))
            .map(|w| Pattern::parse(w, CaseMatching::Smart, Normalization::Smart));
        Self {
            pattern,
            last,
            paths,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pattern.atoms.is_empty()
    }

    /// Whether every match of `longer` is also a match of `self`, i.e. the
    /// matches of `self` can be narrowed instead of searching again.
    pub fn narrows_to(previous: &str, next: &str) -> bool {
        !previous.trim().is_empty()
            && next.starts_with(previous)
            && !previous.contains(['!', '$', '\\'])
            && !next[previous.len()..].contains(['!', '\\'])
            // A new word changes which word gets the file-name bonus, not
            // which candidates match; still a subset.
            && !previous.ends_with(' ')
    }

    fn matcher(&self) -> Matcher {
        let mut config = Config::DEFAULT;
        if self.paths {
            config.set_match_paths();
        }
        Matcher::new(config)
    }

    fn score(&self, haystack: &Haystack, matcher: &mut Matcher) -> Option<u32> {
        let text = haystack.text.slice(..);
        let mut score = self.pattern.score(text, matcher)?;
        if self.paths
            && haystack.name_start > 0
            && let Some(last) = &self.last
            && let Some(name_score) = last.score(text.slice(haystack.name_start..), matcher)
        {
            score += name_score / 2 + 16;
        }
        Some(score)
    }

    /// Byte offsets of the matched characters of `text`.
    fn positions(&self, haystack: &Haystack, text: &str, matcher: &mut Matcher) -> Vec<usize> {
        let mut indices = Vec::new();
        self.pattern
            .indices(haystack.text.slice(..), matcher, &mut indices);
        indices.sort_unstable();
        indices.dedup();
        if text.is_ascii() {
            return indices.into_iter().map(|i| i as usize).collect();
        }
        let offsets: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();
        indices
            .into_iter()
            .filter_map(|i| offsets.get(i as usize).copied())
            .collect()
    }
}

/// Score `candidates` (all of them, or only `subset`) and return the best
/// `limit`, best first; ties keep the input order. `texts` gives the
/// original text of a candidate for the highlight positions.
pub fn rank<'a>(
    query: &Query,
    candidates: &[Haystack],
    subset: Option<&[u32]>,
    limit: usize,
    texts: impl Fn(usize) -> &'a str,
) -> (Vec<Ranked>, Vec<u32>) {
    let indices: Vec<u32> = match subset {
        Some(subset) => subset.to_vec(),
        None => (0..candidates.len() as u32).collect(),
    };
    let score_chunk = |chunk: &[u32]| -> Vec<(u32, u32)> {
        let mut matcher = query.matcher();
        chunk
            .iter()
            .filter_map(|&ix| {
                query
                    .score(&candidates[ix as usize], &mut matcher)
                    .map(|s| (ix, s))
            })
            .collect()
    };
    let mut scored: Vec<(u32, u32)> = if indices.len() >= PARALLEL_FROM {
        let threads = thread::available_parallelism()
            .map_or(4, |n| n.get())
            .clamp(2, 8);
        let chunk = indices.len().div_ceil(threads);
        thread::scope(|scope| {
            let handles: Vec<_> = indices
                .chunks(chunk)
                .map(|part| scope.spawn(move || score_chunk(part)))
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap_or_default())
                .collect()
        })
    } else {
        score_chunk(&indices)
    };
    let matched: Vec<u32> = scored.iter().map(|(ix, _)| *ix).collect();
    if scored.len() > limit {
        scored.select_nth_unstable_by(limit, |a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        scored.truncate(limit);
    }
    scored.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut matcher = query.matcher();
    let ranked = scored
        .into_iter()
        .map(|(ix, score)| Ranked {
            index: ix as usize,
            score,
            positions: query.positions(&candidates[ix as usize], texts(ix as usize), &mut matcher),
        })
        .collect();
    (ranked, matched)
}

/// Paths prepared for Quick Open, remembering the last query so the next
/// keystroke only rescores what still matched.
pub struct PathMatcher {
    shown: Vec<String>,
    haystacks: Vec<Haystack>,
    last: Option<(String, Vec<u32>)>,
}

impl PathMatcher {
    pub fn new(shown: Vec<String>) -> Self {
        let haystacks = shown.iter().map(|s| Haystack::new(s)).collect();
        Self {
            shown,
            haystacks,
            last: None,
        }
    }

    pub fn len(&self) -> usize {
        self.shown.len()
    }

    pub fn is_empty(&self) -> bool {
        self.shown.is_empty()
    }

    pub fn text(&self, index: usize) -> &str {
        &self.shown[index]
    }

    /// The best `limit` paths for `query`, best first.
    pub fn search(&mut self, query: &str, limit: usize) -> Vec<Ranked> {
        let subset = self
            .last
            .take()
            .filter(|(previous, _)| Query::narrows_to(previous, query))
            .map(|(_, matched)| matched);
        let shown = &self.shown;
        let (ranked, matched) = rank(
            &Query::paths(query),
            &self.haystacks,
            subset.as_deref(),
            limit,
            |i| shown[i].as_str(),
        );
        self.last = Some((query.to_string(), matched));
        ranked
    }
}

/// Rank plain strings (small lists: symbols of a file, tests).
pub fn rank_strings(query: &str, texts: &[&str], paths: bool, limit: usize) -> Vec<Ranked> {
    let query = if paths {
        Query::paths(query)
    } else {
        Query::names(query)
    };
    let haystacks: Vec<Haystack> = texts.iter().map(|t| Haystack::new(t)).collect();
    rank(&query, &haystacks, None, limit, |i| texts[i]).0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn best(query: &str, texts: &[&str]) -> String {
        let ranked = rank_strings(query, texts, true, 10);
        texts[ranked[0].index].to_string()
    }

    #[test]
    fn file_names_win_over_directories() {
        let files = [
            "maintenance/notes/x.txt",
            "src/main.rs",
            "very/long/library/path/x.rs",
        ];
        assert_eq!(best("main", &files), "src/main.rs");
        assert_eq!(best("mainrs", &files), "src/main.rs");
        let files = ["src/editor/mod.rs", "crates/editor_settings/lib.rs"];
        assert_eq!(best("editor mod", &files), "src/editor/mod.rs");
    }

    #[test]
    fn words_anchors_and_exclusions() {
        let files = ["src/main.rs", "src/main.ts", "tests/main.rs"];
        let ranked = rank_strings("main !tests .rs$", &files, true, 10);
        let names: Vec<&str> = ranked.iter().map(|r| files[r.index]).collect();
        assert_eq!(names, vec!["src/main.rs"]);
        assert!(rank_strings("^tests", &files, true, 10).len() == 1);
    }

    #[test]
    fn positions_are_byte_offsets() {
        let ranked = rank_strings("mr", &["src/main.rs"], true, 10);
        assert_eq!(ranked[0].positions, vec![4, 9]);
        let text = "stress/żółć-gęślą.rs";
        let ranked = rank_strings("gęś", &[text], true, 10);
        assert!(!ranked.is_empty());
        for p in &ranked[0].positions {
            assert!(text.is_char_boundary(*p));
        }
        assert_eq!(
            &text[ranked[0].positions[0]..ranked[0].positions[0] + 1],
            "g"
        );
    }

    #[test]
    fn narrowing_rules() {
        assert!(Query::narrows_to("ma", "mai"));
        assert!(!Query::narrows_to("", "m"));
        assert!(!Query::narrows_to("mai", "ma"));
        assert!(!Query::narrows_to("ma", "ma !x"));
        assert!(!Query::narrows_to("rs$", "rs$x"));
    }

    #[test]
    fn narrowed_results_equal_a_fresh_search() {
        let paths: Vec<String> = (0..30_000)
            .map(|i| format!("stress/module_{}/sub_{}/file_{i:05}.rs", i % 47, i % 13))
            .collect();
        let haystacks: Vec<Haystack> = paths.iter().map(|p| Haystack::new(p)).collect();
        let text = |i: usize| paths[i].as_str();
        let (_, matched) = rank(&Query::paths("mod7"), &haystacks, None, 50, text);
        let narrowed = rank(
            &Query::paths("mod7file12"),
            &haystacks,
            Some(&matched),
            50,
            text,
        )
        .0;
        let fresh = rank(&Query::paths("mod7file12"), &haystacks, None, 50, text).0;
        assert_eq!(narrowed, fresh);
        assert!(!fresh.is_empty());
    }
}
