//! Exact-source cache acceptance; temporary fixtures and independent Git protect original byte/guard contracts.
use hunkra_core::{CoreError, DiffKind, DiffReader, Presentation, Selection, update_index};
use std::{fs, path::Path, sync::Arc};
mod support;
use support::Repo;

#[test]
/// Unchanged refresh/fast switching share alignment, while SHA-1/SHA-256 and staged/unstaged versions remain distinct.
fn reuse_preserves_sources_and_comparison_modes() {
    for format in ["sha1", "sha256"] {
        let repo = Repo::with_format(format);
        repo.write("f", b"\xef\xbb\xbfold\r\nEOF");
        repo.write("g", b"g\n");
        repo.initial();
        repo.write("f", b"\xef\xbb\xbfstaged\r\nEOF");
        repo.git(&["add", "f"]);
        repo.write("f", b"\xef\xbb\xbfunstaged\r\nEOF");
        let index = fs::read(repo.path().join(".git/index")).unwrap();
        let mut reader = DiffReader::default();
        let first = reader
            .read(repo.path(), Path::new("f"), DiffKind::Unstaged)
            .unwrap();
        assert!(!first.reused);
        assert_eq!(first.snapshot.rows, repo.diff("f", DiffKind::Unstaged).rows);
        reader
            .read(repo.path(), Path::new("g"), DiffKind::Unstaged)
            .unwrap();
        let second = reader
            .read(repo.path(), Path::new("f"), DiffKind::Unstaged)
            .unwrap();
        assert!(second.reused && Arc::ptr_eq(&first.snapshot, &second.snapshot));
        assert_eq!(second.snapshot.left.bytes, repo.git(&["show", ":f"]));
        assert_eq!(
            second.snapshot.right.bytes,
            fs::read(repo.path().join("f")).unwrap()
        );
        let staged = reader
            .read(repo.path(), Path::new("f"), DiffKind::Staged)
            .unwrap();
        assert!(!staged.reused && !Arc::ptr_eq(&staged.snapshot, &second.snapshot));
        assert_eq!(staged.snapshot.left.bytes, repo.git(&["show", "HEAD:f"]));
        assert_eq!(staged.snapshot.right.bytes, repo.git(&["show", ":f"]));
        assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
        assert_eq!(reader.stats().entries, 3);
    }
}

#[test]
/// Equal size/mtime never hides byte replacement; stale accepted coordinates cannot change index or worktree.
fn same_timestamp_replacement_and_index_head_modes_are_revalidated() {
    let repo = Repo::new();
    repo.write("f", b"base\n");
    repo.write("g", b"base g\n");
    repo.initial();
    let mut reader = DiffReader::default();
    repo.write("f", b"aaaa\n");
    let old = reader
        .read(repo.path(), Path::new("f"), DiffKind::Unstaged)
        .unwrap()
        .snapshot;
    let path = repo.path().join("f");
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    repo.write("f", b"bbbb\n");
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    let current = reader
        .read(repo.path(), Path::new("f"), DiffKind::Unstaged)
        .unwrap();
    assert!(!current.reused && current.snapshot.right.bytes == b"bbbb\n");
    let index = fs::read(repo.path().join(".git/index")).unwrap();
    assert!(matches!(
        update_index(repo.path(), &old, &Selection::File),
        Err(CoreError::Stale)
    ));
    assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
    repo.write("g", b"staged other\n");
    repo.git(&["add", "g"]);
    let changed_index = reader
        .read(repo.path(), Path::new("f"), DiffKind::Unstaged)
        .unwrap();
    assert!(!changed_index.reused);
    assert_eq!(changed_index.snapshot.right, current.snapshot.right);
    repo.git(&["commit", "-m", "other"]);
    assert!(
        !reader
            .read(repo.path(), Path::new("f"), DiffKind::Unstaged)
            .unwrap()
            .reused
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        let read = reader
            .read(repo.path(), Path::new("f"), DiffKind::Unstaged)
            .unwrap();
        assert!(!read.reused && read.snapshot.right.mode == Some(0o100755));
    }
    assert_eq!(fs::read(path).unwrap(), b"bbbb\n");
}

