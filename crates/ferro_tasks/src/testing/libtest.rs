//! `cargo test` (libtest's human output) and `cargo nextest run` (its
//! human output), read line by line while the run goes on.
//!
//! libtest has no stable machine-readable format (`--format json` needs
//! nightly), so its text output is the interface:
//!
//! - `Running unittests src/lib.rs (target/debug/deps/demo-1a2b…)` starts
//!   a test binary — the suite; `Doc-tests demo` the doc-tests;
//! - `test a::b::c ... ok | FAILED | ignored[, reason]` is a result;
//! - after the results, `---- a::b::c stdout ----` sections hold each
//!   failed test's captured output (the panic message among it), up to
//!   the second `failures:` list.
//!
//! A test's module path becomes nested suites under its binary, as in the
//! Project view; the [`TestId`] keeps the full path for `--exact`.
//!
//! nextest prints `PASS [ 0.004s] crate::bin a::b` per test, and the
//! output of failed tests in sections opened by `--- STDOUT: crate a::b ---`
//! (older releases) or `──── STDOUT: crate a::b` (newer ones).

use std::sync::LazyLock;

use ferro_services::TestStatus;
use regex::Regex;

use super::{Cases, TestId, millis, strip_ansi};

static RUNNING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*Running (?:(unittests|benches) )?(\S+)(?: \((.+)\))?\s*$").unwrap()
});
static DOC_TESTS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*Doc-tests (\S+)\s*$").unwrap());
static RESULT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^test (.+?) \.\.\. (ok|FAILED|ignored(?:, (.*?))?|bench:.*?)(?:\s+<([\d.]+)s>)?\s*$",
    )
    .unwrap()
});
static SECTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^---- (.+?) std(?:out|err) ----$").unwrap());
static HASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-[0-9a-f]{16}$").unwrap());

/// Reads libtest output.
#[derive(Debug, Default)]
pub struct LibtestParser {
    suite: String,
    /// The failed test whose output is being collected.
    section: Option<TestId>,
    /// `failures:` lines seen in the current binary.
    failures_headers: u32,
}

impl LibtestParser {
    /// Feed one line; returns whether `cases` changed.
    pub fn line(&mut self, raw: &str, cases: &mut Cases) -> bool {
        let line = strip_ansi(raw);
        let trimmed = line.trim_end();
        if let Some(c) = RUNNING.captures(trimmed) {
            self.suite = suite_name(c.get(1).is_some(), &c[2], c.get(3).map(|m| m.as_str()));
            self.section = None;
            self.failures_headers = 0;
            return false;
        }
        if let Some(c) = DOC_TESTS.captures(trimmed) {
            self.suite = format!("{} (doc-tests)", &c[1]);
            self.section = None;
            self.failures_headers = 0;
            return false;
        }
        if trimmed == "failures:" {
            self.failures_headers += 1;
            if self.failures_headers >= 2 {
                self.section = None;
            }
            return false;
        }
        if trimmed.starts_with("test result:") {
            self.section = None;
            return false;
        }
        if let Some(c) = SECTION.captures(trimmed) {
            let id = TestId {
                scope: self.suite.clone(),
                name: c[1].to_string(),
            };
            if let Some(case) = cases.get_mut(&id) {
                case.output.clear();
            }
            self.section = Some(id);
            return false;
        }
        if let Some(id) = &self.section {
            if let Some(case) = cases.get_mut(id) {
                if !(trimmed.is_empty() && case.output.is_empty()) {
                    case.output.push(raw.to_string());
                }
                return true;
            }
            return false;
        }
        if let Some(c) = RESULT.captures(trimmed) {
            let name = c[1].to_string();
            let outcome = &c[2];
            let status = if outcome == "ok" || outcome.starts_with("bench:") {
                TestStatus::Passed
            } else if outcome == "FAILED" {
                TestStatus::Failed
            } else {
                TestStatus::Ignored
            };
            let id = TestId {
                scope: self.suite.clone(),
                name: name.clone(),
            };
            let suite = self.suite.clone();
            let case = cases.upsert(id, || suite_path(&suite, &name), || leaf(&name));
            case.status = status;
            if let Some(reason) = c.get(3) {
                case.output = vec![format!("ignored: {}", reason.as_str())];
            }
            if let Some(secs) = c.get(4).and_then(|m| m.as_str().parse::<f64>().ok()) {
                case.duration_ms = millis(secs);
            }
            return true;
        }
        false
    }
}

/// The suite of a `Running` line: `unittests src/lib.rs (…/demo-1a2b…)` →
/// `demo`, `src/main.rs` → `demo (src/main.rs)`, `tests/smoke.rs (…)` →
/// `smoke`; the old `Running target/debug/deps/smoke-1a2b…` → `smoke`.
fn suite_name(unit: bool, source: &str, binary: Option<&str>) -> String {
    let stem = |path: &str| {
        let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let file = file.strip_suffix(".exe").unwrap_or(file);
        HASH.replace(file, "").into_owned()
    };
    match binary {
        Some(binary) if unit => {
            let crate_name = stem(binary);
            let source = source.replace('\\', "/");
            if source.ends_with("lib.rs") {
                crate_name
            } else {
                format!("{crate_name} ({source})")
            }
        }
        Some(_) => stem(source).trim_end_matches(".rs").to_string(),
        None => stem(source),
    }
}

