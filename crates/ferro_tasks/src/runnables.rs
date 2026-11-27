//! Gutter ▶ icons → temporary run configurations.
//!
//! The language service reports a [`Runnable`] (a `main`, a test, a test
//! module or class) with its name and line; which command runs it depends
//! on the language and on the build tool owning the file, found by
//! walking up from the file to the nearest manifest:
//!
//! | language | main | test | module / class |
//! |---|---|---|---|
//! | Rust | `cargo run [-p] [--bin x / --example x]` | `cargo test [-p] --lib / --bin x / --test x -- mod::name --exact` | `cargo test … -- mod::` |
//! | Python | `python path` | `python -m pytest path::Class::name` | `python -m pytest path[::Class]` |
//! | Go | `go run ./dir` | `go test -run '^Name$' ./dir` | `go test ./dir` |
//! | JS/TS | `node path` / `npx tsx path` | `npx vitest run path -t name` / `npx jest path -t name` | same with the `describe` title |
//! | Java/Kotlin | `java Path.java` / `gradle run` | `mvn test -Dtest=Class#name` / `gradle test --tests pkg.Class.name` | the class |
//! | C# / F# | `dotnet run --project x.csproj` | `dotnet test x.csproj --filter FullyQualifiedName~Ns.Class.Name` | the class |
//!
//! Rust module paths come from the file's place under its target root
//! (`src/a/b.rs` → `a::b`) and the inline `mod x {` blocks enclosing the
//! line. Python classes, Java packages and C# namespaces are read from the
//! file the same cheap way: no parser, the line-based view is enough for
//! the conventional layouts tests are written in.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ferro_services::{RunConfiguration, Runnable, RunnableKind};

use crate::build_tools::{cargo_binaries, cargo_workspace_root, node_tool, relative_dir};
use crate::command::{join, quote, regex_escape};
use crate::store::sanitize_id;

/// The temporary configuration running `runnable` of `path`.
pub fn configuration(path: &Path, runnable: &Runnable, root: &Path) -> Result<RunConfiguration> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let file = File {
        path,
        text: &text,
        runnable,
        root,
    };
    let (kind, name, command, cwd) = match ext.as_str() {
        "rs" => rust(&file)?,
        "py" => python(&file)?,
        "go" => go(&file)?,
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" => javascript(&file)?,
        "java" | "kt" | "kts" => jvm(&file, &ext)?,
        "cs" | "fs" => dotnet(&file)?,
        _ => bail!("don't know how to run {}", path.display()),
    };
    let working_directory = relative_dir(&cwd, root);
    Ok(RunConfiguration {
        id: format!(
            "temp:{}",
            sanitize_id(&format!("{name} {}", relative_dir(path, root)))
        ),
        name,
        kind: kind.into(),
        command,
        working_directory,
        temporary: true,
        ..Default::default()
    })
}

struct File<'a> {
    path: &'a Path,
    text: &'a str,
    runnable: &'a Runnable,
    root: &'a Path,
}

impl File<'_> {
    fn dir(&self) -> &Path {
        self.path.parent().unwrap_or(self.root)
    }

    fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn stem(&self) -> String {
        self.path
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn line(&self, n: u32) -> &str {
        self.text.lines().nth(n as usize).unwrap_or_default()
    }

    fn label(&self) -> &str {
        &self.runnable.label
    }

    /// The test name used in configuration names.
    fn title(&self, main: &str) -> String {
        match self.runnable.kind {
            RunnableKind::Main => main.to_string(),
            RunnableKind::Test => format!("test {}", self.label()),
            RunnableKind::TestModule => format!("tests in {}", self.label()),
        }
    }
}

/// The nearest folder at or above `start` holding a file `accept`s,
/// staying inside `root` when `start` is in it.
fn find_up(start: &Path, root: &Path, accept: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let inside = start.starts_with(root);
    let mut current = Some(start);
    while let Some(dir) = current {
        if accept(dir) {
            return Some(dir.to_path_buf());
        }
        if inside && dir == root {
            break;
        }
        current = dir.parent();
    }
    None
}