#[test]
/// Fresh attribute/filter conversion can invalidate equal raw worktree bytes, and a failed filter never returns cached success.
fn attributes_filters_and_failed_reads_are_not_cached_as_current() {
    let repo = Repo::new();
    repo.write("f", b"old\r\n");
    repo.initial();
    repo.write("f", b"new\r\n");
    let mut reader = DiffReader::default();
    let first = reader
        .read(repo.path(), Path::new("f"), DiffKind::Unstaged)
        .unwrap();
    repo.write(".gitattributes", b"f text\n");
    let converted = reader
        .read(repo.path(), Path::new("f"), DiffKind::Unstaged)
        .unwrap();
    assert!(!converted.reused && converted.snapshot.right.bytes == b"new\n");
    assert_eq!(first.snapshot.right.bytes, b"new\r\n");
    repo.write(".gitattributes", b"f filter=fixture\n");
    repo.git(&[
        "config",
        "filter.fixture.clean",
        "printf 'normalized one\\n'",
    ]);
    repo.git(&["config", "filter.fixture.required", "true"]);
    let filtered = reader
        .read(repo.path(), Path::new("f"), DiffKind::Unstaged)
        .unwrap();
    assert_eq!(filtered.snapshot.right.bytes, b"normalized one\n");
    repo.git(&[
        "config",
        "filter.fixture.clean",
        "printf 'normalized two\\n'",
    ]);
    let changed = reader
        .read(repo.path(), Path::new("f"), DiffKind::Unstaged)
        .unwrap();
    assert!(!changed.reused && changed.snapshot.right.bytes == b"normalized two\n");
    repo.git(&["config", "filter.fixture.clean", "exit 1"]);
    assert!(
        reader
            .read(repo.path(), Path::new("f"), DiffKind::Unstaged)
            .is_err()
    );
    assert_eq!(fs::read(repo.path().join("f")).unwrap(), b"new\r\n");
}

#[test]
/// Retained row/string/document capacities stay within both entry and byte budgets across many large distinct selections.
fn allocation_and_lru_bounds_are_enforced() {
    let repo = Repo::new();
    for number in 0..5 {
        let text = (0..18000)
            .map(|line| format!("file {number} line {line:05} {}\n", "context".repeat(8)))
            .collect::<String>();
        repo.write(&format!("f{number}"), text);
    }
    repo.initial();
    let mut reader = DiffReader::default();
    for number in 0..5 {
        let read = reader
            .read(
                repo.path(),
                Path::new(&format!("f{number}")),
                DiffKind::Unstaged,
            )
            .unwrap();
        assert!(!read.reused && read.snapshot.rows.len() == 18000);
        assert!(reader.stats().entries <= hunkra_core::diff_reader::CACHE_ENTRIES);
        assert!(reader.stats().allocated_bytes <= hunkra_core::diff_reader::CACHE_BYTES);
    }
    assert!(reader.stats().entries < hunkra_core::diff_reader::CACHE_ENTRIES);
    assert!(
        reader
            .read(repo.path(), Path::new("f4"), DiffKind::Unstaged)
            .unwrap()
            .reused
    );
    assert!(
        !reader
            .read(repo.path(), Path::new("f0"), DiffKind::Unstaged)
            .unwrap()
            .reused
    );
    let other = Repo::new();
    other.write("f4", b"different repository\n");
    other.initial();
    assert!(
        !reader
            .read(other.path(), Path::new("f4"), DiffKind::Unstaged)
            .unwrap()
            .reused
    );
}

#[test]
/// Recoverable conflict previews reread actual stages/index and never survive external resolution as a current cached conflict.
fn conflict_stage_changes_invalidate_reuse() {
    let repo = Repo::new();
    repo.write("f", b"base\n");
    repo.initial();
    repo.git(&["switch", "-c", "incoming"]);
    repo.write("f", b"incoming\n");
    repo.git(&["commit", "-am", "incoming"]);
    repo.git(&["switch", "main"]);
    repo.write("f", b"ours\n");
    repo.git(&["commit", "-am", "ours"]);
    assert!(
        !repo
            .command(&["merge", "--no-edit", "incoming"])
            .status
            .success()
    );
    let mut reader = DiffReader::default();
    let first = reader
        .read(repo.path(), Path::new("f"), DiffKind::Unstaged)
        .unwrap();
    assert_eq!(first.snapshot.presentation, Presentation::Conflict);
    assert!(
        reader
            .read(repo.path(), Path::new("f"), DiffKind::Unstaged)
            .unwrap()
            .reused
    );
    repo.write("f", b"resolved externally\n");
    repo.git(&["add", "f"]);
    let read = reader
        .read(repo.path(), Path::new("f"), DiffKind::Unstaged)
        .unwrap();
    assert!(!read.reused && read.snapshot.presentation == Presentation::Text);
    assert_eq!(read.snapshot.right.bytes, b"resolved externally\n");
}
