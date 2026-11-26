//! Headless tests of the project index on real folders.

use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

use ferro_languages::LanguageRegistry;
use ferro_project::FsProjectService;
use ferro_services::{SearchScope, SearchService as _, SymbolKind, TextQuery, TodoKind};

use crate::{IndexOptions, ProjectIndex, TrigramMode};

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn sample(root: &Path) {
    write(
        root,
        "src/main.rs",
        "// TODO: split main\nfn main() {\n    greet();\n}\n\nstruct Config;\n",
    );
    write(
        root,
        "src/lib.rs",
        "pub fn greet() {}\n// FIXME: needle in a lib\n",
    );
    write(root, "docs/notes.md", "# Notes\n\nA needle here too.\n");
    write(root, "target/debug/out.rs", "fn generated() {}\n");
    write(root, ".gitignore", "target/\n");
}

fn open(root: &Path, options: IndexOptions) -> (FsProjectService, ProjectIndex) {
    let project = FsProjectService::open(root).unwrap();
    let index =
        ProjectIndex::open(project.clone(), LanguageRegistry::shared_builtin(), options).unwrap();
    (project, index)
}

fn until(what: &str, mut check: impl FnMut() -> bool) {
    let started = Instant::now();
    while !check() {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn query(text: &str) -> TextQuery {
    TextQuery {
        text: text.into(),
        ..TextQuery::default()
    }
}

#[test]
fn indexes_symbols_and_todos_and_skips_ignored_files() {
    let dir = tempfile::tempdir().unwrap();
    sample(dir.path());
    let (_project, index) = open(dir.path(), IndexOptions::default());
    smol::block_on(index.ready());

    let symbols = index.search_symbols("greet", 10);
    assert_eq!(symbols[0].name, "greet");
    assert_eq!(symbols[0].kind, SymbolKind::Function);
    assert!(index.search_symbols("generated", 10).is_empty(), "ignored");
    assert!(
        index
            .search_symbols("Config", 5)
            .iter()
            .any(|s| s.name == "Config")
    );

    let todos = smol::block_on(index.todos()).unwrap();
    let found: Vec<(&str, TodoKind, &str)> = todos
        .iter()
        .map(|t| (t.relative.as_str(), t.kind, t.text.as_str()))
        .collect();
    assert_eq!(
        found,
        vec![
            ("src/lib.rs", TodoKind::Fixme, "needle in a lib"),
            ("src/main.rs", TodoKind::Todo, "split main"),
        ]
    );
}

#[test]
fn the_index_survives_a_restart_and_catches_up_with_the_disk() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    sample(dir.path());
    let options = IndexOptions {
        db_path: Some(crate::db_path_for(data.path(), &[dir.path().to_path_buf()])),
        ..IndexOptions::default()
    };
    {
        let (_project, index) = open(dir.path(), options.clone());
        smol::block_on(index.ready());
        assert!(index.indexed_files() >= 4);
    }
    // Changes while the IDE was closed.
    fs::remove_file(dir.path().join("docs/notes.md")).unwrap();
    write(dir.path(), "src/extra.rs", "fn added_later() {}\n");

    let (_project, index) = open(dir.path(), options);
    // Answered from the database before the comparison with the disk.
    assert_eq!(index.search_symbols("greet", 5)[0].name, "greet");
    smol::block_on(index.ready());
    assert_eq!(
        index.search_symbols("added_later", 5)[0].name,
        "added_later"
    );
    let files = index.match_files("notes", 10);
    assert!(files.iter().all(|f| f.relative != "docs/notes.md"));
}

#[test]
fn changes_on_disk_are_indexed_again() {
    let dir = tempfile::tempdir().unwrap();
    sample(dir.path());
    let (_project, index) = open(dir.path(), IndexOptions::default());
    smol::block_on(index.ready());

    write(
        dir.path(),
        "src/lib.rs",
        "pub fn renamed_greet() {}\n// TODO: new\n",
    );
    until("the edit to be indexed", || {
        !index.search_symbols("renamed_greet", 5).is_empty()
    });
    let todos = smol::block_on(index.todos()).unwrap();
    assert!(todos.iter().any(|t| t.text == "new"));
    assert!(!todos.iter().any(|t| t.text == "needle in a lib"));

    fs::remove_file(dir.path().join("src/main.rs")).unwrap();
    until("the removal to be indexed", || {
        index
            .search_symbols("main", 5)
            .iter()
            .all(|s| s.name != "main")
    });
}

#[test]
fn quick_open_matches_files_with_nucleo() {
    let dir = tempfile::tempdir().unwrap();
    sample(dir.path());
    let (_project, index) = open(dir.path(), IndexOptions::default());
    let files = smol::block_on(index.find_files("mainrs", 10)).unwrap();
    assert_eq!(files[0].relative, "src/main.rs");
    assert_eq!(files[0].positions.len(), 6);
    // Narrowing the previous query gives the same answer as a fresh one.
    let narrowed = smol::block_on(index.find_files("lib", 10)).unwrap();
    assert_eq!(narrowed[0].relative, "src/lib.rs");
    assert!(
        smol::block_on(index.find_files("out", 10))
            .unwrap()
            .iter()
            .all(|f| !f.relative.starts_with("target")),
        "ignored files are not offered"
    );
}

#[test]
fn find_in_files_honors_scope_and_masks() {
    let dir = tempfile::tempdir().unwrap();
    sample(dir.path());
    let (project, index) = open(dir.path(), IndexOptions::default());
    let root = ferro_services::ProjectService::roots(&project)[0].clone();
    let find = |q: TextQuery| smol::block_on(index.find_text(q, 100)).unwrap();

    let all = find(query("needle"));
    let files: Vec<&str> = all.iter().map(|m| m.relative.as_str()).collect();
    assert_eq!(files, vec!["docs/notes.md", "src/lib.rs"]);

    let masked = find(TextQuery {
        path_filter: Some("*.rs".into()),
        ..query("needle")
    });
    assert_eq!(masked.len(), 1);
    let excluded = find(TextQuery {
        path_filter: Some("!docs/**".into()),
        ..query("needle")
    });
    assert_eq!(excluded.len(), 1);
    assert_eq!(excluded[0].relative, "src/lib.rs");

    let in_docs = find(TextQuery {
        scope: SearchScope::Directory(root.join("docs")),
        ..query("needle")
    });
    assert_eq!(in_docs.len(), 1);
    assert_eq!(in_docs[0].relative, "docs/notes.md");
    let in_files = find(TextQuery {
        scope: SearchScope::Files(vec![root.join("src").join("lib.rs")]),
        ..query("needle")
    });
    assert_eq!(in_files.len(), 1);

    let regex = find(TextQuery {
        text: r"fn \w+\(\)".into(),
        regex: true,
        ..TextQuery::default()
    });
    assert_eq!(regex.len(), 2);
    assert!(
        smol::block_on(index.find_text(
            TextQuery {
                text: "(".into(),
                regex: true,
                ..TextQuery::default()
            },
            10
        ))
        .is_err()
    );
}

#[test]
fn the_trigram_index_narrows_literal_searches() {
    let dir = tempfile::tempdir().unwrap();
    sample(dir.path());
    for i in 0..30 {
        write(
            dir.path(),
            &format!("noise/file{i}.txt"),
            "nothing to see\n",
        );
    }
    let (_project, index) = open(
        dir.path(),
        IndexOptions {
            trigrams: TrigramMode::On,
            ..IndexOptions::default()
        },
    );
    smol::block_on(index.ready());
    assert!(index.uses_trigrams());
    let candidates = index.text_candidates(&query("Needle")).unwrap();
    let mut shown: Vec<&str> = candidates.iter().map(|c| c.shown.as_str()).collect();
    shown.sort();
    assert_eq!(shown, vec!["docs/notes.md", "src/lib.rs"]);
    // Regular expressions cannot be narrowed: every file is a candidate.
    let all = index
        .text_candidates(&TextQuery {
            text: "need.e".into(),
            regex: true,
            ..TextQuery::default()
        })
        .unwrap();
    assert!(all.len() > 30);
    // A new file is found before it is indexed and after.
    write(dir.path(), "noise/late.txt", "a late needle\n");
    until("the new file to be searched", || {
        smol::block_on(index.find_text(query("needle"), 100))
            .unwrap()
            .iter()
            .any(|m| m.relative == "noise/late.txt")
    });

    let (_project, auto) = open(dir.path(), IndexOptions::default());
    smol::block_on(auto.ready());
    assert!(!auto.uses_trigrams(), "small projects do without");
}

#[test]
fn typing_pauses_indexing() {
    let dir = tempfile::tempdir().unwrap();
    sample(dir.path());
    let (_project, index) = open(
        dir.path(),
        IndexOptions {
            typing_pause: Duration::from_millis(300),
            ..IndexOptions::default()
        },
    );
    smol::block_on(index.ready());
    index.user_activity();
    assert!(index.is_paused());
    write(dir.path(), "src/typed.rs", "fn while_typing() {}\n");
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        index.search_symbols("while_typing", 5).is_empty(),
        "nothing is indexed during the pause"
    );
    until("indexing to resume", || {
        !index.search_symbols("while_typing", 5).is_empty()
    });
}