fn has(names: &'static [&'static str]) -> impl Fn(&Path) -> bool {
    move |dir: &Path| names.iter().any(|n| dir.join(n).is_file())
}

fn rel(path: &Path, base: &Path) -> String {
    relative_dir(path, base)
}

type Built = (&'static str, String, String, PathBuf);

// ---------------------------------------------------------------- Rust

#[derive(Debug, PartialEq)]
enum Target {
    Lib,
    Bin(String),
    Example(String),
    Test(String),
    Bench(String),
}

/// Inline `mod name {` blocks open at `line` (0-based), outermost first.
fn enclosing_rust_modules(text: &str, line: u32) -> Vec<String> {
    let mut stack: Vec<(String, i32)> = Vec::new();
    let mut depth = 0i32;
    for (n, l) in text.lines().enumerate() {
        if n as u32 >= line {
            break;
        }
        let code = l.split("//").next().unwrap_or_default();
        let trimmed = code.trim_start();
        let declaration = trimmed
            .strip_prefix("pub ")
            .or_else(|| trimmed.strip_prefix("pub(crate) "))
            .or_else(|| trimmed.strip_prefix("pub(super) "))
            .unwrap_or(trimmed);
        if let Some(rest) = declaration.strip_prefix("mod ")
            && code.contains('{')
        {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            stack.push((name, depth + 1));
        }
        for c in code.chars() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    while stack.last().is_some_and(|(_, d)| *d > depth) {
                        stack.pop();
                    }
                }
                _ => {}
            }
        }
    }
    stack.into_iter().map(|(n, _)| n).collect()
}

/// The target a file belongs to and its module path within the target.
fn rust_target(rel: &str, package: &str, has_lib: bool) -> (Target, Vec<String>) {
    let parts: Vec<&str> = rel.split('/').collect();
    let module_path = |inner: &[&str]| -> Vec<String> {
        let mut out: Vec<String> = inner
            .iter()
            .map(|s| s.trim_end_matches(".rs").to_string())
            .collect();
        if out
            .last()
            .is_some_and(|l| l == "mod" || l == "main" || l == "lib")
        {
            out.pop();
        }
        out
    };
    let named = |parts: &[&str]| -> (String, Vec<String>) {
        // `dir/x.rs` → (x, []), `dir/x/main.rs` → (x, []), `dir/x/a.rs` → (x, [a])
        let name = parts[1].trim_end_matches(".rs").to_string();
        let inner = if parts.len() > 2 {
            module_path(&parts[2..])
        } else {
            Vec::new()
        };
        (name, inner)
    };
    match parts.as_slice() {
        ["src", "main.rs"] => (Target::Bin(package.to_string()), Vec::new()),
        ["src", "bin", rest @ ..] if !rest.is_empty() => {
            let (name, inner) = named(&parts[1..]);
            (Target::Bin(name), inner)
        }
        ["examples", ..] if parts.len() > 1 => {
            let (name, inner) = named(&parts);
            (Target::Example(name), inner)
        }
        ["tests", ..] if parts.len() > 1 => {
            let (name, inner) = named(&parts);
            (Target::Test(name), inner)
        }
        ["benches", ..] if parts.len() > 1 => {
            let (name, inner) = named(&parts);
            (Target::Bench(name), inner)
        }
        ["src", rest @ ..] => {
            let target = if has_lib {
                Target::Lib
            } else {
                Target::Bin(package.to_string())
            };
            (target, module_path(rest))
        }
        _ => (Target::Lib, Vec::new()),
    }
}

