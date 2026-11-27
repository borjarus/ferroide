//! One test run: the arguments it gets, the results read from its output
//! while it runs and from its reports when it ends.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use ferro_services::{TestNode, TestStatus};

use super::frameworks::{Detected, Framework, Report, prepare};
use super::gotest::GoTestParser;
use super::junit::{self, JunitCase};
use super::libtest::{LibtestParser, NextestParser, trim_outputs};
use super::{Cases, TestCase, TestId, build_tree, jsreport, locate, trx};
use crate::walk;

/// Folders a report search never enters.
const REPORT_SKIPPED: &[&str] = &["node_modules", ".git", ".gradle", ".idea", "src", ".venv"];

/// The results of one test run, as they come in.
pub struct TestSession {
    pub detected: Detected,
    cwd: PathBuf,
    report: Report,
    translate_go: bool,
    /// Report files go here; removed with the session.
    _report_dir: Option<tempfile::TempDir>,
    started: SystemTime,
    cases: Cases,
    libtest: LibtestParser,
    nextest: NextestParser,
    go: GoTestParser,
}

/// What a line of output became.
#[derive(Debug, PartialEq, Eq)]
pub struct LineOutcome {
    /// Replacement console lines (`go test -json` events); `None` shows
    /// the line as it is.
    pub display: Option<Vec<String>>,
    /// The test tree changed.
    pub changed: bool,
}

impl TestSession {
    /// A session for `words` run in `cwd`, and the words to actually run.
    pub fn new(
        detected: Detected,
        words: &[String],
        cwd: &Path,
    ) -> std::io::Result<(Self, Vec<String>)> {
        let dir = tempfile::Builder::new()
            .prefix("ferroide-tests-")
            .tempdir()?;
        let (words, report, translate_go) = prepare(&detected, words, dir.path());
        let session = Self {
            detected,
            cwd: cwd.to_path_buf(),
            report,
            translate_go,
            _report_dir: Some(dir),
            started: SystemTime::now(),
            cases: Cases::default(),
            libtest: LibtestParser::default(),
            nextest: NextestParser::default(),
            go: GoTestParser::default(),
        };
        Ok((session, words))
    }

    pub fn framework(&self) -> Framework {
        self.detected.framework
    }

    pub fn cases(&self) -> &[TestCase] {
        self.cases.list()
    }

    pub fn tree(&self) -> Vec<TestNode> {
        build_tree(self.cases.list())
    }

    /// Feed a line of output.
    pub fn line(&mut self, line: &str, error: bool) -> LineOutcome {
        match self.detected.framework {
            Framework::Cargo => LineOutcome {
                display: None,
                changed: self.libtest.line(line, &mut self.cases),
            },
            Framework::Nextest => LineOutcome {
                display: None,
                changed: self.nextest.line(line, &mut self.cases),
            },
            Framework::Go if self.translate_go && !error => {
                let out = self.go.line(line, &mut self.cases);
                LineOutcome {
                    display: Some(out.display),
                    changed: out.changed,
                }
            }
            _ => LineOutcome {
                display: None,
                changed: false,
            },
        }
    }

    /// The run ended: read the reports, settle tests left running and
    /// find where the tests are defined.
    pub fn finish(&mut self, stopped: bool) {
        match self.report.clone() {
            Report::Console => {}
            Report::Junit(file) => {
                for case in junit::parse_file(&file) {
                    self.cases.push(pytest_case(case, &self.cwd));
                }
            }
            Report::JsJson(file) => match std::fs::read_to_string(&file) {
                Ok(json) => match jsreport::parse(&json, &self.cwd) {
                    Ok(cases) => cases.into_iter().for_each(|c| self.cases.push(c)),
                    Err(e) => tracing::warn!(file = %file.display(), "bad test report: {e}"),
                },
                Err(e) => tracing::debug!(file = %file.display(), "no test report: {e}"),
            },
            Report::BuildReports => {
                for file in self.build_reports() {
                    for case in junit::parse_file(&file) {
                        if !case.name.is_empty() {
                            self.cases.push(jvm_case(case));
                        }
                    }
                }
            }
            Report::Trx(dir) => {
                let files = walk::files(&dir, 3, 100, &[], false, |p| {
                    p.extension().is_some_and(|e| e == "trx")
                });
                for file in files {
                    match std::fs::read_to_string(&file)
                        .map_err(anyhow::Error::from)
                        .and_then(|xml| trx::parse(&xml))
                    {
                        Ok(cases) => cases.into_iter().for_each(|c| self.cases.push(c)),
                        Err(e) => tracing::warn!(file = %file.display(), "bad TRX report: {e}"),
                    }
                }
            }
        }
        for case in self.cases.list_mut() {
            if case.status == TestStatus::Running {
                case.status = if stopped {
                    TestStatus::NotRun
                } else {
                    TestStatus::Failed
                };
            }
        }
        trim_outputs(&mut self.cases);
        locate::locate(self.detected.framework, self.cases.list_mut(), &self.cwd);
    }

