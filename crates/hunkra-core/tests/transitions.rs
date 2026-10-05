//! Coordinator writes use disposable repositories and independent Git object/index/worktree oracles.
mod support;
use hunkra_core::{
    Branch, CoreError, DiffKind, Operation, OperationAction as Action,
    OperationCapability as Capability, OperationOutcome as Outcome, OperationSnapshot,
    OperationWrite, prepare_operation, read_branches, run_operation,
};
use std::{fs, path::Path};
use support::Repo;

/// Resolves the selected advertised branch through the production gix read API.
fn branch(repo: &Repo, name: &str) -> Branch {
    read_branches(repo.path())
        .unwrap()
        .branches
        .into_iter()
        .find(|branch| branch.name == name)
        .unwrap()
}
/// Reads an independent Git object id without a transport newline.
fn oid(repo: &Repo, name: &str) -> String {
    String::from_utf8(repo.git(&["rev-parse", name]))
        .unwrap()
        .trim()
        .to_owned()
}
/// Prepares a fresh immutable operation snapshot for one accepted action.
fn run(repo: &Repo, action: Action) -> Result<OperationWrite, CoreError> {
    run_operation(&prepare_operation(repo.path()).unwrap(), &action)
}
/// Creates divergent sides with either clean disjoint changes or one actual content conflict.
fn divergent(format: &str, conflict: bool) -> Repo {
    let repo = if format == "sha1" {
        Repo::new()
    } else {
        Repo::with_format(format)
    };
    repo.write("first", "base one\n");
    repo.write("second", "base two\n");
    repo.initial();
    repo.git(&["switch", "-c", "other"]);
    repo.write("first", "other one\n");
    repo.git(&["commit", "-am", "other"]);
    repo.git(&["switch", "main"]);
    repo.write(if conflict { "first" } else { "second" }, "main change\n");
    repo.git(&["commit", "-am", "main"]);
    repo
}
/// Creates a topic with two independent conflicts, ready to start a rebase onto main.
fn two_stops(format: &str) -> Repo {
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
    repo
}
/// Checks the fixed preflight route and absence of retained arbitrary CLI diagnostics.
fn route(write: &OperationWrite, capability: Capability, outcome: Outcome) {
    assert_eq!(write.route.backend, "git-cli");
    assert_eq!(write.route.capability, capability);
    assert_eq!(write.outcome, outcome);
}

#[test]
/// Clean switching accepts Unicode branches and carries unrelated dirty bytes in both object formats.
fn switch_updates_only_safe_paths_and_never_creates_a_branch() {
    for format in ["sha1", "sha256"] {
        let repo = Repo::with_format(format);
        repo.write("first", "base\n");
        repo.write("second", "unchanged\n");
        repo.initial();
        repo.git(&["switch", "-c", "задача/ветка"]);
        repo.write("first", "target\n");
        repo.git(&["commit", "-am", "target"]);
        repo.git(&["switch", "main"]);
        repo.write("second", "index\n");
        repo.git(&["add", "second"]);
        repo.write("second", "worktree\n");
        repo.write("untracked", "keep\n");
        let target = branch(&repo, "задача/ветка");
        let result = run(&repo, Action::Switch(target.clone())).unwrap();
        route(
            &result,
            Capability::WorktreeSwitchCoordinator,
            Outcome::Completed,
        );
        assert_eq!(result.head, Some(target.id));
        assert!(result.diagnostic_bytes > 0);
        assert_eq!(
            repo.git(&["symbolic-ref", "--short", "HEAD"]),
            "задача/ветка\n".as_bytes()
        );
        assert_eq!(repo.git(&["show", ":first"]), b"target\n");
        assert_eq!(repo.git(&["show", ":second"]), b"index\n");
        let dirty = repo.diff("second", DiffKind::Unstaged);
        assert_eq!(dirty.left.bytes, b"index\n");
        assert_eq!(dirty.right.bytes, b"worktree\n");
        assert_eq!(fs::read(repo.path().join("first")).unwrap(), b"target\n");
        assert_eq!(fs::read(repo.path().join("second")).unwrap(), b"worktree\n");
        assert_eq!(fs::read(repo.path().join("untracked")).unwrap(), b"keep\n");
        assert_eq!(read_branches(repo.path()).unwrap().branches.len(), 2);
        let unchanged = run(&repo, Action::Switch(branch(&repo, "задача/ветка"))).unwrap();
        route(
            &unchanged,
            Capability::WorktreeSwitchCoordinator,
            Outcome::Completed,
        );
    }
}

