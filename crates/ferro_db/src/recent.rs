//! The welcome screen's recent projects, from the state database.

use std::{
    fs,
    path::Path,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use ferro_services::{RecentProject, RecentProjectsService};

use crate::StateDb;

pub struct DbRecentProjects {
    db: Arc<StateDb>,
}

impl DbRecentProjects {
    pub fn new(db: Arc<StateDb>) -> Self {
        Self { db }
    }
}

/// The checked-out branch, read from `.git/HEAD` (no git process needed).
pub fn current_branch(root: &Path) -> Option<String> {
    let head = fs::read_to_string(root.join(".git").join("HEAD")).ok()?;
    let head = head.trim();
    match head.strip_prefix("ref: refs/heads/") {
        Some(branch) => Some(branch.to_string()),
        // A detached HEAD: show the short commit id.
        None if head.len() >= 8 => Some(head[..8].to_string()),
        None => None,
    }
}

impl RecentProjectsService for DbRecentProjects {
    fn recent(&self) -> Vec<RecentProject> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let records = self.db.recent_projects().unwrap_or_else(|err| {
            tracing::warn!("cannot read recent projects: {err:#}");
            Vec::new()
        });
        records
            .into_iter()
            .map(|record| RecentProject {
                opened_days_ago: ((now - record.last_opened).max(0) / 86_400_000) as u32,
                branch: current_branch(&record.path),
                name: record.name,
                path: record.path,
            })
            .collect()
    }
}
