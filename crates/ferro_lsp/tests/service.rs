//! `LspLanguageService` against the in-process fake server.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use ferro_languages::{LanguageRegistry, SyntaxLanguageService};
use ferro_lsp::testing::{FakeLauncher, FakeState};
use ferro_lsp::{LspLanguageService, LspOptions, ServerSettings};
use ferro_services::{
    GotoKind, LanguageEvent, LanguageService, ProjectEvent, ServerHealth, Severity, SymbolKind,
    TextPos, TextRange,
};
use futures::FutureExt as _;
use serde_json::json;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    state: Arc<FakeState>,
    service: LspLanguageService,
}

fn fixture_with(options: LspOptions, trusted: bool, crates: &[&str]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    // `canonicalize` on Windows adds `\\?\`; the editor uses plain paths.
    let root = PathBuf::from(root.to_string_lossy().trim_start_matches(r"\\?\"));
    for name in crates {
        let krate = if name.is_empty() {
            root.clone()
        } else {
            root.join(name)
        };
        std::fs::create_dir_all(krate.join("src")).unwrap();
        std::fs::write(krate.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
    }
    let registry = LanguageRegistry::shared_builtin();
    let files_root = root.clone();
    let fallback =
        SyntaxLanguageService::new(registry.clone(), Arc::new(move || walk(&files_root)));
    let state = Arc::new(FakeState::default());
    let roots_root = root.clone();
    let service = LspLanguageService::new(
        registry,
        fallback,
        Arc::new(move || vec![roots_root.clone()]),
        Arc::new(FakeLauncher {
            state: state.clone(),
        }),
        options,
        trusted,
    );
    Fixture {
        _dir: dir,
        root,
        state,
        service,
    }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

fn fixture() -> Fixture {
    let mut options = LspOptions::default();
    options.servers.insert(
        "rust-analyzer".into(),
        ServerSettings {
            settings: Some(json!({"fake": {"answer": 42}})),
            ..Default::default()
        },
    );
    fixture_with(options, true, &[""])
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

const SOURCE: &str = "fn main() {\n    let x = greet(\"😀\"); bad\n}\nfn greet(name: &str) {}\n";

#[test]
fn requests_reach_the_server_with_synchronized_text() {
    let f = fixture();
    let path = f.root.join("src").join("main.rs");
    let s = &f.service;

    let hover = smol::block_on(s.hover(&path, SOURCE, TextPos::new(1, 13)))
        .unwrap()
        .unwrap();
    assert_eq!(hover.contents, "**greet**");
    assert_eq!(f.state.text_of("main.rs").as_deref(), Some(SOURCE));

    // Edits after the emoji: the server's copy follows incrementally.
    let edited = SOURCE
        .replace("bad", "very bad")
        .replace("fn greet", "fn greet_all");
    let hover = smol::block_on(s.hover(&path, &edited, TextPos::new(3, 4)))
        .unwrap()
        .unwrap();
    assert_eq!(hover.contents, "**greet_all**");
    assert_eq!(f.state.text_of("main.rs").as_deref(), Some(edited.as_str()));
    assert_eq!(f.state.changes.load(Ordering::SeqCst), 1, "one didChange");

    let defs =
        smol::block_on(s.goto(&path, SOURCE, TextPos::new(1, 14), GotoKind::Definition)).unwrap();
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].range.start, TextPos::new(3, 3));
    assert_eq!(defs[0].line_text, "fn greet(name: &str) {}");

    let refs = smol::block_on(s.references(&path, SOURCE, TextPos::new(3, 4), true)).unwrap();
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0].range, TextRange::on_line(1, 12, 17));

    let rename = smol::block_on(s.rename(&path, SOURCE, TextPos::new(3, 4), "welcome")).unwrap();
    assert_eq!(rename.edit_count(), 2);
    assert_eq!(
        rename.files[0].edits[0].line_after,
        "    let x = welcome(\"😀\"); bad"
    );

    let symbols = smol::block_on(s.document_symbols(&path, SOURCE)).unwrap();
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["main", "greet"]);
    assert_eq!(symbols[0].kind, SymbolKind::Function);

    let found = smol::block_on(s.workspace_symbols("gre")).unwrap();
    assert!(
        found
            .iter()
            .any(|s| s.name == "greet" && s.path.ends_with("main.rs"))
    );

    let hints = smol::block_on(s.inlay_hints(&path, SOURCE)).unwrap();
    assert_eq!(hints[0].label, ": i32");

    let tokens = smol::block_on(s.semantic_tokens(&path, SOURCE))
        .unwrap()
        .unwrap();
    assert_eq!(tokens.len(), 2);
    assert_eq!(
        (tokens[1].line, tokens[1].start, tokens[1].kind.as_str()),
        (3, 0, "keyword")
    );

    let help = smol::block_on(s.signature_help(&path, SOURCE, TextPos::new(1, 18)))
        .unwrap()
        .unwrap();
    assert_eq!(&help.label[help.parameters[0].clone()], "name: &str");

    let messy = "fn a() {   \n}  \n";
    let edits = smol::block_on(s.format(&path, messy, None)).unwrap();
    assert_eq!(edits[0].new_text, "fn a() {\n}\n");
}

