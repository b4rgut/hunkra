//! History pages and explicit commit-tree bases checked against independent temporary Git repos.
mod support;
use hunkra_core::{
    CommitBase, CommitDetails, CoreError, HistoryReader, Presentation, read_commit,
    read_commit_diff,
};
use std::{fs, path::Path, process::Command};
use support::Repo;

/// Reads an independent immutable object id without its transport newline.
fn oid(repo: &Repo, name: &str) -> String {
    String::from_utf8(repo.git(&["rev-parse", name]))
        .unwrap()
        .trim()
        .into()
}
/// Commits fixture content with deterministic distinct times without modifying global environment.
fn timed_commit(repo: &Repo, time: usize, message: &str) {
    let date = format!("@{} +0000", 1_700_000_000 + time);
    let output = Command::new("/usr/bin/git")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", repo.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", date)
        .current_dir(repo.path())
        .args(["commit", "-am", message])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
/// Checks changed leaves/modes/content using Git's no-renames raw-tree comparison.
fn assert_tree_oracle(repo: &Repo, detail: &CommitDetails) {
    let mut args = vec![
        "diff-tree",
        "--no-commit-id",
        "--no-renames",
        "--name-status",
        "-r",
        "-z",
    ];
    if let Some(base) = &detail.base_id {
        args.push(base)
    } else {
        args.push("--root")
    }
    args.push(&detail.summary.id);
    let output = repo.git(&args);
    let expected: Vec<_> = output
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect();
    let mut actual = Vec::new();
    for file in &detail.files {
        let kind = match file.change {
            hunkra_core::ChangeKind::Added => b"A".as_slice(),
            hunkra_core::ChangeKind::Deleted => b"D".as_slice(),
            hunkra_core::ChangeKind::Modified => b"M".as_slice(),
            hunkra_core::ChangeKind::TypeChanged => b"T".as_slice(),
            _ => panic!("history emits only exact tree changes"),
        };
        actual.push(kind);
        actual.push(file.path.as_os_str().as_encoded_bytes());
        let diff = read_commit_diff(detail, &file.path).unwrap();
        for (id, document) in [
            (detail.base_id.as_ref(), &diff.left),
            (Some(&detail.summary.id), &diff.right),
        ] {
            if !document.present {
                continue;
            }
            let id = id.unwrap();
            let path = file.path.to_str().unwrap();
            let entry = repo.git(&["ls-tree", id, "--", path]);
            let mode = u32::from_str_radix(std::str::from_utf8(&entry[..6]).unwrap(), 8).unwrap();
            assert_eq!(document.mode, Some(mode));
            if !document.limited && !document.unsupported {
                assert_eq!(document.bytes, repo.git(&["show", &format!("{id}:{path}")]));
            }
        }
    }
    assert_eq!(actual, expected);
}

#[test]
/// More than one maximum page stays incremental, exact and pinned even after HEAD advances.
fn pages_keep_immutable_tip_and_all_rows_in_both_formats() {
    for format in ["sha1", "sha256"] {
        let repo = Repo::with_format(format);
        repo.write("file", "base\n");
        repo.initial();
        for number in 0..210 {
            repo.write("file", format!("строка {number}\n"));
            timed_commit(&repo, number, &format!("Изменение {number} 🦀"));
        }
        let tip = oid(&repo, "HEAD");
        let expected = String::from_utf8(repo.git(&["log", "--format=%H", &tip])).unwrap();
        let mut reader = HistoryReader::open(repo.path()).unwrap();
        assert!(matches!(
            reader.next_page(0),
            Err(CoreError::HistoryRequest)
        ));
        assert!(matches!(
            reader.next_page(201),
            Err(CoreError::HistoryRequest)
        ));
        let first = reader.next_page(1).unwrap();
        assert_eq!(first.offset, 0);
        assert!(first.has_more);
        assert_eq!(
            first.commits[0].subject.as_deref(),
            Some("Изменение 209 🦀")
        );
        repo.write("file", "new HEAD\n");
        timed_commit(&repo, 211, "new head");
        let before = fs::read(repo.path().join(".git/index")).unwrap();
        let mut actual: Vec<_> = first.commits.into_iter().map(|commit| commit.id).collect();
        for size in [7, 200, 3] {
            let page = reader.next_page(size).unwrap();
            assert_eq!(page.offset, actual.len());
            assert_eq!(page.tip.as_ref(), Some(&tip));
            actual.extend(page.commits.into_iter().map(|commit| commit.id));
        }
        assert_eq!(actual, expected.lines().collect::<Vec<_>>());
        let empty = reader.next_page(1).unwrap();
        assert!(empty.commits.is_empty() && !empty.has_more);
        assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), before);
        assert_eq!(fs::read(repo.path().join("file")).unwrap(), b"new HEAD\n");
        let unchanged = repo.diff("file", hunkra_core::DiffKind::Unstaged);
        assert_eq!(unchanged.left.bytes, unchanged.right.bytes);
    }
}