#[test]
/// Overlapping dirty/untracked files, held index locks and checked-out linked branches are preserved on refusal.
fn switch_refusals_protect_local_content_and_linked_worktrees() {
    for format in ["sha1", "sha256"] {
        let repo = Repo::with_format(format);
        repo.write("file", "base\n");
        repo.initial();
        repo.git(&["switch", "-c", "other"]);
        repo.write("file", "target\n");
        repo.write("new", "tracked target\n");
        repo.git(&["add", "."]);
        repo.git(&["commit", "-m", "other"]);
        repo.git(&["switch", "main"]);
        let target = branch(&repo, "other");
        let head = oid(&repo, "HEAD");
        repo.write("file", "dirty keep\n");
        assert!(matches!(
            run(&repo, Action::Switch(target.clone())),
            Err(CoreError::OperationExecution { .. })
        ));
        assert_eq!(fs::read(repo.path().join("file")).unwrap(), b"dirty keep\n");
        assert_eq!(oid(&repo, "HEAD"), head);
        repo.git(&["restore", "file"]);
        repo.write("new", "untracked keep\n");
        assert!(matches!(
            run(&repo, Action::Switch(target.clone())),
            Err(CoreError::OperationExecution { .. })
        ));
        assert_eq!(
            fs::read(repo.path().join("new")).unwrap(),
            b"untracked keep\n"
        );
        assert_eq!(oid(&repo, "HEAD"), head);
        fs::remove_file(repo.path().join("new")).unwrap();
        let index = fs::read(repo.path().join(".git/index")).unwrap();
        repo.write(".git/index.lock", "foreign lock\n");
        assert!(matches!(
            run(&repo, Action::Switch(target.clone())),
            Err(CoreError::OperationExecution { .. })
        ));
        assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
        assert_eq!(
            fs::read(repo.path().join(".git/index.lock")).unwrap(),
            b"foreign lock\n"
        );
        fs::remove_file(repo.path().join(".git/index.lock")).unwrap();
        let linked = repo.path().join("linked");
        repo.git(&["worktree", "add", linked.to_str().unwrap(), "other"]);
        assert!(matches!(
            run(&repo, Action::Switch(target)),
            Err(CoreError::OperationExecution { .. })
        ));
        assert_eq!(oid(&repo, "HEAD"), head);
        assert_eq!(fs::read(linked.join("file")).unwrap(), b"target\n");
    }
}

