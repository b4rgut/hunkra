#!/usr/bin/env python3
"""Check native Wayland window commands in a private repository with all file bytes preserved."""
from pathlib import Path
import json
import os
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def smoke(destination):
    """Verify acknowledged maximize/restore and dispatched minimize without touching owner settings or Git data."""
    assert destination.is_absolute()
    with tempfile.TemporaryDirectory(prefix="hunkra-window-controls-") as temporary:
        fixture = Path(temporary)
        root = fixture / "repo"
        root.mkdir()
        environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        environment.update(HOME=temporary, XDG_CONFIG_HOME=str(fixture / "config"), GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1", GIT_ATTR_NOSYSTEM="1")

        def git(*args):
            """Create fixture history with explicit identity and isolated configuration."""
            subprocess.run(["git", "--no-optional-locks", "-c", "user.name=Window Fixture", "-c", "user.email=fixture@example.invalid", *args], cwd=root, env=environment, capture_output=True, check=True, timeout=20)

        git("init", "-b", "main")
        (root / "f").write_bytes(b"before\r\nEOF")
        git("add", ".")
        git("commit", "-m", "base")
        (root / "f").write_bytes(b"after\r\nEOF")
        before = {path.relative_to(root): path.read_bytes() for path in root.rglob("*") if path.is_file()}
        report = fixture / "report.json"
        binary = Path(os.environ.get("HUNKRA_TEST_BINARY", ROOT / "target/debug/hunkra"))
        result = subprocess.run([str(binary), str(root), "--smoke-test", str(report), "--smoke-operation", "window-controls"], cwd=fixture, env=environment, capture_output=True, timeout=25)
        assert result.returncode == 0, result.stderr.decode(errors="replace")
        evidence = json.loads(report.read_text())
        assert all(evidence[key] for key in ("ok", "native_display", "maximized_acknowledged", "restored_acknowledged", "minimize_dispatched", "read_only"))
        assert {path.relative_to(root): path.read_bytes() for path in root.rglob("*") if path.is_file()} == before
        evidence["oracle"] = "every private Git/worktree byte invariant; compositor maximize/restore states observed; minimize request sent, minimized appearance not captured"
        destination.write_text(json.dumps(evidence, ensure_ascii=False, indent=2) + "\n")
        print("Native window controls: maximize/restore acknowledged, minimize dispatched")


if __name__ == "__main__":
    smoke(Path(sys.argv[1]))