#[test]
/// Root and both merge parents produce their explicit independent files and raw blob diffs.
fn root_and_merge_bases_match_git() {
    for format in ["sha1", "sha256"] {
        let repo = Repo::with_format(format);
        repo.write("начало\nимя.txt", "\u{feff}base\r\nбез LF");
        repo.initial();
        let root = oid(&repo, "HEAD");
        let detail = read_commit(repo.path(), &root, CommitBase::EmptyTree).unwrap();
        assert_tree_oracle(&repo, &detail);
        assert!(matches!(
            read_commit(repo.path(), &root, CommitBase::Parent(0)),
            Err(CoreError::HistoryBase)
        ));
        repo.git(&["switch", "-c", "incoming"]);
        repo.write("incoming", "B\r\n");
        repo.git(&["add", "."]);
        repo.git(&["commit", "-m", "incoming"]);
        repo.git(&["switch", "main"]);
        repo.write("current", "A\n");
        repo.git(&["add", "."]);
        repo.git(&["commit", "-m", "current"]);
        repo.git(&["merge", "--no-ff", "--no-edit", "incoming"]);
        let merge = oid(&repo, "HEAD");
        for index in [0, 1] {
            let detail = read_commit(repo.path(), &merge, CommitBase::Parent(index)).unwrap();
            assert_eq!(detail.summary.parents.len(), 2);
            assert_eq!(
                detail.base_id,
                Some(oid(&repo, &format!("HEAD^{}", index + 1)))
            );
            assert_tree_oracle(&repo, &detail);
        }
        assert!(matches!(
            read_commit(repo.path(), &merge, CommitBase::EmptyTree),
            Err(CoreError::HistoryBase)
        ));
        assert!(matches!(
            read_commit(repo.path(), &merge, CommitBase::Parent(2)),
            Err(CoreError::HistoryBase)
        ));
        let mut reader = HistoryReader::open(repo.path()).unwrap();
        let page = reader.next_page(10).unwrap();
        let native: std::collections::BTreeSet<_> = page
            .commits
            .iter()
            .map(|commit| commit.id.as_str())
            .collect();
        let oracle = String::from_utf8(repo.git(&["log", "--format=%H"])).unwrap();
        assert_eq!(native, oracle.lines().collect());
        assert_eq!(page.commits[0].parents.len(), 2);
    }
}

#[test]
/// An octopus merge exposes every actual parent rather than assuming exactly two.
fn octopus_parent_selection_is_exact() {
    let repo = Repo::new();
    repo.write("base", "base");
    repo.initial();
    for branch in ["one", "two", "three"] {
        repo.git(&["switch", "-c", branch, "main"]);
        repo.write(branch, branch);
        repo.git(&["add", "."]);
        repo.git(&["commit", "-m", branch]);
    }
    repo.git(&["switch", "main"]);
    repo.git(&["merge", "--no-ff", "--no-edit", "one", "two", "three"]);
    let merge = oid(&repo, "HEAD");
    let count = read_commit(repo.path(), &merge, CommitBase::Parent(0))
        .unwrap()
        .summary
        .parents
        .len();
    assert!(count >= 3);
    for index in 0..count {
        assert_tree_oracle(
            &repo,
            &read_commit(repo.path(), &merge, CommitBase::Parent(index)).unwrap(),
        );
    }
}