#[test]
/// Stale target refs, index/HEAD changes and modified todo/message bytes refuse before launching a write.
fn transition_snapshots_bind_targets_and_private_metadata() {
    let repo = divergent("sha1", true);
    let target = branch(&repo, "other");
    let snapshot = prepare_operation(repo.path()).unwrap();
    repo.git(&["update-ref", "refs/heads/other", &oid(&repo, "main")]);
    let index = fs::read(repo.path().join(".git/index")).unwrap();
    assert!(matches!(
        run_operation(&snapshot, &Action::Merge(target)),
        Err(CoreError::Stale)
    ));
    assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
    let target = branch(&repo, "other");
    repo.git(&["branch", "-D", "other"]);
    assert!(matches!(
        run_operation(&snapshot, &Action::Switch(target)),
        Err(CoreError::BranchMissing)
    ));
    repo.git(&["branch", "same"]);
    let snapshot = prepare_operation(repo.path()).unwrap();
    repo.git(&["switch", "same"]);
    assert!(matches!(
        run_operation(&snapshot, &Action::Switch(branch(&repo, "main"))),
        Err(CoreError::Stale)
    ));
    let snapshot = prepare_operation(repo.path()).unwrap();
    repo.write("first", "index changed\n");
    repo.git(&["add", "first"]);
    assert!(matches!(
        run_operation(&snapshot, &Action::Switch(branch(&repo, "main"))),
        Err(CoreError::Stale)
    ));
    for action in [Action::Continue, Action::Abort, Action::Skip] {
        assert!(matches!(run(&repo, action), Err(CoreError::NoOperation)));
    }
    let repo = two_stops("sha1");
    assert_eq!(repo.command(&["rebase", "main"]).status.code(), Some(1));
    let snapshot = prepare_operation(repo.path()).unwrap();
    let todo = repo.path().join(".git/rebase-merge/git-rebase-todo");
    let mut bytes = fs::read(&todo).unwrap();
    bytes.extend_from_slice(b"# externally edited\n");
    fs::write(todo, bytes).unwrap();
    let index = fs::read(repo.path().join(".git/index")).unwrap();
    assert!(matches!(
        run_operation(&snapshot, &Action::Abort),
        Err(CoreError::Stale)
    ));
    assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
}

#[test]
/// Normal merges create exactly two expected parents, while ordinary rebase replays onto the selected base.
fn clean_merge_and_noninteractive_rebase_preserve_expected_trees() {
    for format in ["sha1", "sha256"] {
        let repo = divergent(format, false);
        let old = oid(&repo, "main");
        let other = branch(&repo, "other");
        let result = run(&repo, Action::Merge(other.clone())).unwrap();
        route(
            &result,
            Capability::MergeRepositoryCoordinator,
            Outcome::Completed,
        );
        assert_eq!(oid(&repo, "HEAD^1"), old);
        assert_eq!(oid(&repo, "HEAD^2"), other.id);
        assert_eq!(repo.git(&["show", "HEAD:first"]), b"other one\n");
        assert_eq!(repo.git(&["show", "HEAD:second"]), b"main change\n");
        assert!(result.state.is_none());
        assert_eq!(repo.git(&["status", "--porcelain"]), b"");
        let repo = divergent(format, false);
        repo.git(&["switch", "other"]);
        repo.git(&["branch", "preserve-topic"]);
        repo.git(&["config", "--local", "rebase.updateRefs", "true"]);
        repo.git(&["config", "--local", "rebase.autoSquash", "true"]);
        let old = oid(&repo, "other");
        let main = branch(&repo, "main");
        let result = run(&repo, Action::Rebase(main.clone())).unwrap();
        route(
            &result,
            Capability::RebaseSequencerCoordinator,
            Outcome::Completed,
        );
        assert_eq!(oid(&repo, "HEAD^"), main.id);
        assert_eq!(oid(&repo, "preserve-topic"), old);
        assert_ne!(oid(&repo, "HEAD"), old);
        assert_eq!(repo.git(&["show", "HEAD:first"]), b"other one\n");
        assert_eq!(repo.git(&["show", "HEAD:second"]), b"main change\n");
        assert!(result.state.is_none());
        assert_eq!(repo.git(&["status", "--porcelain"]), b"");
    }
}

