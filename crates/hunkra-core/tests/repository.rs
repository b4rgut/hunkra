//! Independent Git oracles. No test writes to Hunkra or a user repository.
use hunkra_core::{ChangeKind, DiffKind, DiffSnapshot, Operation, Presentation, read_diff, scan};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use tempfile::TempDir;

struct Repo {
    dir: TempDir,
}
impl Repo {
    /// Creates an isolated repository with no global configuration or user hooks.
    fn new() -> Self {
        let repo = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        fs::create_dir(repo.path().join("hooks")).unwrap();
        repo.git(&["init", "--initial-branch=main"]);
        repo
    }
    /// Returns only this test's disposable root.
    fn path(&self) -> &Path {
        self.dir.path()
    }
    /// Runs Git without inherited repository/config/credential environment.
    fn command(&self, args: &[&str]) -> Output {
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
    fn git(&self, args: &[&str]) -> Vec<u8> {
        let output = self.command(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }
    /// Writes fixture bytes without normalizing them.
    fn write(&self, path: &str, bytes: impl AsRef<[u8]>) {
        fs::write(self.path().join(path), bytes).unwrap();
    }
    /// Commits fixture setup only in the temporary repository.
    fn initial(&self) {
        self.git(&["add", "."]);
        self.git(&["commit", "-m", "baseline"]);
    }
    /// Reads one file through the production gix path.
    fn diff(&self, file: &str, kind: DiffKind) -> DiffSnapshot {
        read_diff(self.path(), Path::new(file), kind).unwrap()
    }
}

/// Reconstructs exact versions from aligned rows, including newline markers.
fn reconstruct(diff: &DiffSnapshot, left: bool) -> Vec<u8> {
    let mut result = Vec::new();
    for row in &diff.rows {
        let side = if left { &row.left } else { &row.right };
        if let Some(line) = side {
            result.extend(line.text.as_bytes());
            result.extend(match line.ending {
                "CRLF" => b"\r\n".as_slice(),
                "LF" => b"\n".as_slice(),
                _ => b"".as_slice(),
            });
        }
    }
    result
}

#[test]
/// Proves staged and unstaged comparisons have distinct bases, preserving index bytes.
fn staged_and_unstaged_have_exact_sources() {
    let repo = Repo::new();
    let file = "Имя с пробелом.bsl";
    repo.write(file, "A\nunchanged\nB\nunchanged\nC\n");
    repo.initial();
    repo.write(file, "A staged\nunchanged\nB\nunchanged\nC\n");
    repo.git(&["add", file]);
    repo.write(
        file,
        "A staged\nunchanged\nB worktree\nunchanged\nC worktree\n",
    );
    let index_before = fs::read(repo.path().join(".git/index")).unwrap();
    let snapshot = scan(repo.path()).unwrap();
    assert_eq!(snapshot.backend, "gix");
    assert_eq!(snapshot.branch.as_deref(), Some("main"));
    let status = snapshot
        .files
        .iter()
        .find(|status| status.path == Path::new(file))
        .unwrap();
    assert_eq!(status.staged, Some(ChangeKind::Modified));
    assert_eq!(status.unstaged, Some(ChangeKind::Modified));
    for kind in [DiffKind::Staged, DiffKind::Unstaged] {
        let diff = repo.diff(file, kind);
        assert_eq!(diff.backend, "gix");
        assert_eq!(diff.presentation, Presentation::Text);
        let head = repo.git(&["show", &format!("HEAD:{file}")]);
        let index = repo.git(&["show", &format!(":{file}")]);
        assert_eq!(
            diff.left.bytes,
            if kind == DiffKind::Staged {
                head
            } else {
                index.clone()
            }
        );
        assert_eq!(
            diff.right.bytes,
            if kind == DiffKind::Staged {
                index
            } else {
                fs::read(repo.path().join(file)).unwrap()
            }
        );
        assert_eq!(reconstruct(&diff, true), diff.left.bytes);
        assert_eq!(reconstruct(&diff, false), diff.right.bytes);
        assert!(!diff.hunks.is_empty());
    }
    assert_eq!(
        index_before,
        fs::read(repo.path().join(".git/index")).unwrap()
    );
}

#[test]
/// Covers unborn HEAD, new files, staged additions and both deletion modes.
fn unborn_new_and_deleted_files() {
    let repo = Repo::new();
    assert!(scan(repo.path()).unwrap().unborn);
    repo.write("new.txt", "new\n");
    let diff = repo.diff("new.txt", DiffKind::Unstaged);
    assert!(!diff.left.present);
    assert_eq!(diff.right.bytes, b"new\n");
    repo.git(&["add", "new.txt"]);
    let diff = repo.diff("new.txt", DiffKind::Staged);
    assert!(!diff.left.present);
    assert_eq!(diff.right.bytes, b"new\n");
    repo.git(&["commit", "-m", "baseline"]);
    fs::remove_file(repo.path().join("new.txt")).unwrap();
    let diff = repo.diff("new.txt", DiffKind::Unstaged);
    assert_eq!(diff.left.bytes, b"new\n");
    assert!(!diff.right.present);
    repo.git(&["add", "-u"]);
    let diff = repo.diff("new.txt", DiffKind::Staged);
    assert!(diff.left.present);
    assert!(!diff.right.present);
    assert_eq!(
        scan(repo.path()).unwrap().files[0].staged,
        Some(ChangeKind::Deleted)
    );
}

#[test]
/// Ensures CRLF, BOM and no-final-newline survive unfiltered viewing byte for byte.
fn byte_preservation_and_alignment() {
    let repo = Repo::new();
    let before = b"\xef\xbb\xbfhead\r\nline\r\ntail";
    let after = b"\xef\xbb\xbfhead\r\nchanged\r\ntail!";
    repo.write("raw.xml", before);
    repo.initial();
    repo.write("raw.xml", after);
    let diff = repo.diff("raw.xml", DiffKind::Unstaged);
    assert_eq!(diff.left.bytes, before);
    assert_eq!(diff.right.bytes, after);
    assert_eq!(reconstruct(&diff, true), before);
    assert_eq!(reconstruct(&diff, false), after);
    assert_eq!(fs::read(repo.path().join("raw.xml")).unwrap(), after);
}

#[test]
/// Compares worktree bytes in Git representation with the official gix filter pipeline.
fn attributes_normalize_without_mutating_worktree() {
    let repo = Repo::new();
    repo.write(".gitattributes", "*.txt text eol=crlf\n");
    repo.write("file.txt", "a\r\nb\r\n");
    repo.initial();
    repo.write("file.txt", "a\r\nchanged\r\n");
    let diff = repo.diff("file.txt", DiffKind::Unstaged);
    assert_eq!(diff.left.bytes, b"a\nb\n");
    assert_eq!(diff.right.bytes, b"a\nchanged\n");
    assert_eq!(
        fs::read(repo.path().join("file.txt")).unwrap(),
        b"a\r\nchanged\r\n"
    );
}

#[test]
/// Avoids text allocation for binary/invalid UTF-8/large files and handles mode-only changes.
fn safe_limits_and_modes() {
    let repo = Repo::new();
    repo.write("binary", b"\0\x01");
    repo.write("encoding", b"\xff");
    repo.write("large", vec![b'a'; 2 * 1024 * 1024 + 1]);
    repo.write("mode", "same\n");
    repo.initial();
    repo.write("binary", b"\0\x02");
    repo.write("encoding", b"\xfe");
    repo.write("large", vec![b'b'; 2 * 1024 * 1024 + 1]);
    assert_eq!(
        repo.diff("binary", DiffKind::Unstaged).presentation,
        Presentation::Binary
    );
    assert_eq!(
        repo.diff("encoding", DiffKind::Unstaged).presentation,
        Presentation::InvalidEncoding
    );
    let large = repo.diff("large", DiffKind::Unstaged);
    assert_eq!(large.presentation, Presentation::TooLarge);
    assert!(large.left.bytes.is_empty());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(repo.path().join("mode"), fs::Permissions::from_mode(0o755)).unwrap();
        let diff = repo.diff("mode", DiffKind::Unstaged);
        assert!(diff.hunks.is_empty());
        assert_ne!(diff.left.mode, diff.right.mode);
    }
}

#[test]
/// Discovers externally started conflicts without reading a nonexistent stage-zero blob.
fn detects_external_merge_conflict() {
    let repo = Repo::new();
    repo.write("conflict.txt", "base\n");
    repo.initial();
    repo.git(&["switch", "-c", "other"]);
    repo.write("conflict.txt", "other\n");
    repo.git(&["commit", "-am", "other"]);
    repo.git(&["switch", "main"]);
    repo.write("conflict.txt", "main\n");
    repo.git(&["commit", "-am", "main"]);
    assert!(!repo.command(&["merge", "other"]).status.success());
    let snapshot = scan(repo.path()).unwrap();
    assert_eq!(snapshot.operation, Some(Operation::Merge));
    assert!(snapshot.files.iter().any(|status| status.conflicted));
    assert_eq!(
        repo.diff("conflict.txt", DiffKind::Unstaged).presentation,
        Presentation::Conflict
    );
}

#[test]
/// Supports detached HEAD and linked worktree discovery without touching the original checkout.
fn detached_and_linked_worktrees() {
    let repo = Repo::new();
    repo.write("f", "base\n");
    repo.initial();
    repo.git(&["switch", "--detach"]);
    assert!(scan(repo.path()).unwrap().branch.is_none());
    let destination = repo.path().join("linked");
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "linked",
        destination.to_str().unwrap(),
    ]);
    fs::write(destination.join("f"), "changed\n").unwrap();
    let snapshot = scan(&destination).unwrap();
    assert_eq!(snapshot.branch.as_deref(), Some("linked"));
    assert_eq!(snapshot.root, destination.canonicalize().unwrap());
    assert_eq!(
        read_diff(&destination, Path::new("f"), DiffKind::Unstaged)
            .unwrap()
            .right
            .bytes,
        b"changed\n"
    );
}

#[test]
/// Rejects traversal and does not dereference a symlink into external user data.
fn safe_paths_and_symlinks() {
    let repo = Repo::new();
    assert!(read_diff(repo.path(), Path::new("../outside"), DiffKind::Unstaged).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink("/etc/passwd", repo.path().join("link")).unwrap();
        let diff = repo.diff("link", DiffKind::Unstaged);
        assert_eq!(diff.right.bytes, b"/etc/passwd");
        symlink("/etc", repo.path().join("outside")).unwrap();
        assert!(read_diff(repo.path(), Path::new("outside/passwd"), DiffKind::Unstaged).is_err());
    }
}

#[test]
/// Verifies supported SHA-256 repository status and exact blob reading.
fn sha256_repository() {
    let repo = Repo::new();
    fs::remove_dir_all(repo.path().join(".git")).unwrap();
    repo.git(&["init", "--object-format=sha256", "--initial-branch=main"]);
    repo.write("f", "base\n");
    repo.initial();
    repo.write("f", "new\n");
    assert_eq!(scan(repo.path()).unwrap().head.unwrap().len(), 64);
    assert_eq!(repo.diff("f", DiffKind::Unstaged).left.bytes, b"base\n");
}
