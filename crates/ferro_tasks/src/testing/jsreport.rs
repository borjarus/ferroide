//! Jest's `--json` report, which Vitest's `json` reporter also writes.
//!
//! The report lists test files (`testResults[].name`, an absolute path)
//! with their assertions: `ancestorTitles` (the `describe` blocks),
//! `title`, `fullName`, `status` (`passed`, `failed`, `pending`,
//! `skipped`, `todo`, `disabled`), `duration` in milliseconds,
//! `failureMessages` and, with Jest's `--testLocationInResults` or
//! Vitest's `includeTaskLocation`, a 1-based `location`. A file that could
//! not run at all (a syntax error) has no assertions but a `message`; it
//! becomes a failed node of its own.
//!
//! Suites are the file (relative to the working directory) and its
//! `describe` blocks; the [`TestId`] is the file and the full name, which
//! is what `-t` matches.

use std::path::Path;

use anyhow::Result;
use ferro_services::TestStatus;
use serde::Deserialize;

use super::{TestCase, TestId};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    #[serde(default)]
    test_results: Vec<FileResult>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileResult {
    name: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    assertion_results: Vec<Assertion>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Assertion {
    #[serde(default)]
    ancestor_titles: Vec<String>,
    title: String,
    #[serde(default)]
    full_name: Option<String>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    failure_messages: Vec<String>,
    #[serde(default)]
    location: Option<Location>,
}

#[derive(Debug, Deserialize)]
struct Location {
    line: u32,
}

/// Parse a Jest / Vitest JSON report; `cwd` makes file names relative.
pub fn parse(json: &str, cwd: &Path) -> Result<Vec<TestCase>> {
    let report: Report = serde_json::from_str(json)?;
    let mut cases = Vec::new();
    for file in report.test_results {
        let path = Path::new(&file.name);
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        let relative = absolute
            .strip_prefix(cwd)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| file.name.clone());
        if file.assertion_results.is_empty() && file.status == "failed" {
            let mut case = TestCase::new(
                TestId {
                    scope: relative.clone(),
                    name: String::new(),
                },
                Vec::new(),
                relative.clone(),
            );
            case.status = TestStatus::Failed;
            case.output = file.message.lines().map(str::to_string).collect();
            case.location = Some((absolute.clone(), 0));
            cases.push(case);
            continue;
        }
        for a in file.assertion_results {
            let full_name = a.full_name.clone().unwrap_or_else(|| {
                let mut parts = a.ancestor_titles.clone();
                parts.push(a.title.clone());
                parts.join(" ")
            });
            let mut suite = vec![relative.clone()];
            suite.extend(a.ancestor_titles.iter().cloned());
            let mut case = TestCase::new(
                TestId {
                    scope: relative.clone(),
                    name: full_name,
                },
                suite,
                a.title,
            );
            case.status = match a.status.as_str() {
                "passed" => TestStatus::Passed,
                "failed" => TestStatus::Failed,
                "pending" | "skipped" | "todo" | "disabled" | "focused" => TestStatus::Ignored,
                _ => TestStatus::NotRun,
            };
            case.duration_ms = a.duration.map(|d| d.max(0.0).round() as u32).unwrap_or(0);
            case.output = a
                .failure_messages
                .iter()
                .flat_map(|m| m.lines().map(str::to_string))
                .collect();
            case.location = a
                .location
                .map(|l| (absolute.clone(), l.line.saturating_sub(1)));
            cases.push(case);
        }
    }
    Ok(cases)
}

#[cfg(test)]
mod tests {
    use super::*;

    const JEST: &str = r#"{"numFailedTestSuites":1,"numFailedTests":1,"numPassedTests":2,"numPendingTests":1,"success":false,"testResults":[
      {"assertionResults":[
        {"ancestorTitles":["sum"],"duration":3,"failureMessages":[],"fullName":"sum adds numbers","location":{"column":3,"line":4},"status":"passed","title":"adds numbers"},
        {"ancestorTitles":["sum","edge cases"],"duration":5,"failureMessages":["Error: expect(received).toBe(expected) // Object.is equality\n\nExpected: 0\nReceived: -0\n    at Object.<anonymous> (/work/web/src/sum.test.js:9:24)"],"fullName":"sum edge cases handles zero","location":{"column":5,"line":8},"status":"failed","title":"handles zero"},
        {"ancestorTitles":[],"duration":null,"failureMessages":[],"fullName":"later","location":null,"status":"pending","title":"later"}
      ],"endTime":1,"message":"","name":"/work/web/src/sum.test.js","startTime":0,"status":"failed","summary":""},
      {"assertionResults":[],"message":"  ● Test suite failed to run\n\n    SyntaxError: Unexpected token","name":"/work/web/src/broken.test.js","status":"failed"}
    ]}"#;

    const VITEST: &str = r#"{"numTotalTestSuites":2,"numPassedTests":1,"numFailedTests":0,"success":true,"testResults":[
      {"assertionResults":[{"ancestorTitles":["math"],"fullName":"math multiplies","status":"passed","title":"multiplies","duration":1.4,"failureMessages":[],"meta":{}}],
       "startTime":1,"endTime":2,"status":"passed","message":"","name":"/work/web/src/math.test.ts"}]}"#;

    #[test]
    fn parses_jest() {
        let cwd = Path::new("/work/web");
        let cases = parse(JEST, cwd).unwrap();
        assert_eq!(cases.len(), 4);
        assert_eq!(cases[0].suite, ["src/sum.test.js", "sum"]);
        assert_eq!(cases[0].id.name, "sum adds numbers");
        assert_eq!(cases[0].location, Some((cwd.join("src/sum.test.js"), 3)));
        assert_eq!(cases[1].status, TestStatus::Failed);
        assert_eq!(cases[1].suite, ["src/sum.test.js", "sum", "edge cases"]);
        assert_eq!(
            cases[1].output[0],
            "Error: expect(received).toBe(expected) // Object.is equality"
        );
        assert_eq!(cases[2].status, TestStatus::Ignored);
        assert_eq!(cases[3].name, "src/broken.test.js");
        assert_eq!(cases[3].status, TestStatus::Failed);
        assert!(cases[3].output.iter().any(|l| l.contains("SyntaxError")));
    }

    #[test]
    fn parses_vitest() {
        let cases = parse(VITEST, Path::new("/work/web")).unwrap();
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].duration_ms, 1);
        assert_eq!(cases[0].id.scope, "src/math.test.ts");
        assert_eq!(cases[0].location, None);
    }
}