#[test]
fn pushed_diagnostics_arrive_as_events_in_char_columns() {
    let f = fixture();
    let path = f.root.join("src").join("main.rs");
    let events = f.service.subscribe().unwrap();
    smol::block_on(f.service.diagnostics(&path, SOURCE)).unwrap();
    let event = smol::block_on(async {
        loop {
            match events.recv().await.unwrap() {
                LanguageEvent::DiagnosticsChanged(p) => break p,
                _ => continue,
            }
        }
    });
    assert_eq!(event, path, "the editor's spelling of the path");
    let diagnostics = smol::block_on(f.service.diagnostics(&path, SOURCE)).unwrap();
    assert_eq!(diagnostics.len(), 1);
    // `bad` sits after an emoji (two UTF-16 units, one char).
    assert_eq!(diagnostics[0].range, TextRange::on_line(1, 24, 27));
    assert_eq!(diagnostics[0].severity, Severity::Warning);
    assert_eq!(diagnostics[0].source, "fake");
    let project = smol::block_on(f.service.project_diagnostics()).unwrap();
    assert!(
        project
            .iter()
            .any(|d| d.message == "bad word" && d.path == path)
    );

    // Fixed text: the server publishes an empty set.
    let fixed = SOURCE.replace("bad", "fine");
    smol::block_on(f.service.diagnostics(&path, &fixed)).unwrap();
    wait_until("diagnostics to clear", || {
        smol::block_on(f.service.diagnostics(&path, &fixed))
            .unwrap()
            .is_empty()
    });
}

#[test]
fn completion_resolves_auto_imports_snippets_and_templates() {
    let f = fixture();
    let path = f.root.join("src").join("main.rs");
    let text = "fn main() {\n    let m = Has\n}\n";
    let items = smol::block_on(f.service.completions(&path, text, TextPos::new(1, 15))).unwrap();
    let map = items.iter().find(|i| i.label == "HashMap").unwrap();
    assert_eq!(map.documentation.as_deref(), Some("A hash map."));
    // The import at the top merged with the insertion into one edit.
    assert_eq!(map.replace.unwrap().start, TextPos::new(0, 0));
    assert!(
        map.insert_text
            .starts_with("use std::collections::HashMap;\nfn main()")
    );
    let hello = items.iter().find(|i| i.label == "hello_world").unwrap();
    assert_eq!(hello.insert_text, "hello_world()");

    let text = "fn main() {\n    x.ok\n}\n";
    let items = smol::block_on(f.service.completions(&path, text, TextPos::new(1, 8))).unwrap();
    let ok = items
        .iter()
        .find(|i| i.label == ".ok")
        .expect("postfix template");
    assert_eq!(ok.insert_text, "Ok(x)");
}