/// Suites of a libtest name: the binary, then the module path (doc-tests,
/// named `src/lib.rs - add (line 5)`, have none).
fn suite_path(suite: &str, name: &str) -> Vec<String> {
    let mut path = vec![suite.to_string()];
    if !name.contains(' ') {
        let parts: Vec<&str> = name.split("::").collect();
        path.extend(parts[..parts.len() - 1].iter().map(|s| s.to_string()));
    }
    path
}

fn leaf(name: &str) -> String {
    if name.contains(' ') {
        name.to_string()
    } else {
        name.rsplit("::").next().unwrap_or(name).to_string()
    }
}

static NEXTEST_STATUS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s*(?:TRY \d+ )?(PASS|FAIL|SKIP|SKIPPED|IGNORED|TIMEOUT|FLAKY|LEAK|LEAK-FAIL|ABORT|SIG[A-Z]+|START|RETRY)\s+\[\s*(?:([\d.]+)s)?\s*\]\s+(?:\(\s*\d+/\d+\)\s+)?(\S+)\s+(\S+)\s*$",
    )
    .unwrap()
});
static NEXTEST_SECTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:-{2,}|─+)\s*(?:TRY \d+ )?STD(?:OUT|ERR):\s+(\S+)\s+(\S+?)(?:\s+-{2,})?\s*$")
        .unwrap()
});
static NEXTEST_OUTPUT_HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:stdout|stderr|output)\s+─+\s*$").unwrap());

/// Reads nextest's human output.
#[derive(Debug, Default)]
pub struct NextestParser {
    section: Option<TestId>,
    last_failed: Option<TestId>,
    summary: bool,
}

impl NextestParser {
    pub fn line(&mut self, raw: &str, cases: &mut Cases) -> bool {
        let line = strip_ansi(raw);
        let trimmed = line.trim_end();
        let content = trimmed.trim_start();
        if content.starts_with("Summary [") {
            self.summary = true;
            self.section = None;
            return false;
        }
        if let Some(c) = NEXTEST_SECTION.captures(trimmed) {
            let id = TestId {
                scope: c[1].to_string(),
                name: c[2].to_string(),
            };
            let known = cases.get_mut(&id).is_some();
            self.section = known.then_some(id);
            return false;
        }
        if NEXTEST_OUTPUT_HEADER.is_match(trimmed) {
            self.section = self.last_failed.clone();
            return false;
        }
        if let Some(c) = NEXTEST_STATUS.captures(trimmed) {
            self.section = None;
            if self.summary {
                return false;
            }
            let word = &c[1];
            let id = TestId {
                scope: c[3].to_string(),
                name: c[4].to_string(),
            };
            let status = match word {
                "PASS" | "FLAKY" | "LEAK" => TestStatus::Passed,
                "SKIP" | "SKIPPED" | "IGNORED" => TestStatus::Ignored,
                "START" | "RETRY" => TestStatus::Running,
                _ => TestStatus::Failed,
            };
            let (scope, name) = (id.scope.clone(), id.name.clone());
            let case = cases.upsert(id.clone(), || suite_path(&scope, &name), || leaf(&name));
            case.status = status;
            if let Some(secs) = c.get(2).and_then(|m| m.as_str().parse::<f64>().ok()) {
                case.duration_ms = millis(secs);
            }
            if status == TestStatus::Failed {
                case.output.clear();
                self.last_failed = Some(id);
            }
            return true;
        }
        let separator = content.len() >= 8 && content.chars().all(|c| c == '-');
        if separator
            || content.starts_with("────")
            || content.starts_with("Cancelling")
            || content.starts_with("Canceling")
        {
            self.section = None;
            return false;
        }
        if let Some(id) = &self.section
            && let Some(case) = cases.get_mut(id)
        {
            if !(trimmed.is_empty() && case.output.is_empty()) {
                case.output.push(raw.to_string());
            }
            return true;
        }
        false
    }
}

