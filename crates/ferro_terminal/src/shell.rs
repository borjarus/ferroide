//! Shells: finding them on this machine, and starting one with shell
//! integration.
//!
//! Shell integration makes the shell mark its prompts and commands with
//! OSC 133 (`A` prompt, `B` command line, `C` output, `D;<code>` finished),
//! so the terminal knows command boundaries and exit codes. The scripts in
//! `integration/` are embedded at build time and written to
//! [`TerminalOptions::integration_dir`](crate::TerminalOptions) on first
//! use. Each one loads the user's own configuration first and only adds
//! hooks around it:
//!
//! - bash: `--rcfile` with a script that sources `~/.bashrc` (or imitates a
//!   login shell, which `--rcfile` cannot be) and adds `PROMPT_COMMAND`
//!   hooks and `PS0`;
//! - zsh: a `ZDOTDIR` whose startup files source the user's and add
//!   `precmd`/`preexec` hooks;
//! - fish: `--init-command` with `fish_preexec`/`fish_postexec` events and a
//!   wrapped `fish_prompt`;
//! - PowerShell: `-NoExit -Command` dot-sourcing a script that wraps the
//!   `prompt` function (exit codes from `$?`/`$LASTEXITCODE`) and Enter;
//!   the script is read into a script block, so the execution policy of
//!   Windows PowerShell does not block it;
//! - cmd: the `PROMPT` variable with `$E` escapes (prompts only, no exit
//!   codes).

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

use ferro_services::ShellInfo;

use crate::TerminalOptions;

const BASH: &str = include_str!("integration/bash.sh");
const ZSHENV: &str = include_str!("integration/zshenv.zsh");
const ZPROFILE: &str = include_str!("integration/zprofile.zsh");
const ZSHRC: &str = include_str!("integration/zshrc.zsh");
const FISH: &str = include_str!("integration/fish.fish");
const POWERSHELL: &str = include_str!("integration/powershell.ps1");

/// Shells found on this machine, the default first.
///
/// Windows: PowerShell 7, Windows PowerShell, cmd, Git Bash, WSL. Unix:
/// `$SHELL`, then bash, zsh, fish and sh from `PATH` or `/etc/shells`.
pub fn detect_shells() -> Vec<ShellInfo> {
    let mut shells = Vec::new();
    #[cfg(windows)]
    detect_windows(&mut shells);
    #[cfg(not(windows))]
    detect_unix(&mut shells);
    shells
}

#[cfg(windows)]
fn detect_windows(shells: &mut Vec<ShellInfo>) {
    let env_dir = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let system_root = env_dir("SystemRoot").unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let program_files =
        env_dir("ProgramFiles").unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));

    // PowerShell 7: the newest under Program Files, else on PATH.
    let pwsh = std::fs::read_dir(program_files.join("PowerShell"))
        .ok()
        .and_then(|dir| {
            let mut versions: Vec<PathBuf> = dir
                .flatten()
                .map(|e| e.path().join("pwsh.exe"))
                .filter(|p| p.is_file())
                .collect();
            versions.sort();
            versions.pop()
        })
        .or_else(|| find_in_path("pwsh.exe"));
    if let Some(program) = pwsh {
        shells.push(shell("pwsh", "PowerShell", program, &["-NoLogo"]));
    }

    let powershell = system_root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    if powershell.is_file() {
        shells.push(shell(
            "powershell",
            "Windows PowerShell",
            powershell,
            &["-NoLogo"],
        ));
    }

    let cmd = env_dir("ComSpec")
        .filter(|p| p.is_file())
        .unwrap_or_else(|| system_root.join(r"System32\cmd.exe"));
    if cmd.is_file() {
        shells.push(shell("cmd", "Command Prompt", cmd, &[]));
    }

    // Git Bash, not System32\bash.exe (that is WSL's launcher).
    let git_bash = [
        env_dir("ProgramFiles"),
        env_dir("ProgramFiles(x86)"),
        env_dir("LOCALAPPDATA").map(|p| p.join("Programs")),
    ]
    .into_iter()
    .flatten()
    .map(|dir| dir.join(r"Git\bin\bash.exe"))
    .chain(
        // `<Git>\cmd\git.exe` on PATH → `<Git>\bin\bash.exe`.
        find_in_path("git.exe").and_then(|git| Some(git.parent()?.parent()?.join(r"bin\bash.exe"))),
    )
    .find(|p| p.is_file());
    if let Some(program) = git_bash {
        shells.push(shell("bash", "Git Bash", program, &["--login", "-i"]));
    }

    let wsl = system_root.join(r"System32\wsl.exe");
    if wsl.is_file() {
        shells.push(shell("wsl", "WSL", wsl, &[]));
    }
}

