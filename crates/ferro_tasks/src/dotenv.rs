//! A `.env` file parser for a run configuration's `env_file`.
//!
//! The format is the de-facto one of `dotenv` libraries rather than a
//! shell: `KEY=value` lines, `#` comments and blank lines, an optional
//! `export ` prefix, single quotes taken literally, double quotes with
//! backslash escapes (`\n`, `\t`, `\r`, `\"`, `\\`, `\$`) and both
//! spanning lines, and an unquoted value ending at ` #`.
//!
//! `${VAR}`, `${VAR:-default}` and `$VAR` are expanded in unquoted and
//! double-quoted values, from the entries above first and then from the
//! environment the run inherits (`lookup`), so `PATH=$PATH:./bin` works.
//! An unknown variable expands to nothing, as in a shell.

use std::path::Path;

/// Parse `.env` text; `lookup` resolves variables not defined earlier in
/// the file. Later duplicates override earlier ones.
pub fn parse(text: &str, lookup: impl Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((key, rest)) = line.split_once('=') else {
            tracing::debug!(line, "skipping a .env line without `=`");
            continue;
        };
        let key = key.trim();
        if key.is_empty()
            || !key
                .chars()
                .all(|c| c.is_alphanumeric() || "_.-".contains(c))
        {
            tracing::debug!(key, "skipping a .env entry with an invalid name");
            continue;
        }
        let rest = rest.trim_start();
        let resolve = |name: &str| {
            entries
                .iter()
                .rev()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
                .or_else(|| lookup(name))
        };
        let value = match rest.chars().next() {
            Some(quote @ ('\'' | '"')) => {
                // Gather lines until the closing quote.
                let mut body = rest[1..].to_string();
                let close = loop {
                    if let Some(end) = closing_quote(&body, quote) {
                        break Some(end);
                    }
                    match lines.next() {
                        Some(next) => {
                            body.push('\n');
                            body.push_str(next);
                        }
                        None => break None,
                    }
                };
                let inner = match close {
                    Some(end) => &body[..end],
                    None => body.as_str(),
                };
                if quote == '\'' {
                    inner.to_string()
                } else {
                    expand(&unescape(inner), &resolve)
                }
            }
            _ => {
                let raw = match rest.find(" #").or_else(|| rest.find("\t#")) {
                    Some(comment) => &rest[..comment],
                    None => rest,
                };
                expand(raw.trim_end(), &resolve)
            }
        };
        entries.retain(|(k, _)| k != key);
        entries.push((key.to_string(), value));
    }
    entries
}

/// Read and parse a `.env` file.
pub fn load(
    path: &Path,
    lookup: impl Fn(&str) -> Option<String>,
) -> anyhow::Result<Vec<(String, String)>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
    Ok(parse(&text, lookup))
}

/// Byte offset of the unescaped closing `quote` in `body`.
fn closing_quote(body: &str, quote: char) -> Option<usize> {
    let mut escaped = false;
    for (i, c) in body.char_indices() {
        if quote == '"' && c == '\\' && !escaped {
            escaped = true;
            continue;
        }
        if c == quote && !escaped {
            return Some(i);
        }
        escaped = false;
    }
    None
}

/// Backslash escapes of a double-quoted value. `\$` is kept as a marker
/// (`\u{0}$`) so [`expand`] leaves it alone.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('$') => out.push_str("\u{0}$"),
            Some(other @ ('"' | '\\' | '\'')) => out.push(other),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Expand `${VAR}`, `${VAR:-default}` and `$VAR`.
fn expand(s: &str, resolve: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find('$') {
        let escaped = rest[..pos].ends_with('\u{0}');
        out.push_str(&rest[..pos]);
        if escaped {
            out.pop();
            out.push('$');
            rest = &rest[pos + 1..];
            continue;
        }
        let after = &rest[pos + 1..];
        if let Some(braced) = after.strip_prefix('{')
            && let Some(end) = braced.find('}')
        {
            {
                let inner = &braced[..end];
                let (name, default) = match inner.split_once(":-") {
                    Some((name, default)) => (name, Some(default)),
                    None => (inner, None),
                };
                let value = resolve(name).filter(|v| !v.is_empty() || default.is_none());
                out.push_str(&value.unwrap_or_else(|| default.unwrap_or_default().to_string()));
                rest = &braced[end + 1..];
                continue;
            }
        }
        let len = after
            .char_indices()
            .find(|(_, c)| !(c.is_alphanumeric() || *c == '_'))
            .map_or(after.len(), |(i, _)| i);
        if len == 0 {
            out.push('$');
            rest = after;
        } else {
            out.push_str(&resolve(&after[..len]).unwrap_or_default());
            rest = &after[len..];
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(name: &str) -> Option<String> {
        match name {
            "HOME" => Some("/home/ada".into()),
            "PATH" => Some("/usr/bin".into()),
            _ => None,
        }
    }

    fn get<'a>(entries: &'a [(String, String)], key: &str) -> &'a str {
        &entries.iter().find(|(k, _)| k == key).unwrap().1
    }

    #[test]
    fn parses_the_usual_forms() {
        let text = "\u{feff}# database\n\
            \n\
            DB_HOST=localhost\n\
            export DB_PORT = 5432\n\
            NAME='Ada $HOME \\n'\n\
            GREETING=\"Hello\\n\\\"World\\\" \\$HOME\"\n\
            URL=postgres://${DB_HOST}:$DB_PORT/app # trailing comment\n\
            PATH=$PATH:./bin\n\
            DATA_DIR=${HOME}/data\n\
            FALLBACK=${MISSING:-none}\n\
            EMPTY=\n\
            HASH=a#b\n\
            not a variable\n\
            DB_HOST=127.0.0.1\n";
        let entries = parse(text, env);
        assert_eq!(get(&entries, "DB_PORT"), "5432");
        assert_eq!(get(&entries, "NAME"), "Ada $HOME \\n");
        assert_eq!(get(&entries, "GREETING"), "Hello\n\"World\" $HOME");
        assert_eq!(get(&entries, "URL"), "postgres://localhost:5432/app");
        assert_eq!(get(&entries, "PATH"), "/usr/bin:./bin");
        assert_eq!(get(&entries, "DATA_DIR"), "/home/ada/data");
        assert_eq!(get(&entries, "FALLBACK"), "none");
        assert_eq!(get(&entries, "EMPTY"), "");
        assert_eq!(get(&entries, "HASH"), "a#b");
        // The later definition wins and keeps one entry.
        assert_eq!(get(&entries, "DB_HOST"), "127.0.0.1");
        assert_eq!(entries.iter().filter(|(k, _)| k == "DB_HOST").count(), 1);
        assert_eq!(entries.len(), 10);
    }

    #[test]
    fn quoted_values_span_lines() {
        let text = "KEY=\"-----BEGIN-----\nabc\n-----END-----\"\nNEXT='a\nb'\nLAST=1";
        let entries = parse(text, env);
        assert_eq!(get(&entries, "KEY"), "-----BEGIN-----\nabc\n-----END-----");
        assert_eq!(get(&entries, "NEXT"), "a\nb");
        assert_eq!(get(&entries, "LAST"), "1");
    }

    #[test]
    fn unterminated_quote_takes_the_rest() {
        let entries = parse("A=\"open\nB=2", env);
        assert_eq!(get(&entries, "A"), "open\nB=2");
    }
}
