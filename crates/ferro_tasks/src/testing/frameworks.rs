//! Which test framework a command line runs, what it needs to report its
//! results, and how to run only some of its tests.
//!
//! Detection reads the words of the command line, looking through
//! launchers (`npx`, `pnpm exec`, `yarn`, `uv run`, `poetry run`, …) and,
//! for `npm test` / `pnpm run test` / `yarn test`, into the script's body
//! in `package.json`:
//!
//! | framework | recognised by | results from |
//! |---|---|---|
//! | Cargo | `cargo test` | libtest console output |
//! | nextest | `cargo nextest run` | nextest console output |
//! | pytest | `pytest`, `python -m pytest` | `--junitxml` (`xunit1`, with locations) |
//! | Go | `go test` | `-json` events, shown as text |
//! | Jest | `jest` | `--json --outputFile`, `--testLocationInResults` |
//! | Vitest | `vitest` | `--reporter=json --outputFile.json` (and the default reporter) |
//! | Maven | `mvn test` / `verify` / `install` … | `target/surefire-reports` written by the run |
//! | Gradle | `gradle test` / `check` / `build` | `build/test-results` written by the run |
//! | .NET | `dotnet test` | `--logger trx --results-directory` |
//!
//! Arguments are added only to the process started, never to the saved
//! configuration. A rerun of failed tests ([`rerun_command`]) starts from
//! the configuration's own command line and narrows it with the
//! framework's selection syntax; see each arm for the details.

use std::path::Path;

use crate::command::{append, join, program_stem, regex_escape, split};

use super::{TestCase, junit::method_name};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Framework {
    Cargo,
    Nextest,
    Pytest,
    Go,
    Jest,
    Vitest,
    Maven,
    Gradle,
    Dotnet,
}

/// A recognised test command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub framework: Framework,
    /// Run through `npm` (`npm test`, `npm run x`): extra arguments go
    /// after `--`.
    pub needs_separator: bool,
    /// Run through a package.json script: the script decides the files
    /// and options, only arguments can be appended.
    pub via_script: bool,
}

const PY_LAUNCHERS: &[&str] = &["uv", "poetry", "pipenv", "hatch", "pdm", "rye"];

/// Recognise the test framework run by `line` in `cwd`.
pub fn detect(line: &str, cwd: &Path) -> Option<Detected> {
    detect_words(&split(line), cwd, 0)
}

fn detected(framework: Framework) -> Option<Detected> {
    Some(Detected {
        framework,
        needs_separator: false,
        via_script: false,
    })
}

