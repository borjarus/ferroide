use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use ferro_services::{EntryKind, ProjectEvent, ProjectService, SearchService, TextQuery, TodoKind};

use super::*;

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// A small project with a `.gitignore`, a `.git` directory and nested
/// folders.
fn sample() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, ".gitignore", "/target\n*.log\n");
    write(root, ".git/HEAD", "ref: refs/heads/main\n");
    write(root, "Cargo.toml", "[package]\nname = \"sample\"\n");
    write(
        root,
        "src/main.rs",
        "fn main() {\n    // TODO: greet\n    println!(\"hi\");\n}\n",
    );
    write(
        root,
        "src/lib.rs",
        "pub fn add(a: i32, b: i32) -> i32 { a + b } // FIXME overflow\n",
    );
    write(root, "Zeta.md", "# Zeta\n");
    write(root, "alpha.md", "# alpha\n");
    write(
        root,
        "target/debug/out.rs",
        "fn main() {} // TODO: ignored\n",
    );
    write(root, "build.log", "noise\n");
    dir
}

fn quiet(roots: Vec<PathBuf>) -> FsProjectService {
    FsProjectService::open_with(
        roots,
        ProjectOptions {
            watch: false,
            ..Default::default()
        },
    )
    .unwrap()
}

fn names(entries: &[ferro_services::Entry]) -> Vec<&str> {
    entries.iter().map(|e| e.name.as_str()).collect()
}