fn rust(f: &File) -> Result<Built> {
    let package_dir = find_up(f.dir(), f.root, |d| {
        crate::build_tools::read_toml(&d.join("Cargo.toml"))
            .is_some_and(|t| t.contains_key("package"))
    })
    .context("no Cargo.toml with a [package] above the file")?;
    let table = crate::build_tools::read_toml(&package_dir.join("Cargo.toml")).unwrap_or_default();
    let package = table
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or_default()
        .to_string();
    let workspace = cargo_workspace_root(&package_dir, f.root);
    let member = workspace.as_deref().is_some_and(|w| w != package_dir);
    let cwd = if member {
        workspace.clone().unwrap_or_else(|| package_dir.clone())
    } else {
        package_dir.clone()
    };
    let mut words: Vec<String> = vec!["cargo".into()];
    let p: Vec<String> = if member {
        vec!["-p".into(), package.clone()]
    } else {
        Vec::new()
    };
    let rel_path = rel(f.path, &package_dir);
    let has_lib = package_dir.join("src/lib.rs").is_file();
    let (target, file_modules) = rust_target(&rel_path, &package, has_lib);
    match f.runnable.kind {
        RunnableKind::Main => {
            words.push("run".into());
            words.extend(p);
            match &target {
                Target::Example(x) => words.extend(["--example".into(), x.clone()]),
                Target::Bin(x) if cargo_binaries(&package_dir, &table, &package).len() > 1 => {
                    words.extend(["--bin".into(), x.clone()]);
                }
                _ => {}
            }
            let title = match &target {
                Target::Bin(x) if x != &package => format!("Run {x}"),
                Target::Example(x) => format!("Run {x}"),
                _ => format!("Run {}", f.file_name()),
            };
            Ok(("cargo", title, join(&words), cwd))
        }
        RunnableKind::Test | RunnableKind::TestModule => {
            let bench = matches!(target, Target::Bench(_));
            words.push(if bench { "bench" } else { "test" }.into());
            words.extend(p);
            match &target {
                Target::Lib => words.push("--lib".into()),
                Target::Bin(x) => words.extend(["--bin".into(), x.clone()]),
                Target::Example(x) => words.extend(["--example".into(), x.clone()]),
                Target::Test(x) => words.extend(["--test".into(), x.clone()]),
                Target::Bench(x) => words.extend(["--bench".into(), x.clone()]),
            }
            let mut path = file_modules;
            path.extend(enclosing_rust_modules(f.text, f.runnable.line));
            let module_here = f
                .line(f.runnable.line)
                .contains(&format!("mod {}", f.label()));
            let filter = if f.runnable.kind == RunnableKind::Test {
                path.push(f.label().to_string());
                Some((path.join("::"), true))
            } else if module_here {
                path.push(f.label().to_string());
                Some((format!("{}::", path.join("::")), false))
            } else if path.is_empty() {
                None
            } else {
                Some((format!("{}::", path.join("::")), false))
            };
            if let Some((filter, exact)) = filter {
                words.push("--".into());
                words.push(filter);
                if exact {
                    words.push("--exact".into());
                }
            }
            Ok(("cargo", f.title(""), join(&words), cwd))
        }
    }
}

// ---------------------------------------------------------------- Python

/// The project's virtual environment interpreter, else `python` (Windows)
/// or `python3`.
pub fn python_interpreter(dir: &Path) -> String {
    for venv in [".venv", "venv", "env"] {
        let candidates = if cfg!(windows) {
            [dir.join(venv).join("Scripts").join("python.exe")]
        } else {
            [dir.join(venv).join("bin").join("python")]
        };
        if let Some(found) = candidates.iter().find(|p| p.is_file()) {
            return quote(&found.to_string_lossy());
        }
    }
    if cfg!(windows) { "python" } else { "python3" }.to_string()
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Classes enclosing `line`, outermost first.
fn enclosing_python_classes(text: &str, line: u32) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let Some(current) = lines.get(line as usize) else {
        return Vec::new();
    };
    let mut level = indent(current);
    let mut classes = Vec::new();
    for l in lines[..line as usize].iter().rev() {
        if l.trim().is_empty() || indent(l) >= level {
            continue;
        }
        level = indent(l);
        if let Some(rest) = l.trim_start().strip_prefix("class ") {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            classes.push(name);
        }
        if level == 0 {
            break;
        }
    }
    classes.reverse();
    classes
}

