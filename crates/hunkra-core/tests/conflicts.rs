//! Conflict writes are checked by independent Git in isolated temporary repositories.
mod support;
use hunkra_core::{
    CoreError, Document, OperationAction, OperationOutcome, Presentation,
    conflict_chunks::{ChunkChoice, choose_chunk, chunks},
    mark_resolved, prepare_operation, read_conflict, run_operation, save_result,
};
use std::{fs, path::Path};
use support::Repo;

/// Creates two independently conflicted files with exact stage provenance.
fn merge(format: &str) -> Repo {
    let repo = Repo::with_format(format);
    repo.write("f", "base\n");
    repo.write("g", "base g\n");
    repo.initial();
    repo.git(&["switch", "-c", "incoming"]);
    repo.write("f", "incoming\n");
    repo.write("g", "incoming g\n");
    repo.git(&["commit", "-am", "incoming"]);
    repo.git(&["switch", "main"]);
    repo.write("f", "current\n");
    repo.write("g", "current g\n");
    repo.git(&["commit", "-am", "current"]);
    assert_eq!(
        repo.command(&["merge", "--no-edit", "incoming"])
            .status
            .code(),
        Some(1)
    );
    repo
}

#[test]
/// Save leaves index conflicts intact; resolve keeps REUC and Git can reopen the exact stages.
fn save_resolve_and_restart_preserve_stages_and_other_paths() {
    for format in ["sha1", "sha256"] {
        for version in ["2", "3", "4"] {
            let repo = merge(format);
            repo.git(&["update-index", &format!("--index-version={version}")]);
            repo.write("g", "external resolution\n");
            repo.git(&["add", "g"]);
            let other_undo = repo.git(&["ls-files", "--resolve-undo", "--", "g"]);
            let stages = repo.git(&["ls-files", "-u", "--", "f"]);
            let before = read_conflict(repo.path(), Path::new("f")).unwrap();
            assert_eq!(before.presentation, Presentation::Text);
            assert_eq!(before.base.git.bytes, b"base\n");
            assert_eq!(before.a.git.bytes, b"current\n");
            assert_eq!(before.b.git.bytes, b"incoming\n");
            for (number, side) in [(1, &before.base), (2, &before.a), (3, &before.b)] {
                assert_eq!(side.git.bytes, repo.git(&["show", &format!(":{number}:f")]));
                assert_eq!(
                    side.id.as_deref(),
                    Some(
                        String::from_utf8(repo.git(&["rev-parse", &format!(":{number}:f")]))
                            .unwrap()
                            .trim()
                    )
                );
            }
            let index = fs::read(repo.path().join(".git/index")).unwrap();
            let operation = fs::read(repo.path().join(".git/MERGE_HEAD")).unwrap();
            let result = Document::present(
                "Результат 🦀\r\nбез последнего LF".as_bytes().to_vec(),
                Some(0o100644),
            );
            assert_eq!(save_result(&before, &result).unwrap().backend, "filesystem");
            assert_eq!(fs::read(repo.path().join("f")).unwrap(), result.bytes);
            assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
            assert_eq!(repo.git(&["ls-files", "-u", "--", "f"]), stages);
            assert!(matches!(
                run_operation(
                    &prepare_operation(repo.path()).unwrap(),
                    &OperationAction::Continue
                ),
                Err(CoreError::Unresolved)
            ));
            assert!(matches!(mark_resolved(&before), Err(CoreError::Stale)));
            let restarted = read_conflict(repo.path(), Path::new("f")).unwrap();
            assert_eq!(restarted.result, result);
            assert_eq!(mark_resolved(&restarted).unwrap().backend, "gix");
            assert!(repo.git(&["ls-files", "-u", "--", "f"]).is_empty());
            assert_eq!(repo.git(&["show", ":f"]), result.bytes);
            assert_eq!(
                repo.git(&["ls-files", "--resolve-undo", "--", "g"]),
                other_undo
            );
            assert_eq!(
                fs::read(repo.path().join(".git/MERGE_HEAD")).unwrap(),
                operation
            );
            let resolved = read_conflict(repo.path(), Path::new("f")).unwrap();
            assert!(resolved.resolved);
            assert_eq!(resolved.base, before.base);
            assert_eq!(resolved.a, before.a);
            assert_eq!(resolved.b, before.b);
            repo.git(&["checkout", "-m", "--", "f"]);
            assert_eq!(repo.git(&["ls-files", "-u", "--", "f"]), stages);
            let reopened = read_conflict(repo.path(), Path::new("f")).unwrap();
            save_result(&reopened, &result).unwrap();
            mark_resolved(&read_conflict(repo.path(), Path::new("f")).unwrap()).unwrap();
            assert_eq!(
                run_operation(
                    &prepare_operation(repo.path()).unwrap(),
                    &OperationAction::Continue
                )
                .unwrap()
                .outcome,
                OperationOutcome::Completed
            );
            assert_eq!(repo.git(&["show", "HEAD:f"]), result.bytes);
            assert!(matches!(
                read_conflict(repo.path(), Path::new("f")),
                Err(CoreError::NoConflict)
            ));
        }
    }
}

