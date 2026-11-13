//! FerroIDE's workspace state in SQLite (plan §8, Stage 4; ADR-003).
//!
//! One database, `<data>/FerroIDE/state.db`, holds what the IDE writes and
//! people never edit:
//!
//! - recently opened projects (the welcome screen);
//! - the window layout of each project (the dock state, including the open
//!   editor tabs);
//! - cursor positions per file;
//! - Back / Forward navigation history;
//! - recent files (Ctrl+Tab, Ctrl+P) and Recent Locations;
//! - trusted project folders.
//!
//! The schema is versioned with `PRAGMA user_version` and upgraded by
//! [`MIGRATIONS`] on open. A database that cannot be opened or migrated is
//! moved aside and replaced by an empty one: state is a convenience and must
//! never stop the IDE from starting.
//!
//! Projects are keyed by their first root's path, as text.

use std::{
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result};
use rusqlite::{Connection, OptionalExtension as _, params};

mod recent;

pub use recent::DbRecentProjects;

/// Schema migrations; entry `n` upgrades version `n` to `n + 1`.
pub const MIGRATIONS: &[&str] = &[
    // 1: the Stage 4 schema.
    r#"
    CREATE TABLE projects (
        path        TEXT PRIMARY KEY,
        name        TEXT NOT NULL,
        roots       TEXT NOT NULL DEFAULT '[]',
        last_opened INTEGER NOT NULL
    );
    CREATE TABLE layouts (
        project TEXT PRIMARY KEY,
        layout  TEXT NOT NULL,
        updated INTEGER NOT NULL
    );
    CREATE TABLE cursors (
        project TEXT NOT NULL,
        path    TEXT NOT NULL,
        line    INTEGER NOT NULL,
        column  INTEGER NOT NULL,
        updated INTEGER NOT NULL,
        PRIMARY KEY (project, path)
    );
    CREATE TABLE nav_history (
        project   TEXT NOT NULL,
        direction INTEGER NOT NULL,
        position  INTEGER NOT NULL,
        path      TEXT NOT NULL,
        line      INTEGER,
        column    INTEGER,
        PRIMARY KEY (project, direction, position)
    );
    CREATE TABLE recent_files (
        project TEXT NOT NULL,
        path    TEXT NOT NULL,
        opened  INTEGER NOT NULL,
        PRIMARY KEY (project, path)
    );
    CREATE TABLE recent_locations (
        id      INTEGER PRIMARY KEY AUTOINCREMENT,
        project TEXT NOT NULL,
        path    TEXT NOT NULL,
        line    INTEGER NOT NULL,
        column  INTEGER NOT NULL,
        snippet TEXT NOT NULL,
        visited INTEGER NOT NULL
    );
    CREATE INDEX recent_locations_by_project ON recent_locations (project, visited);
    CREATE TABLE trusted (
        path    TEXT PRIMARY KEY,
        trusted INTEGER NOT NULL
    );
    "#,
];

/// How many entries each history keeps per project.
pub const RECENT_FILES_LIMIT: usize = 50;
pub const RECENT_LOCATIONS_LIMIT: usize = 50;
pub const RECENT_PROJECTS_LIMIT: usize = 30;

/// A place in a file. Lines and columns are 0-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
}

/// A Back / Forward entry: a file, with a position when one was known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavLocation {
    pub path: PathBuf,
    pub position: Option<(u32, u32)>,
}

/// A visited place, with the line's text for the Recent Locations popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentLocation {
    pub location: Location,
    pub snippet: String,
    /// Milliseconds since the Unix epoch.
    pub visited: i64,
}

/// A recently opened project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRecord {
    pub path: PathBuf,
    pub name: String,
    /// Every root directory, the first being `path`.
    pub roots: Vec<PathBuf>,
    /// Milliseconds since the Unix epoch.
    pub last_opened: i64,
}

