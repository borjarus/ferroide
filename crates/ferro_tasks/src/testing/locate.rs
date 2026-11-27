//! Finding where a test is defined, for navigation from the test tree.
//!
//! Reports that carry a location (pytest's `xunit1` family, Jest with
//! `--testLocationInResults`) are used as they are. For the rest the
//! definition is searched for in the sources under the working directory,
//! once per run when it finishes, with one walk per language and only the
//! patterns of the tests that need a location:
//!
//! - Rust: `fn name`, the candidate whose path and `mod` declarations
//!   match most of the test's module path (and, for an integration test,
//!   whose file is `tests/<binary>.rs`); doc-tests by their
//!   `src/lib.rs - item (line N)` name;
//! - Python: `def name` in the reported file, inside the class when there
//!   is one;
//! - Go: `func Name(` in a `_test.go` file of the package's folder;
//! - JS/TS: `it(` / `test(` with the title, in the reported file;
//! - Java/Kotlin: the class's file, then the method (`void name(`,
//!   `fun name(` or a backticked Kotlin name);
//! - C#: the class's file, then `Name(`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use super::{Framework, TestCase, TestId};
use crate::command::regex_escape;
use crate::walk;

/// Fill in `location` for cases that have none.
pub fn locate(framework: Framework, cases: &mut [TestCase], cwd: &Path) {
    let missing: Vec<usize> = (0..cases.len())
        .filter(|i| cases[*i].location.is_none() && !cases[*i].id.name.is_empty())
        .collect();
    if missing.is_empty() {
        return;
    }
    let mut files = FileCache::default();
    match framework {
        Framework::Cargo | Framework::Nextest => rust(cases, &missing, cwd, &mut files),
        Framework::Pytest => {
            for i in missing {
                cases[i].location = python(&cases[i].id, cwd, &mut files);
            }
        }
        Framework::Go => go(cases, &missing, cwd, &mut files),
        Framework::Jest | Framework::Vitest => {
            for i in missing {
                let case = &cases[i];
                let file = cwd.join(&case.id.scope);
                let pattern = format!(
                    r#"\b(?:it|test)(?:\.\w+)*\(\s*['"`]{}"#,
                    regex_escape(&case.name)
                );
                let line = files.find(&file, &pattern);
                cases[i].location = line.map(|l| (file, l));
            }
        }
        Framework::Maven | Framework::Gradle => {
            let sources = walk::sources(cwd, &["java", "kt", "groovy", "scala"]);
            for i in missing {
                cases[i].location = jvm(&cases[i].id, &sources, &mut files);
            }
        }
        Framework::Dotnet => {
            let sources = walk::sources(cwd, &["cs", "fs", "vb"]);
            for i in missing {
                cases[i].location = dotnet(&cases[i].id, &sources, &mut files);
            }
        }
    }
}

/// File contents read at most once per run.
#[derive(Default)]
struct FileCache {
    texts: HashMap<PathBuf, Option<String>>,
}

impl FileCache {
    fn text(&mut self, path: &Path) -> Option<&str> {
        self.texts
            .entry(path.to_path_buf())
            .or_insert_with(|| std::fs::read_to_string(path).ok())
            .as_deref()
    }

    /// 0-based line of the first match of `pattern` in `path`.
    fn find(&mut self, path: &Path, pattern: &str) -> Option<u32> {
        let re = Regex::new(pattern).ok()?;
        let text = self.text(path)?;
        find_line(text, &re, 0)
    }
}

fn find_line(text: &str, re: &Regex, from: usize) -> Option<u32> {
    text.lines()
        .enumerate()
        .skip(from)
        .find(|(_, l)| re.is_match(l))
        .map(|(i, _)| i as u32)
}

fn relative(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

static RUST_FN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+\S+\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)",
    )
    .unwrap()
});
static DOC_TEST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.+?) - .*\(line (\d+)\)$").unwrap());

