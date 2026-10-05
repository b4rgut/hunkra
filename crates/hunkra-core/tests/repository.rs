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
        Self::with_format("sha1")
    }
    /// Initializes the object format before Git creates any files or objects.
    fn with_format(format: &str) -> Self {
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
    let index = fs::read(repo.path().join(".git/index")).unwrap();
    let working = fs::read(repo.path().join("conflict.txt")).unwrap();
    assert!(matches!(
        hunkra_core::update_index(
            repo.path(),
            &repo.diff("conflict.txt", DiffKind::Unstaged),
            &hunkra_core::Selection::File
        ),
        Err(hunkra_core::CoreError::UnsupportedWrite)
    ));
    assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
    assert_eq!(fs::read(repo.path().join("conflict.txt")).unwrap(), working);
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

#[test]
/// Stages B while preserving staged A and unstaged C; Git independently commits exactly A+B.
fn stage_selected_replacement_preserves_existing_index_and_worktree() {
    use hunkra_core::{LineSelection, Selection, update_index};
    let repo = Repo::new();
    repo.write("f", "A\ncontext\nB\ncontext\nC\n");
    repo.initial();
    repo.write("f", "A staged\ncontext\nB\ncontext\nC\n");
    repo.git(&["add", "f"]);
    let working = b"A staged\ncontext\nB selected\ncontext\nC remaining\n";
    repo.write("f", working);
    let diff = repo.diff("f", DiffKind::Unstaged);
    let selection = LineSelection {
        removed: [2].into(),
        added: [2].into(),
    };
    assert_eq!(
        update_index(repo.path(), &diff, &Selection::Lines(selection))
            .unwrap()
            .backend,
        "gix"
    );
    let expected = b"A staged\ncontext\nB selected\ncontext\nC\n";
    assert_eq!(repo.git(&["show", ":f"]), expected);
    assert_eq!(fs::read(repo.path().join("f")).unwrap(), working);
    repo.git(&["commit", "-m", "selected changes"]);
    assert_eq!(repo.git(&["show", "HEAD:f"]), expected);
    assert_eq!(repo.diff("f", DiffKind::Unstaged).right.bytes, working);
}

#[test]
/// Added and removed halves of a replacement can be selected independently.
fn independent_added_and_removed_lines() {
    use hunkra_core::{LineSelection, Selection, update_index};
    for (removed, added, expected) in [
        (true, false, "head\ntail\n"),
        (false, true, "head\nold\nnew\ntail\n"),
        (true, true, "head\nnew\ntail\n"),
    ] {
        let repo = Repo::new();
        repo.write("f", "head\nold\ntail\n");
        repo.initial();
        repo.write("f", "head\nnew\ntail\n");
        let selection = LineSelection {
            removed: if removed {
                [1].into()
            } else {
                Default::default()
            },
            added: if added {
                [1].into()
            } else {
                Default::default()
            },
        };
        update_index(
            repo.path(),
            &repo.diff("f", DiffKind::Unstaged),
            &Selection::Lines(selection),
        )
        .unwrap();
        assert_eq!(repo.git(&["show", ":f"]), expected.as_bytes());
        assert_eq!(
            fs::read(repo.path().join("f")).unwrap(),
            b"head\nnew\ntail\n"
        );
    }
}

#[test]
/// Part of a multiline replacement is reversible without touching working content.
fn unstage_partial_replacement_and_file() {
    use hunkra_core::{LineSelection, Selection, update_index};
    let repo = Repo::new();
    repo.write("f", "old1\nold2\n");
    repo.initial();
    repo.write("f", "new1\nnew2\n");
    repo.git(&["add", "f"]);
    let diff = repo.diff("f", DiffKind::Staged);
    let selection = LineSelection {
        removed: [0].into(),
        added: [0].into(),
    };
    update_index(repo.path(), &diff, &Selection::Lines(selection)).unwrap();
    assert_eq!(repo.git(&["show", ":f"]), b"old1\nnew2\n");
    update_index(
        repo.path(),
        &repo.diff("f", DiffKind::Staged),
        &Selection::File,
    )
    .unwrap();
    assert_eq!(repo.git(&["show", ":f"]), b"old1\nold2\n");
    assert_eq!(fs::read(repo.path().join("f")).unwrap(), b"new1\nnew2\n");
}

#[test]
/// Stale worktree, raw newline change, HEAD and unrelated index changes cause safe refusal.
fn stale_snapshots_do_not_write() {
    use hunkra_core::{CoreError, Selection, update_index};
    for variant in ["worktree", "index", "head", "raw-crlf"] {
        let repo = Repo::new();
        repo.write("f", "base\n");
        repo.write("other", "base\n");
        repo.write(".gitattributes", "f text eol=crlf\n");
        repo.initial();
        repo.write("f", "changed\r\n");
        let diff = repo.diff("f", DiffKind::Unstaged);
        match variant {
            "worktree" => repo.write("f", "external\r\n"),
            "raw-crlf" => repo.write("f", "changed\n"),
            "head" => {
                repo.git(&["commit", "--allow-empty", "-m", "external"]);
            }
            _ => {
                repo.write("other", "external\n");
                repo.git(&["add", "other"]);
            }
        }
        let before = fs::read(repo.path().join(".git/index")).unwrap();
        assert!(
            matches!(
                update_index(repo.path(), &diff, &Selection::File),
                Err(CoreError::Stale)
            ),
            "{variant}"
        );
        assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), before);
        assert!(!repo.path().join(".git/index.lock").exists());
    }
}

