#!/usr/bin/env python3
"""Validate finite native network/creation windows against independent Git in private temporary repositories."""
from pathlib import Path
import json
import os
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def smoke(destination):
    """Exercise actual rendered controls and worker outcomes without mutating any owner remotes/configuration."""
    with tempfile.TemporaryDirectory(prefix="hunkra-network-native-") as temporary:
        fixture = Path(temporary)
        environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        environment.update(HOME=temporary, XDG_CONFIG_HOME=str(fixture / "config"), GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1", GIT_ATTR_NOSYSTEM="1")
        reports = []

        def git(root, *args, success=True):
            """Run the independent oracle solely inside this fixture with explicit identity and finite execution."""
            result = subprocess.run(["git", "-c", "user.name=Native Fixture", "-c", "user.email=fixture@example.invalid", *args], cwd=root, env=environment, capture_output=True, timeout=20)
            if success:
                assert result.returncode == 0, result.stderr.decode(errors="replace")
            return result.stdout

        def source(name, format="sha1"):
            """Build a private committed source and bare remote with no owner identity or hooks."""
            root = fixture / name
            root.mkdir()
            git(root, "init", "-b", "main", f"--object-format={format}")
            (root / "f").write_bytes(b"base\n")
            (root / "local").write_bytes(b"local base\n")
            git(root, "add", "."); git(root, "commit", "-m", "base")
            bare = fixture / (name + ".git")
            git(root, "clone", "--bare", str(root), str(bare))
            git(root, "remote", "add", "origin", str(bare))
            return root, bare

        def native(root, action, target="review/ветка", path="", url="", format="sha1", close=False):
            """Invoke a finite real Wayland window; secret-capable field inputs stay out of its JSON evidence."""
            report_path = fixture / f"report-{len(reports)}.json"
            args = [str(Path(os.environ.get("HUNKRA_TEST_BINARY", ROOT / "target/debug/hunkra")))]
            if root is not None:
                args.append(str(root))
            args += ["--smoke-test", str(report_path), "--smoke-operation", action]
            if close:
                args.append("--smoke-close")
            inputs = dict(environment, HUNKRA_SMOKE_BRANCH=target, HUNKRA_SMOKE_DESTINATION=str(path), HUNKRA_SMOKE_URL=str(url), HUNKRA_SMOKE_INITIAL_BRANCH="initial/ветка", HUNKRA_SMOKE_FORMAT=format)
            result = subprocess.run(args, env=inputs, capture_output=True, timeout=40)
            assert result.returncode == 0, result.stderr.decode(errors="replace")
            report = json.loads(report_path.read_text())
            assert report["ok"], report
            if not report.get("close_deferred"):
                assert report["native_display"] and report["frames"] > 0
                assert report["locales"] == ["Remotes", "Удалённые", "Удалённые"]
                assert report["inputs_retained"]
            reports.append(report)
            return report

        for format in ("sha1", "sha256"):
            init = fixture / ("init-" + format)
            init.mkdir()
            (init / "owned").write_bytes(b"\xef\xbb\xbfexisting\r\nEOF")
            native(None, "network-init", path=init, format=format)
            assert git(init, "rev-parse", "--show-object-format") == (format + "\n").encode()
            assert git(init, "symbolic-ref", "HEAD") == "refs/heads/initial/ветка\n".encode()
            assert (init / "owned").read_bytes() == b"\xef\xbb\xbfexisting\r\nEOF"
            root, bare = source("clone-" + format, format)
            clone = fixture / ("cloned-" + format)
            native(None, "network-clone", path=clone, url=bare)
            assert git(clone, "rev-parse", "HEAD") == git(root, "rev-parse", "HEAD")
            assert (clone / "f").read_bytes() == b"base\n"
            assert not git(clone, "status", "--porcelain")

        root, bare = source("fetch")
        clone = fixture / "fetch-clone"
        git(root, "clone", str(bare), str(clone))
        (clone / "local").write_bytes(b"staged\n"); git(clone, "add", "local"); (clone / "local").write_bytes(b"unstaged\n")
        index = (clone / ".git/index").read_bytes(); head = git(clone, "rev-parse", "HEAD")
        (root / "f").write_bytes(b"incoming\n"); git(root, "commit", "-am", "incoming"); git(root, "push", "origin", "main")
        native(clone, "network-fetch")
        assert git(clone, "rev-parse", "origin/main") == git(root, "rev-parse", "HEAD")
        assert git(clone, "rev-parse", "HEAD") == head and (clone / ".git/index").read_bytes() == index
        assert (clone / "local").read_bytes() == b"unstaged\n"
        native(clone, "network-pull", target="main")
        assert git(clone, "rev-parse", "HEAD") == git(root, "rev-parse", "HEAD")
        assert git(clone, "show", ":local") == b"staged\n" and (clone / "local").read_bytes() == b"unstaged\n"
        native(clone, "network-upstream", target="feature/ветка")
        assert git(clone, "config", "--get", "branch.main.merge") == "refs/heads/feature/ветка\n".encode()

        root, bare = source("push", "sha256")
        (root / "f").write_bytes(b"pushed\n"); git(root, "commit", "-am", "pushed")
        index = (root / ".git/index").read_bytes(); (root / "local").write_bytes(b"dirty\n")
        native(root, "network-push")
        assert git(bare, "rev-parse", "review/ветка") == git(root, "rev-parse", "HEAD")
        assert git(root, "config", "--get", "branch.main.merge") == "refs/heads/review/ветка\n".encode()
        assert (root / ".git/index").read_bytes() == index and (root / "local").read_bytes() == b"dirty\n"

        root, bare = source("hook-refusal")
        (root / "f").write_bytes(b"new\n"); git(root, "commit", "-am", "new")
        hook = root / ".git/hooks/pre-push"
        hook.write_text("#!/bin/sh\nexit 1\n"); hook.chmod(0o755)
        before = git(bare, "rev-parse", "main")
        native(root, "network-push-refusal", target="main", close=True)
        assert git(bare, "rev-parse", "main") == before
        assert not git(root, "config", "--get", "branch.main.remote", success=False)

        root, bare = source("pull-refusal")
        clone = fixture / "pull-refusal-clone"; git(root, "clone", str(bare), str(clone))
        (root / "f").write_bytes(b"incoming\n"); git(root, "commit", "-am", "incoming"); git(root, "push", "origin", "main")
        (clone / "f").write_bytes(b"local\n")
        head = git(clone, "rev-parse", "HEAD"); index = (clone / ".git/index").read_bytes()
        native(clone, "network-pull-refusal", target="main")
        assert git(clone, "rev-parse", "HEAD") == head and (clone / ".git/index").read_bytes() == index
        assert (clone / "f").read_bytes() == b"local\n" and not (clone / ".git/MERGE_HEAD").exists()

        root, bare = source("close-push")
        (root / "f").write_bytes(b"close\n"); git(root, "commit", "-am", "close")
        report = native(root, "network-push", close=True)
        assert report["close_deferred"] and report["writer_finished"]
        assert git(bare, "rev-parse", "review/ветка") == git(root, "rev-parse", "HEAD")
        root, bare = source("close-clone")
        clone = fixture / "close-clone-result"
        report = native(None, "network-clone", path=clone, url=bare, close=True)
        assert report["close_deferred"] and report["writer_finished"]
        assert git(clone, "rev-parse", "HEAD") == git(root, "rev-parse", "HEAD")

        for action in ("network-draft-fetch", "network-draft-clone-cancel", "network-draft-pull-cancel", "network-draft-clone-save"):
            root, bare = source(action)
            git(root, "switch", "-c", "incoming")
            (root / "f").write_bytes(b"incoming\r\n")
            git(root, "commit", "-am", "incoming"); git(root, "switch", "main")
            (root / "f").write_bytes(b"current\r\n")
            git(root, "commit", "-am", "current")
            git(root, "merge", "--no-edit", "incoming", success=False)
            git(root, "config", "branch.main.remote", "origin"); git(root, "config", "branch.main.merge", "refs/heads/main")
            index = (root / ".git/index").read_bytes(); head = git(root, "rev-parse", "HEAD"); content = (root / "f").read_bytes()
            clone = fixture / (action + "-destination")
            native(root, action, path=clone, url=bare)
            assert (root / ".git/index").read_bytes() == index and git(root, "rev-parse", "HEAD") == head
            assert (root / ".git/MERGE_HEAD").exists()
            if action.endswith("save"):
                assert (root / "f").read_bytes() == "Результат 🦀\r\nВведён вручную без последнего LF".encode()
                assert git(clone, "rev-parse", "HEAD") == git(bare, "rev-parse", "HEAD")
            else:
                assert (root / "f").read_bytes() == content and not clone.exists()

        root, bare = source("creation-refusal")
        index = (root / ".git/index").read_bytes(); config = (root / ".git/config").read_bytes()
        native(None, "network-init-refusal", path=root)
        native(None, "network-clone-refusal", path=root, url=bare)
        assert (root / ".git/index").read_bytes() == index and (root / ".git/config").read_bytes() == config
        lock = root / ".git/config.lock"; lock.write_bytes(b"foreign")
        native(root, "network-upstream-refusal")
        assert lock.read_bytes() == b"foreign" and (root / ".git/config").read_bytes() == config
        lock.unlink()

        destination.write_text(json.dumps({"ok": True, "runs": reports, "scope": "isolated native Wayland controls; independent Git oracle; no owner remotes/keys/settings"}, indent=2, ensure_ascii=False) + "\n")
        print(f"Native network smoke: {len(reports)} runs passed")


if __name__ == "__main__":
    smoke(Path(sys.argv[1]))
