//! The index database: one SQLite file per project under
//! `<data>/index/`. It is a cache — a different schema version is dropped
//! and rebuilt, never migrated.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use anyhow::{Context as _, Result};
use ferro_services::{Symbol, SymbolKind, TextPos, TextRange, TodoKind};
use rusqlite::{Connection, OpenFlags, params};

const SCHEMA_VERSION: &str = "1";

const SCHEMA: &str = "
    CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    CREATE TABLE files (
        id INTEGER PRIMARY KEY,
        path TEXT NOT NULL UNIQUE,
        root TEXT NOT NULL,
        relative TEXT NOT NULL,
        size INTEGER NOT NULL,
        mtime INTEGER NOT NULL,
        trigrams INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE symbols (
        file INTEGER NOT NULL REFERENCES files (id) ON DELETE CASCADE,
        name TEXT NOT NULL,
        kind TEXT NOT NULL,
        sel_line INTEGER NOT NULL, sel_col INTEGER NOT NULL,
        start_line INTEGER NOT NULL, start_col INTEGER NOT NULL,
        end_line INTEGER NOT NULL, end_col INTEGER NOT NULL,
        container TEXT
    );
    CREATE INDEX symbols_by_file ON symbols (file);
    CREATE TABLE todos (
        file INTEGER NOT NULL REFERENCES files (id) ON DELETE CASCADE,
        line INTEGER NOT NULL,
        kind TEXT NOT NULL,
        text TEXT NOT NULL
    );
    CREATE INDEX todos_by_file ON todos (file);
    CREATE TABLE trigrams (
        tri INTEGER NOT NULL,
        file INTEGER NOT NULL,
        PRIMARY KEY (tri, file)
    ) WITHOUT ROWID;
    CREATE INDEX trigrams_by_file ON trigrams (file);
";

/// What the database stores about one file.
#[derive(Debug, Clone, PartialEq)]
pub struct FileRow {
    pub id: i64,
    pub path: PathBuf,
    pub root: PathBuf,
    pub relative: String,
    pub size: u64,
    pub mtime: i64,
    pub trigrams: bool,
}

/// A TODO comment as stored (the path comes from its file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoRow {
    pub line: u32,
    pub kind: TodoKind,
    pub text: String,
}

/// Everything indexed for one file.
#[derive(Debug, Clone, Default)]
pub struct FileData {
    pub symbols: Vec<Symbol>,
    pub todos: Vec<TodoRow>,
    /// `None` when trigrams are off.
    pub trigrams: Option<Vec<u32>>,
}

/// What [`Db::load`] restores at startup.
#[derive(Debug, Default)]
pub struct Loaded {
    pub files: Vec<FileRow>,
    pub symbols: HashMap<i64, Vec<Symbol>>,
    pub todos: HashMap<i64, Vec<TodoRow>>,
}

/// A writer connection (the indexing thread) and a reader (searches), so
/// a long write does not block lookups. In memory both are one connection.
#[derive(Clone)]
pub struct Db {
    write: Arc<Mutex<Connection>>,
    read: Arc<Mutex<Connection>>,
}

fn lock(conn: &Mutex<Connection>) -> MutexGuard<'_, Connection> {
    conn.lock().unwrap_or_else(|e| e.into_inner())
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub fn kind_name(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Module => "module",
        SymbolKind::Struct => "struct",
        SymbolKind::Enum => "enum",
        SymbolKind::Trait => "trait",
        SymbolKind::Function => "function",
        SymbolKind::Method => "method",
        SymbolKind::Field => "field",
        SymbolKind::Variable => "variable",
        SymbolKind::Constant => "constant",
        SymbolKind::Keyword => "keyword",
        SymbolKind::Snippet => "snippet",
        SymbolKind::Macro => "macro",
        SymbolKind::Test => "test",
    }
}

fn kind_from(name: &str) -> SymbolKind {
    match name {
        "module" => SymbolKind::Module,
        "struct" => SymbolKind::Struct,
        "enum" => SymbolKind::Enum,
        "trait" => SymbolKind::Trait,
        "function" => SymbolKind::Function,
        "method" => SymbolKind::Method,
        "field" => SymbolKind::Field,
        "constant" => SymbolKind::Constant,
        "keyword" => SymbolKind::Keyword,
        "snippet" => SymbolKind::Snippet,
        "macro" => SymbolKind::Macro,
        "test" => SymbolKind::Test,
        _ => SymbolKind::Variable,
    }
}

