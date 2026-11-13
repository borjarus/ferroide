use std::{fs, path::PathBuf, sync::Arc};

use ferro_services::RecentProjectsService as _;

use super::*;

const P: &str = "/work/app";

fn p(path: &str) -> PathBuf {
    PathBuf::from(path)
}

#[test]
fn a_new_database_is_migrated_to_the_latest_schema() {
    let db = StateDb::open_in_memory().unwrap();
    assert_eq!(db.schema_version().unwrap(), MIGRATIONS.len());
}

#[test]
fn state_survives_reopening_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("nested/state.db");
    {
        let db = StateDb::open(&file).unwrap();
        db.save_layout(P, &serde_json::json!({"version": 2}))
            .unwrap();
        db.save_cursor(
            P,
            &Location {
                path: p("/work/app/src/main.rs"),
                line: 41,
                column: 7,
            },
        )
        .unwrap();
    }
    let db = StateDb::open(&file).unwrap();
    assert_eq!(
        db.load_layout(P).unwrap(),
        Some(serde_json::json!({"version": 2}))
    );
    assert_eq!(
        db.cursor(P, &p("/work/app/src/main.rs")).unwrap(),
        Some((41, 7))
    );
    assert_eq!(
        db.cursor("/other", &p("/work/app/src/main.rs")).unwrap(),
        None
    );
    db.delete_layout(P).unwrap();
    assert_eq!(db.load_layout(P).unwrap(), None);
}

#[test]
fn a_broken_file_is_moved_aside() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("state.db");
    fs::write(&file, b"this is not a database, not even close to one....").unwrap();
    let db = StateDb::open(&file).unwrap();
    assert_eq!(db.schema_version().unwrap(), MIGRATIONS.len());
    assert!(dir.path().join("state.db.broken").exists());
}

#[test]
fn recent_projects_are_ordered_and_capped() {
    let db = StateDb::open_in_memory().unwrap();
    db.record_project_opened(&[p("/a")], "a").unwrap();
    db.record_project_opened(&[p("/b"), p("/shared")], "b")
        .unwrap();
    db.record_project_opened(&[p("/a")], "a renamed").unwrap();
    let recent = db.recent_projects().unwrap();
    let names: Vec<&str> = recent.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["a renamed", "b"]);
    assert_eq!(recent[1].roots, [p("/b"), p("/shared")]);

    for i in 0..RECENT_PROJECTS_LIMIT + 5 {
        db.record_project_opened(&[p(&format!("/many/{i}"))], "x")
            .unwrap();
    }
    assert_eq!(db.recent_projects().unwrap().len(), RECENT_PROJECTS_LIMIT);
    db.forget_project(&p(&format!("/many/{}", RECENT_PROJECTS_LIMIT + 4)))
        .unwrap();
    assert_eq!(
        db.recent_projects().unwrap().len(),
        RECENT_PROJECTS_LIMIT - 1
    );
}

#[test]
fn the_welcome_screen_reads_recent_projects_with_their_branch() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".git")).unwrap();
    fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/feature/x\n").unwrap();
    let db = Arc::new(StateDb::open_in_memory().unwrap());
    db.record_project_opened(&[dir.path().to_path_buf()], "demo")
        .unwrap();
    let recent = DbRecentProjects::new(db).recent();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].name, "demo");
    assert_eq!(recent[0].opened_days_ago, 0);
    assert_eq!(recent[0].branch.as_deref(), Some("feature/x"));
}

#[test]
fn navigation_history_round_trips() {
    let db = StateDb::open_in_memory().unwrap();
    let back = vec![
        NavLocation {
            path: p("/work/app/a.rs"),
            position: Some((1, 2)),
        },
        NavLocation {
            path: p("/work/app/b.rs"),
            position: None,
        },
    ];
    let forward = vec![NavLocation {
        path: p("/work/app/c.rs"),
        position: Some((9, 0)),
    }];
    db.save_nav_history(P, &back, &forward).unwrap();
    assert_eq!(db.nav_history(P).unwrap(), (back.clone(), forward));
    db.save_nav_history(P, &back[..1], &[]).unwrap();
    assert_eq!(db.nav_history(P).unwrap(), (back[..1].to_vec(), Vec::new()));
}

#[test]
fn recent_files_and_locations_keep_the_newest() {
    let db = StateDb::open_in_memory().unwrap();
    db.touch_recent_file(P, &p("/work/app/a.rs")).unwrap();
    db.touch_recent_file(P, &p("/work/app/b.rs")).unwrap();
    db.touch_recent_file(P, &p("/work/app/a.rs")).unwrap();
    assert_eq!(
        db.recent_files(P).unwrap(),
        [p("/work/app/a.rs"), p("/work/app/b.rs")]
    );
    db.forget_recent_file(P, &p("/work/app/a.rs")).unwrap();
    assert_eq!(db.recent_files(P).unwrap(), [p("/work/app/b.rs")]);
    for i in 0..RECENT_FILES_LIMIT + 3 {
        db.touch_recent_file(P, &p(&format!("/work/app/{i}.rs")))
            .unwrap();
    }
    assert_eq!(db.recent_files(P).unwrap().len(), RECENT_FILES_LIMIT);

    let at = |line| Location {
        path: p("/work/app/a.rs"),
        line,
        column: 0,
    };
    db.add_recent_location(P, &at(3), "  let x = 1;  ").unwrap();
    db.add_recent_location(P, &at(10), "fn main()").unwrap();
    db.add_recent_location(P, &at(3), "let x = 2;").unwrap();
    let locations = db.recent_locations(P).unwrap();
    let lines: Vec<(u32, &str)> = locations
        .iter()
        .map(|l| (l.location.line, l.snippet.as_str()))
        .collect();
    assert_eq!(lines, [(3, "let x = 2;"), (10, "fn main()")]);
    for i in 0..RECENT_LOCATIONS_LIMIT as u32 + 3 {
        db.add_recent_location(P, &at(100 + i), "x").unwrap();
    }
    assert_eq!(
        db.recent_locations(P).unwrap().len(),
        RECENT_LOCATIONS_LIMIT
    );
    assert!(db.recent_locations("/elsewhere").unwrap().is_empty());
}

#[test]
fn trust_covers_subfolders() {
    let db = StateDb::open_in_memory().unwrap();
    let work = p("/home/me/work");
    assert!(!db.is_trusted(&work.join("app")).unwrap());
    db.trust(&work).unwrap();
    assert!(db.is_trusted(&work).unwrap());
    assert!(db.is_trusted(&work.join("app/sub")).unwrap());
    assert!(!db.is_trusted(&p("/home/me/other")).unwrap());
    assert!(!db.is_trusted(&p("/home/me/workshop")).unwrap());
    assert_eq!(db.trusted_folders().unwrap(), std::slice::from_ref(&work));
    db.untrust(&work).unwrap();
    assert!(!db.is_trusted(&work.join("app")).unwrap());
}
