//! Independent Git oracles for index-only commit, configured hooks and signatures.
mod support;
use hunkra_core::{CoreError, create_commit, prepare_commit};
use std::{fs, process::Command};
use support::Repo;

#[test]
/// A+B in the index is committed while C and untracked/intent-to-add files stay in the worktree.
fn commit_uses_only_the_approved_index() {
    for format in ["sha1", "sha256"] {
        let repo = Repo::with_format(format);
        repo.write("f", "A\nB\nC\n");
        repo.initial();
        let parent = repo.git(&["rev-parse", "HEAD"]);
        repo.write("f", "A staged\nB staged\nC\n");
        repo.git(&["add", "f"]);
        repo.write("f", "A staged\nB staged\nC working\n");
        repo.write("new", "untracked\n");
        repo.write("intent", "not staged\n");
        repo.git(&["add", "-N", "intent"]);
        let index = fs::read(repo.path().join(".git/index")).unwrap();
        assert_eq!(
            repo.diff("f", hunkra_core::DiffKind::Staged).right.bytes,
            b"A staged\nB staged\nC\n"
        );
        let message = "  Исправить расчёт  \n\n\nОписание\t\n# оставить\n";
        let result = create_commit(&prepare_commit(repo.path()).unwrap(), message).unwrap();
        assert_eq!(result.backend, "gix");
        assert!(!result.signed && !result.post_hook_failed);
        assert_eq!(repo.git(&["show", "HEAD:f"]), b"A staged\nB staged\nC\n");
        assert_eq!(repo.git(&["rev-parse", "HEAD^"]), parent);
        assert_eq!(
            repo.git(&["show", "-s", "--format=%B", "HEAD"]),
            "  Исправить расчёт\n\nОписание\n# оставить\n\n".as_bytes()
        );
        assert_eq!(
            fs::read(repo.path().join("f")).unwrap(),
            b"A staged\nB staged\nC working\n"
        );
        assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
        assert!(
            !repo
                .command(&["cat-file", "-e", "HEAD:new"])
                .status
                .success()
        );
        assert!(
            !repo
                .command(&["cat-file", "-e", "HEAD:intent"])
                .status
                .success()
        );
        let reflog = repo.git(&["reflog", "-1", "--format=%gs", "HEAD"]);
        assert!(reflog.starts_with(b"commit:"));
        assert!(
            String::from_utf8(reflog)
                .unwrap()
                .contains("Исправить расчёт")
        );
        assert!(
            repo.git(&["show", "-s", "--format=%an <%ae>"])
                .starts_with(b"Hunkra Test <test@example.invalid>")
        );
        assert!(!repo.path().join(".git/index.lock").exists());
        assert!(!repo.path().join(".git/HEAD.lock").exists());
    }
}

#[test]
/// Initial, detached and linked-worktree refs are advanced without changing another checkout's HEAD.
fn commit_unborn_detached_and_linked_worktree() {
    let repo = Repo::new();
    repo.write("f", "initial\n");
    repo.git(&["add", "f"]);
    create_commit(&prepare_commit(repo.path()).unwrap(), "first").unwrap();
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), b"1\n");
    repo.git(&["switch", "--detach"]);
    repo.write("f", "detached\n");
    repo.git(&["add", "f"]);
    create_commit(&prepare_commit(repo.path()).unwrap(), "detached").unwrap();
    assert!(!repo.command(&["symbolic-ref", "HEAD"]).status.success());
    let current = repo.git(&["rev-parse", "HEAD"]);
    let linked = repo.path().join("linked");
    repo.git(&["worktree", "add", "-b", "linked", linked.to_str().unwrap()]);
    fs::write(linked.join("f"), "linked\n").unwrap();
    repo.git(&["-C", linked.to_str().unwrap(), "add", "f"]);
    create_commit(&prepare_commit(&linked).unwrap(), "linked").unwrap();
    assert_eq!(
        repo.git(&["-C", linked.to_str().unwrap(), "show", "HEAD:f"]),
        b"linked\n"
    );
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), current);
    assert_eq!(fs::read(repo.path().join("f")).unwrap(), b"detached\n");
}

