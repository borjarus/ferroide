//! JUnit XML reports, the lingua franca of test reports: written by
//! pytest (`--junitxml`), Maven Surefire / Failsafe
//! (`target/surefire-reports/TEST-*.xml`) and Gradle
//! (`build/test-results/<task>/TEST-*.xml`).
//!
//! The dialects differ in details this parser absorbs: the root is
//! `<testsuites>` or a single `<testsuite>`; a failure is `<failure>` or
//! `<error>` with a `message` attribute and the stack trace as text;
//! `<skipped>` marks ignored tests; output is in `<system-out>` /
//! `<system-err>`, per test case or per suite. pytest's `xunit1` family
//! adds `file` and `line` (0-based) attributes, which become locations.

use std::path::Path;

use anyhow::Result;
use ferro_services::TestStatus;
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};

use super::millis;

/// One `<testcase>`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JunitCase {
    /// The enclosing `<testsuite name>`.
    pub suite: String,
    pub classname: String,
    pub name: String,
    pub file: Option<String>,
    /// 0-based (pytest writes 0-based lines).
    pub line: Option<u32>,
    pub duration_ms: u32,
    pub status: Option<TestStatus>,
    /// Failure message and stack trace, then captured output.
    pub output: Vec<String>,
}

impl JunitCase {
    pub fn status(&self) -> TestStatus {
        self.status.unwrap_or(TestStatus::Passed)
    }
}