#[test]
/// Merge conflicts are normal stops; fresh recovery supports resolve/continue and abort without an ordinary Commit.
fn merge_conflict_continue_and_abort_recover_actual_state() {
    for format in ["sha1", "sha256"] {
        for abort in [false, true] {
            let repo = divergent(format, true);
            let old = oid(&repo, "main");
            let other = branch(&repo, "other");
            let result = run(&repo, Action::Merge(other.clone())).unwrap();
            route(
                &result,
                Capability::MergeRepositoryCoordinator,
                Outcome::StoppedWithConflicts,
            );
            assert_eq!(
                result.state.as_ref().unwrap().incoming.as_slice(),
                std::slice::from_ref(&other.id)
            );
            assert!(matches!(
                run(&repo, Action::Continue),
                Err(CoreError::Unresolved)
            ));
            assert!(matches!(
                run(&repo, Action::Skip),
                Err(CoreError::OperationAction)
            ));
            assert!(matches!(
                run(&repo, Action::Switch(branch(&repo, "other"))),
                Err(CoreError::ActiveOperation)
            ));
            if abort {
                let result = run(&repo, Action::Abort).unwrap();
                route(
                    &result,
                    Capability::MergeRepositoryCoordinator,
                    Outcome::Completed,
                );
                assert_eq!(oid(&repo, "HEAD"), old);
                assert_eq!(
                    fs::read(repo.path().join("first")).unwrap(),
                    b"main change\n"
                );
            } else {
                repo.write("first", "Результат слияния\n");
                repo.git(&["add", "first"]);
                let result = run(&repo, Action::Continue).unwrap();
                route(
                    &result,
                    Capability::MergeRepositoryCoordinator,
                    Outcome::Completed,
                );
                assert_eq!(oid(&repo, "HEAD^1"), old);
                assert_eq!(oid(&repo, "HEAD^2"), other.id);
                assert_eq!(
                    repo.git(&["show", "HEAD:first"]),
                    "Результат слияния\n".as_bytes()
                );
            }
            assert!(prepare_operation(repo.path()).unwrap().state.is_none());
            assert_eq!(repo.git(&["status", "--porcelain"]), b"");
        }
    }
}

#[test]
/// Both externally started and app-started rebases resume two conflicts with original topic/upstream provenance.
fn rebase_two_stops_continue_skip_and_abort() {
    for format in ["sha1", "sha256"] {
        for external in [false, true] {
            let repo = two_stops(format);
            let old = oid(&repo, "topic");
            let main = branch(&repo, "main");
            if external {
                assert_eq!(repo.command(&["rebase", "main"]).status.code(), Some(1));
            } else {
                let result = run(&repo, Action::Rebase(main.clone())).unwrap();
                route(
                    &result,
                    Capability::RebaseSequencerCoordinator,
                    Outcome::StoppedWithConflicts,
                );
            }
            let first = prepare_operation(repo.path()).unwrap();
            let state = first.state.as_ref().unwrap();
            assert_eq!(state.kind, Operation::Rebase);
            assert_eq!(state.original_head.as_deref(), Some(old.as_str()));
            assert_eq!(state.onto.as_deref(), Some(main.id.as_str()));
            assert_eq!(state.progress, Some((1, 2)));
            assert!(matches!(
                run_operation(&first, &Action::Continue),
                Err(CoreError::Unresolved)
            ));
            repo.write("first", "Решение первое\n");
            repo.git(&["add", "first"]);
            let result = run(&repo, Action::Continue).unwrap();
            route(
                &result,
                Capability::RebaseSequencerCoordinator,
                Outcome::StoppedWithConflicts,
            );
            assert_eq!(result.state.as_ref().unwrap().progress, Some((2, 2)));
            assert_eq!(repo.git(&["show", ":2:second"]), b"main two\n");
            assert_eq!(repo.git(&["show", ":3:second"]), b"topic two\n");
            repo.write("second", "Решение второе\n");
            repo.git(&["add", "second"]);
            let result = run(&repo, Action::Continue).unwrap();
            route(
                &result,
                Capability::RebaseSequencerCoordinator,
                Outcome::Completed,
            );
            assert_eq!(oid(&repo, "HEAD^^"), main.id);
            assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), b"topic\n");
            assert_eq!(
                repo.git(&["show", "HEAD:first"]),
                "Решение первое\n".as_bytes()
            );
            assert_eq!(
                repo.git(&["show", "HEAD:second"]),
                "Решение второе\n".as_bytes()
            );
            assert_eq!(repo.git(&["status", "--porcelain"]), b"");
        }
        for abort in [false, true] {
            let repo = two_stops(format);
            let old = oid(&repo, "topic");
            let main = oid(&repo, "main");
            assert_eq!(repo.command(&["rebase", "main"]).status.code(), Some(1));
            if abort {
                run(&repo, Action::Abort).unwrap();
                assert_eq!(oid(&repo, "HEAD"), old);
                assert_eq!(fs::read(repo.path().join("first")).unwrap(), b"topic one\n");
                assert_eq!(
                    fs::read(repo.path().join("second")).unwrap(),
                    b"topic two\n"
                );
            } else {
                let result = run(&repo, Action::Skip).unwrap();
                route(
                    &result,
                    Capability::RebaseSequencerCoordinator,
                    Outcome::StoppedWithConflicts,
                );
                assert_eq!(result.state.as_ref().unwrap().progress, Some((2, 2)));
                let result = run(&repo, Action::Skip).unwrap();
                route(
                    &result,
                    Capability::RebaseSequencerCoordinator,
                    Outcome::Completed,
                );
                assert_eq!(oid(&repo, "HEAD"), main);
            }
            assert!(prepare_operation(repo.path()).unwrap().state.is_none());
            assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), b"topic\n");
        }
    }
}