#[test]
/// Empty/invalid/stale/index-lock/ref-lock attempts never change the approved index or HEAD.
fn commit_refusals_keep_index_and_head() {
    let repo = Repo::new();
    repo.write("f", "base\n");
    repo.initial();
    let head = repo.git(&["rev-parse", "HEAD"]);
    assert!(matches!(
        create_commit(&prepare_commit(repo.path()).unwrap(), "nothing"),
        Err(CoreError::NothingStaged)
    ));
    repo.write("f", "staged\n");
    repo.git(&["add", "f"]);
    let snapshot = prepare_commit(repo.path()).unwrap();
    let index = fs::read(repo.path().join(".git/index")).unwrap();
    assert!(matches!(
        create_commit(&snapshot, " \n\t"),
        Err(CoreError::CommitMessage)
    ));
    assert!(matches!(
        create_commit(&snapshot, "bad\0message"),
        Err(CoreError::CommitMessage)
    ));
    assert!(matches!(
        create_commit(&snapshot, &"x".repeat(65537)),
        Err(CoreError::CommitMessage)
    ));
    for (lock, error) in [
        ("index.lock", "index"),
        ("refs/heads/main.lock", "ref"),
        ("HEAD.lock", "ref"),
    ] {
        let path = repo.path().join(".git").join(lock);
        fs::write(&path, "another writer").unwrap();
        let result = create_commit(&snapshot, "locked");
        assert!(
            matches!(
                (&result, error),
                (Err(CoreError::IndexLock), "index") | (Err(CoreError::ReferenceUpdate), "ref")
            ),
            "{result:?}"
        );
        assert_eq!(fs::read(&path).unwrap(), b"another writer");
        fs::remove_file(path).unwrap();
        assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
        assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
    }
    repo.git(&["switch", "-c", "same-oid"]);
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
    assert!(matches!(
        create_commit(&snapshot, "old branch"),
        Err(CoreError::Stale)
    ));
    let snapshot = prepare_commit(repo.path()).unwrap();
    repo.write("f", "new index\n");
    repo.git(&["add", "f"]);
    assert!(matches!(
        create_commit(&snapshot, "old index"),
        Err(CoreError::Stale)
    ));
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(repo.git(&["show", ":f"]), b"new index\n");
}

