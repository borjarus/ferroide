//! Build tool projects and their tasks (the Cargo / Maven / Gradle / npm
//! tool window), plus the default configurations and editor templates
//! derived from them.
//!
//! Manifests are found by a bounded walk of the project roots
//! ([`walk`](crate::walk): `.gitignore` respected, dependency and output
//! folders skipped) and read without running the tools: starting Gradle or
//! Maven just to list tasks costs seconds and may download half the
//! internet. So the task lists are the conventional ones:
//!
//! - **Cargo** — every `Cargo.toml`: build, check, test, clippy, doc,
//!   bench, a `run` per binary; members of a workspace get `-p name` and
//!   run in the workspace root, a virtual workspace gets `--workspace`;
//! - **npm / pnpm / yarn / bun** (from the lock file or `packageManager`)
//!   — install and every `package.json` script, its body as description;
//! - **Maven** — the lifecycle phases, through `./mvnw` when present;
//! - **Gradle** — build, clean, assemble, check, test and `run` with the
//!   `application` plugin, through `./gradlew` when present; one project
//!   per `settings.gradle(.kts)`;
//! - **Go** — build, test, vet, `run .` for a main package, `mod tidy`;
//! - **CMake** — configure / build / ctest, or the presets of
//!   `CMakePresets.json`;
//! - **.NET** — build, test, run, clean for each `.sln`, `.csproj`,
//!   `.fsproj`.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use ferro_services::{BuildProject, BuildTask, RunConfiguration};
use regex::Regex;

use crate::store::sanitize_id;
use crate::walk;

/// Manifest file names looked for.
const MANIFESTS: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "pom.xml",
    "settings.gradle",
    "settings.gradle.kts",
    "build.gradle",
    "build.gradle.kts",
    "go.mod",
    "CMakeLists.txt",
];

/// Build tool projects under `roots`, at most `max_depth` folders down.
pub fn discover(roots: &[PathBuf], max_depth: usize) -> Vec<BuildProject> {
    let mut projects = Vec::new();
    for root in roots {
        let manifests = walk::files(root, max_depth, 5_000, walk::SKIPPED, true, |p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            MANIFESTS.contains(&name)
                || p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| matches!(e, "sln" | "csproj" | "fsproj"))
        });
        let mut cmake_dirs: Vec<PathBuf> = Vec::new();
        let mut gradle_dirs: Vec<PathBuf> = Vec::new();
        // Shallow first: parents are seen before their children.
        let mut manifests = manifests;
        manifests.sort_by_key(|p| (p.components().count(), p.clone()));
        for manifest in &manifests {
            let dir = manifest.parent().unwrap_or(root);
            let name = manifest
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            let project = match name {
                "Cargo.toml" => cargo(manifest, root),
                "package.json" => node(manifest, root),
                "pom.xml" => maven(manifest),
                "settings.gradle" | "settings.gradle.kts" | "build.gradle" | "build.gradle.kts" => {
                    if gradle_dirs.iter().any(|d| dir.starts_with(d)) {
                        None
                    } else {
                        gradle_dirs.push(dir.to_path_buf());
                        gradle(dir)
                    }
                }
                "go.mod" => go(manifest),
                "CMakeLists.txt" => {
                    if cmake_dirs.iter().any(|d| dir.starts_with(d)) {
                        None
                    } else {
                        cmake_dirs.push(dir.to_path_buf());
                        cmake(manifest)
                    }
                }
                _ => dotnet(manifest),
            };
            projects.extend(project);
        }
    }
    projects.sort_by(|a, b| a.manifest.cmp(&b.manifest));
    projects
}

fn task(name: &str, description: &str, command: String, dir: &Path, is_test: bool) -> BuildTask {
    BuildTask {
        name: name.to_string(),
        description: description.to_string(),
        command,
        working_directory: dir.to_path_buf(),
        is_test,
    }
}