#[test]
/// Both replay stops show upstream as A and the original topic commit as B, never swapped.
fn rebase_two_stops_keep_provenance_and_separate_continue() {
    for format in ["sha1", "sha256"] {
        let repo = Repo::with_format(format);
        repo.write("f", "base f\n");
        repo.write("g", "base g\n");
        repo.initial();
        repo.git(&["switch", "-c", "topic"]);
        repo.write("f", "topic f\n");
        repo.git(&["commit", "-am", "first"]);
        repo.write("g", "topic g\n");
        repo.git(&["commit", "-am", "second"]);
        repo.git(&["switch", "main"]);
        repo.write("f", "upstream f\n");
        repo.write("g", "upstream g\n");
        repo.git(&["commit", "-am", "upstream"]);
        repo.git(&["switch", "topic"]);
        assert_eq!(repo.command(&["rebase", "main"]).status.code(), Some(1));
        for (offset, file) in ["f", "g"].into_iter().enumerate() {
            let snapshot = read_conflict(repo.path(), Path::new(file)).unwrap();
            assert_eq!(
                snapshot.a.git.bytes,
                format!("upstream {file}\n").as_bytes()
            );
            assert_eq!(snapshot.b.git.bytes, format!("topic {file}\n").as_bytes());
            assert_eq!(snapshot.base.git.bytes, format!("base {file}\n").as_bytes());
            save_result(&snapshot, &snapshot.b.worktree).unwrap();
            mark_resolved(&read_conflict(repo.path(), Path::new(file)).unwrap()).unwrap();
            let outcome = run_operation(
                &prepare_operation(repo.path()).unwrap(),
                &OperationAction::Continue,
            )
            .unwrap()
            .outcome;
            assert_eq!(
                outcome,
                if offset == 0 {
                    OperationOutcome::StoppedWithConflicts
                } else {
                    OperationOutcome::Completed
                }
            );
        }
        assert_eq!(repo.git(&["show", "HEAD:f"]), b"topic f\n");
        assert_eq!(repo.git(&["show", "HEAD:g"]), b"topic g\n");
    }
}

