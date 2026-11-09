//! Version control on mock data: branches, working-tree changes, a commit
//! graph with merges, diffs against a fictional `HEAD`, and a merge conflict.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{anyhow, bail};
use futures::{FutureExt as _, future::BoxFuture};

use super::{Dice, MOCK_NOW, MockConfig, MockProjectService, diff, lock, mix, simulate};
use crate::{
    Branch, ChangeKind, Commit, FileChange, FileDiff, LineChange, MergeConflict, Result, VcsService,
};

const CONFLICT_FILE: &str = "src/merge_me.rs";
const OURS_LABEL: &str = "HEAD";
const THEIRS_LABEL: &str = "feature/FERRO-207-jitter";

const CONFLICT_BASE: &str = "use std::time::Duration;

/// How long to wait before the next attempt.
pub fn retry_delay(attempt: u32) -> Duration {
    Duration::from_millis(100 * attempt as u64)
}

/// Attempts before giving up.
pub const MAX_RETRIES: u32 = 3;

/// Whether another attempt is allowed.
pub fn should_retry(attempt: u32) -> bool {
    attempt < MAX_RETRIES
}
";

const CONFLICT_OURS: &str = "use std::time::Duration;

/// How long to wait before the next attempt: exponential backoff.
pub fn retry_delay(attempt: u32) -> Duration {
    Duration::from_millis(100 * 2u64.pow(attempt))
}

/// Attempts before giving up.
pub const MAX_RETRIES: u32 = 5;

/// Whether another attempt is allowed.
pub fn should_retry(attempt: u32) -> bool {
    attempt < MAX_RETRIES
}
";

const CONFLICT_THEIRS: &str = "use std::time::Duration;

/// How long to wait before the next attempt.
pub fn retry_delay(attempt: u32) -> Duration {
    Duration::from_millis(100 * attempt as u64 + jitter(attempt))
}

/// Attempts before giving up.
pub const MAX_RETRIES: u32 = 3;

/// Whether another attempt is allowed.
pub fn should_retry(attempt: u32) -> bool {
    attempt < MAX_RETRIES
}

/// Up to 50 ms of deterministic jitter.
fn jitter(attempt: u32) -> u64 {
    (attempt as u64 * 7919) % 50
}
";

const NOTES: &str = "# Notes

- Ask Grace about the retry policy.
- The web front end needs a dark theme.
";

/// The diff3-style text git leaves in a conflicted file.
pub fn conflict_markers(conflict: &MergeConflict) -> String {
    diff::merge3(
        &conflict.base,
        &conflict.ours,
        &conflict.theirs,
        &conflict.ours_label,
        &conflict.theirs_label,
    )
}

fn demo_conflict(root: &Path) -> MergeConflict {
    MergeConflict {
        path: super::join_rel(root, CONFLICT_FILE),
        relative: CONFLICT_FILE.into(),
        base: CONFLICT_BASE.into(),
        ours: CONFLICT_OURS.into(),
        theirs: CONFLICT_THEIRS.into(),
        ours_label: OURS_LABEL.into(),
        theirs_label: THEIRS_LABEL.into(),
    }
}

/// Files the project mock adds to the demo so the VCS data is consistent:
/// an untracked note and the conflicted file (with markers).
pub(super) fn extra_demo_files(root: &Path) -> Vec<(String, String)> {
    vec![
        ("docs/notes.md".into(), NOTES.into()),
        (CONFLICT_FILE.into(), conflict_markers(&demo_conflict(root))),
    ]
}

/// The initial working-tree changes.
const CHANGES: &[(&str, ChangeKind, &str)] = &[
    ("src/config.rs", ChangeKind::Modified, "Changes"),
    ("src/main.rs", ChangeKind::Modified, "Changes"),
    ("README.md", ChangeKind::Modified, "Changes"),
    ("web/src/app.ts", ChangeKind::Added, "Changes"),
    (CONFLICT_FILE, ChangeKind::Conflicted, "Changes"),
    ("docs/notes.md", ChangeKind::Untracked, "Unversioned Files"),
];