#[cfg(unix)]
/// Installs only this fixture's executable hook, preserving the owner's hooks and configuration.
fn install_hook(repo: &Repo, name: &str, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = repo.path().join("hooks").join(name);
    fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
/// Commit hooks run in repository context; message edits are honored and post failure is success.
fn commit_hooks_and_failures() {
    let repo = Repo::new();
    repo.write("f", "base\n");
    repo.initial();
    repo.write("f", "staged\n");
    repo.git(&["add", "f"]);
    install_hook(
        &repo,
        "pre-commit",
        "test \"$GIT_EDITOR\" = : || exit 1\ntest -n \"$GIT_DIR\" || exit 1\ngit show :f > hook-index\necho pre >> hook-order",
    );
    install_hook(
        &repo,
        "prepare-commit-msg",
        "test \"$2\" = message || exit 1\necho prepare >> hook-order\nprintf 'hook title\\n\\nbody\\n' > \"$1\"",
    );
    install_hook(
        &repo,
        "commit-msg",
        "echo message >> hook-order\necho trailer >> \"$1\"",
    );
    install_hook(
        &repo,
        "post-commit",
        "git rev-parse HEAD > hook-head\necho post >> hook-order\nexit 42",
    );
    let result = create_commit(&prepare_commit(repo.path()).unwrap(), "user text").unwrap();
    assert!(result.post_hook_failed);
    assert_eq!(
        fs::read(repo.path().join("hook-order")).unwrap(),
        b"pre\nprepare\nmessage\npost\n"
    );
    assert_eq!(
        fs::read(repo.path().join("hook-index")).unwrap(),
        b"staged\n"
    );
    assert_eq!(
        fs::read(repo.path().join("hook-head")).unwrap(),
        repo.git(&["rev-parse", "HEAD"])
    );
    assert_eq!(
        repo.git(&["show", "-s", "--format=%B"]),
        b"hook title\n\nbody\ntrailer\n\n"
    );
    for name in ["pre-commit", "prepare-commit-msg", "commit-msg"] {
        fs::remove_dir_all(repo.path().join("hooks")).unwrap();
        fs::create_dir(repo.path().join("hooks")).unwrap();
        install_hook(&repo, name, "exit 9");
        repo.write("f", format!("staged {name}\n"));
        repo.git(&["add", "f"]);
        let head = repo.git(&["rev-parse", "HEAD"]);
        let index = fs::read(repo.path().join(".git/index")).unwrap();
        assert!(
            matches!(create_commit(&prepare_commit(repo.path()).unwrap(), "refused"), Err(CoreError::Hook(hook)) if hook == name)
        );
        assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
        assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
        assert!(!repo.path().join(".git/index.lock").exists());
    }
}

#[cfg(unix)]
#[test]
/// Hook-induced index changes require review instead of silently adding them to the commit.
fn hook_index_change_is_not_committed_without_review() {
    let repo = Repo::new();
    repo.write("f", "base\n");
    repo.initial();
    repo.write("f", "selected\n");
    repo.git(&["add", "f"]);
    let head = repo.git(&["rev-parse", "HEAD"]);
    install_hook(
        &repo,
        "pre-commit",
        "printf 'hook change\\n' > f\ngit add f",
    );
    assert!(matches!(
        create_commit(&prepare_commit(repo.path()).unwrap(), "review"),
        Err(CoreError::Stale)
    ));
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(repo.git(&["show", ":f"]), b"hook change\n");
}

#[test]
/// Missing identity and an externally started conflicted merge refuse ordinary Commit.
fn commit_identity_and_operation_errors() {
    let repo = Repo::new();
    repo.write("f", "initial\n");
    repo.git(&["add", "f"]);
    repo.git(&["config", "--unset", "user.name"]);
    repo.git(&["config", "--unset", "user.email"]);
    assert!(matches!(
        create_commit(&prepare_commit(repo.path()).unwrap(), "no identity"),
        Err(CoreError::Identity)
    ));
    repo.git(&["config", "user.name", "Hunkra Test"]);
    repo.git(&["config", "user.email", "test@example.invalid"]);
    repo.initial();
    repo.git(&["switch", "-c", "other"]);
    repo.write("f", "other\n");
    repo.git(&["commit", "-am", "other"]);
    repo.git(&["switch", "main"]);
    repo.write("f", "main\n");
    repo.git(&["commit", "-am", "main"]);
    assert!(!repo.command(&["merge", "other"]).status.success());
    let index = fs::read(repo.path().join(".git/index")).unwrap();
    let head = repo.git(&["rev-parse", "HEAD"]);
    assert!(matches!(
        create_commit(&prepare_commit(repo.path()).unwrap(), "merge"),
        Err(CoreError::ActiveOperation)
    ));
    assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
    // The same unresolved stages without MERGE_HEAD are still not an ordinary commit tree.
    fs::remove_file(repo.path().join(".git/MERGE_HEAD")).unwrap();
    assert!(matches!(
        create_commit(&prepare_commit(repo.path()).unwrap(), "unresolved"),
        Err(CoreError::Unresolved)
    ));
}

#[cfg(unix)]
#[test]
/// Actual SSH signatures verify with Git for both object formats; signer failure never retries unsigned.
fn commit_signing_is_verified_by_git() {
    for format in ["sha1", "sha256"] {
        let repo = Repo::with_format(format);
        repo.write("f", "signed content\n");
        repo.git(&["add", "f"]);
        let key = repo.path().join("signing-key");
        let output = Command::new("/usr/bin/ssh-keygen")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&key)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        repo.git(&["config", "commit.gpgSign", "true"]);
        repo.git(&["config", "gpg.format", "ssh"]);
        repo.git(&["config", "user.signingKey", key.to_str().unwrap()]);
        install_hook(&repo, "pre-commit", "git config commit.gpgSign false");
        let result = create_commit(&prepare_commit(repo.path()).unwrap(), "Подписано").unwrap();
        assert!(result.signed && result.backend == "gix");
        let signers = repo.path().join("allowed-signers");
        fs::write(
            &signers,
            format!(
                "test@example.invalid {}",
                fs::read_to_string(key.with_extension("pub")).unwrap()
            ),
        )
        .unwrap();
        repo.git(&[
            "config",
            "gpg.ssh.allowedSignersFile",
            signers.to_str().unwrap(),
        ]);
        repo.git(&["verify-commit", "HEAD"]);
        repo.write("f", "next\n");
        repo.git(&["add", "f"]);
        fs::remove_file(repo.path().join("hooks/pre-commit")).unwrap();
        repo.git(&["config", "commit.gpgSign", "true"]);
        let head = repo.git(&["rev-parse", "HEAD"]);
        let index = fs::read(repo.path().join(".git/index")).unwrap();
        install_hook(&repo, "failing-signer", "exit 17");
        repo.git(&[
            "config",
            "gpg.ssh.program",
            repo.path().join("hooks/failing-signer").to_str().unwrap(),
        ]);
        assert!(matches!(
            create_commit(&prepare_commit(repo.path()).unwrap(), "refused signature"),
            Err(CoreError::Signing)
        ));
        assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
        assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
        assert!(!repo.path().join(".git/index.lock").exists());
    }
}

