//! Command lines: splitting, quoting and finding the program to run.
//!
//! A run configuration's command line is split here rather than handed to
//! a shell, so a run behaves the same on every platform and stopping it
//! stops the program itself, not a shell around it. The quoting is a
//! small, predictable subset of POSIX shells:
//!
//! - words are separated by whitespace;
//! - `'single quotes'` are literal;
//! - `"double quotes"` treat `\"` and `\\` as escapes and keep any other
//!   backslash;
//! - outside quotes a backslash escapes only whitespace and quotes, so
//!   Windows paths such as `C:\work\app` need no doubling.
//!
//! [`quote`] produces words [`split`] reads back unchanged, for the command
//! lines this crate writes itself (rerunning failed tests, ▶ icons).
//!
//! [`resolve_program`] finds the executable along `PATH`. On Windows it
//! honours `PATHEXT`, so `npm`, `npx`, `mvn` and `gradle` resolve to their
//! `.cmd` / `.bat` launchers (which `std::process::Command` knows how to
//! run). Maven and Gradle prefer a project's wrapper (`mvnw`, `gradlew`) in
//! the working directory or one of its parents inside the project.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Split a command line into words. Unbalanced quotes run to the end.
pub fn split(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    word.push(c);
                }
            }
            '"' => {
                in_word = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' if matches!(chars.peek(), Some('"' | '\\')) => {
                            word.push(chars.next().unwrap_or('\\'));
                        }
                        c => word.push(c),
                    }
                }
            }
            '\\' if chars
                .peek()
                .is_some_and(|n| n.is_whitespace() || *n == '"' || *n == '\'') =>
            {
                in_word = true;
                word.push(chars.next().unwrap_or('\\'));
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

/// Quote `word` so that [`split`] reads it back unchanged.
pub fn quote(word: &str) -> String {
    if word.is_empty() {
        return "''".into();
    }
    // Backslashes may stay bare (Windows paths) unless one would escape
    // the space that follows the word.
    let plain = !word.ends_with('\\')
        && !word.starts_with(['#', '~'])
        && word.chars().all(|c| {
            !c.is_whitespace()
                && !matches!(
                    c,
                    '\'' | '"'
                        | '$'
                        | '`'
                        | '|'
                        | '&'
                        | ';'
                        | '<'
                        | '>'
                        | '('
                        | ')'
                        | '*'
                        | '?'
                        | '['
                        | ']'
                        | '{'
                        | '}'
                        | '^'
                        | '!'
                )
        });
    if plain {
        return word.to_string();
    }
    if !word.contains('\'') {
        return format!("'{word}'");
    }
    let mut out = String::from("\"");
    for c in word.chars() {
        if matches!(c, '"' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// Join words into a command line with [`quote`].
pub fn join<S: AsRef<str>>(words: &[S]) -> String {
    words
        .iter()
        .map(|w| quote(w.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The program's name without directory and executable extension, in
/// lower case: `C:\tools\Gradlew.BAT` → `gradlew`.
pub fn program_stem(program: &str) -> String {
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let lower = name.to_ascii_lowercase();
    for ext in [".exe", ".cmd", ".bat", ".com", ".ps1"] {
        if let Some(stem) = lower.strip_suffix(ext) {
            return stem.to_string();
        }
    }
    lower
}

/// Find the executable for `program`. `path_var` is the run's `PATH`
/// (`None` for the process's own); `cwd` the run's working directory,
/// against which programs with a directory part are resolved; `root` the
/// project root bounding the search for build tool wrappers.
pub fn resolve_program(
    program: &str,
    cwd: &Path,
    root: &Path,
    path_var: Option<&OsStr>,
) -> Option<PathBuf> {
    let wrapper = match program {
        "mvn" => Some("mvnw"),
        "gradle" => Some("gradlew"),
        _ => None,
    };
    if let Some(wrapper) = wrapper {
        let mut dir = Some(cwd);
        while let Some(d) = dir {
            if let Some(found) = executable_in(&d.join(wrapper)) {
                return Some(found);
            }
            if d == root || !d.starts_with(root) {
                break;
            }
            dir = d.parent();
        }
    }
    if program.contains(['/', '\\']) {
        let path = Path::new(program);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        return executable_in(&path);
    }
    let path_var = path_var
        .map(OsStr::to_os_string)
        .or_else(|| std::env::var_os("PATH"))?;
    std::env::split_paths(&path_var)
        .filter(|dir| !dir.as_os_str().is_empty())
        .find_map(|dir| executable_in(&dir.join(program)))
}

/// `path` itself or, on Windows, `path` with one of `PATHEXT`'s extensions
/// — whichever is an executable file.
fn executable_in(path: &Path) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        let exts: Vec<String> = exts
            .split(';')
            .filter(|e| !e.is_empty())
            .map(str::to_ascii_lowercase)
            .collect();
        let has_ext = path
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|e| exts.contains(&format!(".{}", e.to_ascii_lowercase())));
        if has_ext && path.is_file() {
            return Some(path.to_path_buf());
        }
        // Not `path` itself: `npm` next to `npm.cmd` is a shell script.
        exts.iter().find_map(|ext| {
            let mut candidate = path.as_os_str().to_os_string();
            candidate.push(ext);
            let candidate = PathBuf::from(candidate);
            candidate.is_file().then_some(candidate)
        })
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(path).ok()?;
        (meta.is_file() && meta.permissions().mode() & 0o111 != 0).then(|| path.to_path_buf())
    }
}

/// Append `extra` words to a command line, after a `--` separator when
/// `separator` is set and the line has none yet (`npm test -- -t name`).
pub fn append(line: &str, extra: &[String], separator: bool) -> String {
    if extra.is_empty() {
        return line.to_string();
    }
    let mut out = line.trim_end().to_string();
    if separator && !split(line).iter().any(|w| w == "--") {
        out.push_str(" --");
    }
    out.push(' ');
    out.push_str(&join(extra));
    out
}

/// Escape `text` for a regular expression (Go's `-run`, Jest's `-t`).
pub fn regex_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_like_a_shell() {
        assert_eq!(
            split(r#"cargo run -- --names "Ada Lovelace" 'it''s' a\ b"#),
            [
                "cargo",
                "run",
                "--",
                "--names",
                "Ada Lovelace",
                "its",
                "a b"
            ]
        );
        assert_eq!(
            split(r#"go test -run '^(A|B)$' ./..."#),
            ["go", "test", "-run", "^(A|B)$", "./..."]
        );
        assert_eq!(
            split(r#"echo "say \"hi\" C:\dir\" x"#),
            ["echo", r#"say "hi" C:\dir" x"#]
        );
        assert_eq!(
            split(r"C:\tools\app.exe --path C:\work"),
            [r"C:\tools\app.exe", "--path", r"C:\work"]
        );
        assert_eq!(split("a ''  \"\" b"), ["a", "", "", "b"]);
        assert_eq!(split("  "), Vec::<String>::new());
        assert_eq!(split("'unclosed arg"), ["unclosed arg"]);
    }

    #[test]
    fn quote_round_trips() {
        for word in [
            "plain",
            "",
            "two words",
            "it's",
            r#"say "hi""#,
            r"C:\work\app",
            r"C:\Program Files\x",
            r"C:\dir\",
            "^(A|B)$",
            r#"both ' and " \ "#,
            "FullyQualifiedName=A|FullyQualifiedName=B",
        ] {
            let line = join(&["cmd", word]);
            assert_eq!(split(&line), ["cmd", word], "{line}");
        }
        assert_eq!(quote(r"C:\work\app"), r"C:\work\app");
        assert_eq!(quote("^(A|B)$"), "'^(A|B)$'");
    }

    #[test]
    fn program_stems() {
        assert_eq!(program_stem(r"C:\tools\Gradlew.BAT"), "gradlew");
        assert_eq!(program_stem("./mvnw"), "mvnw");
        assert_eq!(program_stem("python3"), "python3");
    }

    #[test]
    fn appends_with_separator() {
        let extra = vec!["-t".to_string(), "adds numbers".to_string()];
        assert_eq!(
            append("npm test", &extra, true),
            "npm test -- -t 'adds numbers'"
        );
        assert_eq!(
            append("npm test -- --ci", &extra, true),
            "npm test -- --ci -t 'adds numbers'"
        );
        assert_eq!(
            append("npx jest", &extra, false),
            "npx jest -t 'adds numbers'"
        );
    }

    #[test]
    fn resolves_wrappers_and_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let module = root.join("module");
        std::fs::create_dir_all(&module).unwrap();
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        #[cfg(windows)]
        let (wrapper, tool) = ("gradlew.bat", "mytool.cmd");
        #[cfg(not(windows))]
        let (wrapper, tool) = ("gradlew", "mytool");
        for path in [root.join(wrapper), bin.join(tool)] {
            std::fs::write(&path, "echo").unwrap();
            #[cfg(not(windows))]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        // A shell script without extension next to the launcher is not it.
        std::fs::write(bin.join("other"), "#!/bin/sh").unwrap();
        let path_var = std::env::join_paths([&bin]).unwrap();
        assert_eq!(
            resolve_program("gradle", &module, root, Some(&path_var)),
            Some(root.join(wrapper))
        );
        assert_eq!(
            resolve_program("mytool", &module, root, Some(&path_var)),
            Some(bin.join(tool))
        );
        assert_eq!(
            resolve_program("other", &module, root, Some(&path_var)),
            None
        );
        assert_eq!(
            resolve_program("missing", &module, root, Some(&path_var)),
            None
        );
        assert_eq!(
            resolve_program("../gradlew", &module, root, Some(&path_var)),
            Some(module.join("..").join(wrapper))
        );
    }

    #[test]
    fn escapes_regex() {
        assert_eq!(regex_escape("adds (1 + 2)"), r"adds \(1 \+ 2\)");
    }
}
