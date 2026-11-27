//! Run configurations stored as committable TOML files in
//! `<project>/.ferroide/run/<id>.toml`.
//!
//! One file per configuration keeps diffs and merge conflicts small and
//! lets a team share some configurations and ignore others. The id is the
//! file stem; [`sanitize_id`] keeps it a portable file name. A file looks
//! like this (empty fields are left out when saving):
//!
//! ```toml
//! name = "demo-app"
//! kind = "cargo"
//! command = "cargo run"
//! arguments = "-- --names Ada"
//! working_directory = "app"
//! env_file = ".env"
//! before_launch = ["build-web"]
//! compound = []
//!
//! [environment]
//! RUST_LOG = "info"
//! ```
//!
//! `[environment]` is written sorted by name. The folder is re-read when
//! its listing (names, sizes, modification times) changes, so edits from
//! outside — a `git pull`, another editor — show up without a file
//! watcher; [`ConfigStore::refresh`] says whether anything changed since
//! the previous call. Malformed files are skipped with a warning.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use anyhow::{Context, Result};
use ferro_services::RunConfiguration;
use serde::{Deserialize, Serialize};

/// Where configurations live, relative to the project root.
pub const RUN_DIR: &str = ".ferroide/run";

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct FileConfig {
    name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    command: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    arguments: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    working_directory: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    env_file: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    before_launch: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    compound: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    environment: BTreeMap<String, String>,
}

/// The TOML text of `config` (its id is the file name, not stored).
pub fn to_toml(config: &RunConfiguration) -> Result<String> {
    let file = FileConfig {
        name: config.name.clone(),
        kind: config.kind.clone(),
        command: config.command.clone(),
        arguments: config.arguments.clone(),
        working_directory: config.working_directory.clone(),
        env_file: config.env_file.clone(),
        before_launch: config.before_launch.clone(),
        compound: config.compound.clone(),
        environment: config.environment.iter().cloned().collect(),
    };
    Ok(toml::to_string_pretty(&file)?)
}

/// The configuration `id` from its TOML text.
pub fn from_toml(id: &str, text: &str) -> Result<RunConfiguration> {
    let file: FileConfig = toml::from_str(text)?;
    Ok(RunConfiguration {
        id: id.to_string(),
        name: if file.name.trim().is_empty() {
            id.to_string()
        } else {
            file.name
        },
        kind: file.kind,
        command: file.command,
        arguments: file.arguments,
        working_directory: file.working_directory,
        environment: file.environment.into_iter().collect(),
        env_file: file.env_file,
        before_launch: file.before_launch,
        compound: file.compound,
        temporary: false,
    })
}

/// A file-name-safe id: letters, digits, `-`, `_` and `.` kept, runs of
/// anything else become one `-`.
pub fn sanitize_id(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for c in id.trim().chars() {
        if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches(['-', '.']).to_string();
    // Reserved device names on Windows.
    let upper = out.to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&upper.as_str())
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.ends_with(|c: char| c.is_ascii_digit()));
    if reserved { format!("{out}-run") } else { out }
}

type Stamp = Vec<(String, SystemTime, u64)>;

#[derive(Default)]
struct Cache {
    stamp: Option<Stamp>,
    configs: Vec<RunConfiguration>,
}

/// The saved configurations of one project.
pub struct ConfigStore {
    dir: PathBuf,
    cache: Mutex<Cache>,
}

