//! The test runner: recognising a test command, collecting its results
//! and running the failed tests again.
//!
//! Every framework's results are first gathered as a flat list of
//! [`TestCase`]s, each knowing its suite path, its [`TestId`] (what the
//! framework needs to select it again) and where it is defined; the
//! [`TestNode`] tree the UI shows is built from that list
//! ([`build_tree`]). Keeping the flat list makes the parsers simple (they
//! only upsert cases) and gives [`frameworks::rerun_command`] what it needs
//! without walking a tree.
//!
//! Results come from the console output where the format streams and is
//! stable enough to read live (libtest, nextest, `go test -json`), and
//! from report files the run is asked to write otherwise (JUnit XML,
//! Jest/Vitest JSON, TRX), read when the run finishes. See
//! [`frameworks`] for which framework gets which arguments.

pub mod frameworks;
pub mod gotest;
pub mod jsreport;
pub mod junit;
pub mod libtest;
pub mod locate;
pub mod session;
pub mod trx;

use std::collections::HashMap;
use std::path::PathBuf;

use ferro_services::{TestNode, TestStatus};

pub use frameworks::{Detected, Framework, detect, rerun_command};
pub use session::TestSession;

/// What a framework needs to select one test again.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct TestId {
    /// The test binary (Cargo), package (Go), file (pytest, Jest, Vitest)
    /// or class (JUnit, .NET).
    pub scope: String,
    /// The test's name within `scope`: a module path (`a::b::t`), a
    /// `Class::test` node id part, `TestA/sub`, a Jest full name, a method.
    pub name: String,
}

/// One test's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestCase {
    pub id: TestId,
    /// Suites from the outermost; the tree's inner nodes.
    pub suite: Vec<String>,
    /// The leaf's label.
    pub name: String,
    pub status: TestStatus,
    pub duration_ms: u32,
    pub output: Vec<String>,
    /// Definition: file and 0-based line.
    pub location: Option<(PathBuf, u32)>,
}

impl TestCase {
    pub fn new(id: TestId, suite: Vec<String>, name: impl Into<String>) -> Self {
        Self {
            id,
            suite,
            name: name.into(),
            status: TestStatus::Running,
            duration_ms: 0,
            output: Vec::new(),
            location: None,
        }
    }
}

/// Cases in the order they were first seen, addressable by id.
#[derive(Debug, Default, Clone)]
pub struct Cases {
    list: Vec<TestCase>,
    index: HashMap<TestId, usize>,
}

impl Cases {
    /// The case `id`, created with `suite` and `name` if new.
    pub fn upsert(
        &mut self,
        id: TestId,
        suite: impl FnOnce() -> Vec<String>,
        name: impl FnOnce() -> String,
    ) -> &mut TestCase {
        let i = match self.index.get(&id) {
            Some(i) => *i,
            None => {
                let i = self.list.len();
                self.list.push(TestCase::new(id.clone(), suite(), name()));
                self.index.insert(id, i);
                i
            }
        };
        &mut self.list[i]
    }

    pub fn push(&mut self, case: TestCase) {
        match self.index.get(&case.id) {
            Some(i) => self.list[*i] = case,
            None => {
                self.index.insert(case.id.clone(), self.list.len());
                self.list.push(case);
            }
        }
    }

    pub fn get_mut(&mut self, id: &TestId) -> Option<&mut TestCase> {
        self.index.get(id).map(|i| &mut self.list[*i])
    }

    pub fn list(&self) -> &[TestCase] {
        &self.list
    }

    pub fn list_mut(&mut self) -> &mut [TestCase] {
        &mut self.list
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }
}