#[test]
/// Missing Base in add/add and missing A/B in modify/delete remain explicit presence choices.
fn add_add_and_modify_delete_keep_or_delete() {
    for format in ["sha1", "sha256"] {
        for deletion in [false, true] {
            for keep in [false, true] {
                let repo = Repo::with_format(format);
                repo.write("unrelated", "unchanged\n");
                if deletion {
                    repo.write("f", "base\n");
                }
                repo.initial();
                repo.git(&["switch", "-c", "incoming"]);
                repo.write("f", "incoming\n");
                repo.git(&["add", "f"]);
                repo.git(&["commit", "-m", "incoming"]);
                repo.git(&["switch", "main"]);
                if deletion {
                    repo.git(&["rm", "f"]);
                } else {
                    repo.write("f", "current\n");
                    repo.git(&["add", "f"]);
                }
                repo.git(&["commit", "-m", "current"]);
                assert_eq!(
                    repo.command(&["merge", "--no-edit", "incoming"])
                        .status
                        .code(),
                    Some(1)
                );
                let snapshot = read_conflict(repo.path(), Path::new("f")).unwrap();
                assert_eq!(snapshot.base.git.present, deletion);
                assert_eq!(snapshot.a.git.present, !deletion);
                assert!(snapshot.b.git.present);
                let desired = if keep {
                    snapshot.b.worktree.clone()
                } else {
                    Document::absent()
                };
                save_result(&snapshot, &desired).unwrap();
                assert_eq!(repo.path().join("f").exists(), keep);
                mark_resolved(&read_conflict(repo.path(), Path::new("f")).unwrap()).unwrap();
                assert!(repo.git(&["ls-files", "-u"]).is_empty());
                assert_eq!(repo.command(&["show", ":f"]).status.success(), keep);
                assert_eq!(repo.git(&["show", ":unrelated"]), b"unchanged\n");
                run_operation(
                    &prepare_operation(repo.path()).unwrap(),
                    &OperationAction::Continue,
                )
                .unwrap();
                assert_eq!(repo.command(&["show", "HEAD:f"]).status.success(), keep);
            }
        }
    }
}

#[test]
/// Deletion on either side is distinct from an empty file, including the REUC stage modes.
fn modify_delete_on_either_side_has_explicit_keep_delete() {
    for format in ["sha1", "sha256"] {
        for delete_a in [false, true] {
            for keep in [false, true] {
                let repo = Repo::with_format(format);
                repo.write("f", "base\n");
                repo.initial();
                repo.git(&["switch", "-c", "incoming"]);
                if delete_a {
                    repo.write("f", "incoming\n");
                } else {
                    repo.git(&["rm", "f"]);
                }
                repo.git(&["commit", "-am", "incoming"]);
                repo.git(&["switch", "main"]);
                if delete_a {
                    repo.git(&["rm", "f"]);
                } else {
                    repo.write("f", "current\n");
                }
                repo.git(&["commit", "-am", "current"]);
                assert_eq!(
                    repo.command(&["merge", "--no-edit", "incoming"])
                        .status
                        .code(),
                    Some(1)
                );
                let snapshot = read_conflict(repo.path(), Path::new("f")).unwrap();
                assert_eq!(snapshot.a.git.present, !delete_a);
                assert_eq!(snapshot.b.git.present, delete_a);
                let desired = if !keep {
                    Document::absent()
                } else if delete_a {
                    snapshot.b.worktree.clone()
                } else {
                    snapshot.a.worktree.clone()
                };
                save_result(&snapshot, &desired).unwrap();
                mark_resolved(&read_conflict(repo.path(), Path::new("f")).unwrap()).unwrap();
                let recovered = read_conflict(repo.path(), Path::new("f")).unwrap();
                assert_eq!(recovered.a, snapshot.a);
                assert_eq!(recovered.b, snapshot.b);
                assert_eq!(repo.command(&["show", ":f"]).status.success(), keep);
                assert_eq!(recovered.result.present, keep);
            }
        }
    }
}

#[test]
/// Whole saved files without markers still require explicit resolution; markers may be deliberate text.
fn marker_absence_is_not_resolution_and_literal_markers_are_allowed() {
    let repo = merge("sha1");
    assert_eq!(
        repo.diff("f", hunkra_core::DiffKind::Unstaged).presentation,
        Presentation::Conflict
    );
    let snapshot = read_conflict(repo.path(), Path::new("f")).unwrap();
    save_result(&snapshot, &snapshot.a.worktree).unwrap();
    assert!(!read_conflict(repo.path(), Path::new("f")).unwrap().resolved);
    assert!(!repo.git(&["ls-files", "-u", "--", "f"]).is_empty());
    let literal = Document::present(b"<<<<<<< literal text\n".to_vec(), Some(0o100644));
    save_result(
        &read_conflict(repo.path(), Path::new("f")).unwrap(),
        &literal,
    )
    .unwrap();
    mark_resolved(&read_conflict(repo.path(), Path::new("f")).unwrap()).unwrap();
    assert_eq!(repo.git(&["show", ":f"]), literal.bytes);
}