    /// JUnit XML files Maven or Gradle wrote during this run.
    fn build_reports(&self) -> Vec<PathBuf> {
        let since = self.started - Duration::from_secs(2);
        walk::files(&self.cwd, 10, 10_000, REPORT_SKIPPED, false, |p| {
            let xml = p.extension().is_some_and(|e| e == "xml");
            let s = p.to_string_lossy().replace('\\', "/");
            let report = s.contains("/surefire-reports/")
                || s.contains("/failsafe-reports/")
                || s.contains("/build/test-results/");
            xml && report
                && std::fs::metadata(p)
                    .and_then(|m| m.modified())
                    .is_ok_and(|t| t >= since)
        })
    }
}

/// A pytest JUnit case: the file is the scope, classes and the test the
/// node id's rest.
fn pytest_case(case: JunitCase, cwd: &Path) -> TestCase {
    let segments: Vec<&str> = case.classname.split('.').collect();
    let (file, classes): (String, Vec<String>) = match &case.file {
        Some(file) => {
            let file = file.replace('\\', "/");
            let module = file.trim_end_matches(".py").replace('/', ".");
            let rest = case
                .classname
                .strip_prefix(&module)
                .map(|r| r.trim_start_matches('.'))
                .unwrap_or("");
            let classes = rest
                .split('.')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            (file, classes)
        }
        None => (1..=segments.len())
            .rev()
            .find_map(|k| {
                let candidate = format!("{}.py", segments[..k].join("/"));
                cwd.join(&candidate).is_file().then(|| {
                    (
                        candidate,
                        segments[k..].iter().map(|s| s.to_string()).collect(),
                    )
                })
            })
            .unwrap_or_else(|| (case.classname.clone(), Vec::new())),
    };
    let mut name_parts = classes.clone();
    name_parts.push(case.name.clone());
    let mut suite = vec![file.clone()];
    suite.extend(classes);
    let mut out = TestCase::new(
        TestId {
            scope: file.clone(),
            name: name_parts.join("::"),
        },
        suite,
        case.name.clone(),
    );
    out.status = case.status();
    out.duration_ms = case.duration_ms;
    out.output = case.output;
    out.location = case.line.map(|line| (cwd.join(&file), line));
    out
}

/// A Maven / Gradle case: suites are classes.
fn jvm_case(case: JunitCase) -> TestCase {
    let class = if case.classname.is_empty() {
        case.suite.clone()
    } else {
        case.classname.clone()
    };
    let mut out = TestCase::new(
        TestId {
            scope: class.clone(),
            name: case.name.clone(),
        },
        vec![class],
        case.name.clone(),
    );
    out.status = case.status();
    out.duration_ms = case.duration_ms;
    out.output = case.output;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::split;

    #[test]
    fn maps_pytest_cases_to_node_ids() {
        let cwd = Path::new("/work");
        let cases: Vec<TestCase> = junit::parse(crate::testing::junit::tests::PYTEST)
            .unwrap()
            .into_iter()
            .map(|c| pytest_case(c, cwd))
            .collect();
        assert_eq!(cases[0].id.scope, "tests/test_calc.py");
        assert_eq!(cases[0].id.name, "test_adds");
        assert_eq!(cases[0].location, Some((cwd.join("tests/test_calc.py"), 3)));
        assert_eq!(cases[1].id.name, "TestCalc::test_divides");
        assert_eq!(cases[1].suite, ["tests/test_calc.py", "TestCalc"]);
        let tree = build_tree(&cases);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].status, TestStatus::Failed);
    }

    #[test]
    fn reads_build_reports_written_during_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let reports = dir.path().join("module/target/surefire-reports");
        std::fs::create_dir_all(&reports).unwrap();
        let detected = Detected {
            framework: Framework::Maven,
            needs_separator: false,
            via_script: false,
        };
        let (mut session, words) =
            TestSession::new(detected, &split("mvn test"), dir.path()).unwrap();
        assert_eq!(words, ["mvn", "test"]);
        std::fs::write(
            reports.join("TEST-com.acme.CalcTest.xml"),
            crate::testing::junit::tests::SUREFIRE,
        )
        .unwrap();
        session.finish(false);
        assert_eq!(session.cases().len(), 3);
        let tree = session.tree();
        assert_eq!(tree[0].name, "com.acme.CalcTest");
        assert_eq!(tree[0].status, TestStatus::Failed);
    }
}
