#!/usr/bin/env python3
"""Check finite native history windows against independent Git, without owner repositories/settings."""
from pathlib import Path
import json
import os
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def smoke(destination):
    """Exercise real pages/parents/files/locales/read-only input and retained host Result drafts."""
    with tempfile.TemporaryDirectory(prefix="hunkra-history-") as temporary:
        fixture = Path(temporary)
        environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        environment.update(HOME=temporary, XDG_CONFIG_HOME=str(fixture / "config"), GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1", GIT_ATTR_NOSYSTEM="1")
        reports = []

        def git(repository, *args, success=True):
            """Run the independent oracle with isolated config and a finite subprocess timeout."""
            result = subprocess.run(["git", *args], cwd=repository, env=environment, capture_output=True, timeout=20)
            if success:
                assert result.returncode == 0, result.stderr.decode(errors="replace")
            return result

        def new(name, format="sha1"):
            """Create only a fresh explicit local fixture identity and empty hooks directory."""
            repository = fixture / name
            repository.mkdir()
            git(repository, "init", "-b", "main", f"--object-format={format}")
            hooks = fixture / (name + "-hooks")
            hooks.mkdir()
            for key, value in [("user.name", "Native Fixture"), ("user.email", "fixture@example.invalid"), ("core.hooksPath", str(hooks)), ("core.autocrlf", "false")]:
                git(repository, "config", "--local", key, value)
            return repository

        def commit(repository, text):
            """Commit only this fixture's deliberately prepared files."""
            git(repository, "add", ".")
            git(repository, "commit", "-m", text)

        def native(repository, action):
            """Wait for an actual native process, then prove all index/worktree files stayed unchanged."""
            index_path = Path(git(repository, "rev-parse", "--path-format=absolute", "--git-path", "index").stdout.decode().strip())
            index = index_path.read_bytes() if index_path.exists() else None
            before = {str(path.relative_to(repository)): path.read_bytes() for path in repository.rglob("*") if ".git" not in path.relative_to(repository).parts and path.is_file() and not path.is_symlink()}
            report_path = fixture / f"report-{len(reports)}.json"
            result = subprocess.run([str(Path(os.environ.get("HUNKRA_TEST_BINARY", ROOT / "target/debug/hunkra"))), str(repository), "--smoke-test", str(report_path), "--smoke-operation", action], env=environment, capture_output=True, timeout=40)
            assert result.returncode == 0, result.stderr.decode(errors="replace")
            report = json.loads(report_path.read_text())
            assert report["ok"] and report["native_display"] and report["frames"] > 0 and report["history_frames"] > 0
            assert report["locales"] == ["History", "История", "История"]
            assert report["read_only"] and report["message_retained"] and report["draft_retained"]
            assert (index_path.read_bytes() if index_path.exists() else None) == index
            after = {str(path.relative_to(repository)): path.read_bytes() for path in repository.rglob("*") if ".git" not in path.relative_to(repository).parts and path.is_file() and not path.is_symlink()}
            assert before == after
            for review in report["reviews"]:
                if review["tip"] is None:
                    assert not review["commit"]
                    continue
                commits = git(repository, "log", "--format=%H", review["tip"]).stdout.decode().splitlines()
                assert review["commit"] in commits
                if action == "history-pages":
                    assert review["offset"] == 100 and review["commit"] == commits[100]
                if review["file"] is not None and review["presentation"] == "Text":
                    for side, id in [("left", review["base_id"]), ("right", review["commit"])]:
                        if review[side + "_present"]:
                            assert id
                            expected = git(repository, "show", id + ":" + review["file"]).stdout.decode()
                            assert review[side] == expected
                        else:
                            assert review[side] == ""
            reports.append(report)
            return report

        for format in ["sha1", "sha256"]:
            repository = new("pages-" + format, format)
            for number in range(205):
                (repository / "файл.txt").write_text(f"строка {number}\r\nбез LF", newline="")
                commit(repository, f"Изменение {number} 🦀")
            # Dirty staged/unstaged content must never be mistaken for historical sources.
            (repository / "файл.txt").write_text("index\n")
            git(repository, "add", ".")
            (repository / "файл.txt").write_text("worktree\n")
            native(repository, "history-pages")

            repository = new("merge-" + format, format)
            (repository / "base").write_text("base\n")
            commit(repository, "Основа")
            git(repository, "switch", "-c", "incoming")
            (repository / "b").write_text("B\r\nбез LF", newline="")
            commit(repository, "Входящий")
            git(repository, "switch", "main")
            (repository / "a").write_text("A\n")
            commit(repository, "Текущий")
            git(repository, "merge", "--no-ff", "--no-edit", "incoming")
            report = native(repository, "history-merge")
            assert [review["parent"] for review in report["reviews"]] == ["Parent(1)", "Parent(0)"]
            assert report["reviews"][0]["base_id"] != report["reviews"][1]["base_id"]

        repository = new("root")
        (repository / "file").write_text("\ufeffroot\r\nбез LF", newline="")
        commit(repository, "Основа")
        (repository / "file").write_text("second\n")
        commit(repository, "Второй")
        report = native(repository, "history-root")
        assert report["reviews"][0]["parent"] == "EmptyTree" and report["reviews"][0]["base_id"] is None
        assert report["reviews"][-1]["commit"] == report["reviews"][-1]["tip"]

        repository = new("types", "sha256")
        (repository / "base").write_text("base\n")
        commit(repository, "Основа")
        (repository / "binary").write_bytes(b"binary\0file")
        (repository / "large").write_bytes(b"x" * (2 * 1024 * 1024 + 1))
        (repository / "link").symlink_to("base")
        commit(repository, "Типы файлов")
        for file, presentation in [("binary", "Binary"), ("large", "TooLarge"), ("link", "Text")]:
            report = native(repository, "history-file:" + file)
            assert report["reviews"][-1]["presentation"] == presentation

        repository = new("unborn")
        native(repository, "history-empty")

        repository = new("encoded-message")
        (repository / "file").write_text("base\n")
        commit(repository, "Основа")
        (repository / "message").write_bytes(b"caf\xe9\n")
        git(repository, "-c", "i18n.commitEncoding=ISO-8859-1", "commit", "--allow-empty", "-F", "message")
        native(repository, "history-encoding")

        repository = new("linked-main")
        (repository / "file").write_text("linked root\n")
        commit(repository, "Основа")
        linked = fixture / "linked-worktree"
        git(repository, "worktree", "add", "--detach", str(linked))
        native(linked, "history-root")

        repository = new("retained-draft")
        (repository / "f").write_text("base\n")
        commit(repository, "Основа")
        git(repository, "switch", "-c", "incoming")
        (repository / "f").write_text("incoming\n")
        commit(repository, "Входящий")
        git(repository, "switch", "main")
        (repository / "f").write_text("current\n")
        commit(repository, "Текущий")
        assert git(repository, "merge", "--no-edit", "incoming", success=False).returncode == 1
        native(repository, "history-draft")
        assert git(repository, "ls-files", "-u").stdout

        destination.write_text(json.dumps({"ok": True, "fixture_only": True, "physical_input_claim": False, "runs": reports}, ensure_ascii=False, indent=2) + "\n")
        print(f"Native read-only history smoke: {len(reports)} runs passed")


if __name__ == "__main__":
    smoke(Path(sys.argv[1]).resolve())