fn folder_name(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub(crate) fn read_toml(path: &Path) -> Option<toml::Table> {
    let text = std::fs::read_to_string(path).ok()?;
    match toml::from_str(&text) {
        Ok(table) => Some(table),
        Err(e) => {
            tracing::warn!(path = %path.display(), "skipping a malformed manifest: {e}");
            None
        }
    }
}

/// The workspace root `Cargo.toml` above (or at) `dir`, within `root`.
pub fn cargo_workspace_root(dir: &Path, root: &Path) -> Option<PathBuf> {
    let mut current = Some(dir);
    while let Some(d) = current {
        let manifest = d.join("Cargo.toml");
        if manifest.is_file() && read_toml(&manifest).is_some_and(|t| t.contains_key("workspace")) {
            return Some(d.to_path_buf());
        }
        if d == root || !d.starts_with(root) {
            break;
        }
        current = d.parent();
    }
    None
}

/// The binaries of a package: `[[bin]]` names, `src/main.rs`,
/// `src/bin/*.rs` and `src/bin/*/main.rs`.
pub fn cargo_binaries(dir: &Path, table: &toml::Table, package: &str) -> Vec<String> {
    let mut bins: Vec<String> = table
        .get("bin")
        .and_then(|b| b.as_array())
        .into_iter()
        .flatten()
        .filter_map(|b| b.get("name").and_then(|n| n.as_str()).map(str::to_string))
        .collect();
    let autobins = table
        .get("package")
        .and_then(|p| p.get("autobins"))
        .and_then(|a| a.as_bool())
        .unwrap_or(true);
    if autobins {
        if dir.join("src/main.rs").is_file() && !bins.iter().any(|b| b == package) {
            bins.insert(0, package.to_string());
        }
        let mut found: Vec<String> = std::fs::read_dir(dir.join("src/bin"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                if path.extension().is_some_and(|x| x == "rs") {
                    path.file_stem().map(|s| s.to_string_lossy().into_owned())
                } else if path.join("main.rs").is_file() {
                    Some(folder_name(&path))
                } else {
                    None
                }
            })
            .collect();
        found.sort();
        for bin in found {
            if !bins.contains(&bin) {
                bins.push(bin);
            }
        }
    }
    bins
}

fn cargo(manifest: &Path, root: &Path) -> Option<BuildProject> {
    let dir = manifest.parent()?;
    let table = read_toml(manifest)?;
    let package = table
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .map(str::to_string);
    let is_workspace = table.contains_key("workspace");
    let mut tasks = Vec::new();
    let Some(package) = package else {
        if !is_workspace {
            return None;
        }
        // A virtual workspace.
        for (name, description, is_test) in [
            ("build", "Compile all packages", false),
            ("check", "Check all packages for errors", false),
            ("test", "Run the tests of all packages", true),
            ("clippy", "Lint all packages", false),
            ("doc", "Build the documentation", false),
            ("bench", "Run the benchmarks", false),
        ] {
            tasks.push(task(
                name,
                description,
                format!("cargo {name} --workspace"),
                dir,
                is_test,
            ));
        }
        tasks.push(task(
            "fmt",
            "Format all packages",
            "cargo fmt --all".into(),
            dir,
            false,
        ));
        return Some(BuildProject {
            tool: "cargo".into(),
            name: folder_name(dir),
            manifest: manifest.to_path_buf(),
            tasks,
        });
    };
    let workspace = cargo_workspace_root(dir, root);
    let member = workspace.as_deref().is_some_and(|w| w != dir);
    let (cwd, p) = match &workspace {
        Some(w) if member => (w.clone(), format!(" -p {package}")),
        _ => (dir.to_path_buf(), String::new()),
    };
    for (name, description, is_test) in [
        ("build", "Compile the package", false),
        ("check", "Check the package for errors", false),
        ("test", "Run the tests", true),
        ("clippy", "Lint the package", false),
        ("doc", "Build the documentation", false),
        ("bench", "Run the benchmarks", false),
    ] {
        tasks.push(task(
            name,
            description,
            format!("cargo {name}{p}"),
            &cwd,
            is_test,
        ));
    }
    if !member {
        tasks.push(task(
            "fmt",
            "Format the code",
            "cargo fmt".into(),
            &cwd,
            false,
        ));
    }
    let bins = cargo_binaries(dir, &table, &package);
    for bin in &bins {
        let (name, command) = if bins.len() == 1 {
            ("run".to_string(), format!("cargo run{p}"))
        } else {
            (
                format!("run --bin {bin}"),
                format!("cargo run{p} --bin {bin}"),
            )
        };
        tasks.push(task(&name, &format!("Run {bin}"), command, &cwd, false));
    }
    Some(BuildProject {
        tool: "cargo".into(),
        name: package,
        manifest: manifest.to_path_buf(),
        tasks,
    })
}

/// The package manager of a Node project: `packageManager`, else the lock
/// file in its folder or a parent (workspaces), else npm.
pub fn node_tool(dir: &Path, package: &serde_json::Value) -> &'static str {
    if let Some(pm) = package.get("packageManager").and_then(|p| p.as_str()) {
        for tool in ["pnpm", "yarn", "bun", "npm"] {
            if pm.starts_with(tool) {
                return tool;
            }
        }
    }
    let mut current = Some(dir);
    let mut levels = 0;
    while let Some(d) = current {
        for (lock, tool) in [
            ("pnpm-lock.yaml", "pnpm"),
            ("yarn.lock", "yarn"),
            ("bun.lockb", "bun"),
            ("bun.lock", "bun"),
            ("package-lock.json", "npm"),
        ] {
            if d.join(lock).is_file() {
                return tool;
            }
        }
        levels += 1;
        if levels > 4 || d.join(".git").exists() {
            break;
        }
        current = d.parent();
    }
    "npm"
}