/// A believable `HEAD` version of a modified file: one line the working
/// copy added is missing, one line differs and one line the working copy
/// deleted is still there.
fn head_version(text: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let n = lines.len();
    if n < 6 {
        return lines.join("\n") + "\n";
    }
    // Deleted in the working copy: an extra line in HEAD.
    let at = n * 3 / 4;
    let indent: String = lines[at]
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();
    lines.insert(at, format!("{indent}// Removed in the working copy."));
    // Modified: the first non-blank line after a quarter.
    if let Some(ix) = (n / 4..n).find(|&i| !lines[i].trim().is_empty()) {
        lines[ix] = lines[ix].replacen("    ", "  ", 1);
        if !lines[ix].contains("  ") {
            lines[ix].push_str(" // old");
        }
    }
    // Added in the working copy: missing in HEAD.
    lines.remove(n / 2);
    lines.join("\n") + "\n"
}

pub struct MockVcsService {
    current: Mutex<String>,
    branches: Vec<Branch>,
    config: MockConfig,
    dice: Dice,
    project: Arc<MockProjectService>,
    /// `HEAD` text of tracked demo files, by relative path.
    head: HashMap<String, String>,
    changes: Mutex<Vec<FileChange>>,
    commits: Mutex<Vec<Commit>>,
    /// Merged text of files whose conflicts were resolved, by relative path.
    resolved: Mutex<HashMap<String, String>>,
}

impl MockVcsService {
    pub fn new(config: MockConfig, project: Arc<MockProjectService>) -> Self {
        let local = |name: &str, ahead, behind| Branch {
            name: name.into(),
            is_remote: false,
            ahead,
            behind,
        };
        let remote = |name: &str| Branch {
            name: name.into(),
            is_remote: true,
            ahead: 0,
            behind: 0,
        };

        let mut head = HashMap::new();
        for (path, text) in project.demo_files() {
            let Some(rel) = project.relative(&path) else {
                continue;
            };
            let kind = CHANGES.iter().find(|(r, ..)| *r == rel).map(|(_, k, _)| *k);
            match kind {
                Some(ChangeKind::Modified) => {
                    head.insert(rel, head_version(&text));
                }
                Some(ChangeKind::Added) => {
                    head.insert(rel, String::new());
                }
                Some(ChangeKind::Conflicted) => {
                    head.insert(rel, CONFLICT_OURS.to_string());
                }
                Some(_) => {}
                None => {
                    head.insert(rel, text);
                }
            }
        }
        let changes = CHANGES
            .iter()
            .map(|(rel, kind, list)| FileChange {
                path: project.absolute(rel),
                relative: rel.to_string(),
                kind: *kind,
                changelist: list.to_string(),
            })
            .collect();

        Self {
            current: Mutex::new("main".into()),
            branches: vec![
                local("main", 0, 2),
                local("develop", 3, 0),
                local("feature/FERRO-101-dock-layout", 5, 1),
                local("feature/FERRO-207-jitter", 2, 4),
                local("fix/crash-on-empty-workspace", 1, 0),
                remote("origin/main"),
                remote("origin/develop"),
                remote("origin/release/1.0"),
            ],
            dice: Dice::new(config.seed ^ 0xbeef),
            commits: Mutex::new(build_log(config.seed)),
            config,
            project,
            head,
            changes: Mutex::new(changes),
            resolved: Mutex::new(HashMap::new()),
        }
    }

    fn should_fail(&self) -> bool {
        self.config.failure_rate > 0.0 && self.dice.roll() < self.config.failure_rate
    }

    fn diff_now(&self, path: &Path) -> Result<FileDiff> {
        let rel = self
            .project
            .relative(path)
            .ok_or_else(|| anyhow!("{} is outside the repository", path.display()))?;
        let resolved = lock(&self.resolved).get(&rel).cloned();
        let new_text = resolved
            .or_else(|| self.project.file_text(path))
            .ok_or_else(|| anyhow!("no such file: {rel}"))?;
        let old_text = self.head.get(&rel).cloned().unwrap_or_default();
        Ok(FileDiff {
            patch: diff::unified_patch(&rel, &old_text, &new_text),
            relative: rel,
            old_text,
            new_text,
        })
    }