fn rust(cases: &mut [TestCase], missing: &[usize], cwd: &Path, files: &mut FileCache) {
    let sources = walk::sources(cwd, &["rs"]);
    let needed: HashSet<String> = missing
        .iter()
        .map(|i| {
            let name = &cases[*i].id.name;
            name.rsplit("::").next().unwrap_or_default().to_string()
        })
        .collect();
    // fn name → definitions
    let mut defs: HashMap<String, Vec<(PathBuf, u32)>> = HashMap::new();
    for file in &sources {
        let Some(text) = files.text(file) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            if let Some(c) = RUST_FN.captures(line)
                && needed.contains(&c[1])
            {
                defs.entry(c[1].to_string())
                    .or_default()
                    .push((file.clone(), n as u32));
            }
        }
    }
    for &i in missing {
        let id = &cases[i].id;
        if let Some(c) = DOC_TEST.captures(&id.name) {
            let wanted = c[1].replace('\\', "/");
            let line: u32 = c[2].parse().unwrap_or(1);
            cases[i].location = sources
                .iter()
                .find(|f| relative(f, cwd).ends_with(&wanted))
                .map(|f| (f.clone(), line.saturating_sub(1)));
            continue;
        }
        let segments: Vec<&str> = id.name.split("::").collect();
        let leaf = segments.last().copied().unwrap_or_default();
        let modules = &segments[..segments.len() - 1];
        let Some(candidates) = defs.get(leaf) else {
            continue;
        };
        let integration = format!("tests/{}.rs", id.scope);
        let integration_dir = format!("tests/{}/", id.scope);
        let mut best: Option<(i32, PathBuf, u32)> = None;
        for (file, line) in candidates {
            let rel = relative(file, cwd);
            let parts: Vec<&str> = rel
                .split('/')
                .map(|p| p.strip_suffix(".rs").unwrap_or(p))
                .collect();
            let text = files.text(file).unwrap_or_default();
            let mut score = 0;
            for m in modules {
                if parts.contains(m) {
                    score += 2;
                } else if text.contains(&format!("mod {m}")) {
                    score += 1;
                }
            }
            if rel.ends_with(&integration) || rel.contains(&integration_dir) {
                score += 3;
            } else if rel.starts_with("tests/") || rel.contains("/tests/") {
                score -= 1;
            }
            // A test without a module path lives in a crate root.
            if modules.is_empty() && matches!(parts.last(), Some(&("lib" | "main"))) {
                score += 1;
            }
            if best.as_ref().is_none_or(|(s, _, _)| score > *s) {
                best = Some((score, file.clone(), *line));
            }
        }
        cases[i].location = best.map(|(_, f, l)| (f, l));
    }
}

fn python(id: &TestId, cwd: &Path, files: &mut FileCache) -> Option<(PathBuf, u32)> {
    let file = cwd.join(&id.scope);
    let mut parts: Vec<&str> = id.name.split("::").collect();
    let test = parts.pop()?;
    let test = test.split('[').next().unwrap_or(test);
    let text = files.text(&file)?.to_string();
    let mut from = 0;
    for class in parts {
        let re = Regex::new(&format!(r"^\s*class\s+{}\b", regex_escape(class))).ok()?;
        from = find_line(&text, &re, from)? as usize;
    }
    let re = Regex::new(&format!(
        r"^\s*(?:async\s+)?def\s+{}\s*\(",
        regex_escape(test)
    ))
    .ok()?;
    find_line(&text, &re, from).map(|l| (file, l))
}