fn detect_words(words: &[String], cwd: &Path, depth: u32) -> Option<Detected> {
    if depth > 3 {
        return None;
    }
    let program = program_stem(words.first()?);
    let args = &words[1..];
    let first_arg = || {
        args.iter()
            .find(|a| !a.starts_with('-') && !a.starts_with('+'))
    };
    match program.as_str() {
        "cargo" => match first_arg().map(String::as_str) {
            Some("test" | "t") => detected(Framework::Cargo),
            Some("nextest") => detected(Framework::Nextest),
            _ => None,
        },
        "cargo-nextest" => detected(Framework::Nextest),
        "pytest" | "py.test" => detected(Framework::Pytest),
        p if p == "py" || p.starts_with("python") => args
            .windows(2)
            .any(|w| w[0] == "-m" && w[1] == "pytest")
            .then_some(())
            .and_then(|_| detected(Framework::Pytest)),
        p if PY_LAUNCHERS.contains(&p) && args.first().is_some_and(|a| a == "run") => {
            detect_words(&args[1..], cwd, depth + 1)
        }
        "go" => (first_arg().map(String::as_str) == Some("test"))
            .then_some(())
            .and_then(|_| detected(Framework::Go)),
        "jest" => detected(Framework::Jest),
        "vitest" => match first_arg().map(String::as_str) {
            Some("bench" | "list" | "init") => None,
            _ => detected(Framework::Vitest),
        },
        "node" => detect_words(args, cwd, depth + 1),
        "npx" | "bunx" => {
            let rest: Vec<String> = args
                .iter()
                .skip_while(|a| a.starts_with('-'))
                .cloned()
                .collect();
            detect_words(&rest, cwd, depth + 1)
        }
        "mvn" | "mvnw" => {
            let skips = args.iter().any(|a| {
                a == "-DskipTests" || a == "-Dmaven.test.skip=true" || a == "-DskipTests=true"
            });
            let phase = args.iter().any(|a| {
                matches!(
                    a.as_str(),
                    "test" | "verify" | "install" | "package" | "deploy" | "integration-test"
                ) || a.ends_with("surefire:test")
            });
            (phase && !skips)
                .then_some(())
                .and_then(|_| detected(Framework::Maven))
        }
        "gradle" | "gradlew" => {
            let excluded = args.windows(2).any(|w| {
                (w[0] == "-x" || w[0] == "--exclude-task")
                    && (w[1] == "test" || w[1].ends_with(":test"))
            });
            let task = args.iter().any(|a| {
                let task = a.rsplit(':').next().unwrap_or(a);
                matches!(task, "test" | "check" | "build") || task.ends_with("Test")
            });
            (task && !excluded)
                .then_some(())
                .and_then(|_| detected(Framework::Gradle))
        }
        "dotnet" => (first_arg().map(String::as_str) == Some("test"))
            .then_some(())
            .and_then(|_| detected(Framework::Dotnet)),
        "npm" | "pnpm" | "yarn" | "bun" => {
            let (script, exec) = package_script(&program, args)?;
            if exec {
                return detect_words(&args[1..], cwd, depth + 1)
                    .or_else(|| detect_words(args, cwd, depth + 1));
            }
            let body = read_script(cwd, &script)?;
            let mut found = body
                .split(['&', ';', '|'])
                .find_map(|part| detect_words(&split(part), cwd, depth + 1))?;
            found.needs_separator = program == "npm";
            found.via_script = true;
            Some(found)
        }
        _ => None,
    }
}

/// The package.json script `npm`/`pnpm`/`yarn`/`bun` runs with `args`,
/// or (`exec = true`) a binary it executes (`pnpm exec jest`, `yarn jest`).
fn package_script(program: &str, args: &[String]) -> Option<(String, bool)> {
    let mut words = args.iter().filter(|a| !a.starts_with('-'));
    let first = words.next()?;
    match first.as_str() {
        "test" | "t" | "tst" => Some(("test".into(), false)),
        "run" | "run-script" => Some((words.next()?.clone(), false)),
        "exec" | "dlx" | "x" => Some((String::new(), true)),
        other if program != "npm" => {
            // `yarn jest` runs the binary; `yarn unit` runs the script.
            if matches!(other, "jest" | "vitest") {
                Some((String::new(), true))
            } else {
                Some((other.to_string(), false))
            }
        }
        _ => None,
    }
}