fn node(manifest: &Path, _root: &Path) -> Option<BuildProject> {
    let dir = manifest.parent()?;
    let text = std::fs::read_to_string(manifest).ok()?;
    let json: serde_json::Value = match serde_json::from_str(&text) {
        Ok(json) => json,
        Err(e) => {
            tracing::warn!(path = %manifest.display(), "skipping a malformed package.json: {e}");
            return None;
        }
    };
    let tool = node_tool(dir, &json);
    let mut tasks = vec![task(
        "install",
        "Install the dependencies",
        format!("{tool} install"),
        dir,
        false,
    )];
    if let Some(scripts) = json.get("scripts").and_then(|s| s.as_object()) {
        for (name, body) in scripts {
            let body = body.as_str().unwrap_or_default();
            let is_test = name == "test"
                || name.starts_with("test:")
                || ["jest", "vitest", "mocha", "node --test", "playwright test"]
                    .iter()
                    .any(|t| body.contains(t));
            tasks.push(task(name, body, format!("{tool} run {name}"), dir, is_test));
        }
    }
    Some(BuildProject {
        tool: tool.into(),
        name: json
            .get("name")
            .and_then(|n| n.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| folder_name(dir)),
        manifest: manifest.to_path_buf(),
        tasks,
    })
}

/// `project > artifactId` of a POM.
fn artifact_id(pom: &str) -> Option<String> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(pom);
    let mut path: Vec<Vec<u8>> = Vec::new();
    loop {
        match reader.read_event().ok()? {
            Event::Start(e) => path.push(e.local_name().as_ref().to_vec()),
            Event::End(_) => {
                path.pop();
            }
            Event::Text(t) if path.len() == 2 && path[1] == b"artifactId" => {
                return Some(t.xml10_content().ok()?.trim().to_string());
            }
            Event::Eof => return None,
            _ => {}
        }
    }
}

fn wrapper(dir: &Path, name: &str, tool: &str) -> String {
    let present = dir.join(name).is_file()
        || dir.join(format!("{name}.cmd")).is_file()
        || dir.join(format!("{name}.bat")).is_file();
    if present {
        format!("./{name}")
    } else {
        tool.to_string()
    }
}