#[test]
/// Holds Git's actual index lock, then proves no fallback writer or lock deletion occurs.
fn index_lock_is_respected() {
    use hunkra_core::{CoreError, Selection, update_index};
    let repo = Repo::new();
    repo.write("f", "base\n");
    repo.initial();
    repo.write("f", "changed\n");
    let diff = repo.diff("f", DiffKind::Unstaged);
    fs::write(repo.path().join(".git/index.lock"), "external lock").unwrap();
    let before = fs::read(repo.path().join(".git/index")).unwrap();
    assert!(matches!(
        update_index(repo.path(), &diff, &Selection::File),
        Err(CoreError::IndexLock)
    ));
    assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), before);
    assert_eq!(
        fs::read(repo.path().join(".git/index.lock")).unwrap(),
        b"external lock"
    );
}

#[test]
/// Whole-file new/delete/binary operations and unborn unstage preserve exact working files.
fn stage_new_delete_binary_and_unborn_unstage() {
    use hunkra_core::{Selection, update_index};
    let repo = Repo::new();
    repo.write("new", "new\r\nno newline");
    let diff = repo.diff("new", DiffKind::Unstaged);
    update_index(repo.path(), &diff, &Selection::File).unwrap();
    assert_eq!(repo.git(&["show", ":new"]), b"new\r\nno newline");
    update_index(
        repo.path(),
        &repo.diff("new", DiffKind::Staged),
        &Selection::File,
    )
    .unwrap();
    assert!(repo.git(&["ls-files"]).is_empty());
    assert_eq!(
        fs::read(repo.path().join("new")).unwrap(),
        b"new\r\nno newline"
    );
    repo.initial();
    fs::remove_file(repo.path().join("new")).unwrap();
    update_index(
        repo.path(),
        &repo.diff("new", DiffKind::Unstaged),
        &Selection::File,
    )
    .unwrap();
    assert!(!repo.command(&["show", ":new"]).status.success());
    update_index(
        repo.path(),
        &repo.diff("new", DiffKind::Staged),
        &Selection::File,
    )
    .unwrap();
    assert_eq!(repo.git(&["show", ":new"]), b"new\r\nno newline");
    assert!(!repo.path().join("new").exists());
    repo.write("binary", b"\0\xff\x01");
    update_index(
        repo.path(),
        &repo.diff("binary", DiffKind::Unstaged),
        &Selection::File,
    )
    .unwrap();
    assert_eq!(repo.git(&["show", ":binary"]), b"\0\xff\x01");
}

#[test]
/// Partial staging applies Git attributes but leaves the CRLF working file byte-identical.
fn partial_stage_crlf_and_no_final_newline() {
    use hunkra_core::{LineSelection, Selection, update_index};
    let repo = Repo::new();
    repo.write(".gitattributes", "*.txt text eol=crlf\n");
    repo.write("f.txt", b"first\r\nsecond");
    repo.initial();
    let working = b"first changed\r\nsecond changed";
    repo.write("f.txt", working);
    update_index(
        repo.path(),
        &repo.diff("f.txt", DiffKind::Unstaged),
        &Selection::Lines(LineSelection {
            removed: [0].into(),
            added: [0].into(),
        }),
    )
    .unwrap();
    assert_eq!(repo.git(&["show", ":f.txt"]), b"first changed\nsecond");
    assert_eq!(fs::read(repo.path().join("f.txt")).unwrap(), working);
}