fn go(cases: &mut [TestCase], missing: &[usize], cwd: &Path, files: &mut FileCache) {
    let sources: Vec<PathBuf> = walk::sources(cwd, &["go"])
        .into_iter()
        .filter(|p| p.to_string_lossy().ends_with("_test.go"))
        .collect();
    for &i in missing {
        let id = &cases[i].id;
        let top = id.name.split('/').next().unwrap_or_default();
        let Ok(re) = Regex::new(&format!(
            r"^func\s+(?:\([^)]*\)\s*)?{}\s*\(",
            regex_escape(top)
        )) else {
            continue;
        };
        let package: Vec<&str> = id.scope.split('/').collect();
        let mut best: Option<(usize, PathBuf, u32)> = None;
        for file in &sources {
            let Some(text) = files.text(file) else {
                continue;
            };
            let Some(line) = find_line(text, &re, 0) else {
                continue;
            };
            // How many trailing segments of the package path the folder
            // shares.
            let rel = relative(file.parent().unwrap_or(cwd), cwd);
            let dirs: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
            let score = dirs
                .iter()
                .rev()
                .zip(package.iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            if best.as_ref().is_none_or(|(s, _, _)| score > *s) {
                best = Some((score, file.clone(), line));
            }
        }
        cases[i].location = best.map(|(_, f, l)| (f, l));
    }
}

fn jvm(id: &TestId, sources: &[PathBuf], files: &mut FileCache) -> Option<(PathBuf, u32)> {
    let class = id.scope.split('$').next().unwrap_or(&id.scope);
    let (package, simple) = class.rsplit_once('.').unwrap_or(("", class));
    let package_path = package.replace('.', "/");
    let file = sources
        .iter()
        .filter(|f| f.file_stem().and_then(|s| s.to_str()) == Some(simple))
        .max_by_key(|f| {
            f.to_string_lossy()
                .replace('\\', "/")
                .contains(&package_path)
        })?
        .clone();
    let method = super::junit::method_name(&id.name);
    let pattern = if method.contains(' ') {
        format!("`{}`", regex_escape(method))
    } else {
        format!(
            r"\b(?:void|fun|def|Unit|[A-Z]\w*)\s+{}\s*\(",
            regex_escape(method)
        )
    };
    let line = files
        .find(&file, &pattern)
        .or_else(|| files.find(&file, &format!(r"\b{}\s*\(", regex_escape(method))))
        .unwrap_or(0);
    Some((file, line))
}

fn dotnet(id: &TestId, sources: &[PathBuf], files: &mut FileCache) -> Option<(PathBuf, u32)> {
    let simple = id.scope.rsplit(['.', '+']).next().unwrap_or(&id.scope);
    let class_re = Regex::new(&format!(
        r"\b(?:class|type|module)\s+{}\b",
        regex_escape(simple)
    ))
    .ok()?;
    let by_name = sources
        .iter()
        .find(|f| f.file_stem().and_then(|s| s.to_str()) == Some(simple))
        .cloned();
    let file = match by_name {
        Some(file) => file,
        None => sources
            .iter()
            .find(|f| files.text(f).is_some_and(|t| class_re.is_match(t)))?
            .clone(),
    };
    let method = id.name.split('(').next().unwrap_or(&id.name);
    let line = files
        .find(&file, &format!(r"\b{}\s*\(", regex_escape(method)))
        .or_else(|| files.find(&file, &format!("``{}``", regex_escape(method))))
        .unwrap_or(0);
    Some((file, line))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(scope: &str, name: &str, label: &str) -> TestCase {
        TestCase::new(
            TestId {
                scope: scope.into(),
                name: name.into(),
            },
            Vec::new(),
            label,
        )
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn locates_rust_tests_by_module_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "src/lib.rs", "mod greet;\n\n#[test]\nfn works() {}\n");
        write(
            root,
            "src/greet.rs",
            "pub fn hi() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn works() {}\n}\n",
        );
        write(root, "tests/smoke.rs", "#[test]\nfn works() {}\n");
        write(root, "target/debug/x.rs", "fn works() {}\n");
        let mut cases = vec![
            case("demo", "greet::tests::works", "works"),
            case("demo", "works", "works"),
            case("smoke", "works", "works"),
            case("demo (doc-tests)", "src/greet.rs - hi (line 1)", "x"),
        ];
        locate(Framework::Cargo, &mut cases, root);
        assert_eq!(
            cases[0].location,
            Some((root.join("src").join("greet.rs"), 5))
        );
        assert_eq!(cases[1].location.as_ref().unwrap().1, 3);
        assert_eq!(
            cases[2].location,
            Some((root.join("tests").join("smoke.rs"), 1))
        );
        assert_eq!(
            cases[3].location,
            Some((root.join("src").join("greet.rs"), 0))
        );
    }

    #[test]
    fn locates_python_go_js_jvm_and_dotnet_tests() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "tests/test_calc.py",
            "def test_adds():\n    pass\n\nclass TestCalc:\n    def test_adds(self):\n        pass\n",
        );
        write(
            root,
            "greet/greet_test.go",
            "package greet\n\nfunc TestGreet(t *testing.T) {}\n",
        );
        write(
            root,
            "other/greet_test.go",
            "package other\n\nfunc TestGreet(t *testing.T) {}\n",
        );
        write(
            root,
            "src/sum.test.js",
            "describe('sum', () => {\n  it(\"adds (1+2)\", () => {});\n});\n",
        );
        write(
            root,
            "src/test/java/com/acme/CalcTest.java",
            "package com.acme;\nclass CalcTest {\n  @Test\n  void adds() {}\n}\n",
        );
        write(
            root,
            "src/test/kotlin/com/acme/GreeterTest.kt",
            "class GreeterTest {\n  @Test\n  fun `greets by name`() {}\n}\n",
        );
        write(
            root,
            "Demo.Tests/CalcTests.cs",
            "public class CalcTests\n{\n    [Fact]\n    public void Adds() {}\n}\n",
        );

        let mut py = vec![
            case("tests/test_calc.py", "TestCalc::test_adds", "test_adds"),
            case("tests/test_calc.py", "test_adds[1-2]", "test_adds[1-2]"),
        ];
        locate(Framework::Pytest, &mut py, root);
        assert_eq!(py[0].location.as_ref().unwrap().1, 4);
        assert_eq!(py[1].location.as_ref().unwrap().1, 0);

        let mut go_cases = vec![case("example.com/demo/greet", "TestGreet/sub", "sub")];
        locate(Framework::Go, &mut go_cases, root);
        assert_eq!(
            go_cases[0].location,
            Some((root.join("greet").join("greet_test.go"), 2))
        );

        let mut js = vec![case("src/sum.test.js", "sum adds (1+2)", "adds (1+2)")];
        locate(Framework::Jest, &mut js, root);
        assert_eq!(js[0].location, Some((root.join("src/sum.test.js"), 1)));

        let mut jvm_cases = vec![
            case("com.acme.CalcTest", "adds()", "adds()"),
            case(
                "com.acme.GreeterTest",
                "greets by name()",
                "greets by name()",
            ),
        ];
        locate(Framework::Gradle, &mut jvm_cases, root);
        assert_eq!(jvm_cases[0].location.as_ref().unwrap().1, 3);
        assert_eq!(jvm_cases[1].location.as_ref().unwrap().1, 2);

        let mut cs = vec![case("Demo.Tests.CalcTests", "Adds", "Adds")];
        locate(Framework::Dotnet, &mut cs, root);
        assert_eq!(
            cs[0].location,
            Some((root.join("Demo.Tests").join("CalcTests.cs"), 3))
        );
    }
}
