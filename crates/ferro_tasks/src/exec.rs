//! From a run configuration to a process launch: working directory,
//! environment, test framework arguments and the program to start.
//!
//! The environment is the IDE's own, then the service-wide extras
//! ([`RunOptions::env`](crate::RunOptions)), then the `env_file`, then the
//! configuration's `environment` — each layer overriding the previous.
//! Colour is forced (`CARGO_TERM_COLOR`, `FORCE_COLOR`, `CLICOLOR_FORCE`,
//! `PY_COLORS`) because output goes to a pipe, which most tools take as
//! "no terminal, no colours", while the console renders ANSI; a
//! configuration setting one of them itself wins.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ferro_services::RunConfiguration;

use crate::command::{join, resolve_program, split};
use crate::dotenv;
use crate::process::Launch;
use crate::testing::{TestSession, detect};

const FORCE_COLORS: &[(&str, &str)] = &[
    ("CARGO_TERM_COLOR", "always"),
    ("FORCE_COLOR", "1"),
    ("CLICOLOR_FORCE", "1"),
    ("PY_COLORS", "1"),
];

/// Where `config` runs: its `working_directory` against the project root.
pub fn working_directory(config: &RunConfiguration, root: &Path) -> PathBuf {
    let dir = config.working_directory.trim();
    if dir.is_empty() || dir == "." {
        root.to_path_buf()
    } else {
        root.join(dir)
    }
}

/// The variables set for `config` on top of the inherited environment.
pub fn environment(
    config: &RunConfiguration,
    root: &Path,
    extra: &[(String, String)],
) -> Result<Vec<(String, String)>> {
    let mut env: Vec<(String, String)> = Vec::new();
    let set = |env: &mut Vec<(String, String)>, key: &str, value: &str| {
        env.retain(|(k, _)| !same_key(k, key));
        env.push((key.to_string(), value.to_string()));
    };
    for (k, v) in extra {
        set(&mut env, k, v);
    }
    let mut own: Vec<(String, String)> = Vec::new();
    if !config.env_file.trim().is_empty() {
        let path = root.join(config.env_file.trim());
        let snapshot = env.clone();
        let lookup = move |name: &str| {
            snapshot
                .iter()
                .rev()
                .find(|(k, _)| same_key(k, name))
                .map(|(_, v)| v.clone())
                .or_else(|| std::env::var(name).ok())
        };
        own.extend(dotenv::load(&path, lookup).context("cannot load the env file")?);
    }
    own.extend(config.environment.iter().cloned());
    for (key, value) in FORCE_COLORS {
        if !own.iter().any(|(k, _)| same_key(k, key)) {
            set(&mut env, key, value);
        }
    }
    for (k, v) in &own {
        set(&mut env, k, v);
    }
    Ok(env)
}

/// Environment names compare case-insensitively on Windows.
fn same_key(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// Everything needed to start one configuration's process.
pub struct Prepared {
    pub launch: Launch,
    /// The command line shown as the first console line.
    pub display: String,
    /// When the command runs tests.
    pub tests: Option<TestSession>,
}

/// Resolve `config` into a launch. Errors are the configuration's
/// problems (no command, a missing folder or program, a bad env file).
pub fn prepare(
    config: &RunConfiguration,
    root: &Path,
    extra_env: &[(String, String)],
    collect_tests: bool,
) -> Result<Prepared> {
    let line = config.command_line();
    let words = split(&line);
    if words.is_empty() {
        bail!("{} has no command", config.name);
    }
    let cwd = working_directory(config, root);
    if !cwd.is_dir() {
        bail!("working directory {} does not exist", cwd.display());
    }
    let env = environment(config, root, extra_env)?;
    let (tests, words) = match collect_tests.then(|| detect(&line, &cwd)).flatten() {
        Some(detected) => {
            let (session, words) = TestSession::new(detected, &words, &cwd)
                .context("cannot create a folder for test reports")?;
            (Some(session), words)
        }
        None => (None, words),
    };
    let path_var = env
        .iter()
        .rev()
        .find(|(k, _)| same_key(k, "PATH"))
        .map(|(_, v)| std::ffi::OsString::from(v));
    let program = resolve_program(&words[0], &cwd, root, path_var.as_deref())
        .with_context(|| format!("cannot find program `{}`", words[0]))?;
    let args = words[1..].to_vec();
    let mut shown = vec![program.to_string_lossy().into_owned()];
    shown.extend(args.iter().cloned());
    Ok(Prepared {
        display: join(&shown),
        launch: Launch {
            program,
            args,
            cwd,
            env,
        },
        tests,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_the_environment() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".env"),
            "FROM_FILE=file\nOVERRIDDEN=file\nFORCE_COLOR=0\nUSES_EXTRA=${EXTRA}-x\n",
        )
        .unwrap();
        let config = RunConfiguration {
            env_file: ".env".into(),
            environment: vec![("OVERRIDDEN".into(), "config".into())],
            ..Default::default()
        };
        let env = environment(&config, dir.path(), &[("EXTRA".into(), "extra".into())]).unwrap();
        let get = |k: &str| {
            env.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("EXTRA"), Some("extra"));
        assert_eq!(get("FROM_FILE"), Some("file"));
        assert_eq!(get("OVERRIDDEN"), Some("config"));
        assert_eq!(get("FORCE_COLOR"), Some("0"));
        assert_eq!(get("CARGO_TERM_COLOR"), Some("always"));
        assert_eq!(get("USES_EXTRA"), Some("extra-x"));

        let missing = RunConfiguration {
            env_file: "nope.env".into(),
            ..Default::default()
        };
        assert!(environment(&missing, dir.path(), &[]).is_err());
    }

    #[test]
    fn prepare_reports_configuration_problems() {
        let dir = tempfile::tempdir().unwrap();
        let empty = RunConfiguration {
            name: "x".into(),
            ..Default::default()
        };
        assert!(prepare(&empty, dir.path(), &[], true).is_err());
        let missing_dir = RunConfiguration {
            name: "x".into(),
            command: "cargo build".into(),
            working_directory: "nope".into(),
            ..Default::default()
        };
        assert!(prepare(&missing_dir, dir.path(), &[], true).is_err());
        let missing_program = RunConfiguration {
            name: "x".into(),
            command: "definitely-not-a-program-ferroide".into(),
            ..Default::default()
        };
        let err = prepare(&missing_program, dir.path(), &[], true)
            .err()
            .unwrap();
        assert!(err.to_string().contains("cannot find program"));
        let cargo = RunConfiguration {
            name: "x".into(),
            command: "cargo test".into(),
            arguments: "-- --nocapture".into(),
            ..Default::default()
        };
        let prepared = prepare(&cargo, dir.path(), &[], true).unwrap();
        assert_eq!(prepared.launch.args, ["test", "--", "--nocapture"]);
        assert!(prepared.tests.is_some());
        assert!(prepared.display.ends_with("test -- --nocapture"));
    }
}
