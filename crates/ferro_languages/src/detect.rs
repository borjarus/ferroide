//! Reading a file's language from its first lines: shebangs and modelines.

use std::sync::LazyLock;

use regex::Regex;

/// The interpreter named on a `#!` line: `#!/usr/bin/env -S python3 -u` →
/// `python3`, `#!/bin/bash` → `bash`.
pub fn shebang_interpreter(first_line: &str) -> Option<&str> {
    let rest = first_line.strip_prefix("#!")?.trim();
    let mut words = rest.split_whitespace();
    let program = words.next()?;
    let name = program.rsplit('/').next()?;
    if name == "env" {
        // Skip env's options (`-S`, `-i`, `VAR=value`).
        return words.find(|w| !w.starts_with('-') && !w.contains('='));
    }
    Some(name)
}

/// Whether an interpreter name matches a pack's shebang entry, ignoring a
/// version suffix: `python3.12` matches `python3` and `python`.
pub fn interpreter_matches(interpreter: &str, entry: &str) -> bool {
    if interpreter == entry {
        return true;
    }
    interpreter
        .strip_prefix(entry)
        .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit() || c == '.'))
}

static VIM: LazyLock<Regex> = LazyLock::new(|| {
    // `vim: set ft=python :`, `vi:ft=sh`, `ex: syntax=ruby`
    Regex::new(r"(?:^|\s)(?:vim?|ex):\s*(?:set?\s+)?(?:[^:]*\s)?(?:ft|filetype|syntax)=([\w.+#-]+)")
        .unwrap()
});
static EMACS: LazyLock<Regex> = LazyLock::new(|| {
    // `-*- mode: python -*-`, `-*- python -*-`, `-*- coding: utf-8; mode: ruby -*-`
    Regex::new(r"-\*-\s*(?:.*?\bmode:\s*([\w.+#-]+)|([\w.+#-]+))\s*(?:;.*)?-\*-").unwrap()
});

/// The language a Vim or Emacs modeline in `text` names. Modelines are
/// looked for in the first and last five lines, as Vim does.
pub fn modeline_language(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let head = lines.iter().take(5);
    let tail = lines.iter().rev().take(5);
    for line in head.chain(tail) {
        if let Some(captures) = VIM.captures(line) {
            return Some(captures[1].to_lowercase());
        }
        if let Some(captures) = EMACS.captures(line) {
            let name = captures.get(1).or_else(|| captures.get(2))?.as_str();
            // `-*- coding: utf-8 -*-` names no mode.
            if !name.contains(':') {
                return Some(name.to_lowercase());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shebangs() {
        assert_eq!(shebang_interpreter("#!/bin/bash"), Some("bash"));
        assert_eq!(
            shebang_interpreter("#!/usr/bin/env python3"),
            Some("python3")
        );
        assert_eq!(
            shebang_interpreter("#!/usr/bin/env -S node --harmony"),
            Some("node")
        );
        assert_eq!(shebang_interpreter("#! /usr/bin/ruby -w"), Some("ruby"));
        assert_eq!(shebang_interpreter("// not a shebang"), None);
        assert!(interpreter_matches("python3.12", "python3"));
        assert!(interpreter_matches("python3.12", "python"));
        assert!(!interpreter_matches("pythonista", "python"));
    }

    #[test]
    fn modelines() {
        assert_eq!(
            modeline_language("# vim: set ft=python :\n"),
            Some("python".into())
        );
        assert_eq!(
            modeline_language("x\n// vim:ft=javascript\n"),
            Some("javascript".into())
        );
        assert_eq!(
            modeline_language("# -*- mode: ruby -*-\n"),
            Some("ruby".into())
        );
        assert_eq!(modeline_language(";; -*- lisp -*-\n"), Some("lisp".into()));
        assert_eq!(
            modeline_language("# -*- coding: utf-8; mode: python -*-"),
            Some("python".into())
        );
        assert_eq!(modeline_language("# -*- coding: utf-8 -*-"), None);
        assert_eq!(modeline_language("no modeline here"), None);
    }
}