#[test]
/// Historical blobs/modes never run current filters or dereference current worktree symlinks.
fn raw_bytes_modes_and_unsupported_leaves_preserve_worktree() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    for format in ["sha1", "sha256"] {
        let repo = Repo::with_format(format);
        fs::create_dir(repo.path().join("nested")).unwrap();
        repo.write("nested/file", "base\r\n");
        repo.write("binary", b"base\0");
        repo.initial();
        let root = oid(&repo, "HEAD");
        repo.write("nested/file", "\u{feff}кириллица\r\nконец");
        repo.write("binary", b"different\0binary");
        repo.write("large", vec![b'x'; 2 * 1024 * 1024 + 1]);
        symlink("nested/file", repo.path().join("link")).unwrap();
        fs::set_permissions(
            repo.path().join("nested/file"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        repo.git(&["add", "."]);
        let missing = if format == "sha1" {
            "1".repeat(40)
        } else {
            "1".repeat(64)
        };
        repo.git(&[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{missing},submodule"),
        ]);
        repo.git(&["commit", "-m", "types"]);
        let tip = oid(&repo, "HEAD");
        let detail = read_commit(repo.path(), &tip, CommitBase::Parent(0)).unwrap();
        assert_tree_oracle(&repo, &detail);
        assert_eq!(
            read_commit_diff(&detail, Path::new("binary"))
                .unwrap()
                .presentation,
            Presentation::Binary
        );
        assert_eq!(
            read_commit_diff(&detail, Path::new("large"))
                .unwrap()
                .presentation,
            Presentation::TooLarge
        );
        assert_eq!(
            read_commit_diff(&detail, Path::new("submodule"))
                .unwrap()
                .presentation,
            Presentation::Unsupported
        );
        assert_eq!(
            read_commit_diff(&detail, Path::new("link"))
                .unwrap()
                .right
                .bytes,
            b"nested/file"
        );
        let external = tempfile::tempdir().unwrap();
        fs::remove_dir_all(repo.path().join("nested")).unwrap();
        symlink(external.path(), repo.path().join("nested")).unwrap();
        repo.write(".gitattributes", "* filter=refusal\n");
        repo.git(&["config", "filter.refusal.clean", "false"]);
        repo.git(&["config", "filter.refusal.required", "true"]);
        repo.write(".git/index.lock", "foreign lock");
        let index = fs::read(repo.path().join(".git/index")).unwrap();
        let diff = read_commit_diff(&detail, Path::new("nested/file")).unwrap();
        assert_eq!(diff.left.bytes, b"base\r\n");
        assert_eq!(diff.right.bytes, "\u{feff}кириллица\r\nконец".as_bytes());
        assert_eq!(diff.right.mode, Some(0o100755));
        assert_tree_oracle(
            &repo,
            &read_commit(repo.path(), &root, CommitBase::EmptyTree).unwrap(),
        );
        assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
        assert_eq!(
            fs::read(repo.path().join(".git/index.lock")).unwrap(),
            b"foreign lock"
        );
        assert!(fs::read_dir(external.path()).unwrap().next().is_none());
    }
}

#[test]
/// File/directory swaps report the removed/added leaves and never display a tree as file bytes.
fn directory_type_swaps_match_leaf_oracle() {
    let repo = Repo::new();
    fs::create_dir(repo.path().join("dir")).unwrap();
    repo.write("file", "before");
    repo.write("dir/child", "before");
    repo.initial();
    fs::remove_file(repo.path().join("file")).unwrap();
    fs::remove_dir_all(repo.path().join("dir")).unwrap();
    fs::create_dir(repo.path().join("file")).unwrap();
    repo.write("file/child", "after");
    repo.write("dir", "after");
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-m", "swap"]);
    let detail = read_commit(repo.path(), &oid(&repo, "HEAD"), CommitBase::Parent(0)).unwrap();
    assert_tree_oracle(&repo, &detail);
    assert_eq!(detail.files.len(), 4);
}

#[test]
/// Unborn/detached/packed/linked histories read only the selected repository's pinned HEAD.
fn unborn_detached_packed_and_linked_history() {
    for format in ["sha1", "sha256"] {
        let repo = Repo::with_format(format);
        let page = HistoryReader::open(repo.path())
            .unwrap()
            .next_page(10)
            .unwrap();
        assert!(page.tip.is_none() && page.commits.is_empty() && !page.has_more);
        repo.write("file", "base");
        repo.initial();
        let tip = oid(&repo, "HEAD");
        repo.git(&["gc"]);
        repo.git(&["switch", "--detach"]);
        let page = HistoryReader::open(repo.path())
            .unwrap()
            .next_page(10)
            .unwrap();
        assert_eq!(page.commits[0].id, tip);
        let temporary = tempfile::tempdir().unwrap();
        let linked = temporary.path().join("linked");
        repo.git(&["worktree", "add", "--detach", linked.to_str().unwrap()]);
        let page = HistoryReader::open(&linked).unwrap().next_page(10).unwrap();
        assert_eq!(page.root, linked.canonicalize().unwrap());
        assert_eq!(page.commits[0].id, tip);
        assert_eq!(
            read_commit(&linked, &tip, CommitBase::EmptyTree)
                .unwrap()
                .files
                .len(),
            1
        );
    }
}

#[test]
/// Invalid objects/parents/path requests cannot become moving revspecs or arbitrary file reads.
fn invalid_requests_and_empty_commits_are_explicit() {
    let repo = Repo::new();
    repo.write("file", "base");
    repo.initial();
    for id in ["HEAD", "--option", "abc", "../../../outside"] {
        assert!(matches!(
            read_commit(repo.path(), id, CommitBase::EmptyTree),
            Err(CoreError::HistoryRequest)
        ));
    }
    let blob = oid(&repo, "HEAD:file");
    assert!(matches!(
        read_commit(repo.path(), &blob, CommitBase::EmptyTree),
        Err(CoreError::HistoryRequest)
    ));
    let detail = read_commit(repo.path(), &oid(&repo, "HEAD"), CommitBase::EmptyTree).unwrap();
    assert!(matches!(
        read_commit_diff(&detail, Path::new("unreviewed")),
        Err(CoreError::InvalidPath)
    ));
    repo.git(&["commit", "--allow-empty", "-m", "empty"]);
    assert!(
        read_commit(repo.path(), &oid(&repo, "HEAD"), CommitBase::Parent(0))
            .unwrap()
            .files
            .is_empty()
    );
}

#[test]
/// Unsupported message encoding and oversized objects are explained before unbounded history loads.
fn message_encoding_and_object_limits_are_bounded() {
    let repo = Repo::new();
    repo.write("file", "base");
    repo.initial();
    repo.write("message", b"caf\xe9\n");
    repo.git(&[
        "-c",
        "i18n.commitEncoding=ISO-8859-1",
        "commit",
        "--allow-empty",
        "-F",
        "message",
    ]);
    let detail = read_commit(repo.path(), &oid(&repo, "HEAD"), CommitBase::Parent(0)).unwrap();
    assert!(detail.message.is_none() && detail.summary.subject.is_none());
    assert_eq!(detail.encoding.as_deref(), Some("ISO-8859-1"));
    repo.write("message", vec![b'x'; 1024 * 1024 + 1]);
    repo.git(&["commit", "--allow-empty", "-F", "message"]);
    let huge = oid(&repo, "HEAD");
    assert!(matches!(
        read_commit(repo.path(), &huge, CommitBase::Parent(0)),
        Err(CoreError::HistoryLimit)
    ));
    assert!(matches!(
        HistoryReader::open(repo.path()),
        Err(CoreError::HistoryLimit)
    ));
    repo.git(&[
        "commit",
        "--allow-empty",
        "-m",
        "small tip with oversized parent",
    ]);
    let mut reader = HistoryReader::open(repo.path()).unwrap();
    assert!(matches!(reader.next_page(1), Err(CoreError::HistoryLimit)));
    assert!(matches!(
        reader.next_page(1),
        Err(CoreError::HistoryRequest)
    ));
}
