//! `go test -json`: test events, translated back to the console text a
//! plain `go test` prints.
//!
//! The run is given `-json` (unless the configuration asked for it
//! itself, in which case the JSON is shown as is), so each line is an
//! event: `run`, `pass` / `fail` / `skip` with `Elapsed`, and `output`
//! carrying the text `go test -v` would print. The console shows those
//! texts, the tree is built from the events. Lines that are not JSON
//! (compiler errors on stderr, a test binary writing to the real stdout)
//! pass through unchanged.
//!
//! Suites are packages; a subtest `TestA/sub` is a child of `TestA`.

use std::collections::HashMap;

use ferro_services::TestStatus;
use serde::Deserialize;

use super::{Cases, TestCase, TestId, millis};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Event {
    action: String,
    #[serde(default)]
    package: String,
    #[serde(default)]
    test: Option<String>,
    #[serde(default)]
    elapsed: Option<f64>,
    #[serde(default)]
    output: Option<String>,
}

/// Reads `go test -json` events.
#[derive(Debug, Default)]
pub struct GoTestParser {
    /// Output of packages, for a package that fails without tests (a
    /// build failure).
    package_output: HashMap<String, Vec<String>>,
}

/// What one line of `go test -json` output turned into.
#[derive(Debug, PartialEq, Eq)]
pub struct GoLine {
    /// The console text: none for events without output.
    pub display: Vec<String>,
    pub changed: bool,
}

impl GoTestParser {
    pub fn line(&mut self, line: &str, cases: &mut Cases) -> GoLine {
        let trimmed = line.trim();
        let event = match trimmed
            .starts_with('{')
            .then(|| serde_json::from_str::<Event>(trimmed))
        {
            Some(Ok(event)) => event,
            _ => {
                return GoLine {
                    display: vec![line.to_string()],
                    changed: false,
                };
            }
        };
        let mut display = Vec::new();
        let mut changed = false;
        let package = event.package.clone();
        match (event.action.as_str(), event.test.as_deref()) {
            ("output" | "build-output", test) => {
                let text = event.output.unwrap_or_default();
                let text = text.strip_suffix('\n').unwrap_or(&text);
                let text = text.strip_suffix('\r').unwrap_or(text);
                for piece in text.split('\n') {
                    display.push(piece.to_string());
                }
                let keep = |l: &str| {
                    let t = l.trim_start();
                    !(t.starts_with("=== RUN")
                        || t.starts_with("=== PAUSE")
                        || t.starts_with("=== CONT")
                        || t.starts_with("=== NAME")
                        || t.starts_with("--- PASS")
                        || t.starts_with("--- SKIP"))
                };
                match test {
                    Some(test) => {
                        let case = upsert(cases, &package, test);
                        case.output
                            .extend(text.split('\n').filter(|l| keep(l)).map(str::to_string));
                        changed = true;
                    }
                    None => self
                        .package_output
                        .entry(package)
                        .or_default()
                        .extend(text.split('\n').map(str::to_string)),
                }
            }
            ("run", Some(test)) => {
                upsert(cases, &package, test).status = TestStatus::Running;
                changed = true;
            }
            (action @ ("pass" | "fail" | "skip"), Some(test)) => {
                let case = upsert(cases, &package, test);
                case.status = match action {
                    "pass" => TestStatus::Passed,
                    "fail" => TestStatus::Failed,
                    _ => TestStatus::Ignored,
                };
                case.duration_ms = millis(event.elapsed.unwrap_or_default());
                if case.status != TestStatus::Failed {
                    // Only a failure's output is worth keeping around.
                    case.output.retain(|l| !l.trim().is_empty());
                }
                changed = true;
            }
            ("fail", None) => {
                let has_tests = cases.list().iter().any(|c| c.id.scope == package);
                if !has_tests {
                    let mut case = TestCase::new(
                        TestId {
                            scope: package.clone(),
                            name: String::new(),
                        },
                        Vec::new(),
                        package.clone(),
                    );
                    case.status = TestStatus::Failed;
                    case.output = self.package_output.remove(&package).unwrap_or_default();
                    cases.push(case);
                    changed = true;
                }
            }
            _ => {}
        }
        GoLine { display, changed }
    }
}