#[test]
/// Editing a previously resolved Result never stages implicitly; a second explicit resolve updates it.
fn saved_result_after_resolution_requires_another_index_action() {
    let repo = merge("sha1");
    let snapshot = read_conflict(repo.path(), Path::new("f")).unwrap();
    save_result(&snapshot, &snapshot.a.worktree).unwrap();
    mark_resolved(&read_conflict(repo.path(), Path::new("f")).unwrap()).unwrap();
    let before = read_conflict(repo.path(), Path::new("f")).unwrap();
    assert!(before.resolved && before.result_staged);
    let desired = Document::present(b"second result\n".to_vec(), Some(0o100644));
    save_result(&before, &desired).unwrap();
    assert_eq!(repo.git(&["show", ":f"]), b"current\n");
    let after = read_conflict(repo.path(), Path::new("f")).unwrap();
    assert!(after.resolved && !after.result_staged);
    mark_resolved(&after).unwrap();
    assert_eq!(repo.git(&["show", ":f"]), desired.bytes);
    let recovered = read_conflict(repo.path(), Path::new("f")).unwrap();
    assert_eq!(recovered.a, before.a);
    assert_eq!(recovered.b, before.b);
    assert!(recovered.result_staged);
}

#[test]
/// Exact snapshots refuse changed Result, index, operation and raw HEAD identity before any write.
fn stale_and_foreign_locks_preserve_every_file() {
    for change in [
        "worktree",
        "index",
        "state",
        "head",
        "index-lock",
        "file-lock",
    ] {
        let repo = merge("sha1");
        let snapshot = read_conflict(repo.path(), Path::new("f")).unwrap();
        match change {
            "worktree" => repo.write("f", "external editor\n"),
            "index" => {
                repo.write("new", "staged\n");
                repo.git(&["add", "new"]);
            }
            "state" => repo.write(".git/MERGE_MSG", "external message\n"),
            "head" => {
                repo.git(&["branch", "alias"]);
                repo.git(&["symbolic-ref", "HEAD", "refs/heads/alias"]);
            }
            "index-lock" => repo.write(".git/index.lock", "foreign index lock"),
            "file-lock" => repo.write("f.lock", "foreign file lock"),
            _ => unreachable!(),
        }
        let index = fs::read(repo.path().join(".git/index")).unwrap();
        let result = fs::read(repo.path().join("f")).unwrap();
        let error = save_result(&snapshot, &snapshot.b.worktree).unwrap_err();
        assert!(
            match change {
                "index-lock" => matches!(error, CoreError::IndexLock),
                "file-lock" => matches!(error, CoreError::WorktreeLock),
                _ => matches!(error, CoreError::Stale),
            },
            "{change}: {error:?}"
        );
        assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
        assert_eq!(fs::read(repo.path().join("f")).unwrap(), result);
        if change == "index-lock" {
            assert_eq!(
                fs::read(repo.path().join(".git/index.lock")).unwrap(),
                b"foreign index lock"
            );
        } else {
            assert!(!repo.path().join(".git/index.lock").exists());
        }
        if change == "file-lock" {
            assert_eq!(
                fs::read(repo.path().join("f.lock")).unwrap(),
                b"foreign file lock"
            );
        } else {
            assert!(!repo.path().join("f.lock").exists());
        }
    }
}