#[cfg(not(windows))]
fn detect_unix(shells: &mut Vec<ShellInfo>) {
    let etc_shells: Vec<PathBuf> = std::fs::read_to_string("/etc/shells")
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|l| l.starts_with('/'))
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default();
    // Login shells on macOS, as Terminal.app starts them.
    let login: &[&str] = if cfg!(target_os = "macos") {
        &["-l"]
    } else {
        &[]
    };
    let mut add = |program: PathBuf| {
        let id = shell_kind(&program);
        if shells.iter().any(|s| s.id == id) || !program.is_file() {
            return;
        }
        let (name, args) = match id.as_str() {
            "bash" => ("Bash", login),
            "zsh" => ("Zsh", login),
            "fish" => ("Fish", login),
            "sh" => ("sh", &[][..]),
            other => (other, &[][..]),
        };
        let name = name.to_string();
        shells.push(shell(&id, &name, program, args));
    };
    if let Some(user_shell) = std::env::var_os("SHELL").filter(|s| !s.is_empty()) {
        add(PathBuf::from(user_shell));
    }
    for name in ["bash", "zsh", "fish", "sh"] {
        let found = find_in_path(name).or_else(|| {
            etc_shells
                .iter()
                .find(|p| p.file_name() == Some(OsStr::new(name)))
                .cloned()
        });
        if let Some(program) = found {
            add(program);
        }
    }
}

fn shell(id: &str, name: &str, program: PathBuf, args: &[&str]) -> ShellInfo {
    ShellInfo {
        id: id.to_string(),
        name: name.to_string(),
        program,
        args: args.iter().map(|a| a.to_string()).collect(),
    }
}

/// `name` in a directory of `PATH`.
fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// The kind of shell a program is (`pwsh`, `bash`, …): its file stem,
/// lowercase.
pub(crate) fn shell_kind(program: &Path) -> String {
    program
        .file_stem()
        .unwrap_or(OsStr::new(""))
        .to_string_lossy()
        .to_lowercase()
}

/// The shell to start for `requested` (a [`ShellInfo::id`] or a program
/// path; `None` is the default, the first detected).
pub(crate) fn resolve_shell(requested: Option<&str>, shells: &[ShellInfo]) -> Option<ShellInfo> {
    let requested = requested.map(str::trim).filter(|r| !r.is_empty());
    let Some(requested) = requested else {
        return shells.first().cloned().or_else(fallback_shell);
    };
    if let Some(found) = shells.iter().find(|s| s.id == requested) {
        return Some(found.clone());
    }
    // A program: a path, or a name on PATH.
    let path = PathBuf::from(requested);
    let program = if path.components().count() > 1 || path.is_absolute() {
        path.is_file().then_some(path)
    } else {
        find_in_path(requested).or_else(|| {
            cfg!(windows)
                .then(|| find_in_path(&format!("{requested}.exe")))
                .flatten()
        })
    }?;
    let id = shell_kind(&program);
    // Known shells keep their usual arguments.
    let args = shells
        .iter()
        .find(|s| s.id == id)
        .map(|s| s.args.clone())
        .unwrap_or_default();
    Some(ShellInfo {
        name: program
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| id.clone()),
        id,
        program,
        args,
    })
}

/// A shell when detection found none.
fn fallback_shell() -> Option<ShellInfo> {
    let program = if cfg!(windows) {
        PathBuf::from(std::env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into()))
    } else {
        PathBuf::from("/bin/sh")
    };
    let id = shell_kind(&program);
    Some(shell(&id.clone(), &id, program, &[]))
}

/// How to start a shell: program, arguments and extra environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// The shell will report its prompt (`OSC 133;B`).
    pub integrated: bool,
}

