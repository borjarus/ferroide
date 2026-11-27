//! Visual Studio TRX reports, written by `dotnet test --logger trx`.
//!
//! `<UnitTestResult testId testName outcome duration>` holds each result,
//! with `<Output>`: `<StdOut>`, `<ErrorInfo><Message>` and
//! `<StackTrace>`. The test's class comes from the matching
//! `<UnitTest id><TestMethod className name>` definition. Suites are
//! classes; the [`TestId`] is the class and the method, which together
//! give the `FullyQualifiedName` filter for a rerun.

use std::collections::HashMap;

use anyhow::Result;
use ferro_services::TestStatus;
use quick_xml::events::Event;

use super::junit::attr;
use super::{TestCase, TestId, millis};

#[derive(Default)]
struct Result_ {
    test_id: String,
    test_name: String,
    outcome: String,
    duration_ms: u32,
    message: String,
    stack: String,
    stdout: String,
}

/// Parse a TRX document.
pub fn parse(xml: &str) -> Result<Vec<TestCase>> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut results: Vec<Result_> = Vec::new();
    // test id → (class name, method name)
    let mut methods: HashMap<String, (String, String)> = HashMap::new();
    let mut current: Option<Result_> = None;
    let mut unit_test: Option<String> = None;
    let mut text_of: Option<Vec<u8>> = None;
    let mut text = String::new();
    loop {
        match reader.read_event()? {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"UnitTestResult" => {
                let result = Result_ {
                    test_id: attr(&e, "testId").unwrap_or_default(),
                    test_name: attr(&e, "testName").unwrap_or_default(),
                    outcome: attr(&e, "outcome").unwrap_or_default(),
                    duration_ms: attr(&e, "duration")
                        .map(|d| parse_duration(&d))
                        .unwrap_or_default(),
                    ..Default::default()
                };
                // `Empty` results have no body: done already.
                if let Some(done) = current.replace(result) {
                    results.push(done);
                }
            }
            Event::Start(e) => match e.local_name().as_ref() {
                b"UnitTest" => unit_test = attr(&e, "id"),
                b"TestMethod" => method(&e, &unit_test, &mut methods),
                name @ (b"StdOut" | b"Message" | b"StackTrace") => {
                    text_of = Some(name.to_vec());
                    text.clear();
                }
                _ => {}
            },
            Event::Empty(e) => {
                if e.local_name().as_ref() == b"TestMethod" {
                    method(&e, &unit_test, &mut methods);
                }
            }
            Event::Text(t) if text_of.is_some() => {
                text.push_str(&t.xml10_content().unwrap_or_default());
            }
            Event::CData(t) if text_of.is_some() => text.push_str(&t.decode().unwrap_or_default()),
            Event::GeneralRef(r) if text_of.is_some() => {
                if let Ok(Some(c)) = r.resolve_char_ref() {
                    text.push(c);
                } else {
                    let name = r.decode().unwrap_or_default();
                    text.push_str(
                        quick_xml::escape::resolve_predefined_entity(&name).unwrap_or_default(),
                    );
                }
            }
            Event::End(e) => {
                let name = e.local_name().as_ref().to_vec();
                if text_of.as_deref() == Some(name.as_slice()) {
                    text_of = None;
                    if let Some(result) = &mut current {
                        let value = std::mem::take(&mut text);
                        match name.as_slice() {
                            b"StdOut" => result.stdout = value,
                            b"Message" => result.message = value,
                            _ => result.stack = value,
                        }
                    }
                }
                match name.as_slice() {
                    b"UnitTestResult" => results.extend(current.take()),
                    b"UnitTest" => unit_test = None,
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    results.extend(current.take());

    Ok(results
        .into_iter()
        .map(|r| {
            let (class, method) = methods.get(&r.test_id).cloned().unwrap_or_else(|| {
                // No definition: split the name at its last dot before any
                // parameters.
                let plain = r.test_name.split('(').next().unwrap_or(&r.test_name);
                match plain.rsplit_once('.') {
                    Some((class, method)) => (class.to_string(), method.to_string()),
                    None => (String::new(), plain.to_string()),
                }
            });
            let label = r
                .test_name
                .strip_prefix(&format!("{class}."))
                .unwrap_or(&r.test_name)
                .to_string();
            let mut case = TestCase::new(
                TestId {
                    scope: class.clone(),
                    name: method,
                },
                if class.is_empty() {
                    Vec::new()
                } else {
                    vec![class]
                },
                label,
            );
            case.status = match r.outcome.as_str() {
                "Passed" => TestStatus::Passed,
                "Failed" | "Error" | "Timeout" | "Aborted" => TestStatus::Failed,
                "NotExecuted" | "Inconclusive" | "Skipped" => TestStatus::Ignored,
                _ => TestStatus::NotRun,
            };
            case.duration_ms = r.duration_ms;
            for block in [&r.message, &r.stack, &r.stdout] {
                let block = block.trim_end();
                if !block.trim().is_empty() {
                    case.output.extend(block.lines().map(str::to_string));
                }
            }
            case
        })
        .collect())
}

fn method(
    e: &quick_xml::events::BytesStart<'_>,
    unit_test: &Option<String>,
    methods: &mut HashMap<String, (String, String)>,
) {
    if let Some(id) = unit_test {
        let class = attr(e, "className").unwrap_or_default();
        // `className` may carry the assembly: `Ns.Class, Assembly, …`.
        let class = class
            .split(',')
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        methods.insert(id.clone(), (class, attr(e, "name").unwrap_or_default()));
    }
}

/// `00:00:01.2345678` → 1235.
fn parse_duration(text: &str) -> u32 {
    let mut seconds = 0.0;
    for part in text.split(':') {
        seconds = seconds * 60.0 + part.parse::<f64>().unwrap_or(0.0);
    }
    millis(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRX: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<TestRun id="1" name="run" xmlns="http://microsoft.com/schemas/VisualStudio/TeamTest/2010">
  <Results>
    <UnitTestResult executionId="e1" testId="t1" testName="Demo.Tests.CalcTests.Adds" computerName="box" duration="00:00:00.0034567" startTime="2026-11-27T10:00:00" endTime="2026-11-27T10:00:00" testType="13cdc9d9" outcome="Passed" testListId="l" relativeResultsDirectory="e1" />
    <UnitTestResult executionId="e2" testId="t2" testName="Demo.Tests.CalcTests.Divides(a: 1, b: 0)" computerName="box" duration="00:00:00.0120000" outcome="Failed" testListId="l">
      <Output>
        <StdOut>dividing &amp; conquering</StdOut>
        <ErrorInfo>
          <Message>Assert.Equal() Failure
Expected: 1
Actual:   0</Message>
          <StackTrace>   at Demo.Tests.CalcTests.Divides(Int32 a, Int32 b) in C:\work\Demo.Tests\CalcTests.cs:line 21</StackTrace>
        </ErrorInfo>
      </Output>
    </UnitTestResult>
    <UnitTestResult executionId="e3" testId="t3" testName="Demo.Tests.CalcTests.Later" duration="00:00:00" outcome="NotExecuted" testListId="l" />
  </Results>
  <TestDefinitions>
    <UnitTest name="Demo.Tests.CalcTests.Adds" storage="c:\work\demo.tests.dll" id="t1">
      <Execution id="e1" />
      <TestMethod codeBase="C:\work\Demo.Tests.dll" adapterTypeName="executor://xunit/VsTestRunner2/netcoreapp" className="Demo.Tests.CalcTests" name="Adds" />
    </UnitTest>
    <UnitTest name="Demo.Tests.CalcTests.Divides(a: 1, b: 0)" storage="c:\work\demo.tests.dll" id="t2">
      <TestMethod codeBase="C:\work\Demo.Tests.dll" className="Demo.Tests.CalcTests" name="Divides" />
    </UnitTest>
  </TestDefinitions>
</TestRun>"#;

    #[test]
    fn parses_trx() {
        let cases = parse(TRX).unwrap();
        assert_eq!(cases.len(), 3);
        assert_eq!(cases[0].suite, ["Demo.Tests.CalcTests"]);
        assert_eq!(cases[0].name, "Adds");
        assert_eq!(cases[0].duration_ms, 3);
        assert_eq!(cases[0].status, TestStatus::Passed);
        let failed = &cases[1];
        assert_eq!(failed.name, "Divides(a: 1, b: 0)");
        assert_eq!(failed.id.name, "Divides");
        assert_eq!(failed.status, TestStatus::Failed);
        assert_eq!(failed.duration_ms, 12);
        assert_eq!(failed.output[0], "Assert.Equal() Failure");
        assert!(
            failed
                .output
                .iter()
                .any(|l| l.contains("CalcTests.cs:line 21"))
        );
        assert_eq!(failed.output.last().unwrap(), "dividing & conquering");
        // No definition: class split from the name.
        assert_eq!(cases[2].id.scope, "Demo.Tests.CalcTests");
        assert_eq!(cases[2].id.name, "Later");
        assert_eq!(cases[2].status, TestStatus::Ignored);
    }
}