#[test]
/// Chunk choices remove only one balanced block and preserve CRLF, BOM, common text and explicit order.
fn chunk_choices_preserve_context_and_custom_diff3_markers() {
    let source = "\u{feff}prefix\r\n<<<<<<<<< A\r\nлево 🦀\r\n||||||||| Base\r\nбаза\r\n=========\r\nправо\r\n>>>>>>>>> B\r\nmiddle\r\n<<<<<<< A\nsecond A\n=======\nsecond B\n>>>>>>> B\nsuffix".as_bytes();
    let parsed = chunks(source).unwrap();
    assert_eq!(parsed.len(), 2);
    assert_eq!(
        &source[parsed[0].base.clone().unwrap()],
        "база\r\n".as_bytes()
    );
    for (choice, expected) in [
        (ChunkChoice::A, "лево 🦀\r\n"),
        (ChunkChoice::B, "право\r\n"),
        (ChunkChoice::AThenB, "лево 🦀\r\nправо\r\n"),
        (ChunkChoice::BThenA, "право\r\nлево 🦀\r\n"),
    ] {
        let chosen = choose_chunk(source, 0, choice).unwrap();
        assert!(chosen.starts_with(format!("\u{feff}prefix\r\n{expected}middle\r\n").as_bytes()));
        assert!(chosen.ends_with(b"suffix"));
        assert_eq!(chunks(&chosen).unwrap().len(), 1);
        let final_result = choose_chunk(&chosen, 0, ChunkChoice::B).unwrap();
        assert!(final_result.ends_with(b"middle\r\nsecond B\nsuffix"));
        assert!(chunks(&final_result).unwrap().is_empty());
    }
    for malformed in [
        b"<<<<<<< A\na\n=======\nb\n".as_slice(),
        b"=======\n",
        b"<<<<<<< A\n<<<<<<< nested\n=======\nb\n>>>>>>> B\n",
    ] {
        assert!(matches!(chunks(malformed), Err(CoreError::ConflictChunk)));
    }
    assert!(matches!(
        choose_chunk(source, 2, ChunkChoice::A),
        Err(CoreError::ConflictChunk)
    ));
}

#[test]
/// Checkout/clean attributes retain exact worktree bytes and Git's normalized stage-zero blob.
fn attributes_encodings_filters_and_binary_external_resolution() {
    for format in ["sha1", "sha256"] {
        for variant in ["crlf", "filter", "utf16", "binary", "invalid"] {
            let repo = Repo::with_format(format);
            match variant {
                "crlf" => repo.write(".gitattributes", "f text eol=crlf\n"),
                "utf16" => repo.write(
                    ".gitattributes",
                    "f working-tree-encoding=UTF-16LE text eol=crlf\n",
                ),
                "filter" => {
                    repo.write(".gitattributes", "f filter=test\n");
                    repo.git(&["config", "filter.test.clean", "sed 's/WORK/GIT/g'"]);
                    repo.git(&["config", "filter.test.smudge", "sed 's/GIT/WORK/g'"]);
                    repo.git(&["config", "filter.test.required", "true"]);
                }
                _ => {}
            }
            let encode = |text: &str| -> Vec<u8> {
                match variant {
                    "utf16" => text.encode_utf16().flat_map(u16::to_le_bytes).collect(),
                    "binary" => [text.as_bytes(), b"\0"].concat(),
                    "invalid" => [text.as_bytes(), b"\xff"].concat(),
                    _ => text.as_bytes().to_vec(),
                }
            };
            repo.write("f", encode("WORK base\r\n"));
            repo.initial();
            repo.git(&["switch", "-c", "incoming"]);
            repo.write("f", encode("WORK incoming\r\n"));
            repo.git(&["commit", "-am", "incoming"]);
            repo.git(&["switch", "main"]);
            repo.write("f", encode("WORK current\r\n"));
            repo.git(&["commit", "-am", "current"]);
            assert_eq!(
                repo.command(&["merge", "--no-edit", "incoming"])
                    .status
                    .code(),
                Some(1)
            );
            let snapshot = read_conflict(repo.path(), Path::new("f")).unwrap();
            for (stage, side) in [(1, &snapshot.base), (2, &snapshot.a), (3, &snapshot.b)] {
                assert_eq!(side.git.bytes, repo.git(&["show", &format!(":{stage}:f")]));
                if variant == "utf16" {
                    assert!(side.worktree.unsupported);
                    continue;
                }
                assert_eq!(
                    side.worktree.bytes,
                    repo.git(&["cat-file", "--filters", &format!(":{stage}:f")])
                );
            }
            let desired = encode("WORK решение\r\nпоследняя строка");
            if matches!(variant, "binary" | "utf16" | "invalid") {
                assert_ne!(snapshot.presentation, Presentation::Text);
                let before = fs::read(repo.path().join("f")).unwrap();
                assert!(matches!(
                    save_result(
                        &snapshot,
                        &Document::present(desired.clone(), Some(0o100644))
                    ),
                    Err(CoreError::ConflictUnsupported)
                ));
                assert_eq!(fs::read(repo.path().join("f")).unwrap(), before);
                repo.write("f", &desired); // The explicitly supported external-editor path.
            } else {
                assert_eq!(snapshot.presentation, Presentation::Text);
                save_result(
                    &snapshot,
                    &Document::present(desired.clone(), Some(0o100644)),
                )
                .unwrap();
            }
            let expected = repo.git(&["hash-object", "--path=f", "f"]);
            mark_resolved(&read_conflict(repo.path(), Path::new("f")).unwrap()).unwrap();
            assert_eq!(repo.git(&["rev-parse", ":f"]), expected);
            assert_eq!(fs::read(repo.path().join("f")).unwrap(), desired);
        }
    }
}

