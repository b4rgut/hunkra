#!/usr/bin/env python3
"""Measure finite real synthetic repositories; all Git writes and data remain in one temporary directory."""
from pathlib import Path
import json
import os
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def measure(destination, uncached=False):
    """Check Git-oracle counts and byte invariance while collecting opening/refresh/change/large-file baselines."""
    with tempfile.TemporaryDirectory(prefix="hunkra-measure-") as temporary:
        root = Path(temporary) / "repository"; root.mkdir()
        environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        environment.update(HOME=temporary, XDG_CONFIG_HOME=str(Path(temporary) / "config"), GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1", GIT_ATTR_NOSYSTEM="1")

        def git(*args):
            """Use independent Git solely inside the isolated fixture with explicit identity and finite execution."""
            result = subprocess.run(["git", "--no-optional-locks", "-c", "user.name=Load Fixture", "-c", "user.email=fixture@example.invalid", *args], cwd=root, env=environment, capture_output=True, timeout=45)
            assert result.returncode == 0, result.stderr.decode(errors="replace")
            return result.stdout

        git("init", "-b", "main")
        content = b"\xef\xbb\xbf" + b"exact CRLF fixture\r\n" * 40 + b"EOF"
        folder = root / "files"; folder.mkdir()
        for number in range(3000):
            (folder / f"{number:04}.txt").write_bytes(content)
        primary = "изменения.txt"; sibling = "sibling.txt"; large = "large.txt"
        (root / primary).write_bytes(content); (root / sibling).write_bytes(content)
        large_content = b"".join(f"line {number:05} stable content\n".encode() for number in range(18000))
        (root / large).write_bytes(large_content)
        git("add", "."); git("commit", "-m", "load base")
        index = (root / ".git/index").read_bytes(); head = git("rev-parse", "HEAD")
        cases = []

        def sample(name, paths):
            """Check all read sources survive exactly and retain separate per-round measurements without claiming cold OS cache."""
            oracle = git("status", "--porcelain=v1", "-z", "--untracked-files=all")
            mode = ["--uncached"] if uncached else []
            result = subprocess.run([str(ROOT / "target/debug/measure"), *mode, str(root), *paths], env=environment, capture_output=True, timeout=45)
            assert result.returncode == 0, result.stderr.decode(errors="replace")
            report = json.loads(result.stdout)
            assert report["cache"]["entries"] <= 4 and report["cache"]["allocated_bytes"] <= 24 * 1024 * 1024
            assert uncached or report["cache"]["hits"] > 0
            assert report["files"] == len([item for item in oracle.split(b"\0") if item])
            assert (root / ".git/index").read_bytes() == index and git("rev-parse", "HEAD") == head
            assert git("status", "--porcelain=v1", "-z", "--untracked-files=all") == oracle
            report["scenario"] = name; cases.append(report)

        sample("first-open-and-repeat-refresh", [primary, sibling, primary, primary, sibling])
        (root / primary).write_bytes(content.replace(b"EOF", b"one change without LF"))
        sample("one-change-and-fast-switch", [primary, sibling, primary, primary, sibling])
        for number in range(3000):
            (folder / f"{number:04}.txt").write_bytes(content)
        sample("3000-unchanged-content-rewrites", [primary, sibling, primary])
        untracked = root / "untracked"; untracked.mkdir()
        for number in range(8000):
            (untracked / f"{number:04}.txt").write_bytes(b"new\n")
        sample("8000-untracked-files", [primary, sibling, primary])
        (root / large).write_bytes(large_content.replace(b"line 09000 stable", b"line 09000 changed"))
        sample("18000-line-diff", [large, large, primary, large])
        (root / "oversized").write_bytes(b"x" * (2 * 1024 * 1024 + 1))
        sample("oversized-file-refusal", ["oversized", "oversized"])
        assert all(row["presentation"] == "TooLarge" and row["rows"] == 0 for row in cases[-1]["diffs"])
        assert (root / primary).read_bytes() == content.replace(b"EOF", b"one change without LF")
        assert (root / sibling).read_bytes() == content
        destination.write_text(json.dumps({"ok": True, "tracked_files":3003,"cases":cases,"scope":"isolated Fedora debug backend only; native prepare/render/GPU and physical input unmeasured"}, indent=2, ensure_ascii=False) + "\n")
        print(f"Backend load measurements: {len(cases)} scenarios passed")


if __name__ == "__main__":
    measure(Path(sys.argv[1]), uncached="--uncached" in sys.argv[2:])
