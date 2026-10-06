#!/usr/bin/env python3
"""Exercise the actual release-plz parser/update only in disposable local workspaces."""
from pathlib import Path
import os
import importlib.util
import shutil
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def scenario(changed_package):
    """Prove core-only and localization-only changes reach the one product version/changelog."""
    home = Path.home()
    with tempfile.TemporaryDirectory(prefix="hunkra-release-check-") as temporary:
        fixture = Path(temporary)
        environment = {
            "PATH": os.environ["PATH"], "HOME": temporary,
            "XDG_CONFIG_HOME": str(fixture / "config"),
            "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_NOSYSTEM": "1",
            "CARGO_HOME": os.environ.get("CARGO_HOME", str(home / ".cargo")),
            "RUSTUP_HOME": os.environ.get("RUSTUP_HOME", str(home / ".rustup")),
            "CARGO_NET_OFFLINE": "true",
        }

        def command(*args):
            """Run an isolated fixture command and preserve actionable diagnostics on failure."""
            result = subprocess.run(args, cwd=fixture, env=environment, capture_output=True, text=True, timeout=180)
            if result.returncode:
                raise RuntimeError(f"{args[0]} failed: {result.stdout}{result.stderr}")
            return result.stdout

        for crate in ("hunkra", "hunkra-core"):
            destination = fixture / "crates" / crate
            (destination / "src").mkdir(parents=True)
            (destination / "src/lib.rs").write_text("/// Fixture API.\npub fn value() -> u32 { 1 }\n")
            manifest = f'[package]\nname = "{crate}"\nversion.workspace = true\nedition = "2024"\npublish = false\n'
            if crate == "hunkra":
                manifest += '[dependencies]\nhunkra-core = { path = "../hunkra-core", version = "=0.1.0" }\n'
                (destination / "locales").mkdir()
                (destination / "locales/ru.ftl").write_text("commit = Фиксация\n")
            (destination / "Cargo.toml").write_text(manifest)
        (fixture / "Cargo.toml").write_text('[workspace]\nmembers = ["crates/hunkra", "crates/hunkra-core"]\nresolver = "3"\n[workspace.package]\nversion = "0.1.0"\n')
        shutil.copyfile(ROOT / "release-plz.toml", fixture / "release-plz.toml")
        command("git", "init", "--initial-branch=main")
        command("git", "config", "user.name", "Hunkra Release Test")
        command("git", "config", "user.email", "release-test@example.invalid")
        (fixture / "empty-hooks").mkdir()
        command("git", "config", "core.hooksPath", str(fixture / "empty-hooks"))
        command("cargo", "generate-lockfile", "--offline")
        command("git", "add", "Cargo.toml", "Cargo.lock", "crates", "release-plz.toml")
        command("git", "commit", "-m", "feat: initial fixture")
        command("git", "tag", "v0.1.0")  # This tag exists only inside TemporaryDirectory.
        if changed_package == "core":
            changed = "crates/hunkra-core/src/lib.rs"
            (fixture / changed).write_text("/// Changed fixture API.\npub fn value() -> u32 { 2 }\n")
        else:
            changed = "crates/hunkra/locales/ru.ftl"
            (fixture / changed).write_text("commit = Фиксация\nopen = Открыть\n")
        message = f"fix({changed_package}): include {changed_package}-only changes"
        command("git", "add", changed)
        command("git", "commit", "-m", message)
        command(str(ROOT / ".tools/bin/release-plz"), "update", "--repo-url", "https://github.com/b4rgut/hunkra", "--release-date", "2026-10-05")
        manifest = tomllib.loads((fixture / "Cargo.toml").read_text())
        assert manifest["workspace"]["package"]["version"] == "0.1.1"
        dependency = tomllib.loads((fixture / "crates/hunkra/Cargo.toml").read_text())
        assert dependency["dependencies"]["hunkra-core"]["version"] == "=0.1.1"
        changelog = (fixture / "CHANGELOG.md").read_text()
        assert f"include {changed_package}-only changes" in changelog
        assert "v0.1.0...v0.1.1" in changelog
        assert command("git", "tag", "--list").splitlines() == ["v0.1.0"]
        assert not (fixture / "crates/hunkra-core/CHANGELOG.md").exists()
        # Simulate only the local acceptance/tag boundary, never a GitHub PR,
        # token permission or release publication. Real update created these bytes.
        command("git", "switch", "-c", "codex/fixture-release")
        command("git", "add", "Cargo.toml", "Cargo.lock", "crates/hunkra/Cargo.toml", "CHANGELOG.md")
        command("git", "commit", "-m", "chore: release v0.1.1")
        command("git", "switch", "main")
        command("git", "merge", "--squash", "codex/fixture-release")
        command("git", "commit", "-m", "chore: release v0.1.1")
        command("git", "tag", "v0.1.1")
        specification = importlib.util.spec_from_file_location("hunkra_artifact_check", ROOT / "scripts/artifact-check.py")
        artifact = importlib.util.module_from_spec(specification)
        specification.loader.exec_module(artifact)
        artifact.check(fixture, "v0.1.1")
        for stale in ("v0.1.0", "hunkra-core-v0.1.1", "v9.9.9"):
            try:
                artifact.check(fixture, stale)
            except AssertionError:
                pass
            else:
                raise AssertionError("non-product or stale tag passed artifact gate")
        assert command("git", "tag", "--list").splitlines() == ["v0.1.0", "v0.1.1"]
        assert not command("git", "status", "--porcelain").strip()
        print(f"release-plz: {changed_package}-only update, local squash acceptance and one matching product tag passed; remote PR/token/publication unverified")


if __name__ == "__main__":
    for changed_package in ("core", "locales"):
        scenario(changed_package)