pub struct StateDb {
    conn: Mutex<Connection>,
    /// Timestamps are strictly increasing, so ordering by time is stable
    /// even within one millisecond.
    clock: Mutex<i64>,
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

impl StateDb {
    /// Open (or create) the database at `path`, migrating it to the current
    /// schema. A broken file is moved aside to `<name>.broken` and replaced.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
        }
        match Self::try_open(path) {
            Ok(db) => Ok(db),
            Err(err) => {
                tracing::warn!("resetting the state database {}: {err:#}", path.display());
                let aside = path.with_extension("db.broken");
                let _ = std::fs::remove_file(&aside);
                std::fs::rename(path, &aside)
                    .with_context(|| format!("moving {} aside", path.display()))?;
                for suffix in ["-wal", "-shm"] {
                    let mut side = path.as_os_str().to_owned();
                    side.push(suffix);
                    let _ = std::fs::remove_file(PathBuf::from(side));
                }
                Self::try_open(path)
            }
        }
    }

    fn try_open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| path.display().to_string())?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(2))?;
        Self::with_connection(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::with_connection(Connection::open_in_memory()?)
    }

    fn with_connection(mut conn: Connection) -> Result<Self> {
        migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            clock: Mutex::new(0),
        })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn now(&self) -> i64 {
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let mut last = self.clock.lock().unwrap_or_else(|e| e.into_inner());
        *last = wall.max(*last + 1);
        *last
    }

    /// The schema version of the database.
    pub fn schema_version(&self) -> Result<usize> {
        Ok(self
            .conn()
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?
            as usize)
    }

    // -----------------------------------------------------------------------
    // Projects
    // -----------------------------------------------------------------------

    pub fn record_project_opened(&self, roots: &[PathBuf], name: &str) -> Result<()> {
        let Some(first) = roots.first() else {
            return Ok(());
        };
        let roots_json = serde_json::to_string(&roots.iter().map(|r| text(r)).collect::<Vec<_>>())?;
        let now = self.now();
        let conn = self.conn();
        conn.execute(
            "INSERT INTO projects (path, name, roots, last_opened) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (path) DO UPDATE SET name = ?2, roots = ?3, last_opened = ?4",
            params![text(first), name, roots_json, now],
        )?;
        conn.execute(
            "DELETE FROM projects WHERE path NOT IN
             (SELECT path FROM projects ORDER BY last_opened DESC LIMIT ?1)",
            params![RECENT_PROJECTS_LIMIT as i64],
        )?;
        Ok(())
    }

    /// Recently opened projects, most recent first.
    pub fn recent_projects(&self) -> Result<Vec<ProjectRecord>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT path, name, roots, last_opened FROM projects ORDER BY last_opened DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let roots: String = row.get(2)?;
            Ok(ProjectRecord {
                path: PathBuf::from(row.get::<_, String>(0)?),
                name: row.get(1)?,
                roots: serde_json::from_str::<Vec<String>>(&roots)
                    .unwrap_or_default()
                    .into_iter()
                    .map(PathBuf::from)
                    .collect(),
                last_opened: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Remove a project from the recent list (its other state stays).
    pub fn forget_project(&self, path: &Path) -> Result<()> {
        self.conn()
            .execute("DELETE FROM projects WHERE path = ?1", params![text(path)])?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Window layout
    // -----------------------------------------------------------------------

    pub fn save_layout(&self, project: &str, layout: &serde_json::Value) -> Result<()> {
        let now = self.now();
        self.conn().execute(
            "INSERT INTO layouts (project, layout, updated) VALUES (?1, ?2, ?3)
             ON CONFLICT (project) DO UPDATE SET layout = ?2, updated = ?3",
            params![project, layout.to_string(), now],
        )?;
        Ok(())
    }

    pub fn load_layout(&self, project: &str) -> Result<Option<serde_json::Value>> {
        let layout: Option<String> = self
            .conn()
            .query_row(
                "SELECT layout FROM layouts WHERE project = ?1",
                params![project],
                |row| row.get(0),
            )
            .optional()?;
        Ok(layout.and_then(|text| serde_json::from_str(&text).ok()))
    }

    pub fn delete_layout(&self, project: &str) -> Result<()> {
        self.conn()
            .execute("DELETE FROM layouts WHERE project = ?1", params![project])?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Cursor positions
    // -----------------------------------------------------------------------

    pub fn save_cursor(&self, project: &str, location: &Location) -> Result<()> {
        let now = self.now();
        self.conn().execute(
            "INSERT INTO cursors (project, path, line, column, updated) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (project, path) DO UPDATE SET line = ?3, column = ?4, updated = ?5",
            params![
                project,
                text(&location.path),
                location.line,
                location.column,
                now
            ],
        )?;
        Ok(())
    }

    /// The last cursor position in `path`: (line, column), 0-based.
    pub fn cursor(&self, project: &str, path: &Path) -> Result<Option<(u32, u32)>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT line, column FROM cursors WHERE project = ?1 AND path = ?2",
                params![project, text(path)],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?)
    }

    // -----------------------------------------------------------------------
    // Back / Forward
    // -----------------------------------------------------------------------

    /// Replace the stored history. Both lists are oldest first.
    pub fn save_nav_history(
        &self,
        project: &str,
        back: &[NavLocation],
        forward: &[NavLocation],
    ) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM nav_history WHERE project = ?1",
            params![project],
        )?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO nav_history (project, direction, position, path, line, column)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for (direction, list) in [(0, back), (1, forward)] {
                for (ix, entry) in list.iter().enumerate() {
                    insert.execute(params![
                        project,
                        direction,
                        ix as i64,
                        text(&entry.path),
                        entry.position.map(|p| p.0),
                        entry.position.map(|p| p.1),
                    ])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// (back, forward), oldest first.
    pub fn nav_history(&self, project: &str) -> Result<(Vec<NavLocation>, Vec<NavLocation>)> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT direction, path, line, column FROM nav_history
             WHERE project = ?1 ORDER BY direction, position",
        )?;
        let mut back = Vec::new();
        let mut forward = Vec::new();
        let rows = stmt.query_map(params![project], |row| {
            let line: Option<u32> = row.get(2)?;
            let column: Option<u32> = row.get(3)?;
            Ok((
                row.get::<_, i64>(0)?,
                NavLocation {
                    path: PathBuf::from(row.get::<_, String>(1)?),
                    position: line.zip(column),
                },
            ))
        })?;
        for row in rows {
            let (direction, entry) = row?;
            if direction == 0 {
                back.push(entry);
            } else {
                forward.push(entry);
            }
        }
        Ok((back, forward))
    }

    // -----------------------------------------------------------------------
    // Recent files and locations
    // -----------------------------------------------------------------------

    pub fn touch_recent_file(&self, project: &str, path: &Path) -> Result<()> {
        let now = self.now();
        let conn = self.conn();
        conn.execute(
            "INSERT INTO recent_files (project, path, opened) VALUES (?1, ?2, ?3)
             ON CONFLICT (project, path) DO UPDATE SET opened = ?3",
            params![project, text(path), now],
        )?;
        conn.execute(
            "DELETE FROM recent_files WHERE project = ?1 AND path NOT IN
             (SELECT path FROM recent_files WHERE project = ?1 ORDER BY opened DESC LIMIT ?2)",
            params![project, RECENT_FILES_LIMIT as i64],
        )?;
        Ok(())
    }

    pub fn forget_recent_file(&self, project: &str, path: &Path) -> Result<()> {
        self.conn().execute(
            "DELETE FROM recent_files WHERE project = ?1 AND path = ?2",
            params![project, text(path)],
        )?;
        Ok(())
    }

    /// Most recent first.
    pub fn recent_files(&self, project: &str) -> Result<Vec<PathBuf>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT path FROM recent_files WHERE project = ?1 ORDER BY opened DESC")?;
        let rows = stmt.query_map(params![project], |row| {
            Ok(PathBuf::from(row.get::<_, String>(0)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Record a visit. A visit to the same line of the same file replaces
    /// the older one.
    pub fn add_recent_location(
        &self,
        project: &str,
        location: &Location,
        snippet: &str,
    ) -> Result<()> {
        let now = self.now();
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM recent_locations WHERE project = ?1 AND path = ?2 AND line = ?3",
            params![project, text(&location.path), location.line],
        )?;
        tx.execute(
            "INSERT INTO recent_locations (project, path, line, column, snippet, visited)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                project,
                text(&location.path),
                location.line,
                location.column,
                snippet.trim(),
                now
            ],
        )?;
        tx.execute(
            "DELETE FROM recent_locations WHERE project = ?1 AND id NOT IN
             (SELECT id FROM recent_locations WHERE project = ?1 ORDER BY visited DESC LIMIT ?2)",
            params![project, RECENT_LOCATIONS_LIMIT as i64],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Most recent first.
    pub fn recent_locations(&self, project: &str) -> Result<Vec<RecentLocation>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT path, line, column, snippet, visited FROM recent_locations
             WHERE project = ?1 ORDER BY visited DESC",
        )?;
        let rows = stmt.query_map(params![project], |row| {
            Ok(RecentLocation {
                location: Location {
                    path: PathBuf::from(row.get::<_, String>(0)?),
                    line: row.get(1)?,
                    column: row.get(2)?,
                },
                snippet: row.get(3)?,
                visited: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // -----------------------------------------------------------------------
    // Trusted projects
    // -----------------------------------------------------------------------

    /// Trust `path` and everything below it.
    pub fn trust(&self, path: &Path) -> Result<()> {
        let now = self.now();
        self.conn().execute(
            "INSERT INTO trusted (path, trusted) VALUES (?1, ?2)
             ON CONFLICT (path) DO UPDATE SET trusted = ?2",
            params![text(path), now],
        )?;
        Ok(())
    }

    pub fn untrust(&self, path: &Path) -> Result<()> {
        self.conn()
            .execute("DELETE FROM trusted WHERE path = ?1", params![text(path)])?;
        Ok(())
    }

    /// Whether `path` or one of its ancestors is trusted.
    pub fn is_trusted(&self, path: &Path) -> Result<bool> {
        let conn = self.conn();
        let mut stmt = conn.prepare_cached("SELECT 1 FROM trusted WHERE path = ?1")?;
        for ancestor in path.ancestors() {
            if ancestor.as_os_str().is_empty() {
                continue;
            }
            if stmt.exists(params![text(ancestor)])? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Every trusted folder, for the settings page.
    pub fn trusted_folders(&self) -> Result<Vec<PathBuf>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT path FROM trusted ORDER BY path")?;
        let rows = stmt.query_map([], |row| Ok(PathBuf::from(row.get::<_, String>(0)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

/// Bring the schema to the newest version, one transaction per step.
fn migrate(conn: &mut Connection) -> Result<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let version = version as usize;
    if version > MIGRATIONS.len() {
        anyhow::bail!(
            "the state database has schema version {version}, newer than this FerroIDE ({})",
            MIGRATIONS.len()
        );
    }
    for (ix, sql) in MIGRATIONS.iter().enumerate().skip(version) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)
            .with_context(|| format!("migrating the state database to version {}", ix + 1))?;
        tx.pragma_update(None, "user_version", (ix + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