impl ConfigStore {
    pub fn new(root: &Path) -> Self {
        Self {
            dir: root.join(RUN_DIR),
            cache: Mutex::new(Cache::default()),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.toml"))
    }

    fn stamp(&self) -> Stamp {
        let mut stamp: Stamp = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.ends_with(".toml") {
                    return None;
                }
                let meta = entry.metadata().ok()?;
                meta.is_file().then(|| {
                    (
                        name,
                        meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                        meta.len(),
                    )
                })
            })
            .collect();
        stamp.sort();
        stamp
    }

    fn load(&self, stamp: &Stamp) -> Vec<RunConfiguration> {
        let mut configs = Vec::new();
        for (name, _, _) in stamp {
            let id = name.trim_end_matches(".toml");
            let path = self.dir.join(name);
            match std::fs::read_to_string(&path)
                .map_err(anyhow::Error::from)
                .and_then(|text| from_toml(id, &text))
            {
                Ok(config) => configs.push(config),
                Err(e) => {
                    tracing::warn!(path = %path.display(), "skipping a run configuration: {e}")
                }
            }
        }
        configs.sort_by_key(|c| c.name.to_lowercase());
        configs
    }

    /// The saved configurations, re-read if the folder changed; and
    /// whether they changed since the previous call (never on the first).
    pub fn refresh(&self) -> (Vec<RunConfiguration>, bool) {
        let stamp = self.stamp();
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.stamp.as_ref() == Some(&stamp) {
            return (cache.configs.clone(), false);
        }
        let first = cache.stamp.is_none();
        let configs = self.load(&stamp);
        let changed = !first && configs != cache.configs;
        cache.configs = configs.clone();
        cache.stamp = Some(stamp);
        (configs, changed)
    }

    /// Write `config` as `<id>.toml` (the id must be sanitized).
    pub fn save(&self, config: &RunConfiguration) -> Result<()> {
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("cannot create {}", self.dir.display()))?;
        let path = self.path(&config.id);
        let text = to_toml(config)?;
        // Write next to the file and rename, so a reader never sees half
        // a file.
        let tmp = self.dir.join(format!(".{}.toml.tmp", config.id));
        std::fs::write(&tmp, text).with_context(|| format!("cannot write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("cannot write {}", path.display()))?;
        self.reload();
        Ok(())
    }

    /// Remove `<id>.toml`; whether there was one.
    pub fn delete(&self, id: &str) -> Result<bool> {
        let path = self.path(id);
        if !path.is_file() {
            return Ok(false);
        }
        std::fs::remove_file(&path).with_context(|| format!("cannot delete {}", path.display()))?;
        self.reload();
        Ok(true)
    }

    /// Take the folder's current state as known: own writes are not
    /// reported as outside changes.
    fn reload(&self) {
        let stamp = self.stamp();
        let configs = self.load(&stamp);
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.configs = configs;
        cache.stamp = Some(stamp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> RunConfiguration {
        RunConfiguration {
            id: "app".into(),
            name: "demo-app".into(),
            kind: "cargo".into(),
            command: "cargo run".into(),
            arguments: "-- --names \"Ada Lovelace\"".into(),
            working_directory: "app".into(),
            environment: vec![
                ("A_FIRST".into(), "1".into()),
                ("RUST_LOG".into(), "info".into()),
            ],
            env_file: ".env".into(),
            before_launch: vec!["build".into()],
            compound: Vec::new(),
            temporary: false,
        }
    }

    #[test]
    fn round_trips_through_toml() {
        let config = sample();
        let text = to_toml(&config).unwrap();
        assert!(text.contains("name = \"demo-app\""), "{text}");
        assert!(text.contains("[environment]"), "{text}");
        assert!(!text.contains("compound"), "{text}");
        assert_eq!(from_toml("app", &text).unwrap(), config);
    }

    #[test]
    fn sanitizes_ids() {
        assert_eq!(
            sanitize_id("temp:test greets_by_name"),
            "temp-test-greets_by_name"
        );
        assert_eq!(sanitize_id("  ../web/dev  "), "web-dev");
        assert_eq!(sanitize_id("con"), "con-run");
        assert_eq!(sanitize_id("Żółw 1"), "Żółw-1");
        assert_eq!(sanitize_id("::"), "");
    }

    #[test]
    fn stores_and_notices_external_edits() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(dir.path());
        assert_eq!(store.refresh(), (Vec::new(), false));

        store.save(&sample()).unwrap();
        let (configs, changed) = store.refresh();
        assert_eq!(configs, [sample()]);
        assert!(!changed, "own writes are not outside changes");
        assert!(dir.path().join(".ferroide/run/app.toml").is_file());

        // Edited outside: a new file, a malformed one, and a changed one.
        let run = dir.path().join(RUN_DIR);
        std::fs::write(
            run.join("tests.toml"),
            "name = \"All tests\"\ncommand = \"cargo test\"\n",
        )
        .unwrap();
        std::fs::write(run.join("broken.toml"), "name = [unclosed").unwrap();
        std::fs::write(run.join("notes.txt"), "ignored").unwrap();
        let (configs, changed) = store.refresh();
        assert!(changed);
        let names: Vec<_> = configs.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["All tests", "demo-app"]);
        assert_eq!(configs[0].id, "tests");

        assert!(!store.refresh().1);
        assert!(store.delete("tests").unwrap());
        assert!(!store.delete("tests").unwrap());
        assert_eq!(store.refresh().0.len(), 1);
    }
}
