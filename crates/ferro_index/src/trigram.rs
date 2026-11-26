//! The optional trigram index for very large repositories: every file's set
//! of byte trigrams (ASCII-lowercased) is stored, and a literal query is
//! answered only from the files holding all of its trigrams.

use ferro_services::TextQuery;

/// Settings value `[search] trigram_index`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrigramMode {
    Off,
    On,
    /// On when the project's indexed files exceed the threshold.
    #[default]
    Auto,
}

/// `[search] trigram_index = "auto"` turns the index on above this many
/// bytes of text (plan §8 Stage 7: "for repositories above a few GB").
pub const AUTO_THRESHOLD: u64 = 2 * 1024 * 1024 * 1024;

/// At most this many trigrams of a query are looked up; more only narrow
/// the result a little and slow the query down.
const MAX_QUERY_TRIGRAMS: usize = 12;

fn pack(window: &[u8]) -> u32 {
    (u32::from(window[0]) << 16) | (u32::from(window[1]) << 8) | u32::from(window[2])
}

/// The distinct trigrams of a file's content, sorted.
pub fn of_text(bytes: &[u8]) -> Vec<u32> {
    let lowered: Vec<u8> = bytes.iter().map(u8::to_ascii_lowercase).collect();
    let mut out: Vec<u32> = lowered
        .windows(3)
        .filter(|w| !w.contains(&b'\n'))
        .map(pack)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The trigrams a matching file must contain, or `None` when the query
/// cannot be narrowed (regular expressions, text shorter than three bytes).
pub fn of_query(query: &TextQuery) -> Option<Vec<u32>> {
    if query.regex {
        return None;
    }
    let lowered: Vec<u8> = query.text.bytes().map(|b| b.to_ascii_lowercase()).collect();
    // Case-insensitive matching folds more than ASCII; only all-ASCII
    // windows are safe then (`Ż` and `ż` differ in bytes).
    let mut out: Vec<u32> = lowered
        .windows(3)
        .filter(|w| !w.contains(&b'\n'))
        .filter(|w| query.case_sensitive || w.is_ascii())
        .map(pack)
        .collect();
    out.sort_unstable();
    out.dedup();
    if out.is_empty() {
        return None;
    }
    // Spread the lookups over the whole query.
    if out.len() > MAX_QUERY_TRIGRAMS {
        let step = out.len() as f64 / MAX_QUERY_TRIGRAMS as f64;
        out = (0..MAX_QUERY_TRIGRAMS)
            .map(|i| out[(i as f64 * step) as usize])
            .collect();
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigrams_fold_ascii_case_and_skip_line_breaks() {
        assert_eq!(of_text(b"AbC"), of_text(b"abc"));
        assert_eq!(of_text(b"ab\ncd"), Vec::<u32>::new());
        assert_eq!(of_text(b"aaaa").len(), 1);
    }

    #[test]
    fn queries_need_three_literal_bytes() {
        let q = |text: &str, regex: bool| TextQuery {
            text: text.into(),
            regex,
            ..TextQuery::default()
        };
        assert!(of_query(&q("ab", false)).is_none());
        assert!(of_query(&q("a.*b", true)).is_none());
        let found = of_query(&q("Needle", false)).unwrap();
        let text = of_text(b"a haystack with a needle in it");
        assert!(found.iter().all(|t| text.binary_search(t).is_ok()));
        assert!(
            of_query(&q("żółw", false)).is_none(),
            "no ASCII-only window"
        );
    }
}
