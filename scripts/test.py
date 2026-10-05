#!/usr/bin/env python3
"""Run Rust tests without the owner's Git configuration, hooks or repository environment."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile


def run():
    """Preserve the installed compiler/cache while giving test processes an isolated home."""
    home = Path.home()
    with tempfile.TemporaryDirectory(prefix="hunkra-test-home-") as temporary:
        environment = {
            "PATH": os.environ["PATH"],
            "HOME": temporary,
            "XDG_CONFIG_HOME": str(Path(temporary) / "config"),
            "CARGO_HOME": os.environ.get("CARGO_HOME", str(home / ".cargo")),
            "RUSTUP_HOME": os.environ.get("RUSTUP_HOME", str(home / ".rustup")),
            "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_ATTR_NOSYSTEM": "1",
            "LC_ALL": "C.UTF-8",
        }
        if "LIBRARY_PATH" in os.environ:
            environment["LIBRARY_PATH"] = os.environ["LIBRARY_PATH"]
        result = subprocess.run(["cargo", "test", *sys.argv[1:]], env=environment)
        return result.returncode


if __name__ == "__main__":
    sys.exit(run())