#[test]
/// Hook rejection and configured autostash never silently bypass failure or discard local dirty bytes.
fn coordinator_hooks_and_dirty_rebase_are_not_retried() {
    use std::os::unix::fs::PermissionsExt;
    let repo = divergent("sha1", false);
    repo.write("hooks/pre-merge-commit", "#!/bin/sh\nexit 1\n");
    fs::set_permissions(
        repo.path().join("hooks/pre-merge-commit"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let old = oid(&repo, "HEAD");
    assert!(matches!(
        run(&repo, Action::Merge(branch(&repo, "other"))),
        Err(CoreError::OperationExecution { .. })
    ));
    assert_eq!(oid(&repo, "HEAD"), old);
    assert_eq!(
        prepare_operation(repo.path()).unwrap().state.unwrap().kind,
        Operation::Merge
    );
    run(&repo, Action::Abort).unwrap();
    let repo = divergent("sha1", false);
    repo.git(&["switch", "other"]);
    repo.git(&["config", "--local", "rebase.autoStash", "true"]);
    repo.write("first", "dirty owner bytes\n");
    let old = oid(&repo, "HEAD");
    assert!(matches!(
        run(&repo, Action::Rebase(branch(&repo, "main"))),
        Err(CoreError::OperationExecution { .. })
    ));
    assert_eq!(oid(&repo, "HEAD"), old);
    assert_eq!(
        fs::read(repo.path().join("first")).unwrap(),
        b"dirty owner bytes\n"
    );
    assert_eq!(repo.git(&["stash", "list"]), b"");
    assert!(prepare_operation(repo.path()).unwrap().state.is_none());
}

#[test]
/// Linked-worktree actions use their actual index/gitdir rather than modifying the main checkout.
fn linked_worktree_coordinator_context_is_explicit() {
    for format in ["sha1", "sha256"] {
        let repo = divergent(format, false);
        repo.git(&["branch", "linked-topic", "other"]);
        let linked = repo.path().join("linked");
        repo.git(&["worktree", "add", linked.to_str().unwrap(), "linked-topic"]);
        let index = fs::read(repo.path().join(".git/index")).unwrap();
        let head = fs::read(repo.path().join(".git/HEAD")).unwrap();
        let first = fs::read(repo.path().join("first")).unwrap();
        let snapshot: OperationSnapshot = prepare_operation(&linked).unwrap();
        let result = run_operation(&snapshot, &Action::Merge(branch(&repo, "main"))).unwrap();
        route(
            &result,
            Capability::MergeRepositoryCoordinator,
            Outcome::Completed,
        );
        assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
        assert_eq!(fs::read(repo.path().join(".git/HEAD")).unwrap(), head);
        assert_eq!(fs::read(repo.path().join("first")).unwrap(), first);
        assert_eq!(fs::read(linked.join("first")).unwrap(), b"other one\n");
        assert_eq!(fs::read(linked.join("second")).unwrap(), b"main change\n");
        assert!(
            prepare_operation(Path::new(&linked))
                .unwrap()
                .state
                .is_none()
        );
    }
}

#[test]
/// Both diagnostic pipes drain concurrently with constant memory, and the child/hook finish before success.
fn large_hook_diagnostics_are_drained_without_retaining_content() {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repo::new();
    repo.write("file", "base\n");
    repo.initial();
    repo.git(&["branch", "other"]);
    repo.write("hooks/post-checkout", "#!/bin/sh\nhead -c 2097152 /dev/zero\nhead -c 2097152 /dev/zero >&2\nprintf done > hook-finished\n");
    fs::set_permissions(
        repo.path().join("hooks/post-checkout"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let result = run(&repo, Action::Switch(branch(&repo, "other"))).unwrap();
    assert!(result.diagnostic_bytes >= 4 * 1024 * 1024);
    assert_eq!(
        fs::read(repo.path().join("hook-finished")).unwrap(),
        b"done"
    );
    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), b"other\n");
}

#[test]
/// Externally started apply-backend rebases use their actual metadata and resume two resolved conflicts.
fn external_apply_rebase_recovers_two_stops() {
    for format in ["sha1", "sha256"] {
        let repo = two_stops(format);
        let main = oid(&repo, "main");
        assert_eq!(
            repo.command(&["rebase", "--apply", "main"]).status.code(),
            Some(1)
        );
        let state = prepare_operation(repo.path()).unwrap().state.unwrap();
        assert_eq!(state.kind, Operation::Rebase);
        assert_eq!(state.progress, Some((1, 2)));
        assert_eq!(state.original_branch.as_deref(), Some("topic"));
        assert_eq!(state.onto.as_deref(), Some(main.as_str()));
        repo.write("first", "apply result first\n");
        repo.git(&["add", "first"]);
        let result = run(&repo, Action::Continue).unwrap();
        route(
            &result,
            Capability::RebaseSequencerCoordinator,
            Outcome::StoppedWithConflicts,
        );
        assert_eq!(result.state.as_ref().unwrap().progress, Some((2, 2)));
        repo.write("second", "apply result second\n");
        repo.git(&["add", "second"]);
        let result = run(&repo, Action::Continue).unwrap();
        route(
            &result,
            Capability::RebaseSequencerCoordinator,
            Outcome::Completed,
        );
        assert_eq!(oid(&repo, "HEAD^^"), main);
        assert_eq!(repo.git(&["show", "HEAD:first"]), b"apply result first\n");
        assert_eq!(repo.git(&["show", "HEAD:second"]), b"apply result second\n");
    }
}

#[test]
/// An external interactive edit stop remains a review stop after Continue, never a falsely completed rebase.
fn external_interactive_edit_stop_is_reported_as_review() {
    use std::os::unix::fs::PermissionsExt;
    let repo = divergent("sha1", false);
    repo.git(&["switch", "other"]);
    repo.write("third", "topic third\n");
    repo.git(&["add", "third"]);
    repo.git(&["commit", "-m", "third"]);
    let main = oid(&repo, "main");
    repo.write(
        "hooks/edit-sequence",
        "#!/bin/sh\nsed -i 's/^pick /edit /' \"$1\"\n",
    );
    let script = repo.path().join("hooks/edit-sequence");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    repo.git(&[
        "-c",
        &format!("sequence.editor={}", script.display()),
        "rebase",
        "-i",
        "main",
    ]);
    let state = prepare_operation(repo.path()).unwrap().state.unwrap();
    assert_eq!(state.progress, Some((1, 2)));
    assert!(state.interactive);
    let result = run(&repo, Action::Continue).unwrap();
    route(
        &result,
        Capability::RebaseSequencerCoordinator,
        Outcome::StoppedForReview,
    );
    assert_eq!(result.state.as_ref().unwrap().progress, Some((2, 2)));
    let result = run(&repo, Action::Continue).unwrap();
    route(
        &result,
        Capability::RebaseSequencerCoordinator,
        Outcome::Completed,
    );
    assert_eq!(oid(&repo, "HEAD^^"), main);
    assert_eq!(repo.git(&["symbolic-ref", "--short", "HEAD"]), b"other\n");
}