    fn commit_now(&self, message: &str, paths: &[PathBuf]) -> Result<String> {
        if message.trim().is_empty() {
            bail!("the commit message is empty");
        }
        if paths.is_empty() {
            bail!("no files selected to commit");
        }
        let mut changes = lock(&self.changes);
        let selected: Vec<FileChange> = changes
            .iter()
            .filter(|c| paths.iter().any(|p| super::norm(p) == super::norm(&c.path)))
            .cloned()
            .collect();
        if selected.is_empty() {
            bail!("none of the selected files has changes");
        }
        if let Some(c) = selected.iter().find(|c| c.kind == ChangeKind::Conflicted) {
            bail!("{} still has merge conflicts", c.relative);
        }
        changes.retain(|c| !selected.contains(c));

        let mut commits = lock(&self.commits);
        let parent = commits
            .iter()
            .find(|c| c.refs.iter().any(|r| r == "HEAD -> main"))
            .map(|c| c.id.clone());
        for c in commits.iter_mut() {
            c.refs.retain(|r| r != "HEAD -> main");
        }
        let n = commits.len() as u64;
        let id = hash(mix(self.config.seed ^ 0xc0ffee ^ n));
        let mut lines = message.trim().lines();
        commits.insert(
            0,
            Commit {
                id: id.clone(),
                parents: parent.into_iter().collect(),
                author: "You".into(),
                timestamp: MOCK_NOW,
                subject: lines.next().unwrap_or_default().to_string(),
                body: lines.collect::<Vec<_>>().join("\n").trim().to_string(),
                refs: vec!["HEAD -> main".into()],
                files: selected.iter().map(|c| c.relative.clone()).collect(),
            },
        );
        Ok(id)
    }

    fn answer<T: Send + 'static>(
        &self,
        result: Result<T>,
        fail: bool,
        latency_factor: u32,
        what: &'static str,
    ) -> BoxFuture<'static, Result<T>> {
        let latency = self.config.latency * latency_factor;
        async move {
            simulate(latency, fail, what).await?;
            result
        }
        .boxed()
    }
}

impl VcsService for MockVcsService {
    fn current_branch(&self) -> BoxFuture<'static, Result<Branch>> {
        let name = lock(&self.current).clone();
        let branch = self
            .branches
            .iter()
            .find(|b| b.name == name)
            .cloned()
            .ok_or_else(|| anyhow!("branch {name} vanished"));
        self.answer(branch, false, 1, "")
    }

    fn branches(&self) -> BoxFuture<'static, Result<Vec<Branch>>> {
        let fail = self.should_fail();
        self.answer(Ok(self.branches.clone()), fail, 1, "listing branches")
    }

    fn checkout(&self, name: &str) -> BoxFuture<'static, Result<()>> {
        let exists = self.branches.iter().any(|b| b.name == name && !b.is_remote);
        if exists {
            *lock(&self.current) = name.to_string();
        }
        let result = if exists {
            Ok(())
        } else {
            Err(anyhow!("no local branch {name}"))
        };
        self.answer(result, false, 3, "")
    }

    fn changes(&self) -> BoxFuture<'static, Result<Vec<FileChange>>> {
        let fail = self.should_fail();
        self.answer(
            Ok(lock(&self.changes).clone()),
            fail,
            1,
            "reading git status",
        )
    }

    fn log(&self, limit: usize) -> BoxFuture<'static, Result<Vec<Commit>>> {
        let fail = self.should_fail();
        let commits = lock(&self.commits).iter().take(limit).cloned().collect();
        self.answer(Ok(commits), fail, 2, "reading the log")
    }

    fn diff(&self, path: &Path) -> BoxFuture<'static, Result<FileDiff>> {
        let fail = self.should_fail();
        let result = self.diff_now(path);
        self.answer(result, fail, 1, "computing a diff")
    }

    fn line_changes(&self, path: &Path, text: &str) -> Vec<LineChange> {
        match self
            .project
            .relative(path)
            .and_then(|rel| self.head.get(&rel))
        {
            Some(head) => diff::line_changes(head, text),
            None => Vec::new(),
        }
    }

    fn conflicts(&self) -> BoxFuture<'static, Result<Vec<MergeConflict>>> {
        let unresolved = lock(&self.changes)
            .iter()
            .any(|c| c.kind == ChangeKind::Conflicted);
        let conflicts = if unresolved {
            vec![demo_conflict(self.project.root())]
        } else {
            Vec::new()
        };
        self.answer(Ok(conflicts), false, 1, "")
    }

    fn resolve_conflict(&self, path: &Path, text: &str) -> BoxFuture<'static, Result<()>> {
        let mut changes = lock(&self.changes);
        let result = match changes
            .iter_mut()
            .find(|c| super::norm(&c.path) == super::norm(path) && c.kind == ChangeKind::Conflicted)
        {
            Some(change) if !text.contains("<<<<<<<") => {
                change.kind = ChangeKind::Modified;
                lock(&self.resolved).insert(change.relative.clone(), text.to_string());
                Ok(())
            }
            Some(_) => Err(anyhow!("the merged text still has conflict markers")),
            None => Err(anyhow!("{} has no conflicts", path.display())),
        };
        self.answer(result, false, 1, "")
    }

    fn commit(&self, message: &str, paths: &[PathBuf]) -> BoxFuture<'static, Result<String>> {
        let fail = self.should_fail();
        let result = self.commit_now(message, paths);
        self.answer(result, fail, 4, "committing")
    }

    fn clone_repository(&self, url: &str, directory: &Path) -> BoxFuture<'static, Result<PathBuf>> {
        let fail = self.should_fail();
        let url = url.trim();
        let looks_like_git = ["https://", "http://", "ssh://", "git://", "file://", "git@"]
            .iter()
            .any(|p| url.starts_with(p));
        let name = url
            .trim_end_matches('/')
            .rsplit(['/', ':'])
            .next()
            .unwrap_or_default()
            .trim_end_matches(".git")
            .to_string();
        let result = if !looks_like_git {
            Err(anyhow!("{url} is not a git repository URL"))
        } else if name.is_empty() {
            Err(anyhow!("cannot tell the repository name from {url}"))
        } else {
            Ok(directory.join(name))
        };
        self.answer(result, fail, 20, "cloning")
    }
}