#[test]
/// A required clean filter failure preserves the original conflicted index and saved Result.
fn failed_filter_and_resolve_lock_never_publish_an_index() {
    let repo = merge("sha1");
    repo.write(".gitattributes", "f filter=fail\n");
    repo.git(&["config", "filter.fail.clean", "false"]);
    repo.git(&["config", "filter.fail.smudge", "cat"]);
    repo.git(&["config", "filter.fail.required", "true"]);
    let snapshot = read_conflict(repo.path(), Path::new("f")).unwrap();
    let index = fs::read(repo.path().join(".git/index")).unwrap();
    let result = fs::read(repo.path().join("f")).unwrap();
    assert!(mark_resolved(&snapshot).is_err());
    assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
    assert_eq!(fs::read(repo.path().join("f")).unwrap(), result);
    assert!(!repo.path().join(".git/index.lock").exists());
    repo.write(".git/index.lock", "foreign");
    assert!(matches!(
        mark_resolved(&snapshot),
        Err(CoreError::IndexLock)
    ));
    assert_eq!(
        fs::read(repo.path().join(".git/index.lock")).unwrap(),
        b"foreign"
    );
}

#[test]
/// Linked writes use their own index/gitdir and do not mutate the main checkout or operation.
fn linked_worktree_result_and_resolution_are_independent() {
    let repo = merge("sha256");
    repo.git(&["merge", "--abort"]);
    let linked = tempfile::tempdir().unwrap();
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "linked",
        linked.path().to_str().unwrap(),
    ]);
    assert_eq!(
        repo.command(&[
            "-C",
            linked.path().to_str().unwrap(),
            "merge",
            "--no-edit",
            "incoming"
        ])
        .status
        .code(),
        Some(1)
    );
    let main_index = fs::read(repo.path().join(".git/index")).unwrap();
    let main_file = fs::read(repo.path().join("f")).unwrap();
    let snapshot = read_conflict(linked.path(), Path::new("f")).unwrap();
    save_result(&snapshot, &snapshot.b.worktree).unwrap();
    mark_resolved(&read_conflict(linked.path(), Path::new("f")).unwrap()).unwrap();
    assert_eq!(fs::read(linked.path().join("f")).unwrap(), b"incoming\n");
    assert_eq!(
        fs::read(repo.path().join(".git/index")).unwrap(),
        main_index
    );
    assert_eq!(fs::read(repo.path().join("f")).unwrap(), main_file);
    assert!(!repo.path().join(".git/MERGE_HEAD").exists());
}

