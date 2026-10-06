#!/usr/bin/env python3
"""Run finite native branch/merge/rebase windows on disposable repositories with independent Git oracles."""
from pathlib import Path
import json
import os
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def smoke(destination):
    """Verify real native handlers, locale/input retention and every supported coordinator lifecycle action."""
    with tempfile.TemporaryDirectory(prefix="hunkra-operations-") as temporary:
        fixture = Path(temporary)
        environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        environment.update(HOME=temporary, XDG_CONFIG_HOME=str(fixture / "config"), GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1", GIT_ATTR_NOSYSTEM="1")
        reports = []

        def git(repository, *args, success=True):
            """Run the independent Git oracle against only this temporary repository."""
            result = subprocess.run(["git", *args], cwd=repository, env=environment, capture_output=True, timeout=20)
            if success:
                assert result.returncode == 0, result.stderr.decode(errors="replace")
            return result

        def new(name, format="sha1"):
            """Initialize object format and local identity/hooks before committing fixture setup."""
            repository = fixture / name
            repository.mkdir()
            git(repository, "init", "-b", "main", f"--object-format={format}")
            hooks = fixture / (name + "-hooks")
            hooks.mkdir()
            for key, value in [("user.name", "Hunkra Native Test"), ("user.email", "fixture@example.invalid"), ("core.hooksPath", str(hooks)), ("core.autocrlf", "false")]:
                git(repository, "config", "--local", key, value)
            (repository / "first").write_text("base one\n")
            (repository / "second").write_text("base two\n")
            git(repository, "add", ".")
            git(repository, "commit", "-m", "base")
            return repository, hooks

        def native(repository, action, close=False):
            """Wait for the finite app process, then require its real-render and localization evidence."""
            report_path = fixture / f"report-{len(reports)}.json"
            args = [str(Path(os.environ.get("HUNKRA_TEST_BINARY", ROOT / "target/debug/hunkra"))), str(repository), "--smoke-test", str(report_path), "--smoke-operation", action]
            if close:
                args.append("--smoke-close")
            result = subprocess.run(args, env=environment, capture_output=True, timeout=40)
            assert result.returncode == 0, result.stderr.decode(errors="replace")
            report = json.loads(report_path.read_text())
            assert report["ok"], report
            if not report.get("close_deferred"):
                assert report["native_display"] and report["frames"] > 0 and report["message_retained"]
                assert report["locales"] == ["Branches", "Ветки", "Ветки"]
            reports.append(report)
            assert not (repository / ".git/index.lock").exists()
            return report

        def oid(repository, revision="HEAD"):
            """Read an exact independent Git object id."""
            return git(repository, "rev-parse", revision).stdout.decode().strip()

        def divergent(name, conflict=False):
            """Create ordinary divergent merge histories, with a content conflict only when requested."""
            repository, hooks = new(name)
            git(repository, "switch", "-c", "other")
            (repository / "first").write_text("other one\n")
            git(repository, "commit", "-am", "other")
            git(repository, "switch", "main")
            (repository / ("first" if conflict else "second")).write_text("main change\n")
            git(repository, "commit", "-am", "main")
            return repository, hooks

        def two_stops(name, external=False):
            """Build two replay commits whose separate paths conflict with the selected upstream."""
            repository, hooks = new(name)
            git(repository, "switch", "-c", "topic")
            (repository / "first").write_text("topic one\n")
            git(repository, "commit", "-am", "first")
            (repository / "second").write_text("topic two\n")
            git(repository, "commit", "-am", "second")
            git(repository, "switch", "main")
            (repository / "first").write_text("main one\n")
            (repository / "second").write_text("main two\n")
            git(repository, "commit", "-am", "main")
            git(repository, "switch", "topic")
            if external:
                assert git(repository, "rebase", "main", success=False).returncode == 1
            return repository, hooks

        for format in ["sha1", "sha256"]:
            repository, _ = new("branches-" + format, format)
            before = (repository / ".git/index").read_bytes()
            head = oid(repository)
            report = native(repository, "branch")
            assert report["write_backend"] == "gix" and report["branch_created"] == "задача/интерфейс"
            assert not report["branch_retained"]
            assert oid(repository, "refs/heads/задача/интерфейс") == head == oid(repository)
            assert (repository / ".git/index").read_bytes() == before
            assert (repository / "first").read_bytes() == b"base one\n"
        repository, _ = new("branch-refusal")
        git(repository, "branch", "задача/интерфейс")
        before = (repository / ".git/index").read_bytes()
        head = oid(repository)
        report = native(repository, "branch-refusal", close=True)
        assert report["write_error"] == "error-branch-exists" and report["branch_retained"]
        assert oid(repository) == head and (repository / ".git/index").read_bytes() == before

        repository, _ = divergent("switch")
        target = oid(repository, "other")
        report = native(repository, "switch")
        assert report["write_backend"] == "git-cli" and report["route"] == "WorktreeSwitchCoordinator"
        assert report["branch"] == "other" and report["head"] == target == oid(repository)
        assert (repository / "first").read_bytes() == b"other one\n"
        repository, _ = divergent("switch-close")
        target = oid(repository, "other")
        report = native(repository, "switch", close=True)
        assert report["close_deferred"] and report["writer_finished"] and report["write_backend"] == "git-cli"
        assert oid(repository) == target

        repository, _ = divergent("merge")
        old, target = oid(repository), oid(repository, "other")
        report = native(repository, "merge")
        assert report["route"] == "MergeRepositoryCoordinator" and report["outcome"] == "Completed"
        assert oid(repository, "HEAD^1") == old and oid(repository, "HEAD^2") == target
        assert git(repository, "show", "HEAD:first").stdout == b"other one\n"
        assert git(repository, "show", "HEAD:second").stdout == b"main change\n"
        repository, hooks = divergent("merge-hook")
        old = oid(repository)
        hook = hooks / "pre-merge-commit"
        hook.write_text("#!/bin/sh\nprintf called >> hook-calls\nexit 1\n")
        hook.chmod(0o755)
        report = native(repository, "merge-hook", close=True)
        assert report["write_error"] == "error-operation-execution" and report["route"] == "MergeRepositoryCoordinator"
        assert report["operation"] == "Merge" and oid(repository) == old
        assert (repository / "hook-calls").read_bytes() == b"called"

        repository, _ = divergent("merge-conflict", conflict=True)
        old = oid(repository)
        report = native(repository, "merge")
        assert report["operation"] == "Merge" and report["outcome"] == "StoppedWithConflicts"
        report = native(repository, "abort")
        assert report["operation"] is None and oid(repository) == old
        assert (repository / "first").read_bytes() == b"main change\n"

        repository, _ = two_stops("native-rebase")
        main = oid(repository, "main")
        report = native(repository, "rebase")
        assert report["operation"] == "Rebase" and report["progress"] == [1, 2]
        assert report["route"] == "RebaseSequencerCoordinator" and report["outcome"] == "StoppedWithConflicts"
        for path, progress in [("first", [2, 2]), ("second", None)]:
            (repository / path).write_text("Решение " + path + "\n")
            git(repository, "add", path)
            report = native(repository, "continue")
            assert report["progress"] == progress
        assert report["operation"] is None and report["branch"] == "topic"
        assert oid(repository, "HEAD^^") == main
        assert git(repository, "show", "HEAD:first").stdout == "Решение first\n".encode()
        assert git(repository, "show", "HEAD:second").stdout == "Решение second\n".encode()

        repository, _ = two_stops("external-skip", external=True)
        main = oid(repository, "main")
        report = native(repository, "skip")
        assert report["progress"] == [2, 2] and report["outcome"] == "StoppedWithConflicts"
        report = native(repository, "skip")
        assert report["operation"] is None and oid(repository) == main
        repository, _ = two_stops("external-abort", external=True)
        original = oid(repository, "topic")
        report = native(repository, "abort")
        assert report["operation"] is None and oid(repository) == original
        assert (repository / "first").read_bytes() == b"topic one\n"
        assert (repository / "second").read_bytes() == b"topic two\n"

        output = {"ok": True, "oracle": git(repository, "--version").stdout.decode().strip(), "runs": reports,
                  "verified": ["gix branch creation SHA-1/SHA-256", "native Unicode name and message retention across three locales", "branch refusal preserves input and cancels pending close", "explicit CLI coordinator route/reason", "switch/close waits for writer", "merge two parents and actual tree", "hook failure refreshes actual state without retry", "merge conflict recovery/abort", "two native rebase stops and separate continue", "external rebase recovery/skip/abort"]}
        Path(destination).write_text(json.dumps(output, indent=2, ensure_ascii=False) + "\n")
        print(f"Native branch/operation scenarios passed ({len(reports)} runs); independent Git oracle passed")


if __name__ == "__main__":
    smoke(sys.argv[1])
