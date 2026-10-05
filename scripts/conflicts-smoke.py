#!/usr/bin/env python3
"""Validate finite Wayland Result windows against independent Git in disposable repositories."""
from pathlib import Path
import json
import os
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
FILE = "f кириллица.txt"
RESULT = "Результат 🦀\r\nВведён вручную без последнего LF".encode()


def smoke(destination):
    """Check real native editing/confirmations and separate Save, resolve and Continue effects."""
    with tempfile.TemporaryDirectory(prefix="hunkra-conflicts-") as temporary:
        fixture = Path(temporary)
        environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        environment.update(HOME=temporary, XDG_CONFIG_HOME=str(fixture / "config"), GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1", GIT_ATTR_NOSYSTEM="1")
        reports = []

        def git(repository, *args, success=True):
            """Run only the fixture's independent Git oracle with bounded execution time."""
            result = subprocess.run(["git", *args], cwd=repository, env=environment, capture_output=True, timeout=20)
            if success:
                assert result.returncode == 0, result.stderr.decode(errors="replace")
            return result

        def new(name, format="sha1"):
            """Create explicit local identity/object format without owner settings or hooks."""
            repository = fixture / name
            repository.mkdir()
            git(repository, "init", "-b", "main", f"--object-format={format}")
            hooks = fixture / (name + "-hooks")
            hooks.mkdir()
            for key, value in [("user.name", "Hunkra Native Test"), ("user.email", "fixture@example.invalid"), ("core.hooksPath", str(hooks)), ("core.autocrlf", "false")]:
                git(repository, "config", "--local", key, value)
            return repository

        def native(repository, action, close=False):
            """Wait for the actual GUI process and require its asserted native-render report."""
            report_path = fixture / f"report-{len(reports)}.json"
            args = [str(ROOT / "target/debug/hunkra"), str(repository), "--smoke-test", str(report_path), "--smoke-operation", action]
            if close:
                args.append("--smoke-close")
            result = subprocess.run(args, env=environment, capture_output=True, timeout=40)
            assert result.returncode == 0, result.stderr.decode(errors="replace")
            report = json.loads(report_path.read_text())
            assert report["ok"], report
            if not report.get("close_deferred"):
                assert report["native_display"] and report["frames"] > 0
                assert report["locales"] == ["Result", "Результат", "Result"]
                assert report["draft_retained"]
                assert report["dirty_confirmation"] == (action != "conflict-external")
            reports.append(report)
            return report

        def merged(name, format="sha1"):
            """Generate two actual textual chunks with common CRLF/BOM and a second conflicted file."""
            repository = new(name, format)
            common = "".join(f"context {number}\r\n" for number in range(20))
            def content(side):
                """Keep exact line endings and nonconflicting context in each committed side."""
                return f"\ufeffheader\r\n{side} one\r\n{common}{side} two\r\nfooter".encode()
            (repository / FILE).write_bytes(content("base"))
            (repository / "g").write_text("base g\n")
            git(repository, "add", "."); git(repository, "commit", "-m", "base")
            git(repository, "switch", "-c", "incoming")
            (repository / FILE).write_bytes(content("incoming")); (repository / "g").write_text("incoming g\n")
            git(repository, "commit", "-am", "incoming"); git(repository, "switch", "main")
            (repository / FILE).write_bytes(content("current")); (repository / "g").write_text("current g\n")
            git(repository, "commit", "-am", "current")
            assert git(repository, "merge", "--no-edit", "incoming", success=False).returncode == 1
            return repository

        for format in ["sha1", "sha256"]:
            repository = merged("separate-" + format, format)
            index = (repository / ".git/index").read_bytes()
            stages = git(repository, "ls-files", "-u").stdout
            native(repository, "conflict-save")
            assert (repository / FILE).read_bytes() == RESULT
            assert (repository / ".git/index").read_bytes() == index
            assert git(repository, "ls-files", "-u").stdout == stages
            # A new native window loads the saved Result and performs a second manual edit.
            report = native(repository, "conflict-resolve")
            assert report["second_edit"]
            expected = RESULT + "\r\nВторая редакция".encode()
            assert git(repository, "show", ":" + FILE).stdout == expected
            assert (repository / FILE).read_bytes() == expected
            assert not git(repository, "ls-files", "-u", "--", FILE).stdout
            assert git(repository, "ls-files", "-u", "--", "g").stdout
            assert git(repository, "ls-files", "--resolve-undo", "--", FILE).stdout
            assert (repository / ".git/MERGE_HEAD").exists()

        repository = merged("refusal")
        before = (repository / FILE).read_bytes()
        index = (repository / ".git/index").read_bytes()
        (repository / ".git/index.lock").write_text("foreign lock")
        report = native(repository, "conflict-refusal", close=True)
        assert report["close_cancelled"]
        assert (repository / FILE).read_bytes() == before and (repository / ".git/index").read_bytes() == index
        assert (repository / ".git/index.lock").read_text() == "foreign lock"

        repository = merged("close-during-save")
        index = (repository / ".git/index").read_bytes()
        report = native(repository, "conflict-save", close=True)
        assert report["close_deferred"] and report["writer_finished"]
        assert (repository / FILE).read_bytes() == RESULT and (repository / ".git/index").read_bytes() == index

        for action in ["conflict-stale", "conflict-delete-cancel"]:
            repository = merged(action)
            before = (repository / FILE).read_bytes()
            index = (repository / ".git/index").read_bytes()
            report = native(repository, action)
            assert report["writer_backend"] is None
            expected = b"external edit\r\n" if action.endswith("stale") else before
            assert (repository / FILE).read_bytes() == expected
            assert (repository / ".git/index").read_bytes() == index

        repository = merged("save-before-navigation")
        index = (repository / ".git/index").read_bytes()
        other = (repository / "g").read_bytes()
        native(repository, "conflict-save-navigate")
        assert (repository / FILE).read_bytes() == RESULT
        assert (repository / "g").read_bytes() == other
        assert (repository / ".git/index").read_bytes() == index

        repository = new("edit-after-resolve")
        (repository / FILE).write_text("base\n")
        git(repository, "add", "."); git(repository, "commit", "-m", "base")
        git(repository, "switch", "-c", "incoming")
        (repository / FILE).write_text("incoming\n")
        git(repository, "commit", "-am", "incoming"); git(repository, "switch", "main")
        (repository / FILE).write_text("current\n")
        git(repository, "commit", "-am", "current")
        assert git(repository, "merge", "--no-edit", "incoming", success=False).returncode == 1
        native(repository, "conflict-restage-continue")
        assert not (repository / ".git/MERGE_HEAD").exists()
        assert git(repository, "show", "HEAD:" + FILE).stdout == b"incoming\n"
        assert not git(repository, "status", "--porcelain").stdout

        repository = new("add-add")
        (repository / "base").write_text("base\n")
        git(repository, "add", "."); git(repository, "commit", "-m", "base")
        git(repository, "switch", "-c", "incoming")
        (repository / FILE).write_text("incoming\n")
        git(repository, "add", "."); git(repository, "commit", "-m", "incoming")
        git(repository, "switch", "main")
        (repository / FILE).write_text("current\n")
        git(repository, "add", "."); git(repository, "commit", "-m", "current")
        assert git(repository, "merge", "--no-edit", "incoming", success=False).returncode == 1
        native(repository, "conflict-resolve")
        assert git(repository, "show", ":" + FILE).stdout == RESULT
        assert not git(repository, "ls-files", "-u").stdout

        for variant in ["binary", "utf16"]:
            repository = new("external-" + variant, "sha256")
            if variant == "utf16":
                (repository / ".gitattributes").write_text(f'"{FILE}" working-tree-encoding=UTF-16LE text eol=crlf\n')
            def encode(text):
                """Prepare real worktree bytes matching the explicitly declared fixture encoding."""
                return text.encode("utf-16-le") if variant == "utf16" else text.encode() + b"\0"
            (repository / FILE).write_bytes(encode("base\r\n"))
            git(repository, "add", "."); git(repository, "commit", "-m", "base")
            git(repository, "switch", "-c", "incoming")
            (repository / FILE).write_bytes(encode("incoming\r\n"))
            git(repository, "commit", "-am", "incoming"); git(repository, "switch", "main")
            (repository / FILE).write_bytes(encode("current\r\n"))
            git(repository, "commit", "-am", "current")
            assert git(repository, "merge", "--no-edit", "incoming", success=False).returncode == 1
            desired = encode("Внешнее решение\r\nбез завершающего LF")
            (repository / FILE).write_bytes(desired)
            expected = git(repository, "hash-object", "--path=" + FILE, FILE).stdout
            native(repository, "conflict-external")
            assert git(repository, "rev-parse", ":" + FILE).stdout == expected
            assert (repository / FILE).read_bytes() == desired
            assert not git(repository, "ls-files", "-u").stdout
            assert (repository / ".git/MERGE_HEAD").exists()

        for delete_a in [False, True]:
            repository = new("modify-delete-" + str(delete_a))
            (repository / FILE).write_text("base\n"); git(repository, "add", "."); git(repository, "commit", "-m", "base")
            git(repository, "switch", "-c", "incoming")
            if delete_a:
                (repository / FILE).write_text("incoming\n")
            else:
                git(repository, "rm", FILE)
            git(repository, "commit", "-am", "incoming"); git(repository, "switch", "main")
            if delete_a:
                git(repository, "rm", FILE)
            else:
                (repository / FILE).write_text("current\n")
            git(repository, "commit", "-am", "current")
            assert git(repository, "merge", "--no-edit", "incoming", success=False).returncode == 1
            report = native(repository, "conflict-delete-resolve")
            assert report["delete_confirmation"]
            assert not (repository / FILE).exists() and not git(repository, "ls-files", "-u").stdout
            assert git(repository, "show", ":" + FILE, success=False).returncode != 0

        repository = new("two-rebase-stops", "sha256")
        for file in [FILE, "g"]:
            (repository / file).write_text("base\n")
        git(repository, "add", "."); git(repository, "commit", "-m", "base"); git(repository, "switch", "-c", "topic")
        (repository / FILE).write_text("topic first\n"); git(repository, "commit", "-am", "first")
        (repository / "g").write_text("topic second\n"); git(repository, "commit", "-am", "second"); git(repository, "switch", "main")
        for file in [FILE, "g"]:
            (repository / file).write_text("upstream\n")
        git(repository, "commit", "-am", "upstream"); git(repository, "switch", "topic")
        assert git(repository, "rebase", "main", success=False).returncode == 1
        for file in [FILE, "g"]:
            report = native(repository, "conflict-continue")
            assert report["saved_file"] == file
            assert git(repository, "show", "HEAD:" + file).stdout == RESULT
        assert not (repository / ".git/rebase-merge").exists()
        assert git(repository, "rev-parse", "HEAD~2").stdout == git(repository, "rev-parse", "main").stdout
        assert not git(repository, "ls-files", "-u").stdout

        destination.write_text(json.dumps({"ok": True, "fixture_only": True, "physical_input_claim": False, "runs": reports}, ensure_ascii=False, indent=2) + "\n")
        print(f"Native conflict editor smoke: {len(reports)} runs passed")


if __name__ == "__main__":
    smoke(Path(sys.argv[1]).resolve())