#[test]
/// Metadata paths, symlink replacement and oversized Result are refused before any file is written.
fn unsafe_paths_symlinks_and_size_limits() {
    let repo = Repo::new();
    repo.write("f", "base\n");
    repo.initial();
    for path in [
        ".git/HEAD",
        ".Git/config",
        "nested/.git/index",
        "../f",
        "/f",
    ] {
        assert!(matches!(
            read_conflict(repo.path(), Path::new(path)),
            Err(CoreError::InvalidPath)
        ));
        assert!(matches!(
            hunkra_core::read_diff(
                repo.path(),
                Path::new(path),
                hunkra_core::DiffKind::Unstaged
            ),
            Err(CoreError::InvalidPath)
        ));
    }
    let repo = merge("sha1");
    let snapshot = read_conflict(repo.path(), Path::new("f")).unwrap();
    let too_large = Document::present(vec![b'a'; hunkra_core::diff::MAX_BYTES + 1], Some(0o100644));
    let before = fs::read(repo.path().join("f")).unwrap();
    assert!(matches!(
        save_result(&snapshot, &too_large),
        Err(CoreError::ConflictUnsupported)
    ));
    assert_eq!(fs::read(repo.path().join("f")).unwrap(), before);
    #[cfg(unix)]
    {
        fs::remove_file(repo.path().join("f")).unwrap();
        std::os::unix::fs::symlink("g", repo.path().join("f")).unwrap();
        let source = read_conflict(repo.path(), Path::new("f")).unwrap();
        assert_eq!(source.presentation, Presentation::Unsupported);
        assert!(matches!(
            save_result(&source, &source.a.worktree),
            Err(CoreError::ConflictUnsupported)
        ));
        assert!(matches!(
            mark_resolved(&source),
            Err(CoreError::ConflictUnsupported)
        ));
        assert_eq!(
            fs::read_link(repo.path().join("f")).unwrap(),
            Path::new("g")
        );
    }
}