fn hash(seed: u64) -> String {
    format!(
        "{:016x}{:016x}{:08x}",
        mix(seed),
        mix(seed ^ 1),
        mix(seed ^ 2) as u32
    )
}

/// Builds a commit graph oldest-first, then returns it newest-first.
struct LogBuilder {
    seed: u64,
    commits: Vec<Commit>,
    tips: HashMap<&'static str, String>,
    time: i64,
}

impl LogBuilder {
    fn next_id(&mut self) -> (String, i64) {
        let n = self.commits.len() as u64;
        self.time += 1800 + (mix(self.seed ^ n) % 6) as i64 * 2400;
        (hash(self.seed ^ (n << 8)), self.time)
    }

    fn commit(&mut self, branch: &'static str, author: &str, subject: &str, files: &[&str]) {
        let (id, timestamp) = self.next_id();
        let parents = self.tips.get(branch).cloned().into_iter().collect();
        self.commits.push(Commit {
            id: id.clone(),
            parents,
            author: author.into(),
            timestamp,
            subject: subject.into(),
            body: String::new(),
            refs: Vec::new(),
            files: files.iter().map(|f| f.to_string()).collect(),
        });
        self.tips.insert(branch, id);
    }

    fn branch(&mut self, new: &'static str, from: &'static str) {
        let tip = self.tips[from].clone();
        self.tips.insert(new, tip);
    }

    fn merge(&mut self, into: &'static str, from: &'static str, author: &str) {
        let (id, timestamp) = self.next_id();
        let subject = if into == "main" {
            format!("Merge branch '{from}'")
        } else {
            format!("Merge branch '{from}' into {into}")
        };
        let files = self
            .commits
            .iter()
            .rev()
            .find(|c| Some(&c.id) == self.tips.get(from))
            .map(|c| c.files.clone())
            .unwrap_or_default();
        self.commits.push(Commit {
            id: id.clone(),
            parents: vec![self.tips[into].clone(), self.tips[from].clone()],
            author: author.into(),
            timestamp,
            subject,
            body: String::new(),
            refs: Vec::new(),
            files,
        });
        self.tips.insert(into, id);
    }

    fn tag(&mut self, name: &str) {
        if let Some(last) = self.commits.last_mut() {
            last.refs.push(format!("tag: {name}"));
        }
    }
}