#[test]
/// Invalidating gix's documented stale tree-cache is verified by an independent Git commit.
fn modified_index_invalidates_tree_cache() {
    use hunkra_core::{Selection, update_index};
    let repo = Repo::new();
    fs::create_dir(repo.path().join("sub")).unwrap();
    repo.write("sub/f", "before\n");
    repo.initial();
    repo.git(&["write-tree"]);
    repo.write("sub/f", "after\n");
    update_index(
        repo.path(),
        &repo.diff("sub/f", DiffKind::Unstaged),
        &Selection::File,
    )
    .unwrap();
    repo.git(&["commit", "-m", "after"]);
    assert_eq!(repo.git(&["show", "HEAD:sub/f"]), b"after\n");
}

#[test]
/// Configured executable index hooks are refused before the index is changed or a hook runs.
fn unsupported_index_hook_is_a_preflight_refusal() {
    use hunkra_core::{CoreError, Selection, update_index};
    let repo = Repo::new();
    repo.write("f", "base\n");
    repo.initial();
    repo.write("f", "new\n");
    let hook = repo.path().join("hooks/post-index-change");
    fs::write(&hook, "#!/bin/sh\ntouch hook-ran\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    repo.git(&[
        "config",
        "core.hooksPath",
        repo.path().join("hooks").to_str().unwrap(),
    ]);
    let before = fs::read(repo.path().join(".git/index")).unwrap();
    assert!(matches!(
        update_index(
            repo.path(),
            &repo.diff("f", DiffKind::Unstaged),
            &Selection::File
        ),
        Err(CoreError::UnsupportedWrite)
    ));
    assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), before);
    assert!(!repo.path().join("hook-ran").exists());
}

