//! Branch/ref and operation recovery checks against independent disposable Git fixtures.
mod support;
use hunkra_core::{
    CoreError, DiffKind, Operation, Selection, create_branch, create_commit, prepare_commit,
    prepare_operation, read_branches, scan, update_index,
};
use std::fs;
use support::Repo;

/// Reads a single independent Git object id without its transport newline.
fn oid(repo: &Repo, name: &str) -> String {
    String::from_utf8(repo.git(&["rev-parse", name]))
        .unwrap()
        .trim()
        .to_owned()
}
/// Installs only this temporary repository's executable reference hook.
fn reference_hook(repo: &Repo, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    repo.write("hooks/reference-transaction", script);
    fs::set_permissions(
        repo.path().join("hooks/reference-transaction"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
}
/// Creates divergent ordinary merge sides and stops in an actual content conflict.
fn merge_conflict(format: &str) -> Repo {
    let repo = Repo::with_format(format);
    repo.write("file", "base\n");
    repo.initial();
    repo.git(&["switch", "-c", "other"]);
    repo.write("file", "B\n");
    repo.git(&["commit", "-am", "B"]);
    repo.git(&["switch", "main"]);
    repo.write("file", "A\n");
    repo.git(&["commit", "-am", "A"]);
    assert_eq!(repo.command(&["merge", "other"]).status.code(), Some(1));
    repo
}
/// Creates a noninteractive rebase with two independently conflicting replay stops.
fn rebase_conflict(format: &str) -> Repo {
    let repo = Repo::with_format(format);
    repo.write("first", "base one\n");
    repo.write("second", "base two\n");
    repo.initial();
    repo.git(&["switch", "-c", "topic"]);
    repo.write("first", "topic one\n");
    repo.git(&["commit", "-am", "first"]);
    repo.write("second", "topic two\n");
    repo.git(&["commit", "-am", "second"]);
    repo.git(&["switch", "main"]);
    repo.write("first", "main one\n");
    repo.write("second", "main two\n");
    repo.git(&["commit", "-am", "main"]);
    repo.git(&["switch", "topic"]);
    assert_eq!(repo.command(&["rebase", "main"]).status.code(), Some(1));
    repo
}

#[test]
/// Native creation preserves all dirty/index/HEAD bytes and loose/packed branch reads match Git in both formats.
fn branches_are_gix_refs_without_checkout() {
    for format in ["sha1", "sha256"] {
        let repo = Repo::with_format(format);
        repo.write("file", "base\n");
        repo.initial();
        repo.write("file", "index\n");
        repo.git(&["add", "file"]);
        repo.write("file", "worktree\n");
        let head = fs::read(repo.path().join(".git/HEAD")).unwrap();
        let index = fs::read(repo.path().join(".git/index")).unwrap();
        repo.git(&["symbolic-ref", "refs/heads/alias", "refs/heads/main"]);
        let snapshot = prepare_commit(repo.path()).unwrap();
        let result = create_branch(&snapshot, "задача/ветка").unwrap();
        assert_eq!(result.backend, "gix");
        assert_eq!(result.branch.id, oid(&repo, "HEAD"));
        assert_eq!(result.branch.id, oid(&repo, "refs/heads/задача/ветка"));
        assert!(!result.post_hook_failed && !result.reference_warning);
        assert!(
            String::from_utf8(repo.git(&["reflog", "show", "--format=%gs", "задача/ветка"]))
                .unwrap()
                .contains("branch: Created from HEAD")
        );
        for packed in [false, true] {
            if packed {
                repo.git(&["pack-refs", "--all"]);
            }
            let branches = read_branches(repo.path()).unwrap();
            let native: Vec<_> = branches
                .branches
                .iter()
                .map(|branch| format!("{}\t{}", branch.name, branch.id))
                .collect();
            let oracle = String::from_utf8(repo.git(&[
                "for-each-ref",
                "--format=%(refname:short)%09%(objectname)",
                "refs/heads/",
            ]))
            .unwrap();
            assert_eq!(native, oracle.lines().collect::<Vec<_>>());
        }
        assert_eq!(fs::read(repo.path().join(".git/HEAD")).unwrap(), head);
        assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
        assert_eq!(fs::read(repo.path().join("file")).unwrap(), b"worktree\n");
    }
}
#[test]
/// Invalid names, existing refs, stale same-oid branch switches and held locks never create another ref.
fn branch_refusals_are_precise() {
    let repo = Repo::new();
    repo.write("file", "base\n");
    repo.initial();
    let snapshot = prepare_commit(repo.path()).unwrap();
    for name in [
        "",
        "HEAD",
        "-option",
        "space name",
        "a..b",
        "a.lock",
        "../bad",
        "a\nb",
    ] {
        assert!(
            !repo
                .command(&["check-ref-format", "--branch", name])
                .status
                .success()
        );
        assert!(matches!(
            create_branch(&snapshot, name),
            Err(CoreError::BranchName)
        ));
    }
    assert!(matches!(
        create_branch(&snapshot, "main"),
        Err(CoreError::BranchExists)
    ));
    repo.write(".git/index.lock", "held");
    assert!(matches!(
        create_branch(&snapshot, "no-index-lock"),
        Err(CoreError::IndexLock)
    ));
    fs::remove_file(repo.path().join(".git/index.lock")).unwrap();
    repo.write(".git/HEAD.lock", "held");
    assert!(matches!(
        create_branch(&snapshot, "no-head-lock"),
        Err(CoreError::ReferenceUpdate)
    ));
    fs::remove_file(repo.path().join(".git/HEAD.lock")).unwrap();
    repo.write(".git/refs/heads/no-ref-lock.lock", "held");
    assert!(matches!(
        create_branch(&snapshot, "no-ref-lock"),
        Err(CoreError::ReferenceUpdate)
    ));
    fs::remove_file(repo.path().join(".git/refs/heads/no-ref-lock.lock")).unwrap();
    repo.git(&["branch", "same"]);
    repo.git(&["switch", "same"]);
    assert!(matches!(
        create_branch(&snapshot, "stale"),
        Err(CoreError::Stale)
    ));
    let names: Vec<_> = read_branches(repo.path())
        .unwrap()
        .branches
        .into_iter()
        .map(|branch| branch.name)
        .collect();
    assert_eq!(names, ["main", "same"]);
    let unborn = Repo::new();
    assert!(matches!(
        create_branch(&prepare_commit(unborn.path()).unwrap(), "new"),
        Err(CoreError::Unborn)
    ));
    let conflict = merge_conflict("sha1");
    assert!(matches!(
        create_branch(&prepare_commit(conflict.path()).unwrap(), "new"),
        Err(CoreError::ActiveOperation)
    ));
}
#[test]
/// Reference hook phases and byte payloads agree with Git; refusal releases locks and committed failure is success.
fn branch_reference_hooks_match_git() {
    for format in ["sha1", "sha256"] {
        let repo = Repo::with_format(format);
        repo.write("file", "base\n");
        repo.initial();
        reference_hook(
            &repo,
            "#!/bin/sh\nprintf '%s\\n' \"$1\" >> hooks/phases\ncat >> hooks/phases\n",
        );
        repo.git(&["branch", "oracle"]);
        let oracle = fs::read_to_string(repo.path().join("hooks/phases")).unwrap();
        fs::remove_file(repo.path().join("hooks/phases")).unwrap();
        create_branch(&prepare_commit(repo.path()).unwrap(), "native").unwrap();
        let native = fs::read_to_string(repo.path().join("hooks/phases")).unwrap();
        assert_eq!(
            native.replace("refs/heads/native", "refs/heads/oracle"),
            oracle
        );
        for phase in ["preparing", "prepared"] {
            reference_hook(&repo, &format!("#!/bin/sh\n[ \"$1\" != '{phase}' ]\n"));
            assert!(matches!(
                create_branch(&prepare_commit(repo.path()).unwrap(), "rejected"),
                Err(CoreError::Hook("reference-transaction"))
            ));
            assert!(
                !repo
                    .command(&["show-ref", "--verify", "refs/heads/rejected"])
                    .status
                    .success()
            );
            assert!(!repo.path().join(".git/index.lock").exists());
            assert!(!repo.path().join(".git/HEAD.lock").exists());
            assert!(!repo.path().join(".git/refs/heads/rejected.lock").exists());
        }
        reference_hook(&repo, "#!/bin/sh\n[ \"$1\" != committed ]\n");
        let created = create_branch(&prepare_commit(repo.path()).unwrap(), "confirmed").unwrap();
        assert!(created.post_hook_failed);
        assert_eq!(created.branch.id, oid(&repo, "confirmed"));
    }
}
#[test]
/// Branch creation handles detached and linked HEAD without switching either worktree.
fn branch_creation_handles_detached_and_linked() {
    for format in ["sha1", "sha256"] {
        let repo = Repo::with_format(format);
        repo.write("file", "base\n");
        repo.initial();
        repo.git(&["switch", "--detach"]);
        let head = oid(&repo, "HEAD");
        create_branch(&prepare_commit(repo.path()).unwrap(), "from-detached").unwrap();
        assert_eq!(oid(&repo, "HEAD"), head);
        assert!(!repo.command(&["symbolic-ref", "HEAD"]).status.success());
        let linked = repo.path().join("linked");
        repo.git(&[
            "worktree",
            "add",
            "-b",
            "linked-branch",
            linked.to_str().unwrap(),
            "main",
        ]);
        let root_index = fs::read(repo.path().join(".git/index")).unwrap();
        let snapshot = prepare_commit(&linked).unwrap();
        let result = create_branch(&snapshot, "from-linked").unwrap();
        assert_eq!(result.branch.id, head);
        assert_eq!(
            repo.git(&[
                "-C",
                linked.to_str().unwrap(),
                "symbolic-ref",
                "--short",
                "HEAD"
            ]),
            b"linked-branch\n"
        );
        assert_eq!(
            fs::read(repo.path().join(".git/index")).unwrap(),
            root_index
        );
    }
}
#[test]
/// Symbolic HEAD chains resolve for reads/branch creation and changed underlying refs invalidate index writes.
fn symbolic_head_is_resolved_or_explicitly_refused() {
    let repo = Repo::new();
    repo.write("file", "base\n");
    repo.initial();
    repo.git(&["symbolic-ref", "refs/heads/alias", "refs/heads/main"]);
    repo.git(&["symbolic-ref", "HEAD", "refs/heads/alias"]);
    let old = oid(&repo, "HEAD");
    assert_eq!(
        prepare_commit(repo.path()).unwrap().head.as_deref(),
        Some(old.as_str())
    );
    assert_eq!(
        scan(repo.path()).unwrap().head.as_deref(),
        Some(old.as_str())
    );
    assert_eq!(
        create_branch(&prepare_commit(repo.path()).unwrap(), "from-alias")
            .unwrap()
            .branch
            .id,
        old
    );
    assert!(
        read_branches(repo.path())
            .unwrap()
            .branches
            .iter()
            .any(|branch| branch.name == "alias" && branch.id == old)
    );
    repo.write("file", "changed\n");
    let diff = repo.diff("file", DiffKind::Unstaged);
    assert!(matches!(
        create_commit(&prepare_commit(repo.path()).unwrap(), "message"),
        Err(CoreError::CommitUnsupported("symbolic-head"))
    ));
    let tree = oid(&repo, "HEAD^{tree}");
    let next =
        String::from_utf8(repo.git(&["commit-tree", &tree, "-p", &old, "-m", "other"])).unwrap();
    repo.git(&["update-ref", "refs/heads/main", next.trim()]);
    let index = fs::read(repo.path().join(".git/index")).unwrap();
    assert!(matches!(
        update_index(repo.path(), &diff, &Selection::File),
        Err(CoreError::Stale)
    ));
    assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
}
#[test]
/// Fresh merge snapshots preserve exact index and recover real side provenance across metadata changes and abort.
fn external_merge_metadata_recovers_and_guards_bytes() {
    for format in ["sha1", "sha256"] {
        let repo = merge_conflict(format);
        let index = fs::read(repo.path().join(".git/index")).unwrap();
        let worktree = fs::read(repo.path().join("file")).unwrap();
        let snapshot = prepare_operation(repo.path()).unwrap();
        let state = snapshot.state.as_ref().unwrap();
        assert_eq!(state.kind, Operation::Merge);
        assert_eq!(state.incoming, [oid(&repo, "other")]);
        assert_eq!(state.original_head, Some(oid(&repo, "main")));
        assert_eq!(prepare_operation(repo.path()).unwrap(), snapshot);
        repo.write(".git/MERGE_MSG", "Different user message\n");
        assert_ne!(prepare_operation(repo.path()).unwrap(), snapshot);
        assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
        assert_eq!(fs::read(repo.path().join("file")).unwrap(), worktree);
        repo.git(&["merge", "--abort"]);
        assert!(prepare_operation(repo.path()).unwrap().state.is_none());
    }
}
#[test]
/// Two external rebase stops recover the original topic, current upstream A and replayed B with correct progress.
fn external_rebase_metadata_tracks_two_stops() {
    for format in ["sha1", "sha256"] {
        let repo = rebase_conflict(format);
        let topic = oid(&repo, "topic");
        let first = oid(&repo, "topic^");
        let main = oid(&repo, "main");
        let snapshot = prepare_operation(repo.path()).unwrap();
        let state = snapshot.state.unwrap();
        assert_eq!(state.kind, Operation::Rebase);
        assert_eq!(state.original_branch.as_deref(), Some("topic"));
        assert_eq!(state.original_head.as_deref(), Some(topic.as_str()));
        assert_eq!(state.onto.as_deref(), Some(main.as_str()));
        assert_eq!(state.progress, Some((1, 2)));
        assert_eq!(state.incoming, [first]);
        assert_eq!(repo.git(&["show", ":2:first"]), b"main one\n");
        assert_eq!(repo.git(&["show", ":3:first"]), b"topic one\n");
        repo.write("first", "решение первое\n");
        repo.git(&["add", "first"]);
        assert_eq!(
            repo.command(&["-c", "core.editor=:", "rebase", "--continue"])
                .status
                .code(),
            Some(1)
        );
        let second = prepare_operation(repo.path()).unwrap();
        let state = second.state.unwrap();
        assert_eq!(state.progress, Some((2, 2)));
        assert_eq!(state.incoming, [topic]);
        assert_eq!(repo.git(&["show", ":2:second"]), b"main two\n");
        assert_eq!(repo.git(&["show", ":3:second"]), b"topic two\n");
        repo.write("second", "решение второе\n");
        repo.git(&["add", "second"]);
        repo.git(&["-c", "core.editor=:", "rebase", "--continue"]);
        assert!(prepare_operation(repo.path()).unwrap().state.is_none());
        assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), b"topic\n");
    }
}
#[test]
/// Unsafe/sized metadata is rejected without following an external symlink or modifying operation/index/worktree.
fn unsafe_operation_metadata_is_an_explicit_refusal() {
    let repo = merge_conflict("sha1");
    let index = fs::read(repo.path().join(".git/index")).unwrap();
    let external = tempfile::tempdir().unwrap();
    let target = external.path().join("private-message");
    fs::write(&target, "outside\n").unwrap();
    fs::remove_file(repo.path().join(".git/MERGE_MSG")).unwrap();
    std::os::unix::fs::symlink(&target, repo.path().join(".git/MERGE_MSG")).unwrap();
    assert!(matches!(
        prepare_operation(repo.path()),
        Err(CoreError::OperationMetadata)
    ));
    assert_eq!(fs::read(&target).unwrap(), b"outside\n");
    fs::remove_file(repo.path().join(".git/MERGE_MSG")).unwrap();
    repo.write(".git/MERGE_MSG", vec![b'x'; 4 * 1024 * 1024 + 1]);
    assert!(matches!(
        prepare_operation(repo.path()),
        Err(CoreError::OperationMetadata)
    ));
    assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
    assert!(repo.path().join(".git/MERGE_HEAD").exists());
    repo.write(".git/MERGE_MSG", "message\n");
    fs::create_dir(repo.path().join(".git/rebase-merge")).unwrap();
    fs::create_dir(repo.path().join(".git/rebase-apply")).unwrap();
    assert!(matches!(
        prepare_operation(repo.path()),
        Err(CoreError::OperationMetadata)
    ));
    assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
}
