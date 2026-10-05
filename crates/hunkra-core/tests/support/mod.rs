//! Disposable Git fixtures shared by independently checked repository services.
use hunkra_core::{DiffKind, DiffSnapshot, read_diff};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use tempfile::TempDir;

pub(crate) struct Repo {
    dir: TempDir,
}
impl Repo {
    /// Creates an isolated repository with no global configuration or user hooks.
    pub(crate) fn new() -> Self {
        Self::with_format("sha1")
    }
    /// Initializes the object format before Git creates any files or objects.
    pub(crate) fn with_format(format: &str) -> Self {
        let repo = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        fs::create_dir(repo.path().join("hooks")).unwrap();
        repo.git(&[
            "init",
            "--initial-branch=main",
            &format!("--object-format={format}"),
        ]);
        for (key, value) in [
            ("user.name", "Hunkra Test"),
            ("user.email", "test@example.invalid"),
            ("core.autocrlf", "false"),
        ] {
            repo.git(&["config", "--local", key, value]);
        }
        repo.git(&[
            "config",
            "--local",
            "core.hooksPath",
            repo.path().join("hooks").to_str().unwrap(),
        ]);
        repo
    }
    /// Returns only this test's disposable root.
    pub(crate) fn path(&self) -> &Path {
        self.dir.path()
    }
    /// Runs Git without inherited repository/config/credential environment.
    pub(crate) fn command(&self, args: &[&str]) -> Output {
        Command::new("/usr/bin/git")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .current_dir(self.path())
            .args([
                "-c",
                "user.name=Hunkra Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
            ])
            .arg(format!(
                "core.hooksPath={}",
                self.path().join("hooks").display()
            ))
            .args(args)
            .output()
            .unwrap()
    }
    /// Requires success from an independent Git command and returns exact stdout bytes.
    pub(crate) fn git(&self, args: &[&str]) -> Vec<u8> {
        let output = self.command(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }
    /// Writes fixture bytes without normalizing them.
    pub(crate) fn write(&self, path: &str, bytes: impl AsRef<[u8]>) {
        fs::write(self.path().join(path), bytes).unwrap();
    }
    /// Commits fixture setup only in the temporary repository.
    pub(crate) fn initial(&self) {
        self.git(&["add", "."]);
        self.git(&["commit", "-m", "baseline"]);
    }
    /// Reads one file through the production gix path.
    pub(crate) fn diff(&self, file: &str, kind: DiffKind) -> DiffSnapshot {
        read_diff(self.path(), Path::new(file), kind).unwrap()
    }
}
