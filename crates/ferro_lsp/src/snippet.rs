//! The LSP snippet format (`$1`, `${1:name}`, `${1|a,b|}`, `$TM_FILENAME`,
//! `\$`) as plain text.
//!
//! GPUI Kit's editor has no snippet session (tab stops you jump between),
//! so a snippet is inserted with each placeholder's default text and the
//! first choice of each choice list.

/// The text of a snippet: placeholders by their default text, tab stops and
/// unknown variables removed, escapes resolved.
pub fn to_text(snippet: &str) -> String {
    let chars: Vec<char> = snippet.chars().collect();
    let mut out = String::with_capacity(snippet.len());
    parse(&chars, &mut 0, &mut out, false);
    out
}

/// Parse until the end, or until an unescaped `}` when `nested`.
fn parse(chars: &[char], i: &mut usize, out: &mut String, nested: bool) {
    while *i < chars.len() {
        let c = chars[*i];
        match c {
            '\\' if *i + 1 < chars.len()
                && matches!(chars[*i + 1], '$' | '}' | '\\' | ',' | '|') =>
            {
                out.push(chars[*i + 1]);
                *i += 2;
            }
            '}' if nested => {
                *i += 1;
                return;
            }
            '$' => {
                *i += 1;
                dollar(chars, i, out);
            }
            _ => {
                out.push(c);
                *i += 1;
            }
        }
    }
}

fn dollar(chars: &[char], i: &mut usize, out: &mut String) {
    let Some(&next) = chars.get(*i) else {
        out.push('$');
        return;
    };
    if next.is_ascii_digit() {
        while chars.get(*i).is_some_and(char::is_ascii_digit) {
            *i += 1;
        }
        return;
    }
    if next.is_ascii_alphabetic() || next == '_' {
        let start = *i;
        while chars
            .get(*i)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
        {
            *i += 1;
        }
        let name: String = chars[start..*i].iter().collect();
        out.push_str(&variable(&name));
        return;
    }
    if next != '{' {
        out.push('$');
        return;
    }
    *i += 1;
    // `${1`, `${name`
    let start = *i;
    while chars
        .get(*i)
        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
    {
        *i += 1;
    }
    let name: String = chars[start..*i].iter().collect();
    match chars.get(*i) {
        Some('}') => {
            *i += 1;
            if !name.chars().all(|c| c.is_ascii_digit()) {
                out.push_str(&variable(&name));
            }
        }
        Some(':') => {
            *i += 1;
            parse(chars, i, out, true);
        }
        Some('|') => {
            *i += 1;
            // The first choice; skip to the closing `|}`.
            let mut first = String::new();
            let mut in_first = true;
            while *i < chars.len() {
                match chars[*i] {
                    '\\' if *i + 1 < chars.len() => {
                        if in_first {
                            first.push(chars[*i + 1]);
                        }
                        *i += 2;
                        continue;
                    }
                    ',' => in_first = false,
                    '|' if chars.get(*i + 1) == Some(&'}') => {
                        *i += 2;
                        break;
                    }
                    c if in_first => first.push(c),
                    _ => {}
                }
                *i += 1;
            }
            out.push_str(&first);
        }
        _ => {
            // `${1/regex/format/}` and malformed input: drop to the brace.
            while *i < chars.len() && chars[*i] != '}' {
                *i += 1;
            }
            *i += 1;
        }
    }
}

/// Snippet variables FerroIDE resolves without context are empty.
fn variable(_name: &str) -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::to_text;

    #[test]
    fn placeholders_become_their_text() {
        assert_eq!(to_text("println!(\"$1\")$0"), "println!(\"\")");
        assert_eq!(
            to_text("fn ${1:name}(${2:args}) {\n\t$0\n}"),
            "fn name(args) {\n\t\n}"
        );
        assert_eq!(to_text("${1:outer ${2:inner}}"), "outer inner");
        assert_eq!(to_text("${1|one,two,three|}"), "one");
        assert_eq!(to_text("cost: \\$5 \\}"), "cost: $5 }");
        assert_eq!(to_text("$TM_SELECTED_TEXT!"), "!");
        assert_eq!(to_text("a $ b"), "a $ b");
        assert_eq!(to_text("${1/(.*)/$1/}x"), "x");
    }
}