/// The command line and environment for `shell`, with shell integration
/// when enabled and supported. Failing to write the scripts only turns
/// integration off.
pub(crate) fn launch(shell: &ShellInfo, options: &TerminalOptions) -> Launch {
    let mut launch = Launch {
        program: shell.program.clone(),
        args: shell.args.clone(),
        env: vec![
            ("TERM".into(), "xterm-256color".into()),
            ("COLORTERM".into(), "truecolor".into()),
            ("TERM_PROGRAM".into(), "FerroIDE".into()),
            (
                "TERM_PROGRAM_VERSION".into(),
                env!("CARGO_PKG_VERSION").into(),
            ),
        ],
        integrated: false,
    };
    if options.shell_integration {
        let kind = shell_kind(&shell.program);
        match integrate(&kind, &mut launch, &options.integration_dir) {
            Ok(integrated) => launch.integrated = integrated,
            Err(err) => {
                tracing::warn!("shell integration for {kind} is off: {err:#}");
            }
        }
    }
    launch.env.extend(options.env.iter().cloned());
    launch
}

fn integrate(kind: &str, launch: &mut Launch, dir: &Path) -> anyhow::Result<bool> {
    match kind {
        "bash" => {
            let script = write_script(&dir.join("bash"), "ferro.bash", BASH)?;
            let login = launch.args.iter().any(|a| a == "-l" || a == "--login");
            if login {
                launch.env.push(("FERRO_BASH_LOGIN".into(), "1".into()));
            }
            launch.args = vec![
                "--rcfile".into(),
                // MSYS bash (Git Bash) takes forward slashes everywhere.
                shell_path(&script),
                "-i".into(),
            ];
        }
        "zsh" => {
            let zdotdir = dir.join("zsh");
            write_script(&zdotdir, ".zshenv", ZSHENV)?;
            write_script(&zdotdir, ".zprofile", ZPROFILE)?;
            write_script(&zdotdir, ".zshrc", ZSHRC)?;
            if let Some(user) = std::env::var_os("ZDOTDIR").filter(|d| !d.is_empty()) {
                launch.env.push((
                    "FERRO_USER_ZDOTDIR".into(),
                    user.to_string_lossy().into_owned(),
                ));
            }
            launch
                .env
                .push(("ZDOTDIR".into(), zdotdir.to_string_lossy().into_owned()));
        }
        "fish" => {
            let script = write_script(&dir.join("fish"), "ferro.fish", FISH)?;
            launch.args.push("--init-command".into());
            launch.args.push(format!(
                "source '{}'",
                shell_path(&script)
                    .replace('\\', "\\\\")
                    .replace('\'', "\\'")
            ));
        }
        "pwsh" | "powershell" => {
            let script = write_script(&dir.join("powershell"), "ferro.ps1", POWERSHELL)?;
            let path = script.to_string_lossy().replace('\'', "''");
            launch.args.extend([
                "-NoExit".into(),
                "-Command".into(),
                format!(". ([ScriptBlock]::Create([IO.File]::ReadAllText('{path}')))"),
            ]);
        }
        "cmd" => {
            let user = std::env::var("PROMPT").unwrap_or_else(|_| "$P$G".into());
            launch.env.push(("PROMPT".into(), cmd_prompt(&user)));
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// cmd's `PROMPT` with OSC 133 A/B around the user's prompt.
pub(crate) fn cmd_prompt(user: &str) -> String {
    format!("$E]133;A$E\\{user}$E]133;B$E\\")
}

/// A path as shells (also MSYS bash) read it.
fn shell_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.into_owned()
    }
}

/// Write `content` to `dir/name` unless it is already there.
fn write_script(dir: &Path, name: &str, content: &str) -> anyhow::Result<PathBuf> {
    use anyhow::Context as _;
    let path = dir.join(name);
    if std::fs::read_to_string(&path).is_ok_and(|current| current == content) {
        return Ok(path);
    }
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    // Through a temporary file, so a shell starting meanwhile never reads
    // half a script.
    let tmp = dir.join(format!("{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, content).with_context(|| format!("writing {}", tmp.display()))?;
    if let Err(err) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        // Another session wrote the same script at the same moment.
        if !std::fs::read_to_string(&path).is_ok_and(|current| current == content) {
            return Err(err).with_context(|| format!("writing {}", path.display()));
        }
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(dir: &Path) -> TerminalOptions {
        TerminalOptions {
            integration_dir: dir.to_path_buf(),
            env: vec![("EXTRA".into(), "1".into())],
            ..TerminalOptions::default()
        }
    }

    fn info(id: &str, program: &str, args: &[&str]) -> ShellInfo {
        shell(id, id, PathBuf::from(program), args)
    }

    #[test]
    fn detection_finds_a_default_shell() {
        let shells = detect_shells();
        assert!(!shells.is_empty(), "no shell found");
        for s in &shells {
            assert!(s.program.is_file(), "{s:?}");
        }
        let mut ids: Vec<_> = shells.iter().map(|s| s.id.as_str()).collect();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "duplicate ids: {shells:?}");
        if cfg!(windows) {
            assert!(shells.iter().any(|s| s.id == "cmd"));
            assert!(matches!(
                shells[0].id.as_str(),
                "pwsh" | "powershell" | "cmd"
            ));
        }
    }

    #[test]
    fn shells_resolve_by_id_or_program() {
        let shells = detect_shells();
        let default = resolve_shell(None, &shells).unwrap();
        assert_eq!(Some(&default), shells.first());
        let first = &shells[0];
        assert_eq!(
            resolve_shell(Some(&first.id), &shells).as_ref(),
            Some(first)
        );
        let by_path = resolve_shell(Some(&first.program.to_string_lossy()), &shells).unwrap();
        assert_eq!(by_path.id, shell_kind(&first.program));
        assert_eq!(by_path.program, first.program);
        assert_eq!(resolve_shell(Some("no-such-shell-xyz"), &shells), None);
        assert_eq!(resolve_shell(Some("  "), &shells).as_ref(), shells.first());
    }

    #[test]
    fn integration_scripts_are_written_and_wired() {
        let dir = tempfile::tempdir().unwrap();
        let options = options(dir.path());

        let bash = launch(&info("bash", "/bin/bash", &["--login", "-i"]), &options);
        assert!(bash.integrated);
        assert_eq!(bash.args[0], "--rcfile");
        assert_eq!(bash.args[2], "-i");
        let script = PathBuf::from(&bash.args[1]);
        assert_eq!(std::fs::read_to_string(script).unwrap(), BASH);
        assert!(bash.env.contains(&("FERRO_BASH_LOGIN".into(), "1".into())));
        assert!(bash.env.contains(&("TERM".into(), "xterm-256color".into())));
        assert!(bash.env.contains(&("COLORTERM".into(), "truecolor".into())));
        assert!(
            bash.env
                .contains(&("TERM_PROGRAM".into(), "FerroIDE".into()))
        );
        assert_eq!(bash.env.last().unwrap(), &("EXTRA".into(), "1".into()));

        let zsh = launch(&info("zsh", "/bin/zsh", &["-l"]), &options);
        assert!(zsh.integrated);
        assert_eq!(zsh.args, ["-l"]);
        let zdotdir = &zsh.env.iter().find(|(k, _)| k == "ZDOTDIR").unwrap().1;
        for name in [".zshenv", ".zprofile", ".zshrc"] {
            assert!(Path::new(zdotdir).join(name).is_file());
        }

        let fish = launch(&info("fish", "/usr/bin/fish", &[]), &options);
        assert_eq!(fish.args[0], "--init-command");
        assert!(fish.args[1].starts_with("source '"));

        let pwsh = launch(&info("pwsh", r"C:\pwsh\pwsh.exe", &["-NoLogo"]), &options);
        assert_eq!(pwsh.args[..3], ["-NoLogo", "-NoExit", "-Command"]);
        assert!(pwsh.args[3].contains("ferro.ps1"));
        assert!(pwsh.args[3].starts_with(". ([ScriptBlock]::Create("));

        let cmd = launch(&info("cmd", r"C:\Windows\System32\cmd.exe", &[]), &options);
        let prompt = &cmd.env.iter().find(|(k, _)| k == "PROMPT").unwrap().1;
        assert!(prompt.starts_with("$E]133;A$E\\"));
        assert!(prompt.ends_with("$E]133;B$E\\"));

        let other = launch(&info("nu", "/usr/bin/nu", &[]), &options);
        assert!(!other.integrated);
        assert!(other.args.is_empty());

        // Off: no scripts, arguments as given.
        let off = TerminalOptions {
            shell_integration: false,
            ..options
        };
        let plain = launch(&info("bash", "/bin/bash", &["-i"]), &off);
        assert!(!plain.integrated);
        assert_eq!(plain.args, ["-i"]);
    }

    #[test]
    fn unwritable_integration_dir_turns_integration_off() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        std::fs::write(&file, "x").unwrap();
        // A directory below a file cannot be created.
        let options = options(&file.join("sub"));
        let bash = launch(&info("bash", "/bin/bash", &[]), &options);
        assert!(!bash.integrated);
        assert!(bash.args.is_empty());
    }

    #[test]
    fn cmd_prompt_wraps_the_users_prompt() {
        assert_eq!(cmd_prompt("$P$G"), "$E]133;A$E\\$P$G$E]133;B$E\\");
    }
}
