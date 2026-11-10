//! Find and replace within one file: plain text or regular expressions,
//! case sensitivity, whole words, and `$1` / `${name}` groups in the
//! replacement.

use std::ops::Range;

use regex::{Regex, RegexBuilder};

use crate::{edit::Edit, unicode::is_word_char};

/// What to look for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct SearchQuery {
    pub pattern: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
}

impl SearchQuery {
    pub fn plain(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            case_sensitive: true,
            ..Self::default()
        }
    }

    pub fn regex(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            regex: true,
            case_sensitive: true,
            ..Self::default()
        }
    }

    pub fn case_insensitive(mut self) -> Self {
        self.case_sensitive = false;
        self
    }

    pub fn whole_word(mut self) -> Self {
        self.whole_word = true;
        self
    }

    pub fn compile(&self) -> Result<Matcher, SearchError> {
        Matcher::new(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SearchError {
    #[error("the search text is empty")]
    Empty,
    #[error("invalid regular expression: {0}")]
    InvalidRegex(String),
}

/// A compiled [`SearchQuery`].
#[derive(Debug, Clone)]
pub struct Matcher {
    regex: Regex,
    is_regex: bool,
    whole_word: bool,
}

/// Matches are not allowed to grow beyond this many bytes of compiled
/// program, so a pathological pattern fails instead of eating memory.
const REGEX_SIZE_LIMIT: usize = 32 * 1024 * 1024;

impl Matcher {
    fn new(query: &SearchQuery) -> Result<Self, SearchError> {
        if query.pattern.is_empty() {
            return Err(SearchError::Empty);
        }
        let source = if query.regex {
            query.pattern.clone()
        } else {
            regex::escape(&query.pattern)
        };
        let regex = RegexBuilder::new(&source)
            .case_insensitive(!query.case_sensitive)
            .multi_line(true)
            .size_limit(REGEX_SIZE_LIMIT)
            .build()
            .map_err(|err| SearchError::InvalidRegex(err.to_string()))?;
        Ok(Self {
            regex,
            is_regex: query.regex,
            whole_word: query.whole_word,
        })
    }

    fn accepts(&self, text: &str, range: &Range<usize>) -> bool {
        if !self.whole_word {
            return true;
        }
        let before = text[..range.start].chars().next_back();
        let after = text[range.end..].chars().next();
        !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
    }

    /// Every match, in order. Empty matches are included (a regex like `^`
    /// matches every line start).
    pub fn find_all(&self, text: &str) -> Vec<Range<usize>> {
        self.regex
            .find_iter(text)
            .map(|m| m.range())
            .filter(|r| self.accepts(text, r))
            .collect()
    }

    /// The first match starting at or after `from`, wrapping around to the
    /// start when `wrap` is set.
    pub fn find_next(&self, text: &str, from: usize, wrap: bool) -> Option<Range<usize>> {
        self.find_from(text, floor_boundary(text, from))
            .or_else(|| if wrap { self.find_from(text, 0) } else { None })
    }

    fn find_from(&self, text: &str, mut at: usize) -> Option<Range<usize>> {
        while at <= text.len() {
            let found = self.regex.find_at(text, at)?.range();
            if self.accepts(text, &found) {
                return Some(found);
            }
            // Try again just after this match's start.
            at = found.start + text[found.start..].chars().next().map_or(1, char::len_utf8);
        }
        None
    }

    /// The last match ending at or before `before`, wrapping around to the
    /// end when `wrap` is set.
    pub fn find_prev(&self, text: &str, before: usize, wrap: bool) -> Option<Range<usize>> {
        let all = self.find_all(text);
        all.iter()
            .rev()
            .find(|r| r.end <= before && r.start < before)
            .or_else(|| if wrap { all.last() } else { None })
            .cloned()
    }

    /// The text that replaces the match at `range`: the replacement with
    /// `$1`, `${name}` and `$$` expanded in regex mode, or taken literally.
    pub fn replacement(&self, text: &str, range: &Range<usize>, replacement: &str) -> String {
        if !self.is_regex {
            return replacement.to_string();
        }
        let Some(captures) = self.regex.captures_at(text, range.start) else {
            return replacement.to_string();
        };
        let mut out = String::new();
        captures.expand(replacement, &mut out);
        out
    }

    /// Edits replacing every match.
    pub fn replace_all(&self, text: &str, replacement: &str) -> Vec<Edit> {
        if !self.is_regex {
            return self
                .find_all(text)
                .into_iter()
                .map(|r| Edit::new(r, replacement))
                .collect();
        }
        self.regex
            .captures_iter(text)
            .filter_map(|captures| {
                let m = captures.get(0)?;
                if !self.accepts(text, &m.range()) {
                    return None;
                }
                let mut out = String::new();
                captures.expand(replacement, &mut out);
                Some(Edit::new(m.range(), out))
            })
            .collect()
    }
}

fn floor_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::apply_to_string;

    #[test]
    fn plain_case_and_whole_word() {
        let text = "Foo foo food FOO";
        let m = SearchQuery::plain("foo").compile().unwrap();
        assert_eq!(m.find_all(text), vec![4..7, 8..11]);
        let m = SearchQuery::plain("foo")
            .case_insensitive()
            .compile()
            .unwrap();
        assert_eq!(m.find_all(text).len(), 4);
        let m = SearchQuery::plain("foo")
            .case_insensitive()
            .whole_word()
            .compile()
            .unwrap();
        assert_eq!(m.find_all(text), vec![0..3, 4..7, 13..16]);
    }

    #[test]
    fn special_characters_are_literal_in_plain_mode() {
        let m = SearchQuery::plain("a.b(").compile().unwrap();
        assert_eq!(m.find_all("axb( a.b("), vec![5..9]);
    }

    #[test]
    fn next_and_previous_wrap() {
        let text = "x1 x2 x3";
        let m = SearchQuery::regex(r"x\d").compile().unwrap();
        assert_eq!(m.find_next(text, 1, true), Some(3..5));
        assert_eq!(m.find_next(text, 7, false), None);
        assert_eq!(m.find_next(text, 7, true), Some(0..2));
        assert_eq!(m.find_prev(text, 3, true), Some(0..2));
        assert_eq!(m.find_prev(text, 0, true), Some(6..8));
    }

    #[test]
    fn regex_replacement_with_groups() {
        let text = "let a = 1;\nlet b = 2;\n";
        let m = SearchQuery::regex(r"^let (\w+) = (\d+);$")
            .compile()
            .unwrap();
        let edits = m.replace_all(text, "const ${1}: i32 = $2;");
        assert_eq!(
            apply_to_string(text, &edits),
            "const a: i32 = 1;\nconst b: i32 = 2;\n"
        );
        let first = m.find_next(text, 0, false).unwrap();
        assert_eq!(m.replacement(text, &first, "[$1]"), "[a]");
    }

    #[test]
    fn literal_replacement_keeps_dollars() {
        let m = SearchQuery::plain("price").compile().unwrap();
        let edits = m.replace_all("price", "$1");
        assert_eq!(edits[0].text, "$1");
    }

    #[test]
    fn errors() {
        assert_eq!(
            SearchQuery::plain("").compile().unwrap_err(),
            SearchError::Empty
        );
        assert!(matches!(
            SearchQuery::regex("(").compile(),
            Err(SearchError::InvalidRegex(_))
        ));
    }

    #[test]
    fn unicode_text() {
        let text = "zażółć gęślą ZAŻÓŁĆ";
        let m = SearchQuery::plain("zażółć")
            .case_insensitive()
            .compile()
            .unwrap();
        assert_eq!(m.find_all(text).len(), 2);
        assert_eq!(
            m.find_next(text, 3, false),
            Some(text.rfind('Z').unwrap()..text.len())
        );
    }
}