fn todo_kind_name(kind: TodoKind) -> &'static str {
    match kind {
        TodoKind::Todo => "todo",
        TodoKind::Fixme => "fixme",
    }
}

impl Db {
    /// Open (or create) the database file, rebuilding it when its schema is
    /// from another version or it is damaged.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
        }
        let write = match Self::open_writer(path) {
            Ok(conn) => conn,
            Err(err) => {
                tracing::warn!("rebuilding the index database {}: {err:#}", path.display());
                for suffix in ["", "-wal", "-shm"] {
                    let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
                }
                Self::open_writer(path)?
            }
        };
        let read = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        read.busy_timeout(std::time::Duration::from_secs(2))?;
        Ok(Self {
            write: Arc::new(Mutex::new(write)),
            read: Arc::new(Mutex::new(read)),
        })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::prepare(&conn)?;
        let conn = Arc::new(Mutex::new(conn));
        Ok(Self {
            write: conn.clone(),
            read: conn,
        })
    }

    fn open_writer(path: &Path) -> Result<Connection> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(2))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::prepare(&conn)?;
        Ok(conn)
    }

    fn prepare(conn: &Connection) -> Result<()> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let version: Option<String> = conn
            .query_row("SELECT value FROM meta WHERE key = 'schema'", [], |r| {
                r.get(0)
            })
            .ok();
        if version.as_deref() != Some(SCHEMA_VERSION) {
            conn.execute_batch(
                "DROP TABLE IF EXISTS trigrams; DROP TABLE IF EXISTS todos;
                 DROP TABLE IF EXISTS symbols; DROP TABLE IF EXISTS files;
                 DROP TABLE IF EXISTS meta;",
            )?;
            conn.execute_batch(SCHEMA)?;
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('schema', ?1)",
                [SCHEMA_VERSION],
            )?;
        }
        Ok(())
    }

    pub fn meta(&self, key: &str) -> Option<String> {
        lock(&self.read)
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .ok()
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        lock(&self.write).execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Every file with its symbols and TODO comments.
    pub fn load(&self) -> Result<Loaded> {
        let conn = lock(&self.read);
        let mut out = Loaded::default();
        let mut stmt =
            conn.prepare("SELECT id, path, root, relative, size, mtime, trigrams FROM files")?;
        let rows = stmt.query_map([], |r| {
            Ok(FileRow {
                id: r.get(0)?,
                path: PathBuf::from(r.get::<_, String>(1)?),
                root: PathBuf::from(r.get::<_, String>(2)?),
                relative: r.get(3)?,
                size: r.get::<_, i64>(4)? as u64,
                mtime: r.get(5)?,
                trigrams: r.get::<_, i64>(6)? != 0,
            })
        })?;
        for row in rows {
            out.files.push(row?);
        }
        let paths: HashMap<i64, PathBuf> =
            out.files.iter().map(|f| (f.id, f.path.clone())).collect();
        let mut stmt = conn.prepare(
            "SELECT file, name, kind, sel_line, sel_col, start_line, start_col, end_line,
                    end_col, container FROM symbols ORDER BY rowid",
        )?;
        let rows = stmt.query_map([], |r| {
            let pos = |l: usize, c: usize| -> rusqlite::Result<TextPos> {
                Ok(TextPos::new(r.get(l)?, r.get(c)?))
            };
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                pos(3, 4)?,
                TextRange::new(pos(5, 6)?, pos(7, 8)?),
                r.get::<_, Option<String>>(9)?,
            ))
        })?;
        for row in rows {
            let (file, name, kind, selection, range, container) = row?;
            let Some(path) = paths.get(&file) else {
                continue;
            };
            out.symbols.entry(file).or_default().push(Symbol {
                name,
                kind: kind_from(&kind),
                path: path.clone(),
                range,
                selection,
                container,
                children: Vec::new(),
            });
        }
        let mut stmt =
            conn.prepare("SELECT file, line, kind, text FROM todos ORDER BY file, line")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                TodoRow {
                    line: r.get(1)?,
                    kind: if r.get::<_, String>(2)? == "fixme" {
                        TodoKind::Fixme
                    } else {
                        TodoKind::Todo
                    },
                    text: r.get(3)?,
                },
            ))
        })?;
        for row in rows {
            let (file, todo) = row?;
            out.todos.entry(file).or_default().push(todo);
        }
        Ok(out)
    }

    /// Store a batch of indexed files in one transaction; returns their ids
    /// in order.
    pub fn store(&self, batch: &[(FileRow, FileData)]) -> Result<Vec<i64>> {
        let mut conn = lock(&self.write);
        let tx = conn.transaction()?;
        let mut ids = Vec::with_capacity(batch.len());
        {
            let mut upsert = tx.prepare_cached(
                "INSERT INTO files (path, root, relative, size, mtime, trigrams)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (path) DO UPDATE SET root = excluded.root,
                    relative = excluded.relative, size = excluded.size,
                    mtime = excluded.mtime, trigrams = excluded.trigrams
                 RETURNING id",
            )?;
            let mut clear_symbols = tx.prepare_cached("DELETE FROM symbols WHERE file = ?1")?;
            let mut clear_todos = tx.prepare_cached("DELETE FROM todos WHERE file = ?1")?;
            let mut clear_trigrams = tx.prepare_cached("DELETE FROM trigrams WHERE file = ?1")?;
            let mut add_symbol = tx.prepare_cached(
                "INSERT INTO symbols (file, name, kind, sel_line, sel_col, start_line, start_col,
                    end_line, end_col, container) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            let mut add_todo = tx.prepare_cached(
                "INSERT INTO todos (file, line, kind, text) VALUES (?1, ?2, ?3, ?4)",
            )?;
            let mut add_trigram =
                tx.prepare_cached("INSERT OR IGNORE INTO trigrams (tri, file) VALUES (?1, ?2)")?;
            for (file, data) in batch {
                let id: i64 = upsert.query_row(
                    params![
                        text(&file.path),
                        text(&file.root),
                        file.relative,
                        file.size as i64,
                        file.mtime,
                        data.trigrams.is_some() as i64
                    ],
                    |r| r.get(0),
                )?;
                clear_symbols.execute([id])?;
                clear_todos.execute([id])?;
                clear_trigrams.execute([id])?;
                for s in &data.symbols {
                    add_symbol.execute(params![
                        id,
                        s.name,
                        kind_name(s.kind),
                        s.selection.line,
                        s.selection.column,
                        s.range.start.line,
                        s.range.start.column,
                        s.range.end.line,
                        s.range.end.column,
                        s.container
                    ])?;
                }
                for t in &data.todos {
                    add_todo.execute(params![id, t.line, todo_kind_name(t.kind), t.text])?;
                }
                for tri in data.trigrams.iter().flatten() {
                    add_trigram.execute(params![*tri as i64, id])?;
                }
                ids.push(id);
            }
        }
        tx.commit()?;
        Ok(ids)
    }

    /// Forget files (and everything indexed for them).
    pub fn remove(&self, ids: &[i64]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut conn = lock(&self.write);
        let tx = conn.transaction()?;
        {
            let mut trigrams = tx.prepare_cached("DELETE FROM trigrams WHERE file = ?1")?;
            let mut files = tx.prepare_cached("DELETE FROM files WHERE id = ?1")?;
            for id in ids {
                trigrams.execute([id])?;
                files.execute([id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Ids of the files containing every trigram of `trigrams`.
    pub fn files_with_trigrams(&self, trigrams: &[u32]) -> Result<Vec<i64>> {
        let conn = lock(&self.read);
        let placeholders = vec!["?"; trigrams.len()].join(",");
        let sql = format!(
            "SELECT file FROM trigrams WHERE tri IN ({placeholders})
             GROUP BY file HAVING COUNT(*) = {}",
            trigrams.len()
        );
        let mut stmt = conn.prepare(&sql)?;
        let ids = stmt
            .query_map(
                rusqlite::params_from_iter(trigrams.iter().map(|t| *t as i64)),
                |r| r.get(0),
            )?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(ids)
    }
}
