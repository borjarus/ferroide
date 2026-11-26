//! The inner loop against real language servers (plan Stage 6 DoD).
//!
//! Ignored by default: they need the servers. CI installs them
//! (`.github/workflows/ci.yml`, job `lsp`) and runs
//! `cargo test -p ferro_lsp --test real_servers -- --ignored`. Locally, set
//! `FERRO_LSP_INSTALL_DIR` to a folder servers were installed into with
//! `ferro_lsp::install`, or put them on `PATH`. A test whose server is not
//! found reports it and passes, unless `FERRO_LSP_REQUIRE` (server ids,
//! comma-separated) names it.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use ferro_languages::{LanguageRegistry, SyntaxLanguageService};
use ferro_lsp::{LspLanguageService, LspOptions, ProcessLauncher, install};
use ferro_services::{GotoKind, LanguageService, TextPos};

struct Project {
    _dir: tempfile::TempDir,
    root: PathBuf,
    service: LspLanguageService,
}

fn install_dir() -> Option<PathBuf> {
    std::env::var_os("FERRO_LSP_INSTALL_DIR").map(PathBuf::from)
}

/// A project of `files`, or `None` (with a note) when `server` is missing.
fn project(language: &str, server: &str, files: &[(&str, &str)]) -> Option<Project> {
    let registry = LanguageRegistry::shared_builtin();
    let manifest = registry
        .get(language)
        .unwrap()
        .manifest()
        .language_servers
        .iter()
        .find(|s| s.id == server)
        .cloned()
        .unwrap();
    if install::resolve_command(&manifest.command, &manifest.id, install_dir().as_deref()).is_none()
    {
        let required = std::env::var("FERRO_LSP_REQUIRE").unwrap_or_default();
        if required.split(',').any(|id| id.trim() == server) {
            panic!("{} is not installed", manifest.command);
        }
        eprintln!("skipped: {} is not installed", manifest.command);
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = PathBuf::from(
        dir.path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .trim_start_matches(r"\\?\"),
    );
    for (name, text) in files {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let mut options = LspOptions {
        install_dir: install_dir(),
        ..Default::default()
    };
    // Only the server under test.
    for other in &registry.get(language).unwrap().manifest().language_servers {
        if other.id != server {
            options.servers.insert(
                other.id.clone(),
                ferro_lsp::ServerSettings {
                    enabled: Some(false),
                    ..Default::default()
                },
            );
        }
    }
    let fallback = SyntaxLanguageService::new(registry.clone(), Arc::new(Vec::new));
    let roots_root = root.clone();
    let service = LspLanguageService::new(
        registry,
        fallback,
        Arc::new(move || vec![roots_root.clone()]),
        Arc::new(ProcessLauncher {
            install_dir: install_dir(),
        }),
        options,
        true,
    );
    Some(Project {
        _dir: dir,
        root,
        service,
    })
}

/// Retry `f` until it returns `Some` (servers index in the background).
fn eventually<T>(what: &str, timeout: Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(value) = f() {
            return value;
        }
        assert!(start.elapsed() < timeout, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Hover, definition, completion and an error diagnostic in `file`.
fn inner_loop(
    p: &Project,
    file: &str,
    hover_at: TextPos,
    definition_line: u32,
    completion: (&str, TextPos, &str),
    broken: &str,
) {
    let path = p.root.join(file);
    let text = std::fs::read_to_string(&path).unwrap();
    let s = &p.service;
    let timeout = Duration::from_secs(120);

    let hover = eventually("hover", timeout, || {
        smol::block_on(s.hover(&path, &text, hover_at)).unwrap()
    });
    assert!(!hover.contents.is_empty());

    let defs = eventually("definition", timeout, || {
        let found = smol::block_on(s.goto(&path, &text, hover_at, GotoKind::Definition)).unwrap();
        (!found.is_empty()).then_some(found)
    });
    assert_eq!(defs[0].range.start.line, definition_line, "{defs:?}");

    let (completion_text, completion_at, expected) = completion;
    let items = eventually("completion", timeout, || {
        let items = smol::block_on(s.completions(&path, completion_text, completion_at)).unwrap();
        items
            .iter()
            .any(|i| i.label.starts_with(expected))
            .then_some(items)
    });
    assert!(items.iter().any(|i| i.label.starts_with(expected)));

    let refs = smol::block_on(s.references(&path, &text, hover_at, true)).unwrap();
    assert!(refs.len() >= 2, "{refs:?}");

    let errors = eventually("diagnostics", timeout, || {
        let found = smol::block_on(s.diagnostics(&path, broken)).unwrap();
        (!found.is_empty()).then_some(found)
    });
    assert!(
        errors.iter().all(|d| d.source != "tree-sitter"),
        "{errors:?}"
    );

    let rename = smol::block_on(s.rename(&path, &text, hover_at, "renamed")).unwrap();
    assert!(rename.edit_count() >= 2);
}

#[test]
#[ignore = "needs rust-analyzer"]
fn rust_analyzer() {
    let main = "fn greet(name: &str) -> String {\n    format!(\"Hi {name}\")\n}\n\nfn main() {\n    let s = greet(\"x\");\n    println!(\"{s}\");\n}\n";
    let Some(p) = project(
        "rust",
        "rust-analyzer",
        &[
            (
                "Cargo.toml",
                "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            ),
            ("src/main.rs", main),
        ],
    ) else {
        return;
    };
    let completion = "fn main() {\n    let v: Vec<u8> = Vec::new();\n    v.\n}\n";
    inner_loop(
        &p,
        "src/main.rs",
        TextPos::new(5, 13),
        0,
        (completion, TextPos::new(2, 6), "len"),
        "fn main() {\n    let x: u32 = \"no\";\n}\n",
    );
}

#[test]
#[ignore = "needs basedpyright"]
fn basedpyright() {
    let main = "def greet(name: str) -> str:\n    return 'Hi ' + name\n\n\nprint(greet('x'))\n";
    let Some(p) = project(
        "python",
        "basedpyright",
        &[
            ("pyproject.toml", "[project]\nname = \"demo\"\n"),
            ("main.py", main),
        ],
    ) else {
        return;
    };
    let completion = "import os\nos.pa";
    inner_loop(
        &p,
        "main.py",
        TextPos::new(4, 7),
        0,
        (completion, TextPos::new(1, 5), "path"),
        "x: int = 'no'\n",
    );
}

#[test]
#[ignore = "needs vtsls"]
fn typescript() {
    let main = "function greet(name: string): string {\n  return `Hi ${name}`;\n}\n\nconsole.log(greet('x'));\n";
    let Some(p) = project(
        "typescript",
        "vtsls",
        &[
            ("package.json", "{\"name\": \"demo\"}\n"),
            ("tsconfig.json", "{}\n"),
            ("main.ts", main),
        ],
    ) else {
        return;
    };
    let completion = "const s = 'x';\ns.toUp";
    inner_loop(
        &p,
        "main.ts",
        TextPos::new(4, 14),
        0,
        (completion, TextPos::new(1, 6), "toUpperCase"),
        "const x: number = 'no';\n",
    );
}

#[test]
#[ignore = "needs gopls"]
fn gopls() {
    let main = "package main\n\nimport \"fmt\"\n\nfunc greet(name string) string {\n\treturn \"Hi \" + name\n}\n\nfunc main() {\n\tfmt.Println(greet(\"x\"))\n}\n";
    let Some(p) = project(
        "go",
        "gopls",
        &[("go.mod", "module demo\n\ngo 1.21\n"), ("main.go", main)],
    ) else {
        return;
    };
    let completion = "package main\n\nimport \"fmt\"\n\nfunc main() {\n\tfmt.Prin\n}\n";
    inner_loop(
        &p,
        "main.go",
        TextPos::new(9, 14),
        4,
        (completion, TextPos::new(5, 9), "Print"),
        "package main\n\nfunc main() {\n\tvar x int = \"no\"\n}\n",
    );
}

#[test]
#[ignore = "needs jdtls and a JDK"]
fn jdtls() {
    let main = "public class Main {\n    static String greet(String name) {\n        return \"Hi \" + name;\n    }\n\n    public static void main(String[] args) {\n        System.out.println(greet(\"x\"));\n    }\n}\n";
    let Some(p) = project("java", "jdtls", &[("Main.java", main)]) else {
        return;
    };
    let completion = "public class Main {\n    void f(String s) {\n        s.toUp\n    }\n}\n";
    inner_loop(
        &p,
        "Main.java",
        TextPos::new(6, 28),
        1,
        (completion, TextPos::new(2, 14), "toUpperCase"),
        "public class Main {\n    int x = \"no\";\n}\n",
    );
}

/// A missing server: the tree-sitter fallback answers and the reason
/// names how to install it.
#[test]
fn a_missing_server_falls_back_to_tree_sitter() {
    let registry = LanguageRegistry::shared_builtin();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let mut options = LspOptions {
        install_dir: Some(root.join("servers")),
        ..Default::default()
    };
    options.servers.insert(
        "gopls".into(),
        ferro_lsp::ServerSettings {
            command: Some("surely-not-a-real-gopls".into()),
            ..Default::default()
        },
    );
    let roots_root = root.clone();
    let service = LspLanguageService::new(
        registry.clone(),
        SyntaxLanguageService::new(registry, Arc::new(Vec::new)),
        Arc::new(move || vec![roots_root.clone()]),
        Arc::new(ProcessLauncher {
            install_dir: Some(root.join("servers")),
        }),
        options,
        true,
    );
    let path: &Path = &root.join("main.go");
    let text = "package main\n\nfunc main() {\n";
    let diagnostics = smol::block_on(service.diagnostics(path, text)).unwrap();
    assert!(
        diagnostics.iter().any(|d| d.source == "tree-sitter"),
        "{diagnostics:?}"
    );
    let problems = service.problems();
    assert_eq!(problems[0].0, "gopls");
    assert!(problems[0].1.contains("go install"), "{problems:?}");
}
