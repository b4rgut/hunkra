#!/usr/bin/env python3
"""Validate task dependencies and the intentionally non-publishing release configuration."""
from pathlib import Path
import re
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def plan():
    """Check task IDs, status vocabulary, complete dependency references and an acyclic queue."""
    graph = {}
    statuses = {"todo", "ready", "in_progress", "blocked", "implemented", "done", "deferred"}
    for line in (ROOT / "TASKS.md").read_text().splitlines():
        if not line.startswith("| HNK-"):
            continue
        cells = [cell.strip() for cell in line.strip("|").split("|")]
        assert cells[0] not in graph, f"duplicate task {cells[0]}"
        assert cells[2] in statuses, f"invalid status {cells[2]}"
        graph[cells[0]] = re.findall(r"HNK-\d+", cells[3])
    assert graph, "no tasks"

    def visit(task, trail):
        """Reject missing dependency IDs and cyclic paths."""
        assert task in graph, f"unknown dependency {task}"
        assert task not in trail, f"cycle {trail + [task]}"
        for dependency in graph[task]:
            visit(dependency, trail + [task])

    for task in graph:
        visit(task, [])
    print(f"TASKS: {len(graph)} IDs, valid statuses and acyclic dependencies")


def release():
    """Check release-plz policy invariants; never run release/update in this checkout."""
    config = tomllib.loads((ROOT / "release-plz.toml").read_text())
    assert config["workspace"]["git_only"] and not config["workspace"]["publish"]
    packages = {entry["name"]: entry for entry in config["package"]}
    assert set(packages) == {"hunkra", "hunkra-core"}
    assert packages["hunkra"]["git_tag_name"] == "v{{ version }}"
    assert not packages["hunkra-core"]["git_tag_enable"]
    assert not packages["hunkra-core"]["git_release_enable"]
    assert packages["hunkra"]["version_group"] == packages["hunkra-core"]["version_group"]
    assert "hunkra-core" in packages["hunkra"]["changelog_include"]
    assert not config["workspace"]["release_always"]
    subprocess.run([str(ROOT / ".tools/bin/release-plz"), "--version"], check=True)
    # Actual parser/update behavior is tested in a disposable copy at a checkpoint.
    print("Release policy valid: one product tag, git-only, internal changes included; no release executed")


if __name__ == "__main__":
    {"plan": plan, "release": release}[sys.argv[1]]()