fn maven(manifest: &Path) -> Option<BuildProject> {
    let dir = manifest.parent()?;
    let pom = std::fs::read_to_string(manifest).ok()?;
    let mvn = wrapper(dir, "mvnw", "mvn");
    let tasks = [
        ("clean", "Remove the build output", false),
        ("validate", "Validate the project", false),
        ("compile", "Compile the sources", false),
        ("test", "Run the unit tests", true),
        ("package", "Package the compiled code", false),
        ("verify", "Run integration tests and checks", true),
        ("install", "Install into the local repository", false),
    ]
    .into_iter()
    .map(|(name, description, is_test)| {
        task(name, description, format!("{mvn} {name}"), dir, is_test)
    })
    .collect();
    Some(BuildProject {
        tool: "maven".into(),
        name: artifact_id(&pom).unwrap_or_else(|| folder_name(dir)),
        manifest: manifest.to_path_buf(),
        tasks,
    })
}

static ROOT_PROJECT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"rootProject\.name\s*=\s*["']([^"']+)["']"#).unwrap());
static APPLICATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)^\s*(?:id\s*\(?\s*["']application["']|application\b|apply\s+plugin:\s*["']application["'])"#).unwrap()
});

fn gradle(dir: &Path) -> Option<BuildProject> {
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
    let settings = read("settings.gradle.kts").or_else(|| read("settings.gradle"));
    let manifest = [
        "settings.gradle.kts",
        "settings.gradle",
        "build.gradle.kts",
        "build.gradle",
    ]
    .iter()
    .map(|n| dir.join(n))
    .find(|p| p.is_file())?;
    let name = settings
        .as_deref()
        .and_then(|s| ROOT_PROJECT.captures(s))
        .map(|c| c[1].to_string())
        .unwrap_or_else(|| folder_name(dir));
    // `application` in the root build file or a subproject's.
    let application = walk::files(dir, 3, 200, walk::SKIPPED, true, |p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n == "build.gradle" || n == "build.gradle.kts")
    })
    .iter()
    .any(|p| std::fs::read_to_string(p).is_ok_and(|t| APPLICATION.is_match(&t)));
    let gradle = wrapper(dir, "gradlew", "gradle");
    let mut tasks: Vec<BuildTask> = [
        ("build", "Assemble and test the project", false),
        ("clean", "Delete the build folder", false),
        ("assemble", "Assemble the outputs", false),
        ("check", "Run all checks", true),
        ("test", "Run the unit tests", true),
    ]
    .into_iter()
    .map(|(n, d, t)| task(n, d, format!("{gradle} {n}"), dir, t))
    .collect();
    if application {
        tasks.push(task(
            "run",
            "Run the application",
            format!("{gradle} run"),
            dir,
            false,
        ));
    }
    Some(BuildProject {
        tool: "gradle".into(),
        name,
        manifest,
        tasks,
    })
}

fn go(manifest: &Path) -> Option<BuildProject> {
    let dir = manifest.parent()?;
    let text = std::fs::read_to_string(manifest).ok()?;
    let module = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("module "))
        .map(|m| m.trim().trim_matches('"').to_string())
        .unwrap_or_else(|| folder_name(dir));
    let main = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "go"))
        .any(|e| {
            std::fs::read_to_string(e.path())
                .is_ok_and(|t| t.lines().any(|l| l.trim() == "package main"))
        });
    let mut tasks = vec![
        task(
            "build",
            "Compile all packages",
            "go build ./...".into(),
            dir,
            false,
        ),
        task(
            "test",
            "Test all packages",
            "go test ./...".into(),
            dir,
            true,
        ),
        task(
            "vet",
            "Report suspicious constructs",
            "go vet ./...".into(),
            dir,
            false,
        ),
    ];
    if main {
        tasks.push(task(
            "run",
            "Run the main package",
            "go run .".into(),
            dir,
            false,
        ));
    }
    tasks.push(task(
        "mod tidy",
        "Add missing and remove unused modules",
        "go mod tidy".into(),
        dir,
        false,
    ));
    Some(BuildProject {
        tool: "go".into(),
        name: module,
        manifest: manifest.to_path_buf(),
        tasks,
    })
}

