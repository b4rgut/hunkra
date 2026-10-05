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
    binary = ROOT / "target/debug/hunkra"
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
            """Read only this isolated fixture through an independent Git oracle."""
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
        report["oracle"] = git("--version").decode().strip()
        report["verified"] = ["stage B preserves A", "unstage hunk B preserves A", "worktree A+B+C unchanged", "shift range selects changed +/- lines only", "locale selection preserved", "close waits for index transaction"]
        Path(destination).write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
        print(f"Native GPUI stage/unstage passed: {report['frames']} frames; Git index/worktree oracle passed")


if __name__ == "__main__":
    smoke(sys.argv[1])