/// Build the tree of suites from flat cases. Suites keep the order their
/// first case appeared in; a suite's status is failed if any child failed,
/// running if any still runs, ignored if all were ignored, else passed.
/// A case whose name is also a suite (a Go test with subtests) becomes
/// that suite's node, with its own status unless a child failed.
pub fn build_tree(cases: &[TestCase]) -> Vec<TestNode> {
    #[derive(Default)]
    struct Draft<'a> {
        name: String,
        case: Option<&'a TestCase>,
        children: Vec<Draft<'a>>,
    }
    fn child<'a, 'b>(nodes: &'b mut Vec<Draft<'a>>, name: &str) -> &'b mut Draft<'a> {
        let i = match nodes.iter().position(|n| n.name == name) {
            Some(i) => i,
            None => {
                nodes.push(Draft {
                    name: name.to_string(),
                    ..Default::default()
                });
                nodes.len() - 1
            }
        };
        &mut nodes[i]
    }
    fn finish(draft: Draft<'_>) -> TestNode {
        let children: Vec<TestNode> = draft.children.into_iter().map(finish).collect();
        let summary = summarize(&children);
        match draft.case {
            Some(case) => TestNode {
                name: draft.name,
                status: if summary == Some(TestStatus::Failed) {
                    TestStatus::Failed
                } else {
                    case.status
                },
                duration_ms: if case.duration_ms == 0 {
                    children.iter().map(|c| c.duration_ms).sum()
                } else {
                    case.duration_ms
                },
                output: case.output.clone(),
                location: case.location.clone(),
                children,
            },
            None => TestNode {
                name: draft.name,
                status: summary.unwrap_or(TestStatus::NotRun),
                duration_ms: children.iter().map(|c| c.duration_ms).sum(),
                output: Vec::new(),
                location: None,
                children,
            },
        }
    }
    let mut roots: Vec<Draft> = Vec::new();
    for case in cases {
        let mut level = &mut roots;
        for suite in &case.suite {
            level = &mut child(level, suite).children;
        }
        child(level, &case.name).case = Some(case);
    }
    roots.into_iter().map(finish).collect()
}

/// The status of a suite with these children (`None` without children).
fn summarize(children: &[TestNode]) -> Option<TestStatus> {
    if children.is_empty() {
        return None;
    }
    let any = |s| children.iter().any(|c| c.status == s);
    Some(if any(TestStatus::Failed) {
        TestStatus::Failed
    } else if any(TestStatus::Running) {
        TestStatus::Running
    } else if children.iter().all(|c| c.status == TestStatus::Ignored) {
        TestStatus::Ignored
    } else if any(TestStatus::Passed) {
        TestStatus::Passed
    } else if any(TestStatus::NotRun) {
        TestStatus::NotRun
    } else {
        TestStatus::Ignored
    })
}

/// Remove ANSI escape sequences (CSI and OSC) for parsing.
pub fn strip_ansi(line: &str) -> std::borrow::Cow<'_, str> {
    if !line.contains('\x1b') {
        return line.into();
    }
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out.into()
}

/// Seconds as reported by a framework → whole milliseconds.
pub(crate) fn millis(seconds: f64) -> u32 {
    if seconds.is_finite() && seconds > 0.0 {
        (seconds * 1000.0).round().min(u32::MAX as f64) as u32
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(suite: &[&str], name: &str, status: TestStatus, ms: u32) -> TestCase {
        let mut c = TestCase::new(
            TestId {
                scope: suite.join("/"),
                name: name.into(),
            },
            suite.iter().map(|s| s.to_string()).collect(),
            name,
        );
        c.status = status;
        c.duration_ms = ms;
        c
    }

    #[test]
    fn builds_suites_with_bubbling_status() {
        let cases = vec![
            case(&["demo", "tests"], "a", TestStatus::Passed, 2),
            case(&["smoke"], "b", TestStatus::Failed, 5),
            case(&["demo", "tests"], "c", TestStatus::Ignored, 0),
            case(&["demo"], "d", TestStatus::Passed, 1),
            case(&["skipped"], "e", TestStatus::Ignored, 0),
        ];
        let tree = build_tree(&cases);
        let names: Vec<_> = tree.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["demo", "smoke", "skipped"]);
        assert_eq!(tree[0].status, TestStatus::Passed);
        assert_eq!(tree[0].duration_ms, 3);
        assert_eq!(tree[0].children[0].name, "tests");
        assert_eq!(tree[0].children[0].children.len(), 2);
        assert_eq!(tree[1].status, TestStatus::Failed);
        assert_eq!(tree[2].status, TestStatus::Ignored);
    }

    #[test]
    fn a_case_can_also_be_a_suite() {
        let cases = vec![
            case(&["pkg", "TestA"], "sub1", TestStatus::Passed, 1),
            case(&["pkg", "TestA"], "sub2", TestStatus::Failed, 1),
            case(&["pkg"], "TestA", TestStatus::Passed, 7),
        ];
        let tree = build_tree(&cases);
        let a = &tree[0].children[0];
        assert_eq!(a.name, "TestA");
        assert_eq!(a.children.len(), 2);
        assert_eq!(a.status, TestStatus::Failed);
        assert_eq!(a.duration_ms, 7);
        assert_eq!(tree[0].status, TestStatus::Failed);
    }

    #[test]
    fn strips_ansi() {
        assert_eq!(
            strip_ansi("\x1b[1m\x1b[32mtest\x1b[0m ok \x1b]8;;file:///x\x1b\\link\x1b]8;;\x07"),
            "test ok link"
        );
    }
}