/// Drop trailing blank lines of failure outputs.
pub fn trim_outputs(cases: &mut Cases) {
    for case in cases.list_mut() {
        while case
            .output
            .last()
            .is_some_and(|l| strip_ansi(l).trim().is_empty())
        {
            case.output.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::build_tree;

    const CARGO_TEST: &str = r#"   Compiling demo v0.1.0 (D:\work\demo)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.02s
     Running unittests src\lib.rs (target\debug\deps\demo-0f3c8a9d2b1e4f5a.exe)

running 4 tests
test greet::tests::greets_by_name ... ok
test greet::tests::greets_in_unicode ... ignored, needs ICU data
test greet::tests::shouts ... FAILED
test top_level ... ok

failures:

---- greet::tests::shouts stdout ----

thread 'greet::tests::shouts' panicked at src\greet.rs:21:9:
assertion `left == right` failed
  left: "hello"
 right: "HELLO"
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    greet::tests::shouts

test result: FAILED. 2 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/smoke.rs (target/debug/deps/smoke-aa3c8a9d2b1e4f5a)

running 1 test
test prints_greeting ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

   Doc-tests demo

running 1 test
test src/lib.rs - add (line 5) ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.20s
"#;

    #[test]
    fn parses_cargo_test_output() {
        let mut cases = Cases::default();
        let mut parser = LibtestParser::default();
        for line in CARGO_TEST.lines() {
            parser.line(line, &mut cases);
        }
        trim_outputs(&mut cases);
        let list = cases.list();
        assert_eq!(list.len(), 6);
        let shouts = &list[2];
        assert_eq!(shouts.id.scope, "demo");
        assert_eq!(shouts.id.name, "greet::tests::shouts");
        assert_eq!(shouts.suite, ["demo", "greet", "tests"]);
        assert_eq!(shouts.name, "shouts");
        assert_eq!(shouts.status, TestStatus::Failed);
        assert_eq!(
            shouts.output.first().map(String::as_str),
            Some(r"thread 'greet::tests::shouts' panicked at src\greet.rs:21:9:")
        );
        assert_eq!(shouts.output.len(), 5);
        assert_eq!(list[1].status, TestStatus::Ignored);
        assert_eq!(list[1].output, ["ignored: needs ICU data"]);
        assert_eq!(list[4].suite, ["smoke"]);
        assert_eq!(list[5].suite, ["demo (doc-tests)"]);
        assert_eq!(list[5].name, "src/lib.rs - add (line 5)");

        let tree = build_tree(list);
        assert_eq!(tree.len(), 3);
        assert_eq!(tree[0].name, "demo");
        assert_eq!(tree[0].status, TestStatus::Failed);
        assert_eq!(tree[0].children[0].name, "greet");
        assert_eq!(tree[0].children[1].name, "top_level");
        assert_eq!(tree[1].status, TestStatus::Passed);
    }

    #[test]
    fn suite_names() {
        assert_eq!(
            suite_name(
                true,
                "src/main.rs",
                Some("target/debug/deps/app-0123456789abcdef")
            ),
            "app (src/main.rs)"
        );
        assert_eq!(
            suite_name(false, "target/debug/deps/smoke-0123456789abcdef", None),
            "smoke"
        );
        assert_eq!(
            suite_name(
                false,
                "tests/api.rs",
                Some("target/debug/deps/api-0123456789abcdef")
            ),
            "api"
        );
    }

    const NEXTEST_OLD: &str = r#"    Finished test [unoptimized + debuginfo] target(s) in 0.05s
    Starting 3 tests across 2 binaries
        PASS [   0.004s] demo greet::tests::greets_by_name
        FAIL [   0.005s] demo greet::tests::shouts

--- STDOUT:              demo greet::tests::shouts ---

running 1 test
test greet::tests::shouts ... FAILED

--- STDERR:              demo greet::tests::shouts ---
thread 'greet::tests::shouts' panicked at src/greet.rs:21:9:
assertion failed

        SKIP [         ] demo::smoke prints_greeting
------------
     Summary [   0.012s] 3 tests run: 1 passed, 1 failed, 1 skipped
        FAIL [   0.005s] demo greet::tests::shouts
error: test run failed
"#;

    const NEXTEST_NEW: &str = r#"────────────
 Nextest run ID 7e2b with nextest profile: default
    Starting 2 tests across 1 binary
        PASS [   0.003s] (1/2) demo greet::tests::greets_by_name
        FAIL [   0.004s] (2/2) demo greet::tests::shouts
  stdout ───

    running 1 test
    test greet::tests::shouts ... FAILED
  stderr ───
    thread 'greet::tests::shouts' panicked at src/greet.rs:21:9:
    assertion failed

────────────
     Summary [   0.010s] 2 tests run: 1 passed, 1 failed, 0 skipped
        FAIL [   0.004s] (2/2) demo greet::tests::shouts
"#;

    #[test]
    fn parses_nextest_output() {
        for text in [NEXTEST_OLD, NEXTEST_NEW] {
            let mut cases = Cases::default();
            let mut parser = NextestParser::default();
            for line in text.lines() {
                parser.line(line, &mut cases);
            }
            trim_outputs(&mut cases);
            let list = cases.list();
            assert_eq!(list[0].status, TestStatus::Passed);
            assert_eq!(list[0].duration_ms, 3 + (text == NEXTEST_OLD) as u32);
            assert_eq!(list[1].status, TestStatus::Failed);
            assert_eq!(list[1].id.scope, "demo");
            assert_eq!(list[1].id.name, "greet::tests::shouts");
            assert!(
                list[1]
                    .output
                    .iter()
                    .any(|l| l.contains("panicked at src/greet.rs:21:9")),
                "{:?}",
                list[1].output
            );
            assert!(list[1].output.iter().all(|l| !l.contains("Summary")));
        }
        let mut cases = Cases::default();
        let mut parser = NextestParser::default();
        for line in NEXTEST_OLD.lines() {
            parser.line(line, &mut cases);
        }
        assert_eq!(cases.list()[2].status, TestStatus::Ignored);
        assert_eq!(cases.list()[2].suite, ["demo::smoke"]);
    }
}