#[cfg(unix)]
#[test]
/// Atomic message replacement never follows an existing message symlink to an unrelated file.
fn commit_message_symlink_does_not_overwrite_external_content() {
    use std::os::unix::fs::symlink;
    let repo = Repo::new();
    repo.write("f", "initial\n");
    repo.git(&["add", "f"]);
    let external = tempfile::tempdir().unwrap();
    let target = external.path().join("untouched");
    fs::write(&target, "external").unwrap();
    symlink(&target, repo.path().join(".git/COMMIT_EDITMSG")).unwrap();
    create_commit(&prepare_commit(repo.path()).unwrap(), "message").unwrap();
    assert_eq!(fs::read(target).unwrap(), b"external");
    assert!(
        !fs::symlink_metadata(repo.path().join(".git/COMMIT_EDITMSG"))
            .unwrap()
            .is_symlink()
    );
}

#[cfg(unix)]
#[test]
/// Reference hooks observe queued HEAD and locked branch, rejecting before refs change.
fn reference_transaction_hook_matches_git_phases() {
    for format in ["sha1", "sha256"] {
        for detached in [false, true] {
            let repo = Repo::with_format(format);
            repo.write("f", "base\n");
            repo.initial();
            if detached {
                repo.git(&["switch", "--detach"]);
            }
            let parent = String::from_utf8(repo.git(&["rev-parse", "HEAD"]))
                .unwrap()
                .trim()
                .to_string();
            repo.write("f", "staged\n");
            repo.git(&["add", "f"]);
            install_hook(
                &repo,
                "reference-transaction",
                "echo \"$1\" >> ref-events\ncat >> ref-events\nif test \"$1\" = prepared; then test -f \"$GIT_DIR/HEAD.lock\" || exit 4; fi",
            );
            let result =
                create_commit(&prepare_commit(repo.path()).unwrap(), "reference hooks").unwrap();
            let resolved = if detached { "HEAD" } else { "refs/heads/main" };
            let expected = format!(
                "preparing\n{parent} {} HEAD\nprepared\n{parent} {} {resolved}\ncommitted\n{parent} {} {resolved}\n",
                result.id, result.id, result.id
            );
            assert_eq!(
                fs::read_to_string(repo.path().join("ref-events")).unwrap(),
                expected
            );
            assert!(!result.post_hook_failed && !result.reference_warning);
            for phase in ["preparing", "prepared"] {
                repo.write("f", format!("staged {phase}\n"));
                repo.git(&["add", "f"]);
                let head = repo.git(&["rev-parse", "HEAD"]);
                let index = fs::read(repo.path().join(".git/index")).unwrap();
                install_hook(
                    &repo,
                    "reference-transaction",
                    &format!("cat > /dev/null\ntest \"$1\" != {phase}"),
                );
                assert!(matches!(
                    create_commit(&prepare_commit(repo.path()).unwrap(), "rejected refs"),
                    Err(CoreError::Hook("reference-transaction"))
                ));
                assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
                assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
                assert!(!repo.path().join(".git/index.lock").exists());
                assert!(!repo.path().join(".git/HEAD.lock").exists());
            }
        }
    }
}

