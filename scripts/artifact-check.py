#!/usr/bin/env python3
"""Reject stale/non-product tags before a distribution build; never create a tag or change product versions."""
from pathlib import Path
import re
import os
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def check(root, tag):
    """Require a single exact product tag/version at HEAD and a synchronized internal dependency/lock version."""
    manifest = tomllib.loads((root / "Cargo.toml").read_text())
    version = manifest["workspace"]["package"]["version"]
    assert re.fullmatch(r"\d+\.\d+\.\d+", version)
    assert tag == f"v{version}", "tag/workspace version mismatch"
    application = tomllib.loads((root / "crates/hunkra/Cargo.toml").read_text())
    assert application["dependencies"]["hunkra-core"]["version"] == f"={version}"
    lock = tomllib.loads((root / "Cargo.lock").read_text())
    assert {package["name"]: package["version"] for package in lock["package"] if package["name"] in ("hunkra", "hunkra-core")} == {"hunkra": version, "hunkra-core": version}
    environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    environment.update(GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1")
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, env=environment, text=True, timeout=30).strip()
    target = subprocess.check_output(["git", "rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}"], cwd=root, env=environment, text=True, timeout=30).strip()
    assert target == head, "tag points to another commit"
    print(f"Product tag {tag} matches HEAD, workspace and locked internal versions")


if __name__ == "__main__":
    check(ROOT, sys.argv[1])