fn build_log(seed: u64) -> Vec<Commit> {
    const ADA: &str = "Ada Lovelace";
    const LINUS: &str = "Linus Torvalds";
    const GRACE: &str = "Grace Hopper";
    const MARGARET: &str = "Margaret Hamilton";
    const KEN: &str = "Ken Thompson";
    const BARBARA: &str = "Barbara Liskov";
    const F_WEB: &str = "feature/web";
    const F42: &str = "feature/FERRO-42-config-errors";
    const FIX: &str = "fix/crash-on-empty-workspace";
    const F101: &str = "feature/FERRO-101-dock-layout";
    const F207: &str = "feature/FERRO-207-jitter";

    let mut b = LogBuilder {
        seed,
        commits: Vec::new(),
        tips: HashMap::new(),
        time: MOCK_NOW - 60 * 24 * 3600,
    };
    b.commit(
        "main",
        ADA,
        "Initial commit",
        &["Cargo.toml", "src/main.rs"],
    );
    b.commit("main", ADA, "Add README", &["README.md"]);
    b.commit("main", LINUS, "Add greeting library", &["src/lib.rs"]);
    b.branch("develop", "main");
    b.commit(
        "develop",
        GRACE,
        "Load names from demo.toml",
        &["src/config.rs", "src/main.rs"],
    );
    b.commit("develop", GRACE, "Default retries to 3", &["src/config.rs"]);
    b.branch(F_WEB, "develop");
    b.commit(
        F_WEB,
        MARGARET,
        "Scaffold web front end",
        &["web/package.json"],
    );
    b.commit(
        "develop",
        LINUS,
        "Greet strangers politely",
        &["src/lib.rs"],
    );
    b.commit(
        F_WEB,
        MARGARET,
        "Render greeting in the page",
        &["web/src/app.ts"],
    );
    b.commit(F_WEB, MARGARET, "Add excited flag", &["web/src/app.ts"]);
    b.merge("develop", F_WEB, MARGARET);
    b.commit("develop", ADA, "Add smoke test", &["tests/smoke.rs"]);
    b.commit("main", ADA, "Fix typo in README", &["README.md"]);
    b.merge("main", "develop", ADA);
    b.tag("v0.1.0");
    b.branch(F42, "develop");
    b.commit(
        F42,
        GRACE,
        "Report the config path on read errors",
        &["src/config.rs"],
    );
    b.commit("develop", LINUS, "Bump toml to 0.9", &["Cargo.toml"]);
    b.commit(
        F42,
        GRACE,
        "Test a missing config file",
        &["tests/smoke.rs"],
    );
    b.merge("develop", F42, GRACE);
    b.branch(FIX, "main");
    b.commit(
        FIX,
        KEN,
        "Do not panic on an empty workspace",
        &["src/main.rs"],
    );
    b.merge("main", FIX, KEN);
    b.merge("develop", "main", LINUS);
    b.branch(F101, "develop");
    b.commit(F101, BARBARA, "Sketch the dock layout", &["web/src/app.ts"]);
    b.commit("develop", ADA, "Document the web front end", &["README.md"]);
    b.commit(F101, BARBARA, "Persist dock sizes", &["web/src/app.ts"]);
    b.commit(
        "develop",
        LINUS,
        "Use anyhow context everywhere",
        &["src/config.rs", "src/main.rs"],
    );
    b.commit(F101, BARBARA, "Animate dock toggles", &["web/src/app.ts"]);
    b.commit("develop", GRACE, "Add vitest", &["web/package.json"]);
    b.commit(F101, BARBARA, "Respect reduced motion", &["web/src/app.ts"]);
    b.commit(
        F101,
        BARBARA,
        "Keyboard shortcuts for docks",
        &["web/src/app.ts"],
    );
    b.merge("main", "develop", ADA);
    b.tag("v0.2.0");
    b.branch(F207, "main");
    b.commit(F207, KEN, "Add jitter to retry delays", &[CONFLICT_FILE]);
    b.commit(
        "main",
        ADA,
        "Exponential backoff for retries",
        &[CONFLICT_FILE],
    );
    b.commit(F207, KEN, "Tune the jitter range", &[CONFLICT_FILE]);
    b.commit("main", LINUS, "Raise MAX_RETRIES to 5", &[CONFLICT_FILE]);
    b.commit(
        "develop",
        GRACE,
        "Prepare the 0.3 changelog",
        &["README.md"],
    );
    b.commit(
        "develop",
        MARGARET,
        "Dark mode for the web page",
        &["web/src/app.ts"],
    );
    b.commit("main", ADA, "Update dependencies", &["Cargo.toml"]);
    b.commit("develop", LINUS, "Speed up greeting", &["src/lib.rs"]);
    b.commit("main", KEN, "CI: cache the cargo registry", &["Cargo.toml"]);
    b.commit(
        "main",
        ADA,
        "Log the config path at startup",
        &["src/main.rs"],
    );
    b.commit(
        "develop",
        GRACE,
        "Retry the flaky smoke test",
        &["tests/smoke.rs"],
    );

    let refs: [(&str, &[&str]); 4] = [
        ("main", &["HEAD -> main", "origin/main"]),
        ("develop", &["develop", "origin/develop"]),
        (F101, &[F101]),
        (F207, &[F207]),
    ];
    for (branch, names) in refs {
        let tip = b.tips[branch].clone();
        if let Some(c) = b.commits.iter_mut().find(|c| c.id == tip) {
            c.refs.splice(0..0, names.iter().map(|n| n.to_string()));
        }
    }
    for (ix, c) in b.commits.iter_mut().enumerate() {
        if c.parents.len() < 2 && mix(seed ^ ix as u64).is_multiple_of(3) {
            c.body = format!(
                "{}\n\nRefs: FERRO-{}",
                "Keeps behavior the same for existing configurations.",
                100 + ix
            );
        }
    }
    let mut commits = b.commits;
    commits.reverse();
    commits
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::LineChangeKind;

    fn block<T>(f: BoxFuture<'static, T>) -> T {
        smol::block_on(f)
    }

    fn vcs() -> (MockVcsService, Arc<MockProjectService>) {
        let project = Arc::new(MockProjectService::demo(MockConfig::instant()));
        (
            MockVcsService::new(MockConfig::instant(), project.clone()),
            project,
        )
    }

    #[test]
    fn checkout_switches_branch() {
        let (vcs, _) = vcs();
        assert_eq!(block(vcs.current_branch()).unwrap().name, "main");
        block(vcs.checkout("develop")).unwrap();
        assert_eq!(block(vcs.current_branch()).unwrap().name, "develop");
        assert!(block(vcs.checkout("origin/main")).is_err());
    }

    #[test]
    fn the_log_is_a_consistent_graph() {
        let (vcs, _) = vcs();
        let log = block(vcs.log(1000)).unwrap();
        assert!(log.len() >= 40, "{}", log.len());
        assert!(log.windows(2).all(|w| w[0].timestamp >= w[1].timestamp));
        let ids: HashSet<&str> = log.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids.len(), log.len(), "unique ids");
        for (ix, c) in log.iter().enumerate() {
            for p in &c.parents {
                let parent_ix = log.iter().position(|x| &x.id == p).expect("parent exists");
                assert!(parent_ix > ix, "parents are older");
            }
        }
        assert!(log.iter().filter(|c| c.parents.len() == 2).count() >= 5);
        assert!(
            log.iter()
                .any(|c| c.refs.contains(&"HEAD -> main".to_string()))
        );
        assert!(
            log.iter()
                .any(|c| c.refs.contains(&"tag: v0.1.0".to_string()))
        );
        assert_eq!(log.iter().filter(|c| c.parents.is_empty()).count(), 1);
        assert_eq!(block(vcs.log(5)).unwrap().len(), 5);
        assert_eq!(build_log(42), build_log(42), "deterministic");
    }

    #[test]
    fn diffs_and_line_changes_agree() {
        let (vcs, project) = vcs();
        let main = project.absolute("src/main.rs");
        let diff = block(vcs.diff(&main)).unwrap();
        assert_ne!(diff.old_text, diff.new_text);
        assert!(
            diff.patch
                .starts_with("diff --git a/src/main.rs b/src/main.rs\n")
        );
        assert!(diff.patch.contains("\n@@ -"));
        let changes = vcs.line_changes(&main, &diff.new_text);
        assert!(!changes.is_empty());
        // The HEAD text against itself has no changes; an inserted line is
        // reported as added.
        assert!(vcs.line_changes(&main, &diff.old_text).is_empty());
        let mut lines: Vec<&str> = diff.old_text.lines().collect();
        lines.insert(1, "// new line");
        let edited = lines.join("\n") + "\n";
        assert_eq!(
            vcs.line_changes(&main, &edited),
            vec![LineChange {
                lines: 1..2,
                kind: LineChangeKind::Added
            }]
        );

        // Unchanged tracked files have empty patches; added files diff
        // against /dev/null.
        let lib = project.absolute("src/lib.rs");
        let text = project.file_text(&lib).unwrap();
        assert!(vcs.line_changes(&lib, &text).is_empty());
        let app = block(vcs.diff(&project.absolute("web/src/app.ts"))).unwrap();
        assert!(app.patch.contains("--- /dev/null"));
        assert!(block(vcs.diff(&project.absolute("nope.rs"))).is_err());
    }

    #[test]
    fn the_conflicted_file_has_markers() {
        let (vcs, project) = vcs();
        let conflicts = block(vcs.conflicts()).unwrap();
        assert_eq!(conflicts.len(), 1);
        let text = project.file_text(&conflicts[0].path).unwrap();
        assert_eq!(text, conflict_markers(&conflicts[0]));
        assert!(text.contains("<<<<<<< HEAD"));
        assert!(text.contains("||||||| base"));
        assert!(text.contains(">>>>>>> feature/FERRO-207-jitter"));
        // Only theirs added `jitter`: it merges cleanly, outside markers.
        let tail = text.rsplit(">>>>>>>").next().unwrap();
        assert!(tail.contains("fn jitter"), "{text}");
    }

    #[test]
    fn resolving_a_conflict_marks_the_file_modified() {
        let project = Arc::new(MockProjectService::demo(MockConfig::instant()));
        let vcs = MockVcsService::new(MockConfig::instant(), project.clone());
        let path = project.absolute(CONFLICT_FILE);
        assert!(
            smol::block_on(vcs.resolve_conflict(
                &path,
                "<<<<<<< x
"
            ))
            .is_err()
        );
        smol::block_on(vcs.resolve_conflict(
            &path, "merged
",
        ))
        .unwrap();
        assert!(smol::block_on(vcs.conflicts()).unwrap().is_empty());
        let changes = smol::block_on(vcs.changes()).unwrap();
        let change = changes
            .iter()
            .find(|c| c.relative == CONFLICT_FILE)
            .unwrap();
        assert_eq!(change.kind, ChangeKind::Modified);
        assert!(
            smol::block_on(vcs.resolve_conflict(
                &path, "again
"
            ))
            .is_err()
        );
    }

    #[test]
    fn commit_validates_and_removes_changes() {
        let (vcs, project) = vcs();
        let main = project.absolute("src/main.rs");
        assert!(block(vcs.commit("", std::slice::from_ref(&main))).is_err());
        assert!(block(vcs.commit("msg", &[])).is_err());
        assert!(
            block(vcs.commit("msg", &[project.absolute("src/merge_me.rs")])).is_err(),
            "conflicts block commits"
        );
        let before = block(vcs.changes()).unwrap().len();
        let id = block(vcs.commit("Fix main\n\nDetails.", &[main])).unwrap();
        assert_eq!(block(vcs.changes()).unwrap().len(), before - 1);
        let log = block(vcs.log(1)).unwrap();
        assert_eq!(log[0].id, id);
        assert_eq!(log[0].subject, "Fix main");
        assert_eq!(log[0].body, "Details.");
        assert!(log[0].refs.contains(&"HEAD -> main".to_string()));
    }

    #[test]
    fn clone_checks_the_url() {
        let (vcs, _) = vcs();
        let dir = Path::new("/tmp/clones");
        assert_eq!(
            block(vcs.clone_repository("https://github.com/ferroide/ferroide.git", dir)).unwrap(),
            dir.join("ferroide")
        );
        assert_eq!(
            block(vcs.clone_repository("git@github.com:me/dotfiles.git", dir)).unwrap(),
            dir.join("dotfiles")
        );
        assert!(block(vcs.clone_repository("not a url", dir)).is_err());
    }
}
