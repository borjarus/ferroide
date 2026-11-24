use std::path::Path;

use ferro_syntax::QueryKind;

use crate::{LanguageManifest, LanguageRegistry};

#[test]
fn every_builtin_pack_loads_and_compiles() {
    let registry = LanguageRegistry::builtin();
    let ids: Vec<&str> = registry.packs().map(|p| p.id()).collect();
    assert!(
        registry.visible_packs().count() >= 15,
        "plan: 15+ languages, got {ids:?}"
    );
    for pack in registry.packs() {
        let syntax = pack
            .syntax()
            .unwrap_or_else(|err| panic!("{}: {err}", pack.id()));
        assert!(
            syntax.has_query(QueryKind::Highlights),
            "{} has no highlights",
            pack.id()
        );
        let manifest = pack.manifest();
        for pattern in [
            &manifest.increase_indent_pattern,
            &manifest.decrease_indent_pattern,
        ]
        .into_iter()
        .flatten()
        {
            regex::Regex::new(pattern)
                .unwrap_or_else(|err| panic!("{}: bad indent pattern: {err}", pack.id()));
        }
        // Query files a pack lists must exist (in the pack or the grammar).
        for file in &manifest.grammar.as_ref().unwrap().queries {
            let kind = QueryKind::from_file_name(file)
                .unwrap_or_else(|| panic!("{}: unknown query file {file}", pack.id()));
            assert!(
                !pack.query_source(kind).trim().is_empty(),
                "{}: {file} is listed but empty",
                pack.id()
            );
        }
    }
}

#[test]
fn grammars_are_built_on_first_use() {
    let registry = LanguageRegistry::builtin();
    assert_eq!(registry.loaded_count(), 0);
    let python = registry.get("python").unwrap();
    assert!(!python.is_loaded());
    python.syntax().unwrap();
    python.syntax().unwrap();
    assert!(python.is_loaded());
    assert_eq!(registry.loaded_count(), 1);
    assert!(!registry.get("rust").unwrap().is_loaded());
}

#[test]
fn detection_by_name_extension_shebang_and_modeline() {
    let registry = LanguageRegistry::builtin();
    let id = |path: &str, head: Option<&str>| {
        registry
            .detect(Path::new(path), head)
            .map(|p| p.id().to_string())
    };
    assert_eq!(id("src/main.rs", None).as_deref(), Some("rust"));
    assert_eq!(id("App.TSX", None).as_deref(), Some("tsx"));
    assert_eq!(id("types.d.ts", None).as_deref(), Some("typescript"));
    assert_eq!(id("Gemfile", None).as_deref(), Some("ruby"));
    assert_eq!(id("Cargo.lock", None).as_deref(), Some("toml"));
    assert_eq!(id(".bashrc", None).as_deref(), Some("bash"));
    assert_eq!(id("notes.MD", None).as_deref(), Some("markdown"));
    assert_eq!(
        id("bin/tool", Some("#!/usr/bin/env python3.12\nprint(1)\n")).as_deref(),
        Some("python")
    );
    assert_eq!(id("deploy", Some("#!/bin/bash\n")).as_deref(), Some("bash"));
    assert_eq!(
        id("page", Some("<!DOCTYPE html>\n<html>")).as_deref(),
        Some("html")
    );
    // A modeline wins over the extension.
    assert_eq!(
        id("config.txt", Some("# vim: set ft=yaml :\nkey: value\n")).as_deref(),
        Some("yaml")
    );
    assert_eq!(
        id("script.txt", Some("#!/bin/sh\n# -*- mode: ruby -*-\n")).as_deref(),
        Some("ruby")
    );
    assert_eq!(id("unknown.xyz", Some("plain text")), None);
    // Hidden helper grammars are never picked for files.
    assert_eq!(id("x", Some("# vim: ft=markdown_inline")), None);
}

#[test]
fn aliases_resolve() {
    let registry = LanguageRegistry::builtin();
    for (alias, id) in [
        ("js", "javascript"),
        ("ts", "typescript"),
        ("py", "python"),
        ("C#", "csharp"),
        ("c++", "cpp"),
        ("sh", "bash"),
        ("yml", "yaml"),
        ("postgres", "sql"),
        ("md", "markdown"),
    ] {
        assert_eq!(registry.get(alias).map(|p| p.id()), Some(id), "{alias}");
    }
}

#[test]
fn packs_from_a_folder_replace_builtin_ones() {
    let dir = tempfile::tempdir().unwrap();
    let pack = dir.path().join("python");
    std::fs::create_dir(&pack).unwrap();
    std::fs::write(
        pack.join("language.toml"),
        r##"
id = "python"
name = "Python (custom)"
file_extensions = ["py", "pyx"]
line_comment = "#"

[grammar]
name = "python"
queries = ["highlights.scm", "folds.scm"]
"##,
    )
    .unwrap();
    std::fs::write(pack.join("folds.scm"), "(function_definition) @fold\n").unwrap();
    let broken = dir.path().join("broken");
    std::fs::create_dir(&broken).unwrap();
    std::fs::write(broken.join("language.toml"), "id = 1").unwrap();
    let wasm = dir.path().join("zig");
    std::fs::create_dir(&wasm).unwrap();
    std::fs::write(
        wasm.join("language.toml"),
        "id = \"zig\"\nname = \"Zig\"\nfile_extensions = [\"zig\"]\n[grammar]\nname = \"zig\"\nsource = { wasm = \"zig.wasm\" }\n",
    )
    .unwrap();

    let mut registry = LanguageRegistry::builtin();
    let errors = registry.load_dir(dir.path());
    assert_eq!(errors.len(), 1, "{errors:?}");
    let python = registry.detect_path(Path::new("x.pyx")).unwrap();
    assert_eq!(python.name(), "Python (custom)");
    assert!(
        python
            .query_source(QueryKind::Folds)
            .contains("function_definition")
    );
    // Bundled highlights are still used; tags were not listed.
    assert!(!python.query_source(QueryKind::Highlights).is_empty());
    assert!(python.query_source(QueryKind::Tags).is_empty());
    python.syntax().unwrap();
    let zig = registry.detect_path(Path::new("main.zig")).unwrap();
    assert!(zig.syntax().unwrap_err().contains("WebAssembly"));
}

#[test]
fn appendix_b_manifest_parses() {
    let manifest = LanguageManifest::parse(
        r##"
id = "python"
name = "Python"
version = "1.0.0"
file_extensions = ["py", "pyi", "pyw"]
shebangs = ["python", "python3"]
project_markers = ["pyproject.toml", "setup.py", "requirements.txt"]
line_comment = "#"
indent = { unit = "    ", tab_size = 4 }

[grammar]
name = "python"
source = { git = "https://github.com/tree-sitter/tree-sitter-python", rev = "abc" }
queries = ["highlights.scm", "injections.scm", "outline.scm", "folds.scm",
           "indents.scm", "runnables.scm", "tags.scm"]

[[language_servers]]
id = "basedpyright"
command = "basedpyright-langserver"
args = ["--stdio"]
install = { pypi = "basedpyright" }
features = ["completion", "hover"]

[debugger]
adapter = "debugpy"
install = { pypi = "debugpy" }

[[run_templates]]
name = "Python: current file"
command = "${interpreter}"
args = ["${file}"]

[[test_frameworks]]
name = "pytest"
detect = ["pytest.ini", "conftest.py"]
"##,
    )
    .unwrap();
    assert_eq!(manifest.language_servers[0].install["pypi"], "basedpyright");
    assert_eq!(manifest.brackets.len(), 3);
    assert_eq!(manifest.auto_pairs().len(), 5);
}