#[test]
/// Configured cleanup produces the same Unicode message bytes as noninteractive Git commit -F.
fn commit_cleanup_matches_independent_git() {
    for mode in ["default", "whitespace", "strip", "scissors", "verbatim"] {
        let actual = Repo::new();
        let oracle = Repo::new();
        let message = "\n  Кириллица  \n\n\nТекст\t\n# comment\n\n";
        for repo in [&actual, &oracle] {
            repo.write("f", "initial\n");
            repo.git(&["add", "f"]);
            repo.git(&["config", "commit.cleanup", mode]);
            repo.write("message", message);
        }
        oracle.git(&["commit", "-F", "message"]);
        create_commit(&prepare_commit(actual.path()).unwrap(), message).unwrap();
        assert_eq!(
            actual.git(&["show", "-s", "--format=%B"]),
            oracle.git(&["show", "-s", "--format=%B"]),
            "{mode}"
        );
    }
}

#[test]
/// Unsupported message configuration fails before writing messages, objects, index or refs.
fn commit_configuration_limits_are_explicit() {
    for (key, value, reason) in [
        ("i18n.commitEncoding", "ISO-8859-1", "encoding"),
        ("commit.cleanup", "unknown", "cleanup"),
    ] {
        let repo = Repo::new();
        repo.write("f", "initial\n");
        repo.git(&["add", "f"]);
        repo.git(&["config", key, value]);
        let index = fs::read(repo.path().join(".git/index")).unwrap();
        assert!(
            matches!(create_commit(&prepare_commit(repo.path()).unwrap(), "not created"), Err(CoreError::CommitUnsupported(actual)) if actual == reason)
        );
        assert!(
            !repo
                .command(&["rev-parse", "--verify", "HEAD"])
                .status
                .success()
        );
        assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
        assert!(!repo.path().join(".git/COMMIT_EDITMSG").exists());
    }
    let repo = Repo::new();
    repo.write("f", "initial\n");
    repo.git(&["add", "f"]);
    repo.git(&["config", "commit.cleanup", "strip"]);
    repo.git(&["config", "core.commentChar", "auto"]);
    assert!(matches!(
        create_commit(&prepare_commit(repo.path()).unwrap(), "message"),
        Err(CoreError::CommitUnsupported("comment-auto"))
    ));
    repo.git(&["config", "commit.cleanup", "default"]);
    repo.git(&["config", "commit.gpgSign", "invalid"]);
    assert!(matches!(
        create_commit(&prepare_commit(repo.path()).unwrap(), "invalid signing"),
        Err(CoreError::Signing)
    ));
}

#[cfg(unix)]
#[test]
/// Deep Unicode paths, deletions, executable modes, symlinks and binary blobs are index content only.
fn commit_preserves_staged_paths_modes_and_binary_bytes() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let repo = Repo::new();
    repo.write("deleted", "remove me\n");
    repo.initial();
    repo.git(&["rm", "deleted"]);
    fs::create_dir_all(repo.path().join("Папка с пробелом/sub")).unwrap();
    let file = "Папка с пробелом/sub/data";
    repo.write(file, b"\0\xff\x01");
    repo.write("exec", "executable\n");
    fs::set_permissions(repo.path().join("exec"), fs::Permissions::from_mode(0o755)).unwrap();
    symlink("nonexistent", repo.path().join("link")).unwrap();
    repo.git(&["add", file, "exec", "link"]);
    let index = fs::read(repo.path().join(".git/index")).unwrap();
    repo.write(file, b"worktree\0other");
    create_commit(&prepare_commit(repo.path()).unwrap(), "modes").unwrap();
    assert_eq!(repo.git(&["show", &format!("HEAD:{file}")]), b"\0\xff\x01");
    assert_eq!(repo.git(&["show", "HEAD:link"]), b"nonexistent");
    assert!(
        repo.git(&["ls-tree", "HEAD", "exec"])
            .starts_with(b"100755 blob ")
    );
    assert!(
        repo.git(&["ls-tree", "HEAD", "link"])
            .starts_with(b"120000 blob ")
    );
    assert!(
        !repo
            .command(&["cat-file", "-e", "HEAD:deleted"])
            .status
            .success()
    );
    assert_eq!(
        fs::read(repo.path().join(file)).unwrap(),
        b"worktree\0other"
    );
    assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
}
