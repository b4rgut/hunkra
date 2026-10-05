#!/usr/bin/env python3
"""Create genuine isolated repositories; an existing destination is never overwritten."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile


def git(root, *args, conflict=False):
    """Run fixture Git with isolated config/identity/hooks and no inherited repo environment."""
    environment = {"PATH": "/usr/bin:/bin", "HOME": str(root), "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_NOSYSTEM": "1"}
    command = ["git", "-c", "user.name=Hunkra Demo", "-c", "user.email=demo@example.invalid", "-c", f"core.hooksPath={root / '.git/empty-hooks'}", *args]
    result = subprocess.run(command, cwd=root, env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if conflict:
        assert result.returncode == 1, result.stderr.decode()
    elif result.returncode:
        raise RuntimeError(result.stderr.decode())
    return result


def repository(parent, name):
    """Create one new repository directory with a known main branch."""
    root = parent / name
    root.mkdir()
    git(root, "init", "--initial-branch=main")
    return root


def commit(root, message):
    """Commit only demo data in its isolated fixture."""
    git(root, "add", ".")
    git(root, "commit", "-m", message)


def demos(destination=None):
    """Build staged+unstaged, ordinary merge and two-stop rebase conflict examples."""
    if destination:
        parent = Path(destination).expanduser().absolute()
        parent.mkdir(parents=True, exist_ok=False)
    else:
        parent = Path(tempfile.mkdtemp(prefix="hunkra-demos-"))
    root = repository(parent, "changes")
    file = root / "Изменения с пробелом.bsl"
    original = "Процедура Пример()\n    A = 1;\n    // Контекст\n    B = 2;\n    // Контекст\n    C = 3;\nКонецПроцедуры\n"
    file.write_text(original)
    (root / "deleted.txt").write_text("This file will be deleted.\n")
    (root / "newline.xml").write_bytes(b'\xef\xbb\xbf<root>\r\n  <value>before</value>\r\n</root>')
    commit(root, "Initial demo")
    file.write_text(original.replace("A = 1", "A = 10"))
    git(root, "add", file.name)
    file.write_text(original.replace("A = 1", "A = 10").replace("B = 2", "B = 20").replace("C = 3", "C = 30"))
    (root / "deleted.txt").unlink()
    (root / "new.txt").write_text("A new, untracked file.\n")
    (root / "binary.dat").write_bytes(b'\x00\x01Hunkra')
    (root / "newline.xml").write_bytes(b'\xef\xbb\xbf<root>\r\n  <value>after</value>\r\n</root>')
    merge = repository(parent, "merge-conflict")
    (merge / "conflict.txt").write_text("base\n")
    commit(merge, "Base")
    git(merge, "switch", "-c", "side-b")
    (merge / "conflict.txt").write_text("Side B\n")
    commit(merge, "Change B")
    git(merge, "switch", "main")
    (merge / "conflict.txt").write_text("Side A\n")
    commit(merge, "Change A")
    git(merge, "merge", "side-b", conflict=True)
    rebase = repository(parent, "rebase-conflict")
    (rebase / "first.txt").write_text("base one\n")
    (rebase / "second.txt").write_text("base two\n")
    commit(rebase, "Base")
    git(rebase, "switch", "-c", "topic")
    (rebase / "first.txt").write_text("topic one\n")
    commit(rebase, "Topic first")
    (rebase / "second.txt").write_text("topic two\n")
    commit(rebase, "Topic second")
    git(rebase, "switch", "main")
    (rebase / "first.txt").write_text("main one\n")
    (rebase / "second.txt").write_text("main two\n")
    commit(rebase, "Main changes")
    git(rebase, "switch", "topic")
    git(rebase, "rebase", "main", conflict=True)
    for name in ("changes", "merge-conflict", "rebase-conflict"):
        print(parent / name)


if __name__ == "__main__":
    demos(sys.argv[1] if len(sys.argv) > 1 else None)