fn upsert<'a>(cases: &'a mut Cases, package: &str, test: &str) -> &'a mut TestCase {
    let id = TestId {
        scope: package.to_string(),
        name: test.to_string(),
    };
    cases.upsert(
        id,
        || {
            let mut suite = vec![package.to_string()];
            let parts: Vec<&str> = test.split('/').collect();
            suite.extend(parts[..parts.len() - 1].iter().map(|s| s.to_string()));
            suite
        },
        || test.rsplit('/').next().unwrap_or(test).to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::build_tree;

    const GO_JSON: &str = r##"{"Time":"2026-11-27T10:00:00Z","Action":"start","Package":"example.com/demo/greet"}
{"Time":"2026-11-27T10:00:00Z","Action":"run","Package":"example.com/demo/greet","Test":"TestGreet"}
{"Time":"2026-11-27T10:00:00Z","Action":"output","Package":"example.com/demo/greet","Test":"TestGreet","Output":"=== RUN   TestGreet\n"}
{"Time":"2026-11-27T10:00:00Z","Action":"run","Package":"example.com/demo/greet","Test":"TestGreet/by_name"}
{"Time":"2026-11-27T10:00:00Z","Action":"output","Package":"example.com/demo/greet","Test":"TestGreet/by_name","Output":"=== RUN   TestGreet/by_name\n"}
{"Time":"2026-11-27T10:00:00Z","Action":"output","Package":"example.com/demo/greet","Test":"TestGreet/by_name","Output":"    greet_test.go:12: got \"hi\", want \"hello\"\n"}
{"Time":"2026-11-27T10:00:00Z","Action":"output","Package":"example.com/demo/greet","Test":"TestGreet/by_name","Output":"--- FAIL: TestGreet/by_name (0.00s)\n"}
{"Time":"2026-11-27T10:00:00Z","Action":"fail","Package":"example.com/demo/greet","Test":"TestGreet/by_name","Elapsed":0}
{"Time":"2026-11-27T10:00:00Z","Action":"output","Package":"example.com/demo/greet","Test":"TestGreet","Output":"--- FAIL: TestGreet (0.01s)\n"}
{"Time":"2026-11-27T10:00:00Z","Action":"fail","Package":"example.com/demo/greet","Test":"TestGreet","Elapsed":0.01}
{"Time":"2026-11-27T10:00:00Z","Action":"run","Package":"example.com/demo/greet","Test":"TestSkip"}
{"Time":"2026-11-27T10:00:00Z","Action":"output","Package":"example.com/demo/greet","Test":"TestSkip","Output":"--- SKIP: TestSkip (0.00s)\n"}
{"Time":"2026-11-27T10:00:00Z","Action":"skip","Package":"example.com/demo/greet","Test":"TestSkip","Elapsed":0}
{"Time":"2026-11-27T10:00:00Z","Action":"run","Package":"example.com/demo/greet","Test":"TestOk"}
{"Time":"2026-11-27T10:00:00Z","Action":"pass","Package":"example.com/demo/greet","Test":"TestOk","Elapsed":0.002}
{"Time":"2026-11-27T10:00:00Z","Action":"output","Package":"example.com/demo/greet","Output":"FAIL\n"}
{"Time":"2026-11-27T10:00:00Z","Action":"output","Package":"example.com/demo/greet","Output":"FAIL\texample.com/demo/greet\t0.012s\n"}
{"Time":"2026-11-27T10:00:00Z","Action":"fail","Package":"example.com/demo/greet","Elapsed":0.012}
{"ImportPath":"example.com/demo/broken","Action":"build-output","Output":"# example.com/demo/broken\n"}
{"Time":"2026-11-27T10:00:00Z","Action":"output","Package":"example.com/demo/broken","Output":"FAIL\texample.com/demo/broken [build failed]\n"}
{"Time":"2026-11-27T10:00:00Z","Action":"fail","Package":"example.com/demo/broken","Elapsed":0}
broken/broken.go:3:1: syntax error: non-declaration statement outside function body"##;

    #[test]
    fn translates_and_parses_go_test_json() {
        let mut cases = Cases::default();
        let mut parser = GoTestParser::default();
        let mut console = Vec::new();
        for line in GO_JSON.lines() {
            console.extend(parser.line(line, &mut cases).display);
        }
        assert_eq!(console[0], "=== RUN   TestGreet");
        assert!(console.contains(&"    greet_test.go:12: got \"hi\", want \"hello\"".to_string()));
        assert!(console.contains(&"FAIL\texample.com/demo/greet\t0.012s".to_string()));
        assert_eq!(
            console.last().unwrap(),
            "broken/broken.go:3:1: syntax error: non-declaration statement outside function body"
        );
        assert!(console.iter().all(|l| !l.starts_with('{')));

        let list = cases.list();
        let sub = list
            .iter()
            .find(|c| c.id.name == "TestGreet/by_name")
            .unwrap();
        assert_eq!(sub.suite, ["example.com/demo/greet", "TestGreet"]);
        assert_eq!(sub.name, "by_name");
        assert_eq!(sub.status, TestStatus::Failed);
        assert_eq!(
            sub.output,
            [
                "    greet_test.go:12: got \"hi\", want \"hello\"",
                "--- FAIL: TestGreet/by_name (0.00s)"
            ]
        );
        let ok = list.iter().find(|c| c.id.name == "TestOk").unwrap();
        assert_eq!((ok.status, ok.duration_ms), (TestStatus::Passed, 2));
        let skip = list.iter().find(|c| c.id.name == "TestSkip").unwrap();
        assert_eq!(skip.status, TestStatus::Ignored);
        let broken = list
            .iter()
            .find(|c| c.id.scope == "example.com/demo/broken")
            .unwrap();
        assert_eq!(broken.status, TestStatus::Failed);

        let tree = build_tree(list);
        let greet = &tree[0];
        assert_eq!(greet.name, "example.com/demo/greet");
        assert_eq!(greet.status, TestStatus::Failed);
        assert_eq!(greet.children[0].name, "TestGreet");
        assert_eq!(greet.children[0].children[0].name, "by_name");
    }
}