#[test]
fn quick_fixes_resolve_lazily_and_commands_apply_edits() {
    let f = fixture();
    let path = f.root.join("src").join("main.rs");
    let fixes = smol::block_on(f.service.code_actions(
        &path,
        SOURCE,
        TextRange::on_line(1, 24, 27),
    ))
    .unwrap();
    assert_eq!(fixes.len(), 2);
    assert!(fixes[0].preferred);
    assert!(fixes[0].edits.is_empty(), "computed on resolve");
    let edit = smol::block_on(f.service.resolve_quick_fix(&path, SOURCE, &fixes[0])).unwrap();
    assert_eq!(edit.files.len(), 1);
    assert_eq!(edit.files[0].edits[0].edit.new_text, "good");
    assert_eq!(
        edit.files[0].edits[0].edit.range,
        TextRange::on_line(1, 24, 27)
    );

    let edit = smol::block_on(f.service.resolve_quick_fix(&path, SOURCE, &fixes[1])).unwrap();
    assert_eq!(edit.files[0].edits[0].edit.new_text, "// fixed\n");
}

#[test]
fn configuration_watchers_save_and_close_reach_the_server() {
    let f = fixture();
    let path = f.root.join("src").join("main.rs");
    smol::block_on(f.service.hover(&path, SOURCE, TextPos::new(0, 4))).unwrap();
    wait_until("workspace/configuration", || {
        f.state.configuration.lock().unwrap().is_some()
    });
    assert_eq!(
        f.state.configuration.lock().unwrap().clone().unwrap(),
        json!([{"answer": 42}])
    );

    f.service.files_changed(&[
        ProjectEvent::Changed(f.root.join("Cargo.toml")),
        ProjectEvent::Changed(f.root.join("README.md")),
    ]);
    wait_until("watched files", || {
        !f.state.watched.lock().unwrap().is_empty()
    });
    std::thread::sleep(Duration::from_millis(50));
    let watched = f.state.watched.lock().unwrap().clone();
    assert_eq!(watched.len(), 1, "only *.toml is watched: {watched:?}");
    assert_eq!(watched[0]["type"], 2);

    f.service.document_saved(&path, SOURCE);
    f.service.document_closed(&path);
    wait_until("didClose", || !f.state.closed.lock().unwrap().is_empty());
    assert_eq!(f.state.saved.lock().unwrap().len(), 1);
    assert!(f.state.text_of("main.rs").is_none());
}

#[test]
fn a_crashed_server_restarts_then_gives_up() {
    let f = fixture();
    let path = f.root.join("src").join("main.rs");
    smol::block_on(f.service.hover(&path, SOURCE, TextPos::new(0, 4))).unwrap();
    assert_eq!(f.state.launches.load(Ordering::SeqCst), 1);
    assert_eq!(f.service.servers()[0].health, ServerHealth::Ready);
    for round in 1..=4 {
        let crash = format!("{SOURCE}// CRASH {round}\n");
        smol::block_on(f.service.hover(&path, &crash, TextPos::new(0, 4))).unwrap();
        wait_until("the crash", || {
            f.service.servers()[0].health == ServerHealth::Crashed
        });
        // The next request restarts it (three times in three minutes).
        let hover = smol::block_on(f.service.hover(&path, SOURCE, TextPos::new(0, 4))).unwrap();
        if round <= 3 {
            assert_eq!(hover.unwrap().contents, "**main**", "round {round}");
            assert_eq!(f.state.launches.load(Ordering::SeqCst), round + 1);
        } else {
            assert!(hover.is_none(), "given up after three restarts");
            assert_eq!(f.state.launches.load(Ordering::SeqCst), 4);
        }
    }
    // The tree-sitter fallback still answers.
    let symbols = smol::block_on(f.service.document_symbols(&path, SOURCE)).unwrap();
    assert!(symbols.iter().any(|s| s.name == "greet"));
}

