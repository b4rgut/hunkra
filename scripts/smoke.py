#!/usr/bin/env python3
"""Run real native GPUI stage/unstage on disposable demos and verify exact versions with Git."""
from pathlib import Path
import json
import os
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def smoke(destination):
    """Require a working display, real renders, both index writes and unchanged working bytes."""
    binary = Path(os.environ.get("HUNKRA_TEST_BINARY", ROOT / "target/debug/hunkra"))
    assert binary.exists(), "Run scripts/dev build-ui first"
    with tempfile.TemporaryDirectory(prefix="hunkra-native-smoke-") as temporary:
        fixture = Path(temporary)
        subprocess.run([sys.executable, str(ROOT / "scripts/demo.py"), str(fixture / "demos")], check=True, capture_output=True)
        repository = fixture / "demos/changes"
        path = "Изменения с пробелом.bsl"
        before = (repository / path).read_bytes()
        environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        environment.update(HOME=temporary, XDG_CONFIG_HOME=str(fixture / "config"), GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1", GIT_ATTR_NOSYSTEM="1")
        report_path = fixture / "report.json"
        result = subprocess.run([str(binary), str(repository), "--smoke-test", str(report_path), "--smoke-stage"], env=environment, capture_output=True, timeout=40)
        assert result.returncode == 0, result.stderr.decode(errors="replace")
        report = json.loads(report_path.read_text())
        assert report["ok"] and report["native_display"] and report["index_written"]
        assert report["backend"] == report["write_backend"] == "gix"
        assert report["locales"] == ["Commit", "Фиксация", "Коммит"]
        assert report["file"] == path

        def git(*args):
            """Operate only on this isolated fixture through an independent Git oracle."""
            return subprocess.run(["git", *args], cwd=repository, env=environment, check=True, capture_output=True).stdout

        original = git("show", f"HEAD:{path}").decode()
        staged = original.replace("A = 1", "A = 10")
        selected = staged.replace("B = 2", "B = 20")
        assert report["index_versions"] == [selected, staged]
        assert git("show", f":{path}").decode() == staged
        assert (repository / path).read_bytes() == before
        # A second native run requests closure immediately after accepting stage B.
        close_path = fixture / "close-report.json"
        result = subprocess.run([str(binary), str(repository), "--smoke-test", str(close_path), "--smoke-stage", "--smoke-close"], env=environment, capture_output=True, timeout=40)
        assert result.returncode == 0, result.stderr.decode(errors="replace")
        close = json.loads(close_path.read_text())
        assert close["ok"] and close["close_deferred"] and close["writer_finished"]
        assert close["write_backend"] == "gix"
        assert git("show", f":{path}").decode() == selected
        assert (repository / path).read_bytes() == before
        assert not (repository / ".git/index.lock").exists()
        report["close_during_write"] = close
        # A held lock must refuse safely, retaining the selection and owned error after refresh.
        index = (repository / ".git/index").read_bytes()
        lock = repository / ".git/index.lock"
        lock.write_bytes(b"fixture holds the lock")
        refusal_path = fixture / "refusal-report.json"
        result = subprocess.run([str(binary), str(repository), "--smoke-test", str(refusal_path), "--smoke-stage", "--smoke-refusal"], env=environment, capture_output=True, timeout=40)
        assert result.returncode == 0, result.stderr.decode(errors="replace")
        refusal = json.loads(refusal_path.read_text())
        assert refusal["ok"] and refusal["selection_preserved"]
        assert refusal["write_error"] == "error-index-lock"
        assert (repository / ".git/index").read_bytes() == index
        assert lock.read_bytes() == b"fixture holds the lock"
        assert (repository / path).read_bytes() == before
        lock.unlink()
        report["lock_refusal"] = refusal
        # Commit from the existing A+B index through native input and the shared writer.
        head = git("rev-parse", "HEAD")
        index = (repository / ".git/index").read_bytes()
        hook_dir = fixture / "hooks"
        hook_dir.mkdir()
        hook = hook_dir / "commit-msg"
        hook.write_text("#!/bin/sh\nexit 1\n")
        hook.chmod(0o755)
        git("config", "core.hooksPath", str(hook_dir))
        hook_path = fixture / "hook-report.json"
        result = subprocess.run([str(binary), str(repository), "--smoke-test", str(hook_path), "--smoke-commit", "--smoke-hook", "--smoke-close"], env=environment, capture_output=True, timeout=40)
        assert result.returncode == 0, result.stderr.decode(errors="replace")
        rejected = json.loads(hook_path.read_text())
        assert rejected["ok"] and rejected["message_retained"] and rejected["input_checked"]
        assert rejected["write_error"] == "error-hook-message"
        assert git("rev-parse", "HEAD") == head
        assert (repository / ".git/index").read_bytes() == index
        assert (repository / path).read_bytes() == before
        hook.unlink()
        commit_path = fixture / "commit-report.json"
        result = subprocess.run([str(binary), str(repository), "--smoke-test", str(commit_path), "--smoke-commit"], env=environment, capture_output=True, timeout=40)
        assert result.returncode == 0, result.stderr.decode(errors="replace")
        committed = json.loads(commit_path.read_text())
        assert committed["ok"] and committed["input_checked"] and committed["message_cleared"]
        assert committed["write_backend"] == "gix"
        assert committed["commit_id"] == git("rev-parse", "HEAD").decode().strip()
        assert git("rev-parse", "HEAD^") == head
        assert git("show", f"HEAD:{path}").decode() == selected
        assert git("show", f":{path}").decode() == selected
        assert git("log", "-1", "--format=%B").decode().rstrip("\n") == "Подготовить A+B 🦀\n\nСообщение из поля"
        assert (repository / ".git/index").read_bytes() == index
        assert (repository / path).read_bytes() == before
        assert not (repository / ".git/index.lock").exists()
        report["commit_hook_refusal"] = rejected
        report["native_commit"] = committed
        # Closing during Commit must also wait for the same serialized writer.
        git("add", "--", path)
        index = (repository / ".git/index").read_bytes()
        close_path = fixture / "commit-close-report.json"
        result = subprocess.run([str(binary), str(repository), "--smoke-test", str(close_path), "--smoke-commit", "--smoke-close"], env=environment, capture_output=True, timeout=40)
        assert result.returncode == 0, result.stderr.decode(errors="replace")
        close_commit = json.loads(close_path.read_text())
        assert close_commit["ok"] and close_commit["close_deferred"] and close_commit["writer_finished"] and close_commit["message_cleared"]
        assert close_commit["commit_id"] == git("rev-parse", "HEAD").decode().strip()
        assert git("show", f"HEAD:{path}") == before
        assert (repository / ".git/index").read_bytes() == index
        assert (repository / path).read_bytes() == before
        assert not (repository / ".git/index.lock").exists()
        report["close_during_commit"] = close_commit
        report["oracle"] = git("--version").decode().strip()
        report["verified"] = ["stage B preserves A", "unstage hunk B preserves A", "worktree A+B+C unchanged", "shift range selects changed +/- lines only", "locale selection preserved", "close waits for index transaction", "lock refusal preserves selection and index bytes"]
        report["verified"] += ["native Unicode/IME and undo/redo callbacks", "native shaped selection/hit-testing", "native message and selection retained across three locales", "commit-msg refusal cancels pending close and preserves message/selection/index/HEAD", "gix commit contains A+B only; C remains in worktree", "successful commit clears message", "close waits for commit transaction"]
        Path(destination).write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
        print(f"Native GPUI staging, input, commit/refusal/close passed; Git index/HEAD/worktree oracle passed")


if __name__ == "__main__":
    smoke(sys.argv[1])