fn read_script(cwd: &Path, script: &str) -> Option<String> {
    let text = std::fs::read_to_string(cwd.join("package.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("scripts")?
        .get(script)?
        .as_str()
        .map(str::to_string)
}

/// Where a finished run's results are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Report {
    /// Read from the console while it runs.
    Console,
    /// A JUnit XML file.
    Junit(std::path::PathBuf),
    /// A Jest / Vitest JSON file.
    JsJson(std::path::PathBuf),
    /// JUnit XML files under `target/surefire-reports` /
    /// `build/test-results`, newer than the run.
    BuildReports,
    /// TRX files in a folder.
    Trx(std::path::PathBuf),
}

/// The arguments to run `words` with so the results can be read, and
/// where they will be. `report_dir` is a fresh folder for report files.
/// The second value is whether `go test -json` output must be translated.
pub fn prepare(
    detected: &Detected,
    words: &[String],
    report_dir: &Path,
) -> (Vec<String>, Report, bool) {
    let mut words = words.to_vec();
    let add = |words: &mut Vec<String>, extra: Vec<String>| {
        if detected.needs_separator && !words.iter().any(|w| w == "--") {
            words.push("--".into());
        }
        words.extend(extra);
    };
    let path = |name: &str| report_dir.join(name).to_string_lossy().into_owned();
    match detected.framework {
        Framework::Cargo | Framework::Nextest => (words, Report::Console, false),
        Framework::Pytest => {
            let file = report_dir.join("pytest.xml");
            add(
                &mut words,
                vec![
                    format!("--junitxml={}", path("pytest.xml")),
                    "-o".into(),
                    "junit_family=xunit1".into(),
                ],
            );
            (words, Report::Junit(file), false)
        }
        Framework::Go => {
            if words.iter().any(|w| w == "-json" || w == "--json") {
                return (words, Report::Console, false);
            }
            match words.iter().position(|w| w == "test") {
                Some(i) if !detected.via_script => {
                    words.insert(i + 1, "-json".into());
                    (words, Report::Console, true)
                }
                _ => (words, Report::Console, false),
            }
        }
        Framework::Jest => {
            add(
                &mut words,
                vec![
                    "--json".into(),
                    format!("--outputFile={}", path("jest.json")),
                    "--testLocationInResults".into(),
                ],
            );
            (words, Report::JsJson(report_dir.join("jest.json")), false)
        }
        Framework::Vitest => {
            add(
                &mut words,
                vec![
                    "--reporter=default".into(),
                    "--reporter=json".into(),
                    format!("--outputFile.json={}", path("vitest.json")),
                ],
            );
            (words, Report::JsJson(report_dir.join("vitest.json")), false)
        }
        Framework::Maven | Framework::Gradle => (words, Report::BuildReports, false),
        Framework::Dotnet => {
            add(
                &mut words,
                vec![
                    "--logger".into(),
                    "trx".into(),
                    "--results-directory".into(),
                    path("trx"),
                ],
            );
            (words, Report::Trx(report_dir.join("trx")), false)
        }
    }
}

/// `line` narrowed to the `failed` tests, or `None` when the framework's
/// selection cannot express them.
pub fn rerun_command(
    detected: &Detected,
    line: &str,
    failed: &[&TestCase],
    cwd: &Path,
) -> Option<String> {
    if failed.is_empty() {
        return None;
    }
    let mut words = split(line);
    let extra: Vec<String> = match detected.framework {
        // `--exact` makes each name select one test; libtest filters are
        // OR-ed, and every test binary gets all of them.
        Framework::Cargo => {
            let mut extra = vec!["--exact".to_string()];
            extra.extend(unique(failed.iter().map(|c| c.id.name.clone())));
            return Some(append(line, &extra, true));
        }
        Framework::Nextest => {
            let expr = unique(failed.iter().map(|c| format!("test(={})", c.id.name))).join(" | ");
            let at = words.iter().position(|w| w == "--").unwrap_or(words.len());
            words.splice(at..at, ["-E".to_string(), expr]);
            return Some(join(&words));
        }
        // Node ids replace the paths and `-k` of the original.
        Framework::Pytest => {
            let mut kept = Vec::new();
            let mut skip_next = false;
            for (i, w) in words.iter().enumerate() {
                if skip_next {
                    skip_next = false;
                    continue;
                }
                if i > 0
                    && (w == "-k" || (w == "-m" && words.get(i + 1).is_some_and(|n| n != "pytest")))
                {
                    skip_next = true;
                    continue;
                }
                let path_like = !w.starts_with('-')
                    && (w.contains("::") || (i > 0 && cwd.join(w).exists() && w != "pytest"));
                if !path_like || detected.via_script {
                    kept.push(w.clone());
                }
            }
            words = kept;
            unique(
                failed
                    .iter()
                    .map(|c| format!("{}::{}", c.id.scope, c.id.name)),
            )
        }
        // One `-run` for the top-level tests (a failed subtest fails its
        // parent), and the failed packages instead of the original ones.
        Framework::Go => {
            let test_at = words.iter().position(|w| w == "test")?;
            const VALUED: &[&str] = &[
                "-run",
                "-bench",
                "-count",
                "-timeout",
                "-tags",
                "-cpu",
                "-parallel",
                "-p",
                "-coverprofile",
                "-covermode",
                "-coverpkg",
                "-o",
                "-exec",
                "-ldflags",
                "-gcflags",
                "-skip",
                "-benchtime",
                "-shuffle",
                "-vet",
                "-mod",
                "-C",
            ];
            let mut kept: Vec<String> = words[..=test_at].to_vec();
            let mut i = test_at + 1;
            while i < words.len() {
                let w = &words[i];
                let name = w.split('=').next().unwrap_or(w);
                if name == "-run" || name == "--run" {
                    i += if w.contains('=') { 1 } else { 2 };
                    continue;
                }
                if w.starts_with('-') {
                    kept.push(w.clone());
                    if VALUED.contains(&name) && !w.contains('=') {
                        if let Some(value) = words.get(i + 1) {
                            kept.push(value.clone());
                        }
                        i += 1;
                    }
                } else if w == "--" {
                    kept.extend(words[i..].iter().cloned());
                    break;
                }
                i += 1;
            }
            let tests = unique(
                failed
                    .iter()
                    .map(|c| regex_escape(c.id.name.split('/').next().unwrap_or(&c.id.name))),
            );
            let packages = unique(failed.iter().map(|c| c.id.scope.clone()));
            let at = kept.iter().position(|w| w == "--").unwrap_or(kept.len());
            let mut extra = vec!["-run".to_string(), format!("^({})$", tests.join("|"))];
            extra.extend(packages);
            kept.splice(at..at, extra);
            return Some(join(&kept));
        }
        // `-t` matches the full name; the files limit what is loaded.
        Framework::Jest | Framework::Vitest => {
            let names = unique(failed.iter().map(|c| regex_escape(&c.id.name)));
            let mut extra = vec!["-t".to_string(), format!("^(?:{})$", names.join("|"))];
            extra.extend(unique(failed.iter().map(|c| c.id.scope.clone())));
            return Some(append(line, &extra, detected.needs_separator));
        }
        // `-Dtest=Class#a+b,Other#c`.
        Framework::Maven => {
            words.retain(|w| !w.starts_with("-Dtest="));
            let mut classes: Vec<(String, Vec<String>)> = Vec::new();
            for case in failed {
                let method = method_name(&case.id.name).to_string();
                match classes.iter_mut().find(|(c, _)| *c == case.id.scope) {
                    Some((_, methods)) if !methods.contains(&method) => methods.push(method),
                    Some(_) => {}
                    None => classes.push((case.id.scope.clone(), vec![method])),
                }
            }
            let selection = classes
                .iter()
                .map(|(class, methods)| format!("{class}#{}", methods.join("+")))
                .collect::<Vec<_>>()
                .join(",");
            vec![
                format!("-Dtest={selection}"),
                "-Dsurefire.failIfNoSpecifiedTests=false".into(),
            ]
        }
        Framework::Gradle => unique(
            failed
                .iter()
                .map(|c| format!("{}.{}", c.id.scope, method_name(&c.id.name))),
        )
        .into_iter()
        .flat_map(|t| ["--tests".to_string(), t])
        .collect(),
        Framework::Dotnet => {
            if let Some(i) = words.iter().position(|w| w == "--filter") {
                words.drain(i..(i + 2).min(words.len()));
            }
            let filter = unique(failed.iter().map(|c| {
                format!(
                    "FullyQualifiedName={}.{}",
                    c.id.scope,
                    c.id.name.split('(').next().unwrap_or(&c.id.name)
                )
            }))
            .join("|");
            vec!["--filter".into(), filter]
        }
    };
    let base = join(&words);
    Some(append(&base, &extra, detected.needs_separator))
}

fn unique(items: impl Iterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestId;

    fn det(line: &str) -> Option<Framework> {
        detect(line, Path::new("/nonexistent")).map(|d| d.framework)
    }

    #[test]
    fn detects_frameworks() {
        assert_eq!(det("cargo test -p core"), Some(Framework::Cargo));
        assert_eq!(det("cargo +nightly test"), Some(Framework::Cargo));
        assert_eq!(det("cargo nextest run"), Some(Framework::Nextest));
        assert_eq!(det("cargo run"), None);
        assert_eq!(det("pytest tests"), Some(Framework::Pytest));
        assert_eq!(det("python -m pytest -x"), Some(Framework::Pytest));
        assert_eq!(
            det(r"C:\venv\Scripts\python.exe -m pytest"),
            Some(Framework::Pytest)
        );
        assert_eq!(det("uv run pytest"), Some(Framework::Pytest));
        assert_eq!(det("python main.py"), None);
        assert_eq!(det("go test ./..."), Some(Framework::Go));
        assert_eq!(det("go run ."), None);
        assert_eq!(det("npx jest src"), Some(Framework::Jest));
        assert_eq!(det("npx vitest run"), Some(Framework::Vitest));
        assert_eq!(det("pnpm exec vitest run"), Some(Framework::Vitest));
        assert_eq!(det("yarn jest"), Some(Framework::Jest));
        assert_eq!(det("mvn test"), Some(Framework::Maven));
        assert_eq!(det("./mvnw -q verify"), Some(Framework::Maven));
        assert_eq!(det("mvn package -DskipTests"), None);
        assert_eq!(det("gradle test"), Some(Framework::Gradle));
        assert_eq!(det("./gradlew :app:test"), Some(Framework::Gradle));
        assert_eq!(det("gradle build -x test"), None);
        assert_eq!(det("dotnet test"), Some(Framework::Dotnet));
        assert_eq!(det("dotnet run"), None);
    }

    #[test]
    fn detects_through_package_scripts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"test":"tsc --noEmit && vitest run","unit":"jest --ci","dev":"vite"}}"#,
        )
        .unwrap();
        let npm = detect("npm test", dir.path()).unwrap();
        assert_eq!(npm.framework, Framework::Vitest);
        assert!(npm.needs_separator && npm.via_script);
        let pnpm = detect("pnpm run unit", dir.path()).unwrap();
        assert_eq!(pnpm.framework, Framework::Jest);
        assert!(!pnpm.needs_separator);
        assert_eq!(
            detect("yarn unit", dir.path()).unwrap().framework,
            Framework::Jest
        );
        assert_eq!(detect("npm run dev", dir.path()), None);
    }

    #[test]
    fn prepares_reporting_arguments() {
        let dir = Path::new("/tmp/r");
        let words = |l: &str| split(l);
        let go = detected(Framework::Go).unwrap();
        let (args, report, translate) = prepare(&go, &words("go test -v ./..."), dir);
        assert_eq!(args, ["go", "test", "-json", "-v", "./..."]);
        assert_eq!(report, Report::Console);
        assert!(translate);
        let (args, _, translate) = prepare(&go, &words("go test -json ./..."), dir);
        assert_eq!(args.len(), 4);
        assert!(!translate);

        let npm_jest = Detected {
            framework: Framework::Jest,
            needs_separator: true,
            via_script: true,
        };
        let (args, report, _) = prepare(&npm_jest, &words("npm test"), dir);
        assert_eq!(args[2], "--");
        assert_eq!(args[3], "--json");
        assert_eq!(report, Report::JsJson(dir.join("jest.json")));

        let pytest = detected(Framework::Pytest).unwrap();
        let (args, _, _) = prepare(&pytest, &words("python -m pytest"), dir);
        assert!(args[3].starts_with("--junitxml="));
        assert_eq!(&args[4..], ["-o", "junit_family=xunit1"]);

        let dotnet = detected(Framework::Dotnet).unwrap();
        let (args, report, _) = prepare(&dotnet, &words("dotnet test"), dir);
        assert_eq!(&args[2..4], ["--logger", "trx"]);
        assert_eq!(report, Report::Trx(dir.join("trx")));
    }

    fn failed(scope: &str, name: &str) -> TestCase {
        TestCase::new(
            TestId {
                scope: scope.into(),
                name: name.into(),
            },
            Vec::new(),
            name,
        )
    }

    fn rerun(framework: Framework, line: &str, cases: &[TestCase]) -> String {
        let refs: Vec<&TestCase> = cases.iter().collect();
        rerun_command(
            &detected(framework).unwrap(),
            line,
            &refs,
            Path::new("/nonexistent"),
        )
        .unwrap()
    }

    #[test]
    fn builds_rerun_filters() {
        let rust = [
            failed("demo", "a::b"),
            failed("smoke", "c"),
            failed("x", "a::b"),
        ];
        assert_eq!(
            rerun(Framework::Cargo, "cargo test", &rust),
            "cargo test -- --exact a::b c"
        );
        assert_eq!(
            rerun(Framework::Cargo, "cargo test -- --nocapture", &rust),
            "cargo test -- --nocapture --exact a::b c"
        );
        assert_eq!(
            rerun(Framework::Nextest, "cargo nextest run", &rust[..2]),
            "cargo nextest run -E 'test(=a::b) | test(=c)'"
        );
        assert_eq!(
            rerun(
                Framework::Pytest,
                "python -m pytest -k slow tests::x -x",
                &[
                    failed("tests/test_a.py", "TestA::test_x"),
                    failed("tests/test_b.py", "test_y[1-2]"),
                ]
            ),
            "python -m pytest -x tests/test_a.py::TestA::test_x 'tests/test_b.py::test_y[1-2]'"
        );
        assert_eq!(
            rerun(
                Framework::Go,
                "go test -v -count 1 -run Old ./...",
                &[
                    failed("example.com/m/a", "TestA/sub"),
                    failed("example.com/m/a", "TestA"),
                    failed("example.com/m/b", "TestB"),
                ]
            ),
            "go test -v -count 1 -run '^(TestA|TestB)$' example.com/m/a example.com/m/b"
        );
        assert_eq!(
            rerun(
                Framework::Jest,
                "npx jest",
                &[failed("src/sum.test.js", "sum adds (1+2)")]
            ),
            r"npx jest -t '^(?:sum adds \(1\+2\))$' src/sum.test.js"
        );
        let npm = Detected {
            framework: Framework::Vitest,
            needs_separator: true,
            via_script: true,
        };
        let case = failed("src/a.test.ts", "a b");
        assert_eq!(
            rerun_command(&npm, "npm test", &[&case], Path::new("/x")).unwrap(),
            "npm test -- -t '^(?:a b)$' src/a.test.ts"
        );
        assert_eq!(
            rerun(
                Framework::Maven,
                "mvn test -Dtest=Old",
                &[
                    failed("com.acme.CalcTest", "divides()"),
                    failed("com.acme.CalcTest", "explodes"),
                    failed("com.acme.GreeterTest", "greets(String)[1]"),
                ]
            ),
            "mvn test -Dtest=com.acme.CalcTest#divides+explodes,com.acme.GreeterTest#greets -Dsurefire.failIfNoSpecifiedTests=false"
        );
        assert_eq!(
            rerun(
                Framework::Gradle,
                "./gradlew test",
                &[
                    failed("com.acme.GreeterTest", "greetsMany(String)[1]"),
                    failed("com.acme.GreeterTest", "greetsMany(String)[2]"),
                ]
            ),
            "./gradlew test --tests com.acme.GreeterTest.greetsMany"
        );
        assert_eq!(
            rerun(
                Framework::Dotnet,
                "dotnet test --filter Category=Unit",
                &[
                    failed("Demo.CalcTests", "Divides"),
                    failed("Demo.CalcTests", "Adds"),
                ]
            ),
            "dotnet test --filter 'FullyQualifiedName=Demo.CalcTests.Divides|FullyQualifiedName=Demo.CalcTests.Adds'"
        );
    }
}
