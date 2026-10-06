#!/usr/bin/env python3
"""Verify bounded native Result layout and text recovery with every private Git/worktree byte preserved."""
from pathlib import Path
import json
import os
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def smoke(destination):
    """Create a private actual merge conflict, run one finite Wayland window, and compare independent Git/byte snapshots."""
    assert destination.is_absolute()
    with tempfile.TemporaryDirectory(prefix="hunkra-native-editor-") as temporary:
        fixture = Path(temporary)
        root = fixture / "repo"
        root.mkdir()
        environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        environment.update(HOME=temporary, XDG_CONFIG_HOME=str(fixture / "config"), GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1", GIT_ATTR_NOSYSTEM="1")

        def git(*args, success=True):
            """Use explicit fixture identity and disable optional index updates on every independent Git command."""
            result = subprocess.run(["git", "--no-optional-locks", "-c", "user.name=Editor Fixture", "-c", "user.email=fixture@example.invalid", *args], cwd=root, env=environment, capture_output=True, timeout=30)
            if success:
                assert result.returncode == 0, result.stderr.decode(errors="replace")
            return result.stdout

        git("init", "-b", "main")
        (root / "f").write_bytes(b"base\n")
        (root / "sibling").write_bytes(b"\xef\xbb\xbfstable\r\nEOF")
        git("add", "."); git("commit", "-m", "base")
        git("switch", "-c", "incoming")
        (root / "f").write_bytes(b"incoming\n"); git("commit", "-am", "incoming")
        git("switch", "main")
        (root / "f").write_bytes(b"current\n"); git("commit", "-am", "current")
        git("merge", "--no-edit", "incoming", success=False)
        assert git("ls-files", "--unmerged")
        index = (root / ".git/index").read_bytes()
        head = git("rev-parse", "HEAD"); stages = git("ls-files", "--stage")
        status = git("status", "--porcelain=v1", "-z")
        before = {path.relative_to(root): path.read_bytes() for path in root.rglob("*") if path.is_file()}
        report = fixture / "result.json"
        binary = Path(os.environ.get("HUNKRA_TEST_BINARY", ROOT / "target/debug/hunkra"))
        result = subprocess.run([str(binary), str(root), "--smoke-test", str(report), "--smoke-operation", "editor-layout"], cwd=root, env=environment, capture_output=True, timeout=35)
        assert result.returncode == 0, result.stderr.decode(errors="replace")
        evidence = json.loads(report.read_text())
        assert evidence["ok"] and evidence["native_display"] and evidence["text_retained"] and evidence["replacement_undo_redo"]
        assert evidence["wheel_preserves_text_cursor_and_revision"] and evidence["explicit_home_restores_source_coordinates"]
        assert evidence["page_navigation_preserves_text_revision"] and evidence["page_selection_and_source_coordinates"]
        assert evidence["word_navigation_preserves_graphemes_and_revision"]
        assert evidence["multiple_click_selection_preserves_text_revision"]
        queries = evidence["utf16_queries"]
        assert queries["repeated_pairs"] == 32 and queries["text_bytes"] == evidence["large_result_bytes"]
        assert queries["first_selection_query_ms"] >= 0 and queries["repeated_selection_and_text_ms"] >= 0
        navigation=evidence["grapheme_navigation"]
        assert navigation["handler_calls"]==32 and navigation["text_bytes"]==evidence["large_result_bytes"] and navigation["total_ms"]>=0
        assert evidence["locales"] == ["Changes", "Изменения", "Изменения"]
        before_summary = evidence["marker_summary_before"]
        after_summary = evidence["marker_summary"]
        assert after_summary["requests"] - before_summary["requests"] >= evidence["stable_result_phases"]
        assert after_summary["parser"]["count"] - before_summary["parser"]["count"] == 1
        evidence["stable_marker_summary_parses"] = 1
        assert (root / ".git/index").read_bytes() == index and git("rev-parse", "HEAD") == head and git("ls-files", "--stage") == stages
        report.unlink()
        assert git("status", "--porcelain=v1", "-z") == status
        assert {path.relative_to(root): path.read_bytes() for path in root.rglob("*") if path.is_file()} == before
        evidence["oracle"] = "independent Git HEAD/index/stages/status and every private worktree/Git metadata file byte invariant"
        destination.write_text(json.dumps(evidence, indent=2, ensure_ascii=False) + "\n")
        print("Native editor layout smoke: passed")


if __name__ == "__main__":
    smoke(Path(sys.argv[1]))