#[test]
fn nothing_starts_in_an_untrusted_project() {
    let f = fixture_with(LspOptions::default(), false, &[""]);
    let path = f.root.join("src").join("main.rs");
    let diagnostics = smol::block_on(f.service.diagnostics(&path, "fn main( {}")).unwrap();
    assert_eq!(f.state.launches.load(Ordering::SeqCst), 0);
    assert!(
        diagnostics.iter().all(|d| d.source == "tree-sitter"),
        "{diagnostics:?}"
    );
    assert!(!diagnostics.is_empty(), "syntax errors from the fallback");

    f.service.set_trusted(true);
    smol::block_on(f.service.hover(&path, SOURCE, TextPos::new(0, 4))).unwrap();
    assert_eq!(f.state.launches.load(Ordering::SeqCst), 1);
    f.service.set_trusted(false);
    assert!(
        f.service.servers().is_empty(),
        "stopped when trust is withdrawn"
    );
}

#[test]
fn the_governor_limits_running_servers_and_disabled_servers_never_start() {
    let options = LspOptions {
        max_servers: 1,
        ..Default::default()
    };
    // A project with two crates: each gets its own server root.
    let f = fixture_with(options, true, &["a", "b"]);
    let a = f.root.join("a").join("src").join("lib.rs");
    let b = f.root.join("b").join("src").join("lib.rs");
    smol::block_on(f.service.hover(&a, SOURCE, TextPos::new(0, 4))).unwrap();
    smol::block_on(f.service.hover(&b, SOURCE, TextPos::new(0, 4))).unwrap();
    assert_eq!(f.state.launches.load(Ordering::SeqCst), 2);
    let roots = f.state.roots.lock().unwrap().clone();
    assert!(
        roots[0].ends_with('a') && roots[1].ends_with('b'),
        "{roots:?}"
    );
    assert_eq!(
        f.service.servers().len(),
        1,
        "the least recently used one stopped"
    );

    let mut options = LspOptions::default();
    options.servers.insert(
        "rust-analyzer".into(),
        ServerSettings {
            enabled: Some(false),
            ..Default::default()
        },
    );
    let f = fixture_with(options, true, &[""]);
    let path = f.root.join("src").join("main.rs");
    smol::block_on(f.service.hover(&path, SOURCE, TextPos::new(0, 4))).unwrap();
    assert_eq!(f.state.launches.load(Ordering::SeqCst), 0);
}

#[test]
fn an_abandoned_request_is_cancelled_on_the_server() {
    let f = fixture();
    let path = f.root.join("src").join("main.rs");
    let text = "fn slow() {}\n";
    smol::block_on(f.service.document_symbols(&path, text)).unwrap();
    let hover = f.service.hover(&path, text, TextPos::new(0, 4));
    let outcome = smol::block_on(async {
        futures::select! {
            _ = hover.fuse() => "answered",
            _ = smol::Timer::after(Duration::from_millis(300)).fuse() => "gave up",
        }
    });
    assert_eq!(outcome, "gave up");
    wait_until("$/cancelRequest", || {
        !f.state.cancelled.lock().unwrap().is_empty()
    });
}

#[test]
fn power_save_skips_cosmetic_requests() {
    let f = fixture();
    let path = f.root.join("src").join("main.rs");
    f.service.set_options(LspOptions {
        power_save: true,
        ..f.service.options()
    });
    assert!(
        smol::block_on(f.service.inlay_hints(&path, SOURCE))
            .unwrap()
            .is_empty()
    );
    assert!(
        smol::block_on(f.service.semantic_tokens(&path, SOURCE))
            .unwrap()
            .is_none()
    );
    assert!(
        smol::block_on(f.service.code_lenses(&path, SOURCE))
            .unwrap()
            .is_empty()
    );
}