/// Wait for an event matching `want`, up to `timeout`.
fn wait_for(
    rx: &async_channel::Receiver<ProjectEvent>,
    timeout: Duration,
    want: impl Fn(&ProjectEvent) -> bool,
) -> Option<Duration> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        while let Ok(event) = rx.try_recv() {
            if want(&event) {
                return Some(started.elapsed());
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    None
}

#[test]
fn listing_respects_gitignore_and_sorts_directories_first() {
    let dir = sample();
    let project = quiet(vec![dir.path().into()]);
    let root = project.roots()[0].clone();
    let entries = smol::block_on(project.children(&root)).unwrap();
    assert_eq!(
        names(&entries),
        ["src", ".gitignore", "alpha.md", "Cargo.toml", "Zeta.md"]
    );
    assert_eq!(entries[0].kind, EntryKind::Directory);
    let src = smol::block_on(project.children(&root.join("src"))).unwrap();
    assert_eq!(names(&src), ["lib.rs", "main.rs"]);

    let all = list_dir(&root, true).unwrap();
    assert!(names(&all).contains(&"target"), "{:?}", names(&all));
    assert!(names(&all).contains(&"build.log"));
    assert!(!names(&all).contains(&".git"));
    assert_eq!(project.name(), root.file_name().unwrap().to_string_lossy());
}

#[test]
fn listing_a_missing_directory_fails() {
    let dir = sample();
    let project = quiet(vec![dir.path().into()]);
    let missing = project.roots()[0].join("nope");
    assert!(smol::block_on(project.children(&missing)).is_err());
}

#[test]
fn file_operations_change_the_disk_and_report_events() {
    let dir = sample();
    let project = quiet(vec![dir.path().into()]);
    let root = project.roots()[0].clone();
    let events = project.subscribe();

    let new_file = root.join("src/util/strings.rs");
    smol::block_on(project.create(&new_file, EntryKind::File)).unwrap();
    assert!(new_file.is_file());
    assert_eq!(
        events.try_recv().unwrap(),
        ProjectEvent::Added(new_file.clone())
    );
    assert!(smol::block_on(project.create(&new_file, EntryKind::File)).is_err());

    smol::block_on(project.write_file(&new_file, b"pub fn x() {}\n".to_vec())).unwrap();
    assert_eq!(fs::read_to_string(&new_file).unwrap(), "pub fn x() {}\n");
    assert_eq!(
        events.try_recv().unwrap(),
        ProjectEvent::Changed(new_file.clone())
    );
    assert_eq!(
        smol::block_on(project.read_file(&new_file)).unwrap(),
        "pub fn x() {}\n"
    );

    let moved = root.join("src/text.rs");
    smol::block_on(project.rename(&new_file, &moved)).unwrap();
    assert!(!new_file.exists() && moved.is_file());
    assert_eq!(events.try_recv().unwrap(), ProjectEvent::Removed(new_file));
    assert_eq!(
        events.try_recv().unwrap(),
        ProjectEvent::Added(moved.clone())
    );
    // Renaming onto an existing file or into itself is refused.
    assert!(smol::block_on(project.rename(&moved, &root.join("src/lib.rs"))).is_err());
    let src = root.join("src");
    assert!(smol::block_on(project.rename(&src, &src.join("inner"))).is_err());

    let folder = root.join("docs/guide");
    smol::block_on(project.create(&folder, EntryKind::Directory)).unwrap();
    assert!(folder.is_dir());

    project.delete_permanently(&moved).unwrap();
    assert!(!moved.exists());

    // Nothing outside the project can be touched.
    let outside = tempfile::tempdir().unwrap();
    let stranger = outside.path().join("x.txt");
    assert!(smol::block_on(project.create(&stranger, EntryKind::File)).is_err());
    assert!(!stranger.exists());
    // Roots are removed from the project, never deleted.
    assert!(smol::block_on(project.delete(&root)).is_err());
}

#[test]
fn the_index_skips_ignored_files_and_follows_changes() {
    let dir = sample();
    let project = quiet(vec![dir.path().into()]);
    smol::block_on(project.index().ready());
    let rel: Vec<String> = project
        .index()
        .snapshot()
        .iter()
        .map(|f| f.relative.clone())
        .collect();
    assert_eq!(
        rel,
        [
            ".gitignore",
            "Cargo.toml",
            "Zeta.md",
            "alpha.md",
            "src/lib.rs",
            "src/main.rs"
        ]
    );

    let root = project.roots()[0].clone();
    smol::block_on(project.create(&root.join("src/new.rs"), EntryKind::File)).unwrap();
    smol::block_on(project.create(&root.join("debug.log"), EntryKind::File)).unwrap();
    let rel: Vec<String> = project
        .index()
        .snapshot()
        .iter()
        .map(|f| f.relative.clone())
        .collect();
    assert!(rel.contains(&"src/new.rs".to_string()), "{rel:?}");
    assert!(!rel.contains(&"debug.log".to_string()), "{rel:?}");

    project.delete_permanently(&root.join("src")).unwrap();
    assert!(
        project
            .index()
            .snapshot()
            .iter()
            .all(|f| !f.relative.starts_with("src/"))
    );
}

#[test]
fn search_finds_files_text_and_todos() {
    let dir = sample();
    let project = quiet(vec![dir.path().into()]);
    let search = FsSearchService::new(project.clone());

    let files = smol::block_on(search.find_files("mnrs", 10)).unwrap();
    assert_eq!(files[0].relative, "src/main.rs");

    let hits = smol::block_on(search.find_text(
        TextQuery {
            text: "fn".into(),
            whole_word: true,
            case_sensitive: true,
            ..Default::default()
        },
        100,
    ))
    .unwrap();
    let found: Vec<(&str, u32)> = hits.iter().map(|m| (m.relative.as_str(), m.line)).collect();
    // `target/` is ignored.
    assert_eq!(found, [("src/lib.rs", 0), ("src/main.rs", 0)]);
    assert_eq!(hits[0].ranges.len(), 1);
    assert_eq!(hits[0].ranges[0], 4..6);

    let hits = smol::block_on(search.find_text(
        TextQuery {
            text: r"print\w+!".into(),
            regex: true,
            path_filter: Some("src/".into()),
            ..Default::default()
        },
        100,
    ))
    .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(&hits[0].line_text[hits[0].ranges[0].clone()], "println!");

    let bad = TextQuery {
        text: "(".into(),
        regex: true,
        ..Default::default()
    };
    assert!(smol::block_on(search.find_text(bad, 10)).is_err());

    let todos = smol::block_on(search.todos()).unwrap();
    let todos: Vec<(&str, TodoKind, &str)> = todos
        .iter()
        .map(|t| (t.relative.as_str(), t.kind, t.text.as_str()))
        .collect();
    assert_eq!(
        todos,
        [
            ("src/lib.rs", TodoKind::Fixme, "overflow"),
            ("src/main.rs", TodoKind::Todo, "greet")
        ]
    );
}

#[test]
fn several_roots_live_in_one_project() {
    let first = sample();
    let second = tempfile::tempdir().unwrap();
    write(second.path(), "lib/util.rs", "// TODO: shared\n");
    let project = quiet(vec![first.path().into()]);
    let events = project.subscribe();

    let added = project.add_root(second.path()).unwrap();
    assert_eq!(project.roots().len(), 2);
    assert_eq!(events.try_recv().unwrap(), ProjectEvent::RootsChanged);
    // The same folder, or one inside a root, cannot be added twice.
    assert!(project.add_root(second.path()).is_err());
    assert!(project.add_root(&project.roots()[0].join("src")).is_err());

    let search = FsSearchService::new(project.clone());
    let files = smol::block_on(search.find_files("util", 10)).unwrap();
    let second_name = added.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(files[0].relative, format!("{second_name}/lib/util.rs"));

    project.remove_root(&added).unwrap();
    assert_eq!(project.roots().len(), 1);
    assert!(
        project
            .index()
            .snapshot()
            .iter()
            .all(|f| !f.path.starts_with(&added))
    );
    // The last root stays.
    let only = project.roots()[0].clone();
    assert!(project.remove_root(&only).is_err());
}

#[test]
fn outside_changes_arrive_within_the_budget() {
    let dir = sample();
    let project = FsProjectService::open(dir.path()).unwrap();
    let root = project.roots()[0].clone();
    let events = project.subscribe();
    let started = Instant::now();
    while !project.is_watching() && started.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(project.is_watching());
    // Let the watcher settle (FSEvents reports recent history on start).
    std::thread::sleep(Duration::from_millis(300));
    while events.try_recv().is_ok() {}

    let budget = Duration::from_secs(2);
    let file = root.join("src/outside.rs");
    fs::write(&file, "fn outside() {}\n").unwrap();
    let added = wait_for(
        &events,
        budget,
        |e| matches!(e, ProjectEvent::Added(p) | ProjectEvent::Changed(p) if *p == file),
    )
    .expect("a new file is reported");

    std::thread::sleep(Duration::from_millis(100));
    while events.try_recv().is_ok() {}
    fs::write(&file, "fn outside() { 1; }\n").unwrap();
    let changed = wait_for(
        &events,
        budget,
        |e| matches!(e, ProjectEvent::Changed(p) | ProjectEvent::Added(p) if *p == file),
    )
    .expect("a change is reported");

    fs::remove_file(&file).unwrap();
    let removed = wait_for(
        &events,
        budget,
        |e| matches!(e, ProjectEvent::Removed(p) if *p == file),
    )
    .expect("a removal is reported");

    // Ignored paths stay quiet.
    while events.try_recv().is_ok() {}
    write(&root, "target/debug/more.rs", "x");
    assert!(
        wait_for(&events, Duration::from_millis(400), |e| {
            matches!(e, ProjectEvent::Added(p) | ProjectEvent::Changed(p) if p.starts_with(root.join("target")))
        })
        .is_none()
    );

    eprintln!("watcher latency: added {added:?}, changed {changed:?}, removed {removed:?}");
    // The Stage 4 budget is 200 ms; CI machines get headroom.
    for latency in [added, changed, removed] {
        assert!(latency < Duration::from_millis(1000), "{latency:?}");
    }
    assert!(
        project.index().snapshot().iter().all(|f| f.path != file),
        "the index follows the watcher"
    );
}

/// Write `count` files spread over nested directories.
fn generate(root: &Path, count: usize) {
    for i in 0..count {
        let dir = root.join(format!("pkg{:02}/mod{:03}", i % 50, (i / 50) % 200));
        if i < 50 * 200 {
            fs::create_dir_all(&dir).unwrap();
        }
        fs::write(dir.join(format!("file{i}.rs")), "").unwrap();
    }
}

fn open_large(count: usize) -> (Duration, Duration) {
    let dir = tempfile::tempdir().unwrap();
    generate(dir.path(), count);
    let started = Instant::now();
    let project = FsProjectService::open(dir.path()).unwrap();
    let root = project.roots()[0].clone();
    let first_level = smol::block_on(project.children(&root)).unwrap();
    let first = started.elapsed();
    assert_eq!(first_level.len(), 50);
    smol::block_on(project.index().ready());
    let scanned = started.elapsed();
    assert_eq!(project.index().snapshot().len(), count);
    eprintln!("{count} files: first level after {first:?}, full scan after {scanned:?}");
    (first, scanned)
}

#[test]
fn a_large_project_shows_its_first_level_immediately() {
    let (first, scanned) = open_large(20_000);
    assert!(first < Duration::from_millis(500), "{first:?}");
    assert!(scanned < Duration::from_secs(10), "{scanned:?}");
}

/// The Stage 4 Definition of Done: a 100k-file project opens in < 1 s.
/// Writing 100k files takes a while, so it runs on request:
/// `cargo test -p ferro_project --release -- --ignored hundred_thousand`.
#[test]
#[ignore]
fn hundred_thousand_files_open_within_a_second() {
    let (first, scanned) = open_large(100_000);
    assert!(first < Duration::from_millis(100), "{first:?}");
    assert!(scanned < Duration::from_secs(1), "{scanned:?}");
}

/// Moves a file to the system trash. Needs a desktop session (Finder on
/// macOS), so it runs on request.
#[test]
#[ignore]
fn delete_moves_to_the_trash() {
    let dir = sample();
    let project = quiet(vec![dir.path().into()]);
    let file = project.roots()[0].join("alpha.md");
    smol::block_on(project.delete(&file)).unwrap();
    assert!(!file.exists());
}