fn python(f: &File) -> Result<Built> {
    let project = find_up(
        f.dir(),
        f.root,
        has(&[
            "pyproject.toml",
            "setup.py",
            "setup.cfg",
            "pytest.ini",
            "tox.ini",
        ]),
    )
    .unwrap_or_else(|| f.root.to_path_buf());
    let python = python_interpreter(&project);
    let path = rel(f.path, &project);
    let (title, command) = match f.runnable.kind {
        RunnableKind::Main => (
            format!("Run {}", f.file_name()),
            format!("{python} {}", quote(&path)),
        ),
        RunnableKind::Test => {
            let mut node = vec![path];
            node.extend(enclosing_python_classes(f.text, f.runnable.line));
            node.push(f.label().to_string());
            (
                f.title(""),
                format!("{python} -m pytest {}", quote(&node.join("::"))),
            )
        }
        RunnableKind::TestModule => {
            let line = f.line(f.runnable.line).trim_start();
            let node = if line.starts_with("class ") {
                let mut node = vec![path];
                node.extend(enclosing_python_classes(f.text, f.runnable.line));
                node.push(f.label().to_string());
                node.join("::")
            } else {
                path
            };
            (f.title(""), format!("{python} -m pytest {}", quote(&node)))
        }
    };
    Ok(("python", title, command, project))
}

// ---------------------------------------------------------------- Go

fn go(f: &File) -> Result<Built> {
    let module =
        find_up(f.dir(), f.root, has(&["go.mod"])).unwrap_or_else(|| f.dir().to_path_buf());
    let package = match rel(f.dir(), &module) {
        dir if dir.is_empty() => ".".to_string(),
        dir => format!("./{dir}"),
    };
    let label = f.label();
    let command = match f.runnable.kind {
        RunnableKind::Main => format!("go run {package}"),
        RunnableKind::TestModule => format!("go test {package}"),
        RunnableKind::Test if label.starts_with("Benchmark") => format!(
            "go test -run '^$' -bench {} {package}",
            quote(&format!("^{}$", regex_escape(label)))
        ),
        RunnableKind::Test => format!(
            "go test -run {} {package}",
            quote(&format!("^{}$", regex_escape(label)))
        ),
    };
    Ok((
        "go",
        f.title(&format!("Run {}", f.file_name())),
        command,
        module,
    ))
}

// ---------------------------------------------------------------- JS / TS