#[test]
#[cfg(unix)]
/// Atomic Result replacement keeps file/index permissions and explicit executable side choices.
fn executable_modes_and_permissions_match_git_add() {
    use std::os::unix::fs::PermissionsExt;
    for file_mode in ["true", "false"] {
        let repo = merge("sha1");
        repo.git(&["config", "core.fileMode", file_mode]);
        fs::set_permissions(repo.path().join("f"), fs::Permissions::from_mode(0o640)).unwrap();
        fs::set_permissions(
            repo.path().join(".git/index"),
            fs::Permissions::from_mode(0o640),
        )
        .unwrap();
        let snapshot = read_conflict(repo.path(), Path::new("f")).unwrap();
        let desired = Document::present(b"executable result\n".to_vec(), Some(0o100755));
        save_result(&snapshot, &desired).unwrap();
        assert_eq!(
            fs::metadata(repo.path().join("f"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o751
        );
        let index_before = fs::read(repo.path().join(".git/index")).unwrap();
        let snapshot = read_conflict(repo.path(), Path::new("f")).unwrap();
        mark_resolved(&snapshot).unwrap();
        let actual = repo.git(&["ls-files", "--stage", "--", "f"]);
        assert_eq!(
            fs::metadata(repo.path().join(".git/index"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
        fs::write(repo.path().join(".git/index"), index_before).unwrap();
        repo.git(&["add", "f"]);
        assert_eq!(repo.git(&["ls-files", "--stage", "--", "f"]), actual);
    }
}

#[test]
/// Git marker display newlines do not silently replace actual EOF bytes or concatenate both sides.
fn terminal_chunks_use_actual_side_eof_information() {
    for (a, b) in [
        ("left", "right"),
        ("left\n", "right"),
        ("left", "right\n"),
        ("лево\r", "право\r"),
    ] {
        let repo = Repo::new();
        repo.write("f", "base");
        repo.initial();
        repo.git(&["switch", "-c", "incoming"]);
        repo.write("f", b);
        repo.git(&["commit", "-am", "incoming"]);
        repo.git(&["switch", "main"]);
        repo.write("f", a);
        repo.git(&["commit", "-am", "current"]);
        assert_eq!(
            repo.command(&["merge", "--no-edit", "incoming"])
                .status
                .code(),
            Some(1)
        );
        let snapshot = read_conflict(repo.path(), Path::new("f")).unwrap();
        assert_eq!(
            snapshot
                .choose_chunk(&snapshot.result.bytes, 0, ChunkChoice::A)
                .unwrap(),
            a.as_bytes()
        );
        assert_eq!(
            snapshot
                .choose_chunk(&snapshot.result.bytes, 0, ChunkChoice::B)
                .unwrap(),
            b.as_bytes()
        );
        for (choice, first, second) in [(ChunkChoice::AThenB, a, b), (ChunkChoice::BThenA, b, a)] {
            let actual = snapshot.choose_chunk(&snapshot.result.bytes, 0, choice);
            if first.ends_with('\n') {
                assert_eq!(actual.unwrap(), format!("{first}{second}").as_bytes());
            } else {
                assert!(matches!(actual, Err(CoreError::ConflictBoundary)));
            }
        }
    }
}

#[test]
#[cfg(unix)]
/// With fileMode disabled Git retains the existing executable stage, even if disk permissions differ.
fn resolve_retains_executable_stage_when_file_mode_is_disabled() {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repo::new();
    repo.write("f", "base\n");
    fs::set_permissions(repo.path().join("f"), fs::Permissions::from_mode(0o755)).unwrap();
    repo.initial();
    repo.git(&["switch", "-c", "incoming"]);
    repo.write("f", "incoming\n");
    repo.git(&["commit", "-am", "incoming"]);
    repo.git(&["switch", "main"]);
    repo.write("f", "current\n");
    repo.git(&["commit", "-am", "current"]);
    assert_eq!(
        repo.command(&["merge", "--no-edit", "incoming"])
            .status
            .code(),
        Some(1)
    );
    repo.git(&["config", "core.fileMode", "false"]);
    fs::set_permissions(repo.path().join("f"), fs::Permissions::from_mode(0o644)).unwrap();
    repo.write("f", "resolved\n");
    let index = fs::read(repo.path().join(".git/index")).unwrap();
    mark_resolved(&read_conflict(repo.path(), Path::new("f")).unwrap()).unwrap();
    let actual = repo.git(&["ls-files", "--stage", "--", "f"]);
    fs::write(repo.path().join(".git/index"), index).unwrap();
    repo.git(&["add", "f"]);
    assert_eq!(repo.git(&["ls-files", "--stage", "--", "f"]), actual);
    assert!(actual.starts_with(b"100755 "));
}

#[test]
/// Unknown index extensions and changed attribute conversion are refused without index publication.
fn unknown_extension_and_changed_attributes_are_guarded() {
    let repo = merge("sha256");
    let index_path = repo.path().join(".git/index");
    let mut index = fs::read(&index_path).unwrap();
    index.truncate(index.len() - 32);
    index.extend(b"XTRA\0\0\0\x04data");
    let mut hasher = gix::hash::hasher(gix::hash::Kind::Sha256);
    hasher.update(&index);
    index.extend(hasher.try_finalize().unwrap().as_slice());
    fs::write(&index_path, &index).unwrap();
    let snapshot = read_conflict(repo.path(), Path::new("f")).unwrap();
    assert!(matches!(
        mark_resolved(&snapshot),
        Err(CoreError::IndexVariant(
            hunkra_core::IndexLimitation::UnknownExtension
        ))
    ));
    assert_eq!(fs::read(index_path).unwrap(), index);
    repo.write(".gitattributes", "f text eol=crlf\n");
    assert!(matches!(
        save_result(&snapshot, &snapshot.b.worktree),
        Err(CoreError::Stale)
    ));
    assert_eq!(
        fs::read(repo.path().join("f")).unwrap(),
        snapshot.result.bytes
    );
}
