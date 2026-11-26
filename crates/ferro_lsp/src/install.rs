//! Finding and installing language servers (plan §7.3 "Tool management").
//!
//! A server's command is looked up in its own folder under FerroIDE's data
//! directory first (`<data>/servers/<id>/…`, where the installer puts it),
//! then on `PATH`. The installer runs the ecosystem's package manager into
//! that folder: `npm`, `pip` (in a virtual environment), `go install`,
//! `cargo install`, `gem`, `dotnet tool` or `rustup`. Servers distributed
//! only as GitHub release archives are not downloaded yet; their install
//! hint names the release page.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context as _, bail};
use ferro_languages::manifest::LanguageServerManifest;

fn executable_names(command: &str) -> Vec<String> {
    if cfg!(windows) && Path::new(command).extension().is_none() {
        vec![
            format!("{command}.exe"),
            format!("{command}.cmd"),
            format!("{command}.bat"),
            command.to_string(),
        ]
    } else {
        vec![command.to_string()]
    }
}

/// Where an installed server keeps its executables.
fn candidate_dirs(server_dir: &Path) -> Vec<PathBuf> {
    vec![
        server_dir.join("bin"),
        server_dir.join("node_modules").join(".bin"),
        server_dir
            .join("venv")
            .join(if cfg!(windows) { "Scripts" } else { "bin" }),
        server_dir.to_path_buf(),
    ]
}