fn javascript(f: &File) -> Result<Built> {
    let package_dir = find_up(f.dir(), f.root, has(&["package.json"]));
    let Some(package_dir) = package_dir else {
        let command = match f.runnable.kind {
            RunnableKind::Main => format!("node {}", quote(&f.file_name())),
            _ => format!(
                "node --test --test-name-pattern {} {}",
                quote(&regex_escape(f.label())),
                quote(&f.file_name())
            ),
        };
        return Ok((
            "node",
            f.title(&format!("Run {}", f.file_name())),
            command,
            f.dir().to_path_buf(),
        ));
    };
    let json: serde_json::Value = std::fs::read_to_string(package_dir.join("package.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let depends = |name: &str| {
        ["dependencies", "devDependencies"]
            .iter()
            .any(|k| json.get(k).and_then(|d| d.get(name)).is_some())
    };
    let exec = match node_tool(&package_dir, &json) {
        "pnpm" => "pnpm exec",
        "yarn" => "yarn",
        "bun" => "bunx",
        _ => "npx",
    };
    let path = quote(&rel(f.path, &package_dir));
    let typescript = matches!(
        f.path.extension().and_then(|e| e.to_str()),
        Some("ts" | "tsx" | "mts" | "cts")
    );
    let pattern = quote(&regex_escape(f.label()));
    let (kind, command) = match f.runnable.kind {
        RunnableKind::Main if typescript => ("node", format!("{exec} tsx {path}")),
        RunnableKind::Main => ("node", format!("node {path}")),
        _ if depends("vitest") => ("npm", format!("{exec} vitest run {path} -t {pattern}")),
        _ if depends("jest") => ("npm", format!("{exec} jest {path} -t {pattern}")),
        _ => (
            "node",
            format!("node --test --test-name-pattern {pattern} {path}"),
        ),
    };
    Ok((
        kind,
        f.title(&format!("Run {}", f.file_name())),
        command,
        package_dir,
    ))
}

// ---------------------------------------------------------------- JVM

fn jvm(f: &File, ext: &str) -> Result<Built> {
    let package = f
        .text
        .lines()
        .find_map(|l| l.trim().strip_prefix("package "))
        .map(|p| p.trim().trim_end_matches(';').trim().to_string())
        .unwrap_or_default();
    let class = if f.runnable.kind == RunnableKind::TestModule {
        f.label().to_string()
    } else {
        f.stem()
    };
    let qualified = if package.is_empty() {
        class.clone()
    } else {
        format!("{package}.{class}")
    };
    let build = find_up(
        f.dir(),
        f.root,
        has(&[
            "pom.xml",
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
        ]),
    );
    let Some(build) = build else {
        if f.runnable.kind == RunnableKind::Main && ext == "java" {
            return Ok((
                "java",
                format!("Run {}", f.file_name()),
                format!("java {}", quote(&f.file_name())),
                f.dir().to_path_buf(),
            ));
        }
        bail!("no Maven or Gradle build above {}", f.path.display());
    };
    let title = f.title(&format!("Run {}", f.file_name()));
    if build.join("pom.xml").is_file() {
        let command = match f.runnable.kind {
            RunnableKind::Main if ext == "java" => format!("java {}", quote(&rel(f.path, &build))),
            RunnableKind::Main => format!("mvn compile exec:java -Dexec.mainClass={qualified}Kt"),
            RunnableKind::Test => format!(
                "mvn test {} -Dsurefire.failIfNoSpecifiedTests=false",
                quote(&format!("-Dtest={class}#{}", f.label()))
            ),
            RunnableKind::TestModule => {
                format!("mvn test -Dtest={class} -Dsurefire.failIfNoSpecifiedTests=false")
            }
        };
        let kind = if command.starts_with("java ") {
            "java"
        } else {
            "maven"
        };
        return Ok((kind, title, command, build));
    }
    // Gradle: the root holds settings.gradle; tasks are addressed by
    // project path (`:app:test`).
    let gradle_root = find_up(
        &build,
        f.root,
        has(&["settings.gradle", "settings.gradle.kts"]),
    )
    .unwrap_or_else(|| build.clone());
    let project = rel(&build, &gradle_root);
    let prefix = if project.is_empty() {
        String::new()
    } else {
        format!(":{}:", project.replace('/', ":"))
    };
    let command = match f.runnable.kind {
        RunnableKind::Main if ext == "java" => {
            return Ok((
                "java",
                title,
                format!("java {}", quote(&rel(f.path, &build))),
                build,
            ));
        }
        RunnableKind::Main => format!("gradle {prefix}run"),
        RunnableKind::Test => format!(
            "gradle {prefix}test --tests {}",
            quote(&format!("{qualified}.{}", f.label()))
        ),
        RunnableKind::TestModule => format!("gradle {prefix}test --tests {}", quote(&qualified)),
    };
    Ok(("gradle", title, command, gradle_root))
}

// ---------------------------------------------------------------- .NET

fn dotnet(f: &File) -> Result<Built> {
    let is_project = |d: &Path| {
        std::fs::read_dir(d)
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| {
                e.path()
                    .extension()
                    .is_some_and(|x| x == "csproj" || x == "fsproj")
            })
    };
    let dir =
        find_up(f.dir(), f.root, is_project).context("no .csproj / .fsproj above the file")?;
    let project = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.extension()
                .is_some_and(|x| x == "csproj" || x == "fsproj")
        })
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let namespace = f.text.lines().find_map(|l| {
        let rest = l.trim().strip_prefix("namespace ")?;
        Some(rest.trim().trim_end_matches([';', '{']).trim().to_string())
    });
    let class = if f.runnable.kind == RunnableKind::TestModule {
        Some(f.label().to_string())
    } else {
        f.text
            .lines()
            .take(f.runnable.line as usize)
            .filter_map(|l| {
                let at = l.find("class ")?;
                let name: String = l[at + 6..]
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                (!name.is_empty()).then_some(name)
            })
            .last()
    };
    let mut qualified: Vec<String> = namespace.into_iter().collect();
    qualified.extend(class);
    if f.runnable.kind == RunnableKind::Test {
        qualified.push(f.label().to_string());
    }
    let project_arg = quote(&project);
    let command = match f.runnable.kind {
        RunnableKind::Main => format!("dotnet run --project {project_arg}"),
        _ => format!(
            "dotnet test {project_arg} --filter {}",
            quote(&format!("FullyQualifiedName~{}", qualified.join(".")))
        ),
    };
    Ok((
        "dotnet",
        f.title(&format!("Run {}", f.file_name())),
        command,
        dir,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, text: &str) -> PathBuf {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    fn run(
        path: &Path,
        kind: RunnableKind,
        label: &str,
        line: u32,
        root: &Path,
    ) -> RunConfiguration {
        configuration(
            path,
            &Runnable {
                line,
                kind,
                label: label.into(),
            },
            root,
        )
        .unwrap()
    }

    #[test]
    fn rust_runnables() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "Cargo.toml", "[workspace]\nmembers = [\"app\"]\n");
        write(root, "app/Cargo.toml", "[package]\nname = \"app\"\n");
        let main = write(root, "app/src/main.rs", "fn main() {}\n");
        write(root, "app/src/lib.rs", "mod greet;\n");
        let greet = write(
            root,
            "app/src/greet/mod.rs",
            "pub fn hi() {}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn greets_by_name() {}\n}\n",
        );
        let smoke = write(root, "app/tests/smoke.rs", "#[test]\nfn prints() {}\n");
        let tool = write(root, "app/src/bin/tool.rs", "fn main() {}\n");

        let c = run(&main, RunnableKind::Main, "main", 0, root);
        assert_eq!(c.command, "cargo run -p app --bin app");
        assert_eq!(c.name, "Run main.rs");
        assert_eq!(c.working_directory, "");
        assert!(c.temporary);

        let c = run(&tool, RunnableKind::Main, "main", 0, root);
        assert_eq!(c.command, "cargo run -p app --bin tool");
        assert_eq!(c.name, "Run tool");

        let c = run(&greet, RunnableKind::Test, "greets_by_name", 7, root);
        assert_eq!(
            c.command,
            "cargo test -p app --lib -- greet::tests::greets_by_name --exact"
        );
        assert_eq!(c.name, "test greets_by_name");

        let c = run(&greet, RunnableKind::TestModule, "tests", 3, root);
        assert_eq!(c.command, "cargo test -p app --lib -- greet::tests::");

        let c = run(&smoke, RunnableKind::Test, "prints", 1, root);
        assert_eq!(
            c.command,
            "cargo test -p app --test smoke -- prints --exact"
        );
    }

    #[test]
    fn rust_targets_and_modules() {
        assert_eq!(
            rust_target("src/a/b.rs", "p", true),
            (Target::Lib, vec!["a".into(), "b".into()])
        );
        assert_eq!(
            rust_target("src/bin/x/main.rs", "p", true),
            (Target::Bin("x".into()), vec![])
        );
        assert_eq!(
            rust_target("examples/demo.rs", "p", true),
            (Target::Example("demo".into()), vec![])
        );
        assert_eq!(
            rust_target("src/util.rs", "p", false),
            (Target::Bin("p".into()), vec!["util".into()])
        );
        let text = "mod a {\n    mod b {\n        fn x() {}\n    }\n    fn y() {}\n}\nfn z() {}\n";
        assert_eq!(enclosing_rust_modules(text, 2), ["a", "b"]);
        assert_eq!(enclosing_rust_modules(text, 4), ["a"]);
        assert!(enclosing_rust_modules(text, 6).is_empty());
    }

    #[test]
    fn python_go_js_jvm_dotnet_runnables() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let python = python_interpreter(root);

        write(root, "py/pyproject.toml", "");
        let test_py = write(
            root,
            "py/tests/test_calc.py",
            "def test_adds():\n    pass\n\nclass TestCalc:\n    def test_divides(self):\n        pass\n",
        );
        let c = run(&test_py, RunnableKind::Test, "test_divides", 4, root);
        assert_eq!(
            c.command,
            format!("{python} -m pytest tests/test_calc.py::TestCalc::test_divides")
        );
        assert_eq!(c.working_directory, "py");
        let c = run(&test_py, RunnableKind::TestModule, "TestCalc", 3, root);
        assert_eq!(
            c.command,
            format!("{python} -m pytest tests/test_calc.py::TestCalc")
        );
        let c = run(&test_py, RunnableKind::Test, "test_adds", 0, root);
        assert_eq!(
            c.command,
            format!("{python} -m pytest tests/test_calc.py::test_adds")
        );

        write(root, "go/go.mod", "module example.com/demo\n");
        let go_test = write(
            root,
            "go/greet/greet_test.go",
            "package greet\n\nfunc TestGreet(t *testing.T) {}\n",
        );
        let c = run(&go_test, RunnableKind::Test, "TestGreet", 2, root);
        assert_eq!(c.command, "go test -run '^TestGreet$' ./greet");
        assert_eq!(c.working_directory, "go");
        let go_main = write(root, "go/main.go", "package main\n\nfunc main() {}\n");
        assert_eq!(
            run(&go_main, RunnableKind::Main, "main", 2, root).command,
            "go run ."
        );

        write(
            root,
            "web/package.json",
            r#"{"devDependencies":{"vitest":"2"}}"#,
        );
        let js = write(root, "web/src/sum.test.ts", "it('adds (1+2)', () => {})\n");
        let c = run(&js, RunnableKind::Test, "adds (1+2)", 0, root);
        assert_eq!(
            c.command,
            r"npx vitest run src/sum.test.ts -t 'adds \(1\+2\)'"
        );
        let main_ts = write(root, "web/src/main.ts", "console.log(1)\n");
        assert_eq!(
            run(&main_ts, RunnableKind::Main, "main", 0, root).command,
            "npx tsx src/main.ts"
        );

        write(root, "jvm/pom.xml", "<project/>");
        let java = write(
            root,
            "jvm/src/test/java/com/acme/CalcTest.java",
            "package com.acme;\n\nclass CalcTest {\n  @Test\n  void adds() {}\n}\n",
        );
        let c = run(&java, RunnableKind::Test, "adds", 4, root);
        assert_eq!(
            c.command,
            "mvn test -Dtest=CalcTest#adds -Dsurefire.failIfNoSpecifiedTests=false"
        );

        write(root, "kt/settings.gradle.kts", "");
        write(root, "kt/app/build.gradle.kts", "");
        let kt = write(
            root,
            "kt/app/src/test/kotlin/GreeterTest.kt",
            "package com.acme\n\nclass GreeterTest {\n  @Test\n  fun greets() {}\n}\n",
        );
        let c = run(&kt, RunnableKind::Test, "greets", 4, root);
        assert_eq!(
            c.command,
            "gradle :app:test --tests com.acme.GreeterTest.greets"
        );
        assert_eq!(c.working_directory, "kt");

        write(root, "cs/Demo.Tests.csproj", "<Project/>");
        let cs = write(
            root,
            "cs/CalcTests.cs",
            "namespace Demo.Tests;\n\npublic class CalcTests\n{\n    [Fact]\n    public void Adds() {}\n}\n",
        );
        let c = run(&cs, RunnableKind::Test, "Adds", 5, root);
        assert_eq!(
            c.command,
            "dotnet test Demo.Tests.csproj --filter FullyQualifiedName~Demo.Tests.CalcTests.Adds"
        );
    }
}