#[cfg(unix)]
#[test]
/// Staging honors core.fileMode and preserves the index file's existing access permissions.
fn stage_honors_filemode_and_index_permissions() {
    use hunkra_core::{Selection, update_index};
    use std::os::unix::fs::PermissionsExt;
    let repo = Repo::new();
    repo.write("f", "base\n");
    repo.initial();
    repo.git(&["config", "core.fileMode", "false"]);
    fs::set_permissions(repo.path().join("f"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(
        repo.path().join(".git/index"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    repo.write("f", "changed\n");
    update_index(
        repo.path(),
        &repo.diff("f", DiffKind::Unstaged),
        &Selection::File,
    )
    .unwrap();
    assert!(
        repo.git(&["ls-files", "--stage", "f"])
            .starts_with(b"100644 ")
    );
    assert_eq!(
        fs::metadata(repo.path().join(".git/index"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(repo.path().join("f"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    repo.git(&["config", "core.fileMode", "true"]);
    update_index(
        repo.path(),
        &repo.diff("f", DiffKind::Unstaged),
        &Selection::File,
    )
    .unwrap();
    assert!(
        repo.git(&["ls-files", "--stage", "f"])
            .starts_with(b"100755 ")
    );
    update_index(
        repo.path(),
        &repo.diff("f", DiffKind::Staged),
        &Selection::File,
    )
    .unwrap();
    assert!(
        repo.git(&["ls-files", "--stage", "f"])
            .starts_with(b"100644 ")
    );
    assert_eq!(fs::read(repo.path().join("f")).unwrap(), b"changed\n");
}

#[cfg(unix)]
#[test]
/// Symbolic-link staging reads target bytes, including core.symlinks=false regular checkouts.
fn stage_symlink_and_regular_link_checkout() {
    use hunkra_core::{Selection, update_index};
    use std::os::unix::fs::symlink;
    let repo = Repo::new();
    let external = tempfile::tempdir().unwrap();
    let target = external.path().join("untouched");
    fs::write(&target, "external content").unwrap();
    symlink(&target, repo.path().join("link")).unwrap();
    update_index(
        repo.path(),
        &repo.diff("link", DiffKind::Unstaged),
        &Selection::File,
    )
    .unwrap();
    assert_eq!(
        repo.git(&["show", ":link"]),
        target.as_os_str().as_encoded_bytes()
    );
    assert!(
        repo.git(&["ls-files", "--stage", "link"])
            .starts_with(b"120000 ")
    );
    repo.initial();
    fs::remove_file(repo.path().join("link")).unwrap();
    repo.write("link", "other target");
    repo.git(&["config", "core.symlinks", "false"]);
    update_index(
        repo.path(),
        &repo.diff("link", DiffKind::Unstaged),
        &Selection::File,
    )
    .unwrap();
    assert_eq!(repo.git(&["show", ":link"]), b"other target");
    assert!(
        repo.git(&["ls-files", "--stage", "link"])
            .starts_with(b"120000 ")
    );
    assert_eq!(fs::read(target).unwrap(), b"external content");
    assert_eq!(fs::read(repo.path().join("link")).unwrap(), b"other target");
}

#[test]
/// File/directory collisions and shared-repository permission policy are refused before writing.
fn unsupported_index_variants_leave_bytes_unchanged() {
    use hunkra_core::{CoreError, Selection, update_index};
    for variant in [
        "collision",
        "shared",
        "split",
        "skip",
        "sparse",
        "case",
        "unicode",
    ] {
        let repo = Repo::new();
        repo.write("f", "base\n");
        if variant == "sparse" {
            for directory in ["kept", "excluded"] {
                fs::create_dir(repo.path().join(directory)).unwrap();
                repo.write(&format!("{directory}/entry"), "base\n");
            }
        }
        repo.initial();
        let file = if variant == "collision" {
            fs::remove_file(repo.path().join("f")).unwrap();
            fs::create_dir(repo.path().join("f")).unwrap();
            repo.write("f/child", "new\n");
            "f/child"
        } else {
            repo.write("f", "changed\n");
            match variant {
                "shared" => {
                    repo.git(&["config", "core.sharedRepository", "group"]);
                }
                "split" => {
                    repo.git(&["update-index", "--split-index"]);
                }
                "skip" => {
                    repo.git(&["update-index", "--skip-worktree", "f"]);
                }
                "sparse" => {
                    repo.git(&["sparse-checkout", "init", "--cone", "--sparse-index"]);
                    repo.git(&["sparse-checkout", "set", "kept"]);
                }
                "case" => {
                    repo.git(&["config", "core.ignoreCase", "true"]);
                }
                "unicode" => {
                    repo.git(&["config", "core.precomposeUnicode", "true"]);
                }
                _ => unreachable!(),
            }
            "f"
        };
        let diff = repo.diff(file, DiffKind::Unstaged);
        let before = fs::read(repo.path().join(".git/index")).unwrap();
        assert!(
            matches!(
                update_index(repo.path(), &diff, &Selection::File),
                Err(CoreError::UnsupportedWrite)
            ),
            "{variant}"
        );
        assert_eq!(
            fs::read(repo.path().join(".git/index")).unwrap(),
            before,
            "{variant}"
        );
        assert!(!repo.path().join(".git/index.lock").exists());
        assert_eq!(
            fs::read(repo.path().join(file)).unwrap(),
            if variant == "collision" {
                b"new\n".as_slice()
            } else {
                b"changed\n".as_slice()
            },
            "{variant}"
        );
    }
}

#[test]
/// SHA-256 object IDs and a linked-worktree index are written using the same gix path.
fn stage_sha256_and_linked_worktree() {
    use hunkra_core::{Selection, update_index};
    let repo = Repo::with_format("sha256");
    repo.write("f", "before\n");
    repo.initial();
    repo.write("f", "after\n");
    assert_eq!(
        update_index(
            repo.path(),
            &repo.diff("f", DiffKind::Unstaged),
            &Selection::File
        )
        .unwrap()
        .backend,
        "gix"
    );
    assert_eq!(repo.git(&["show", ":f"]), b"after\n");
    repo.git(&["commit", "-m", "sha256"]);
    let linked_parent = tempfile::tempdir().unwrap();
    let linked = linked_parent.path().join("linked");
    repo.git(&["worktree", "add", "-b", "linked", linked.to_str().unwrap()]);
    fs::write(linked.join("f"), "linked\n").unwrap();
    let diff = read_diff(&linked, Path::new("f"), DiffKind::Unstaged).unwrap();
    assert_eq!(
        update_index(&linked, &diff, &Selection::File)
            .unwrap()
            .backend,
        "gix"
    );
    let linked_index = gix::open(&linked).unwrap().index_path();
    assert!(linked_index.exists());
    let output = Command::new("/usr/bin/git")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", linked_parent.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .current_dir(&linked)
        .args(["show", ":f"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"linked\n");
    assert_eq!(repo.git(&["show", ":f"]), b"after\n");
    assert_eq!(fs::read(linked.join("f")).unwrap(), b"linked\n");
}

#[test]
/// Clean filters are applied through gix; a required filter failure never writes the index.
fn clean_filter_and_failed_filter_preserve_worktree_and_index() {
    use hunkra_core::{Selection, update_index};
    let repo = Repo::new();
    repo.git(&["config", "filter.fixture.clean", "sed s/WORK/CLEAN/g"]);
    repo.git(&["config", "filter.fixture.required", "true"]);
    repo.write(".gitattributes", "f filter=fixture\n");
    repo.write("f", "WORK base\n");
    repo.initial();
    repo.write("f", "WORK changed\n");
    let diff = repo.diff("f", DiffKind::Unstaged);
    assert_eq!(diff.right.bytes, b"CLEAN changed\n");
    assert_eq!(
        update_index(repo.path(), &diff, &Selection::File)
            .unwrap()
            .backend,
        "gix"
    );
    assert_eq!(repo.git(&["show", ":f"]), b"CLEAN changed\n");
    assert_eq!(fs::read(repo.path().join("f")).unwrap(), b"WORK changed\n");
    repo.git(&["config", "filter.fixture.clean", "false"]);
    let before = fs::read(repo.path().join(".git/index")).unwrap();
    assert!(update_index(repo.path(), &diff, &Selection::File).is_err());
    assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), before);
    assert!(!repo.path().join(".git/index.lock").exists());
}

#[test]
/// UTF-16LE attribute conversion permits partial UTF-8 index changes without rewriting disk bytes.
fn working_tree_encoding_partial_stage() {
    use hunkra_core::{LineSelection, Selection, update_index};
    let repo = Repo::new();
    repo.write(
        ".gitattributes",
        "f working-tree-encoding=UTF-16LE text eol=crlf\n",
    );
    let original: Vec<u8> = "строка A\r\nстрока B"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    repo.write("f", original);
    repo.initial();
    let working: Vec<u8> = "новая A\r\nновая B"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    repo.write("f", &working);
    let diff = repo.diff("f", DiffKind::Unstaged);
    assert_eq!(diff.right.bytes, "новая A\nновая B".as_bytes());
    update_index(
        repo.path(),
        &diff,
        &Selection::Lines(LineSelection {
            removed: [0].into(),
            added: [0].into(),
        }),
    )
    .unwrap();
    assert_eq!(repo.git(&["show", ":f"]), "новая A\nстрока B".as_bytes());
    assert_eq!(fs::read(repo.path().join("f")).unwrap(), working);
}

#[test]
/// An independent added EOF half is refused rather than concatenating lines; the pair is exact.
fn eof_replacement_does_not_concatenate_unselected_lines() {
    use hunkra_core::{CoreError, LineSelection, Selection, update_index};
    let repo = Repo::new();
    repo.write("f", "old");
    repo.initial();
    repo.write("f", "new");
    let diff = repo.diff("f", DiffKind::Unstaged);
    let before = fs::read(repo.path().join(".git/index")).unwrap();
    assert!(matches!(
        update_index(
            repo.path(),
            &diff,
            &Selection::Lines(LineSelection {
                removed: Default::default(),
                added: [0].into()
            })
        ),
        Err(CoreError::SelectionBoundary)
    ));
    assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), before);
    update_index(
        repo.path(),
        &diff,
        &Selection::Lines(LineSelection {
            removed: [0].into(),
            added: [0].into(),
        }),
    )
    .unwrap();
    assert_eq!(repo.git(&["show", ":f"]), b"new");
    assert_eq!(fs::read(repo.path().join("f")).unwrap(), b"new");
}

#[test]
/// Existing conflict undo records for another path survive a refused index rewrite.
fn resolve_undo_extension_is_not_silently_discarded() {
    use hunkra_core::{CoreError, Selection, update_index};
    let repo = Repo::new();
    repo.write("f", "base\n");
    repo.write("conflict", "base\n");
    repo.initial();
    repo.git(&["switch", "-c", "other"]);
    repo.write("conflict", "other\n");
    repo.git(&["commit", "-am", "other"]);
    repo.git(&["switch", "main"]);
    repo.write("conflict", "main\n");
    repo.git(&["commit", "-am", "main"]);
    assert!(!repo.command(&["merge", "other"]).status.success());
    repo.write("conflict", "resolved\n");
    repo.git(&["add", "conflict"]);
    let undo = repo.git(&["ls-files", "--resolve-undo"]);
    assert!(!undo.is_empty());
    repo.write("f", "changed\n");
    let before = fs::read(repo.path().join(".git/index")).unwrap();
    assert!(matches!(
        update_index(
            repo.path(),
            &repo.diff("f", DiffKind::Unstaged),
            &Selection::File
        ),
        Err(CoreError::UnsupportedWrite)
    ));
    assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), before);
    assert_eq!(repo.git(&["ls-files", "--resolve-undo"]), undo);
    assert_eq!(
        fs::read(repo.path().join("conflict")).unwrap(),
        b"resolved\n"
    );
    assert_eq!(fs::read(repo.path().join("f")).unwrap(), b"changed\n");
}