static CMAKE_PROJECT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bproject\s*\(\s*([A-Za-z0-9_.+-]+)").unwrap());

fn cmake(manifest: &Path) -> Option<BuildProject> {
    let dir = manifest.parent()?;
    let text = std::fs::read_to_string(manifest).ok()?;
    let name = CMAKE_PROJECT
        .captures(&text)
        .map(|c| c[1].to_string())
        .unwrap_or_else(|| folder_name(dir));
    let presets: Option<serde_json::Value> = std::fs::read_to_string(dir.join("CMakePresets.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    let names = |kind: &str| -> Vec<String> {
        presets
            .as_ref()
            .and_then(|p| p.get(kind))
            .and_then(|a| a.as_array())
            .into_iter()
            .flatten()
            .filter(|p| !p.get("hidden").and_then(|h| h.as_bool()).unwrap_or(false))
            .filter_map(|p| p.get("name").and_then(|n| n.as_str()).map(str::to_string))
            .collect()
    };
    let mut tasks = Vec::new();
    let configure = names("configurePresets");
    if configure.is_empty() {
        tasks.push(task(
            "configure",
            "Generate the build system in build/",
            "cmake -S . -B build".into(),
            dir,
            false,
        ));
        tasks.push(task(
            "build",
            "Build",
            "cmake --build build".into(),
            dir,
            false,
        ));
        tasks.push(task(
            "test",
            "Run the tests with CTest",
            "ctest --test-dir build".into(),
            dir,
            true,
        ));
    } else {
        for p in configure {
            tasks.push(task(
                &format!("configure {p}"),
                "Configure preset",
                format!("cmake --preset {p}"),
                dir,
                false,
            ));
        }
        for p in names("buildPresets") {
            tasks.push(task(
                &format!("build {p}"),
                "Build preset",
                format!("cmake --build --preset {p}"),
                dir,
                false,
            ));
        }
        for p in names("testPresets") {
            tasks.push(task(
                &format!("test {p}"),
                "Test preset",
                format!("ctest --preset {p}"),
                dir,
                true,
            ));
        }
    }
    Some(BuildProject {
        tool: "cmake".into(),
        name,
        manifest: manifest.to_path_buf(),
        tasks,
    })
}

fn dotnet(manifest: &Path) -> Option<BuildProject> {
    let dir = manifest.parent()?;
    let file = manifest.file_name()?.to_string_lossy().into_owned();
    let solution = file.ends_with(".sln");
    let file_arg = crate::command::quote(&file);
    let mut tasks = vec![
        task(
            "build",
            "Build",
            format!("dotnet build {file_arg}"),
            dir,
            false,
        ),
        task(
            "test",
            "Run the tests",
            format!("dotnet test {file_arg}"),
            dir,
            true,
        ),
    ];
    if !solution {
        tasks.push(task(
            "run",
            "Run the project",
            format!("dotnet run --project {file_arg}"),
            dir,
            false,
        ));
    }
    tasks.push(task(
        "clean",
        "Clean the output",
        format!("dotnet clean {file_arg}"),
        dir,
        false,
    ));
    Some(BuildProject {
        tool: "dotnet".into(),
        name: manifest.file_stem()?.to_string_lossy().into_owned(),
        manifest: manifest.to_path_buf(),
        tasks,
    })
}

/// A task as a temporary configuration (id `task:<…>`).
pub fn task_configuration(task: &BuildTask, tool: &str, root: &Path) -> RunConfiguration {
    let relative = relative_dir(&task.working_directory, root);
    let name = if relative.is_empty() {
        task.command.clone()
    } else {
        format!("{} ({relative})", task.command)
    };
    RunConfiguration {
        id: format!(
            "task:{}",
            sanitize_id(&format!("{relative} {}", task.command))
        ),
        name,
        kind: tool.to_string(),
        command: task.command.clone(),
        working_directory: relative,
        temporary: true,
        ..Default::default()
    }
}

/// `dir` relative to `root` with `/`, or absolute when outside it.
pub fn relative_dir(dir: &Path, root: &Path) -> String {
    match dir.strip_prefix(root) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => dir.to_string_lossy().into_owned(),
    }
}

fn is_python_project(root: &Path) -> bool {
    [
        "pyproject.toml",
        "setup.py",
        "setup.cfg",
        "pytest.ini",
        "requirements.txt",
        "tox.ini",
    ]
    .iter()
    .any(|f| root.join(f).is_file())
}

/// Run configurations to offer when a project has none saved: running
/// and testing each project (`run`, `test`, `dev`, `start` tasks).
pub fn default_configurations(projects: &[BuildProject], root: &Path) -> Vec<RunConfiguration> {
    let mut out: Vec<RunConfiguration> = Vec::new();
    for project in projects {
        for t in &project.tasks {
            let wanted = matches!(t.name.as_str(), "run" | "test" | "dev" | "start")
                || t.name.starts_with("run --bin ");
            if !wanted {
                continue;
            }
            let mut config = task_configuration(t, &project.tool, root);
            config.id = format!(
                "default:{}",
                sanitize_id(&format!("{} {} {}", project.tool, project.name, t.name))
            );
            config.name = format!("{} ({})", t.name, project.name);
            if !out.iter().any(|c| c.id == config.id) {
                out.push(config);
            }
        }
    }
    if is_python_project(root) {
        out.push(RunConfiguration {
            id: "default:pytest".into(),
            name: "pytest".into(),
            kind: "python".into(),
            command: format!("{} -m pytest", crate::runnables::python_interpreter(root)),
            temporary: true,
            ..Default::default()
        });
    }
    out
}

/// One starter configuration per build tool (and Python) in the project.
pub fn templates(projects: &[BuildProject], root: &Path) -> Vec<RunConfiguration> {
    let mut out: Vec<RunConfiguration> = Vec::new();
    for project in projects {
        let id = format!("template:{}", project.tool);
        if out.iter().any(|c| c.id == id) {
            continue;
        }
        let pick = ["run", "dev", "start", "test", "build"]
            .iter()
            .find_map(|n| project.tasks.iter().find(|t| t.name == *n))
            .or_else(|| project.tasks.iter().find(|t| t.name.starts_with("run")))
            .or(project.tasks.first());
        let Some(t) = pick else { continue };
        let label = match project.tool.as_str() {
            "cargo" => "Cargo",
            "maven" => "Maven",
            "gradle" => "Gradle",
            "go" => "Go",
            "cmake" => "CMake",
            "dotnet" => ".NET",
            other => other,
        };
        out.push(RunConfiguration {
            id,
            name: label.to_string(),
            kind: project.tool.clone(),
            command: t.command.clone(),
            working_directory: relative_dir(&t.working_directory, root),
            ..Default::default()
        });
    }
    if is_python_project(root) || root.join("main.py").is_file() {
        let python = crate::runnables::python_interpreter(root);
        let command = if root.join("main.py").is_file() {
            format!("{python} main.py")
        } else {
            format!("{python} -m pytest")
        };
        out.push(RunConfiguration {
            id: "template:python".into(),
            name: "Python".into(),
            kind: "python".into(),
            command,
            ..Default::default()
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn commands(p: &BuildProject) -> Vec<&str> {
        p.tasks.iter().map(|t| t.command.as_str()).collect()
    }

    #[test]
    fn discovers_build_tools() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\"]\n",
        );
        write(
            root,
            "crates/core/Cargo.toml",
            "[package]\nname = \"core\"\n",
        );
        write(root, "crates/core/src/lib.rs", "");
        write(
            root,
            "crates/app/Cargo.toml",
            "[package]\nname = \"app\"\n\n[[bin]]\nname = \"server\"\npath = \"src/server.rs\"\n",
        );
        write(root, "crates/app/src/main.rs", "fn main() {}");
        write(root, "crates/app/src/bin/tool.rs", "fn main() {}");
        write(
            root,
            "target/debug/Cargo.toml",
            "[package]\nname = \"ignored\"\n",
        );
        write(
            root,
            "web/package.json",
            r#"{"name":"web","scripts":{"dev":"vite","test":"vitest run","lint":"eslint ."}}"#,
        );
        write(root, "web/pnpm-lock.yaml", "");
        write(root, "web/node_modules/x/package.json", "{}");
        write(
            root,
            "java/pom.xml",
            "<project><parent><artifactId>parent</artifactId></parent><artifactId>calc</artifactId></project>",
        );
        write(root, "java/mvnw", "");
        write(
            root,
            "kt/settings.gradle.kts",
            "rootProject.name = \"greeter\"\ninclude(\"app\")\n",
        );
        write(
            root,
            "kt/app/build.gradle.kts",
            "plugins {\n    application\n}\n",
        );
        write(root, "go/go.mod", "module example.com/demo\n\ngo 1.22\n");
        write(root, "go/main.go", "package main\n\nfunc main() {}\n");
        write(
            root,
            "cpp/CMakeLists.txt",
            "cmake_minimum_required(VERSION 3.20)\nproject(engine CXX)\n",
        );
        write(root, "cpp/sub/CMakeLists.txt", "add_library(x x.cpp)\n");
        write(
            root,
            "cpp/CMakePresets.json",
            r#"{"version":3,"configurePresets":[{"name":"base","hidden":true},{"name":"debug"}],"buildPresets":[{"name":"debug","configurePreset":"debug"}]}"#,
        );
        write(root, "cs/Demo.csproj", "<Project/>");
        write(root, "broken/Cargo.toml", "[package\nname=");

        let projects = discover(&[root.to_path_buf()], 6);
        let find = |tool: &str, name: &str| {
            projects
                .iter()
                .find(|p| p.tool == tool && p.name == name)
                .unwrap_or_else(|| panic!("no {tool} {name} in {projects:#?}"))
        };
        let ws = projects
            .iter()
            .find(|p| p.manifest == root.join("Cargo.toml"))
            .unwrap();
        assert!(commands(ws).contains(&"cargo test --workspace"));
        assert!(commands(ws).contains(&"cargo fmt --all"));

        let core = find("cargo", "core");
        assert_eq!(core.tasks[0].command, "cargo build -p core");
        assert_eq!(core.tasks[0].working_directory, root);
        assert!(core.tasks.iter().all(|t| !t.name.starts_with("run")));
        assert!(core.tasks.iter().any(|t| t.name == "test" && t.is_test));

        let app = find("cargo", "app");
        let runs: Vec<_> = app
            .tasks
            .iter()
            .filter(|t| t.name.starts_with("run"))
            .map(|t| t.command.as_str())
            .collect();
        assert_eq!(
            runs,
            [
                "cargo run -p app --bin app",
                "cargo run -p app --bin server",
                "cargo run -p app --bin tool"
            ]
        );

        let web = find("pnpm", "web");
        assert_eq!(
            commands(web),
            [
                "pnpm install",
                "pnpm run dev",
                "pnpm run test",
                "pnpm run lint"
            ]
        );
        let test = web.tasks.iter().find(|t| t.name == "test").unwrap();
        assert!(test.is_test);
        assert_eq!(test.description, "vitest run");

        let maven = find("maven", "calc");
        assert_eq!(maven.tasks[3].command, "./mvnw test");

        let gradle = find("gradle", "greeter");
        assert!(commands(gradle).contains(&"gradle run"));
        assert_eq!(projects.iter().filter(|p| p.tool == "gradle").count(), 1);

        let go = find("go", "example.com/demo");
        assert!(commands(go).contains(&"go run ."));
        assert!(
            go.tasks
                .iter()
                .any(|t| t.command == "go test ./..." && t.is_test)
        );

        let cmake = find("cmake", "engine");
        assert_eq!(
            commands(cmake),
            ["cmake --preset debug", "cmake --build --preset debug"]
        );
        assert_eq!(projects.iter().filter(|p| p.tool == "cmake").count(), 1);

        let cs = find("dotnet", "Demo");
        assert!(commands(cs).contains(&"dotnet test Demo.csproj"));

        assert!(projects.iter().all(|p| p.name != "ignored"));

        let defaults = default_configurations(&projects, root);
        let names: Vec<_> = defaults.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"test (core)"), "{names:?}");
        assert!(names.contains(&"dev (web)"), "{names:?}");
        let dev = defaults.iter().find(|c| c.name == "dev (web)").unwrap();
        assert_eq!(dev.working_directory, "web");
        assert!(dev.temporary);

        let templates = templates(&projects, root);
        let tools: Vec<_> = templates.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            tools,
            ["Cargo", "CMake", ".NET", "Go", "Maven", "Gradle", "pnpm"]
        );
    }
}