/// The executable to run for `command`: an existing path, the server's
/// install folder, or `PATH`.
pub fn resolve_command(
    command: &str,
    server_id: &str,
    install_dir: Option<&Path>,
) -> Option<PathBuf> {
    let path = Path::new(command);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    if let Some(dir) = install_dir {
        for dir in candidate_dirs(&dir.join(server_id)) {
            for name in executable_names(command) {
                let candidate = dir.join(&name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    which::which(command).ok()
}

/// One step of an installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallStep {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl InstallStep {
    fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
            env: Vec::new(),
        }
    }

    pub fn display(&self) -> String {
        std::iter::once(self.program.clone())
            .chain(self.args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// How to install a server, or a hint when FerroIDE cannot do it itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallPlan {
    Steps(Vec<InstallStep>),
    Manual(String),
}

/// The install plan of a server into `<install_dir>/<id>`.
pub fn plan(server: &LanguageServerManifest, install_dir: &Path) -> InstallPlan {
    let dir = install_dir.join(&server.id);
    let dir_text = dir.to_string_lossy().into_owned();
    let get = |key: &str| server.install.get(key).map(String::as_str);
    if let Some(component) = get("rustup") {
        return InstallPlan::Steps(vec![InstallStep::new(
            "rustup",
            &["component", "add", component],
        )]);
    }
    if let Some(package) = get("npm") {
        return InstallPlan::Steps(vec![InstallStep::new(
            "npm",
            &[
                "install",
                "--prefix",
                &dir_text,
                "--no-save",
                "--no-audit",
                "--no-fund",
                package,
            ],
        )]);
    }
    if let Some(package) = get("pypi") {
        let venv = dir.join("venv");
        let python = if cfg!(windows) { "python" } else { "python3" };
        let pip = venv
            .join(if cfg!(windows) { "Scripts" } else { "bin" })
            .join(if cfg!(windows) { "pip.exe" } else { "pip" });
        return InstallPlan::Steps(vec![
            InstallStep::new(python, &["-m", "venv", &venv.to_string_lossy()]),
            InstallStep::new(&pip.to_string_lossy(), &["install", "--upgrade", package]),
        ]);
    }
    if let Some(package) = get("go") {
        let mut step = InstallStep::new("go", &["install", package]);
        step.env.push((
            "GOBIN".into(),
            dir.join("bin").to_string_lossy().into_owned(),
        ));
        return InstallPlan::Steps(vec![step]);
    }
    if let Some(krate) = get("cargo") {
        return InstallPlan::Steps(vec![InstallStep::new(
            "cargo",
            &["install", "--root", &dir_text, krate],
        )]);
    }
    if let Some(gem) = get("gem") {
        return InstallPlan::Steps(vec![InstallStep::new(
            "gem",
            &[
                "install",
                "--install-dir",
                &dir_text,
                "--bindir",
                &dir.join("bin").to_string_lossy(),
                gem,
            ],
        )]);
    }
    if let Some(tool) = get("nuget") {
        return InstallPlan::Steps(vec![InstallStep::new(
            "dotnet",
            &[
                "tool",
                "install",
                "--tool-path",
                &dir.join("bin").to_string_lossy(),
                tool,
            ],
        )]);
    }
    if let Some(repo) = get("github") {
        return InstallPlan::Manual(format!(
            "Download {} from https://github.com/{repo}/releases and put it on PATH or in {}",
            server.command,
            dir.join("bin").display()
        ));
    }
    InstallPlan::Manual(format!("Install {} and put it on PATH", server.command))
}

/// Run an install plan; blocking. Returns the combined output on failure.
pub fn install(server: &LanguageServerManifest, install_dir: &Path) -> anyhow::Result<()> {
    let steps = match plan(server, install_dir) {
        InstallPlan::Steps(steps) => steps,
        InstallPlan::Manual(hint) => bail!(hint),
    };
    std::fs::create_dir_all(install_dir.join(&server.id))
        .with_context(|| format!("cannot create {}", install_dir.display()))?;
    for step in steps {
        let program = which::which(&step.program).unwrap_or_else(|_| PathBuf::from(&step.program));
        let output = Command::new(&program)
            .args(&step.args)
            .envs(step.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("cannot run {}", step.program))?;
        if !output.status.success() {
            bail!(
                "`{}` failed:\n{}{}",
                step.display(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn server(id: &str, key: &str, value: &str) -> LanguageServerManifest {
        LanguageServerManifest {
            id: id.into(),
            command: id.into(),
            args: Vec::new(),
            install: BTreeMap::from([(key.to_string(), value.to_string())]),
            features: Vec::new(),
        }
    }

    #[test]
    fn plans_install_into_the_server_folder() {
        let dir = Path::new("data").join("servers");
        let InstallPlan::Steps(steps) =
            plan(&server("vtsls", "npm", "@vtsls/language-server"), &dir)
        else {
            panic!("npm is installable");
        };
        assert_eq!(steps[0].program, "npm");
        assert!(
            steps[0]
                .args
                .contains(&"@vtsls/language-server".to_string())
        );
        assert!(steps[0].args.iter().any(|a| a.ends_with("vtsls")));

        let InstallPlan::Steps(steps) = plan(
            &server("gopls", "go", "golang.org/x/tools/gopls@latest"),
            &dir,
        ) else {
            panic!("go is installable");
        };
        assert_eq!(steps[0].env[0].0, "GOBIN");
        let InstallPlan::Steps(steps) = plan(&server("ruff", "pypi", "ruff"), &dir) else {
            panic!("pypi is installable");
        };
        assert_eq!(steps.len(), 2, "venv, then pip");
        assert!(matches!(
            plan(&server("clangd", "github", "clangd/clangd"), &dir),
            InstallPlan::Manual(hint) if hint.contains("github.com/clangd/clangd/releases")
        ));
    }

    #[test]
    fn installed_servers_are_found_before_path() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("fake-ls").join("node_modules").join(".bin");
        std::fs::create_dir_all(&bin).unwrap();
        let name = if cfg!(windows) {
            "fake-ls.cmd"
        } else {
            "fake-ls"
        };
        std::fs::write(bin.join(name), "").unwrap();
        assert_eq!(
            resolve_command("fake-ls", "fake-ls", Some(dir.path())),
            Some(bin.join(name))
        );
        assert_eq!(
            resolve_command("surely-not-installed-ls", "x", Some(dir.path())),
            None
        );
    }
}
