//! The tree-sitter language service: what projects on disk get before
//! language servers.

use std::{path::PathBuf, sync::Arc};

use ferro_languages::{LanguageRegistry, SyntaxLanguageService};
use ferro_services::{LanguageService, RunnableKind, Severity, SymbolKind, TextPos};

fn service(files: Vec<PathBuf>) -> SyntaxLanguageService {
    SyntaxLanguageService::new(
        LanguageRegistry::shared_builtin(),
        Arc::new(move || files.clone()),
    )
}

#[test]
fn syntax_errors_become_diagnostics() {
    let service = service(Vec::new());
    let path = PathBuf::from("src/lib.rs");
    let broken = "fn main() {\n    let x = ;\n}\n";
    let diagnostics = smol::block_on(service.diagnostics(&path, broken)).unwrap();
    assert!(!diagnostics.is_empty());
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert_eq!(diagnostics[0].range.start.line, 1);
    assert_eq!(diagnostics[0].source, "tree-sitter");
    let all = smol::block_on(service.project_diagnostics()).unwrap();
    assert_eq!(all.len(), diagnostics.len());

    // Fixing the file clears them, also in the Problems list.
    let fixed = "fn main() {\n    let x = 1;\n}\n";
    assert!(
        smol::block_on(service.diagnostics(&path, fixed))
            .unwrap()
            .is_empty()
    );
    assert!(
        smol::block_on(service.project_diagnostics())
            .unwrap()
            .is_empty()
    );

    // SQL dialects are not reported.
    let sql = PathBuf::from("q.sql");
    assert!(
        smol::block_on(service.diagnostics(&sql, "SELEC broken (("))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn structure_and_runnables() {
    let service = service(Vec::new());
    let path = PathBuf::from("app.py");
    let text =
        "class Greeter:\n    def hello(self):\n        pass\n\ndef test_hello():\n    pass\n";
    let symbols = smol::block_on(service.document_symbols(&path, text)).unwrap();
    assert_eq!(symbols[0].name, "Greeter");
    assert_eq!(symbols[0].kind, SymbolKind::Struct);
    assert_eq!(symbols[0].children[0].name, "hello");
    assert_eq!(symbols[0].children[0].selection, TextPos::new(1, 8));
    assert_eq!(symbols[0].children[0].container.as_deref(), Some("Greeter"));
    let runnables = service.runnables(&path, text);
    assert_eq!(runnables.len(), 1);
    assert_eq!(
        (runnables[0].line, runnables[0].kind),
        (4, RunnableKind::Test)
    );
}

#[test]
fn completes_keywords_and_words() {
    let service = service(Vec::new());
    let path = PathBuf::from("main.rs");
    let text = "fn compute_total() {}\nfn main() {\n    let counter = 1;\n    co\n}\n";
    let items = smol::block_on(service.completions(&path, text, TextPos::new(3, 6))).unwrap();
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"const"), "{labels:?}");
    assert!(labels.contains(&"continue"), "{labels:?}");
    assert!(labels.contains(&"counter"), "{labels:?}");
    let compute = items.iter().find(|i| i.label == "compute_total").unwrap();
    assert_eq!(compute.kind, SymbolKind::Function);
    let none = smol::block_on(service.completions(&path, text, TextPos::new(3, 4))).unwrap();
    assert!(none.is_empty(), "no prefix, no list");
}

#[test]
fn workspace_symbols_come_from_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("shapes.rs");
    let b = dir.path().join("util.py");
    let c = dir.path().join("notes.txt");
    std::fs::write(
        &a,
        "pub struct Circle;\nimpl Circle {\n    pub fn area(&self) -> f64 { 0.0 }\n}\n",
    )
    .unwrap();
    std::fs::write(&b, "def circle_area(r):\n    return r\n").unwrap();
    std::fs::write(&c, "Circle").unwrap();
    let service = service(vec![a.clone(), b.clone(), c]);
    let found = smol::block_on(service.workspace_symbols("circ")).unwrap();
    let names: Vec<&str> = found.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"Circle"), "{names:?}");
    assert!(names.contains(&"circle_area"), "{names:?}");
    let area = smol::block_on(service.workspace_symbols("area")).unwrap();
    let method = area.iter().find(|s| s.name == "area").unwrap();
    assert_eq!(method.path, a);
    assert_eq!(method.container.as_deref(), Some("Circle"));

    // A changed file is indexed again.
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&b, "def square_area(r):\n    return r\n").unwrap();
    let file = std::fs::File::options().append(true).open(&b).unwrap();
    file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
        .unwrap();
    let found = smol::block_on(service.workspace_symbols("square")).unwrap();
    assert!(found.iter().any(|s| s.name == "square_area"));
    assert!(
        !smol::block_on(service.workspace_symbols("circle_area"))
            .unwrap()
            .iter()
            .any(|s| s.name == "circle_area")
    );
}