/// Parse a JUnit XML document.
pub fn parse(xml: &str) -> Result<Vec<JunitCase>> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut cases = Vec::new();
    let mut suites: Vec<String> = Vec::new();
    let mut current: Option<JunitCase> = None;
    // The element whose text is being collected, and the text.
    let mut text_of: Option<Vec<u8>> = None;
    let mut text = String::new();
    loop {
        match reader.read_event()? {
            Event::Start(e) => {
                let name = e.local_name().as_ref().to_vec();
                match name.as_slice() {
                    b"testsuite" => suites.push(attr(&e, "name").unwrap_or_default()),
                    b"testcase" => current = Some(testcase(&e, &suites)),
                    b"failure" | b"error" | b"skipped" => {
                        if let Some(case) = &mut current {
                            begin_outcome(case, &name, &e);
                        }
                        text_of = Some(name);
                        text.clear();
                    }
                    b"system-out" | b"system-err" => {
                        text_of = Some(name);
                        text.clear();
                    }
                    _ => {}
                }
            }
            Event::Empty(e) => {
                let name = e.local_name().as_ref().to_vec();
                match name.as_slice() {
                    b"testcase" => cases.push(testcase(&e, &suites)),
                    b"failure" | b"error" | b"skipped" => {
                        if let Some(case) = &mut current {
                            begin_outcome(case, &name, &e);
                        }
                    }
                    _ => {}
                }
            }
            Event::Text(t) => {
                if text_of.is_some() {
                    text.push_str(&t.xml10_content().unwrap_or_default());
                }
            }
            Event::CData(t) => {
                if text_of.is_some() {
                    text.push_str(&t.decode().unwrap_or_default());
                }
            }
            Event::GeneralRef(r) => {
                if text_of.is_some() {
                    if let Ok(Some(c)) = r.resolve_char_ref() {
                        text.push(c);
                    } else {
                        let name = r.decode().unwrap_or_default();
                        text.push_str(
                            quick_xml::escape::resolve_predefined_entity(&name).unwrap_or_default(),
                        );
                    }
                }
            }
            Event::End(e) => {
                let name = e.local_name().as_ref().to_vec();
                if text_of.as_deref() == Some(name.as_slice()) {
                    text_of = None;
                    if let Some(case) = &mut current {
                        let body = text.trim_matches(['\n', '\r']);
                        if !body.trim().is_empty() {
                            if (name == b"system-out" || name == b"system-err")
                                && !case.output.is_empty()
                            {
                                case.output.push(String::new());
                            }
                            case.output.extend(body.lines().map(str::to_string));
                        }
                    }
                    text.clear();
                }
                match name.as_slice() {
                    b"testcase" => cases.extend(current.take()),
                    b"testsuite" => {
                        suites.pop();
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(cases)
}

/// Parse a report file; an unreadable or malformed one is skipped with a
/// warning.
pub fn parse_file(path: &Path) -> Vec<JunitCase> {
    match std::fs::read_to_string(path)
        .map_err(anyhow::Error::from)
        .and_then(|xml| parse(&xml))
    {
        Ok(cases) => cases,
        Err(e) => {
            tracing::warn!(path = %path.display(), "skipping a JUnit report: {e}");
            Vec::new()
        }
    }
}

pub(crate) fn attr(e: &BytesStart<'_>, name: &str) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        (a.key.local_name().as_ref() == name.as_bytes())
            .then(|| {
                a.normalized_value(XmlVersion::Implicit1_0)
                    .ok()
                    .map(|v| v.into_owned())
            })
            .flatten()
    })
}

fn testcase(e: &BytesStart<'_>, suites: &[String]) -> JunitCase {
    JunitCase {
        suite: suites.last().cloned().unwrap_or_default(),
        classname: attr(e, "classname").unwrap_or_default(),
        name: attr(e, "name").unwrap_or_default(),
        file: attr(e, "file"),
        line: attr(e, "line").and_then(|l| l.parse().ok()),
        duration_ms: attr(e, "time")
            .and_then(|t| t.replace(',', "").parse::<f64>().ok())
            .map(millis)
            .unwrap_or_default(),
        status: None,
        output: Vec::new(),
    }
}

fn begin_outcome(case: &mut JunitCase, element: &[u8], e: &BytesStart<'_>) {
    case.status = Some(if element == b"skipped" {
        TestStatus::Ignored
    } else {
        TestStatus::Failed
    });
    if let Some(message) = attr(e, "message").filter(|m| !m.trim().is_empty()) {
        let message = if element == b"skipped" {
            format!("ignored: {message}")
        } else {
            message
        };
        case.output.extend(message.lines().map(str::to_string));
    }
}

/// A JUnit test method's plain name: `adds()` → `adds`,
/// `adds(int)[2]` → `adds`, `[1] 2, 3` stays.
pub fn method_name(name: &str) -> &str {
    let end = name.find(['(', '[']).unwrap_or(name.len());
    let plain = name[..end].trim();
    if plain.is_empty() { name } else { plain }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// pytest with `-o junit_family=xunit1`.
    pub(crate) const PYTEST: &str = r#"<?xml version="1.0" encoding="utf-8"?><testsuites><testsuite name="pytest" errors="0" failures="1" skipped="1" tests="4" time="0.051" timestamp="2026-11-27T10:00:00" hostname="box"><testcase classname="tests.test_calc" name="test_adds" file="tests/test_calc.py" line="3" time="0.001" /><testcase classname="tests.test_calc.TestCalc" name="test_divides" file="tests/test_calc.py" line="11" time="0.002"><failure message="ZeroDivisionError: division by zero">self = &lt;tests.test_calc.TestCalc object at 0x7f&gt;

    def test_divides(self):
&gt;       assert 1 / 0 == 0
E       ZeroDivisionError: division by zero

tests/test_calc.py:13: ZeroDivisionError</failure><system-out>debug &amp; trace&#33;</system-out></testcase><testcase classname="tests.test_calc" name="test_params[1-2]" file="tests/test_calc.py" line="6" time="0.001" /><testcase classname="tests.test_calc" name="test_later" file="tests/test_calc.py" line="16" time="0.000"><skipped type="pytest.skip" message="not yet">tests/test_calc.py:17: not yet</skipped></testcase></testsuite></testsuites>"#;

    /// Maven Surefire (JUnit 5).
    pub(crate) const SUREFIRE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuite xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:noNamespaceSchemaLocation="https://maven.apache.org/surefire/maven-surefire-plugin/xsd/surefire-test-report.xsd" version="3.0" name="com.acme.CalcTest" time="0.045" tests="3" errors="1" skipped="0" failures="1">
  <properties>
    <property name="java.version" value="21"/>
  </properties>
  <testcase name="adds" classname="com.acme.CalcTest" time="0.012"/>
  <testcase name="divides" classname="com.acme.CalcTest" time="0.003">
    <failure message="expected: &lt;2&gt; but was: &lt;3&gt;" type="org.opentest4j.AssertionFailedError"><![CDATA[org.opentest4j.AssertionFailedError: expected: <2> but was: <3>
	at com.acme.CalcTest.divides(CalcTest.java:18)
]]></failure>
    <system-out><![CDATA[dividing
]]></system-out>
  </testcase>
  <testcase name="explodes" classname="com.acme.CalcTest" time="0.001">
    <error message="boom" type="java.lang.IllegalStateException">java.lang.IllegalStateException: boom
	at com.acme.CalcTest.explodes(CalcTest.java:23)</error>
  </testcase>
</testsuite>"#;

    /// Gradle (JUnit 5, parameterized).
    pub(crate) const GRADLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuite name="com.acme.GreeterTest" tests="3" skipped="1" failures="1" errors="0" timestamp="2026-11-27T10:00:00" hostname="box" time="0.031">
  <properties/>
  <testcase name="greets()" classname="com.acme.GreeterTest" time="0.01"/>
  <testcase name="greetsMany(String)[1]" classname="com.acme.GreeterTest" time="0.004">
    <failure message="org.opentest4j.AssertionFailedError: expected: &lt;Hi Ada&gt; but was: &lt;Hello Ada&gt;" type="org.opentest4j.AssertionFailedError">org.opentest4j.AssertionFailedError: expected: &lt;Hi Ada&gt; but was: &lt;Hello Ada&gt;
	at app//com.acme.GreeterTest.greetsMany(GreeterTest.kt:14)
</failure>
  </testcase>
  <testcase name="later()" classname="com.acme.GreeterTest" time="0.0">
    <skipped/>
  </testcase>
  <system-out><![CDATA[]]></system-out>
  <system-err><![CDATA[]]></system-err>
</testsuite>"#;

    #[test]
    fn parses_pytest_xunit1() {
        let cases = parse(PYTEST).unwrap();
        assert_eq!(cases.len(), 4);
        assert_eq!(cases[0].name, "test_adds");
        assert_eq!(cases[0].status(), TestStatus::Passed);
        assert_eq!(cases[0].line, Some(3));
        assert_eq!(cases[0].file.as_deref(), Some("tests/test_calc.py"));
        let failed = &cases[1];
        assert_eq!(failed.classname, "tests.test_calc.TestCalc");
        assert_eq!(failed.status(), TestStatus::Failed);
        assert_eq!(failed.duration_ms, 2);
        assert_eq!(failed.output[0], "ZeroDivisionError: division by zero");
        assert!(
            failed
                .output
                .contains(&">       assert 1 / 0 == 0".to_string())
        );
        assert_eq!(failed.output.last().unwrap(), "debug & trace!");
        assert_eq!(cases[3].status(), TestStatus::Ignored);
        assert_eq!(cases[3].output[0], "ignored: not yet");
    }

    #[test]
    fn parses_surefire() {
        let cases = parse(SUREFIRE).unwrap();
        assert_eq!(cases.len(), 3);
        assert_eq!(cases[0].suite, "com.acme.CalcTest");
        assert_eq!(cases[0].status(), TestStatus::Passed);
        assert_eq!(cases[1].status(), TestStatus::Failed);
        assert_eq!(cases[1].output[0], "expected: <2> but was: <3>");
        assert!(
            cases[1]
                .output
                .contains(&"\tat com.acme.CalcTest.divides(CalcTest.java:18)".to_string())
        );
        assert_eq!(cases[1].output.last().unwrap(), "dividing");
        assert_eq!(cases[2].status(), TestStatus::Failed);
    }

    #[test]
    fn parses_gradle() {
        let cases = parse(GRADLE).unwrap();
        assert_eq!(cases.len(), 3);
        assert_eq!(method_name(&cases[0].name), "greets");
        assert_eq!(method_name(&cases[1].name), "greetsMany");
        assert_eq!(cases[1].status(), TestStatus::Failed);
        assert_eq!(cases[2].status(), TestStatus::Ignored);
        assert!(cases[2].output.is_empty());
    }

    #[test]
    fn malformed_reports_are_errors() {
        assert!(parse("<testsuite><testcase></testsuite>").is_err());
    }
}
