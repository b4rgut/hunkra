#!/usr/bin/env python3
"""Check actual RPM payload/dependencies/reproducibility and optionally run all native scenarios from its extracted binary."""
from pathlib import Path, PurePosixPath
import argparse
import hashlib
import importlib.util
import json
import os
import stat
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def command(arguments, **kwargs):
    """Run a finite inspection and keep failure diagnostics without touching system installation/configuration."""
    result = subprocess.run(arguments, capture_output=True, text=True, timeout=180, **kwargs)
    if result.returncode:
        raise RuntimeError(f"{arguments[0]} failed: {result.stdout}{result.stderr}")
    return result.stdout


def digest(path):
    """Hash an artifact or payload file using the same SHA-256 algorithm as the RPM header."""
    return hashlib.sha256(path.read_bytes()).hexdigest()


def check(rpm, report_path, native):
    """Compare a second real assembly, verify each extracted file and execute the packaged application's fixture-only checks."""
    rpm = rpm.resolve(strict=True)
    if not rpm.is_file():
        raise RuntimeError("one existing local RPM is required")
    with tempfile.TemporaryDirectory(prefix="hunkra-package-check-") as temporary:
        work = Path(temporary)
        metadata = json.loads((rpm.parent / "build-info.json").read_text())
        assert digest(rpm) == metadata["rpm_sha256"]
        identity = command(["rpm", "-qp", "--qf", "%{NAME}\t%{VERSION}\t%{ARCH}\t%{LICENSE}", str(rpm)]).split("\t")
        assert identity == ["hunkra", metadata["version"], "x86_64", "LicenseRef-Hunkra-Unselected"]
        assert not command(["rpm", "-qp", "--scripts", str(rpm)]).strip(), "unexpected package scripts"
        assert command(["rpm", "-qp", "--qf", "%{FILEDIGESTALGO}", str(rpm)]).strip() == "8"
        requirements = command(["rpm", "-qp", "--requires", str(rpm)]).splitlines()
        assert requirements == metadata["requirements"]
        assert any(item.startswith("libm.so.6(GLIBC_") for item in requirements)
        providers = {}
        for requirement in requirements:
            if requirement.startswith(("rpmlib(", "rtld(")):
                continue
            providers[requirement] = command(["rpm", "-q", "--whatprovides", requirement]).splitlines()
        # Reassemble the same binary/payload/epoch in a fresh private buildroot.
        # This does not claim independent full Rust builds are bit-for-bit equal.
        specification = importlib.util.spec_from_file_location("hunkra_package", ROOT / "scripts/package.py")
        packaging = importlib.util.module_from_spec(specification)
        specification.loader.exec_module(packaging)
        input_binary = ROOT / "target/release/hunkra"
        assert digest(input_binary) == metadata["input_binary_sha256"], "input binary changed; rebuild the review package first"
        second = packaging.package(input_binary, work / "second", metadata["source_date_epoch"])
        assert digest(second) == digest(rpm), "identical payload/epoch produced different RPM bytes"
        # A non-empty output must fail before mutating any caller-owned files.
        sentinel = work / "preserved"
        sentinel.mkdir()
        (sentinel / "owner-file").write_bytes(b"preserve\r\nEOF")
        try:
            packaging.package(input_binary, sentinel, metadata["source_date_epoch"])
        except RuntimeError as error:
            assert "new or empty" in str(error)
        else:
            raise AssertionError("non-empty packaging output was accepted")
        assert (sentinel / "owner-file").read_bytes() == b"preserve\r\nEOF"
        archive = work / "payload.cpio"
        with archive.open("wb") as output:
            subprocess.run(["rpm2cpio", str(rpm)], stdout=output, check=True, timeout=30)
        with archive.open("rb") as data:
            names = command(["cpio", "-it", "--quiet"], stdin=data).splitlines()
        for name in names:
            path = PurePosixPath(name)
            assert not path.is_absolute() and ".." not in path.parts
            assert str(path).startswith(("usr/bin/hunkra", "usr/share/applications/", "usr/share/icons/", "usr/share/doc/hunkra", "usr/share/hunkra", "usr/share/licenses/hunkra"))
        extracted = work / "extracted"
        extracted.mkdir()
        with archive.open("rb") as data:
            subprocess.run(["cpio", "-idm", "--quiet", "--no-absolute-filenames"], stdin=data, cwd=extracted, check=True, timeout=30)
        inventory = command(["rpm", "-qp", "--qf", "[%{FILENAMES}\t%{FILEDIGESTS}\t%{FILEMODES}\t%{FILEFLAGS}\n]", str(rpm)])
        files = 0
        for line in inventory.splitlines():
            name, expected_digest, raw_mode, raw_flags = line.split("\t")
            # RPM propagates %license to regular descendants, not directories.
            if name.startswith("/usr/share/licenses/hunkra") and stat.S_ISREG(int(raw_mode)):
                assert int(raw_flags) & 128, "dependency license/source excluded by nodocs"
            if name.startswith("/usr/share/hunkra"):
                assert not int(raw_flags) & 2, "build identity excluded by nodocs"
            path = extracted / name.removeprefix("/")
            mode = int(raw_mode)
            assert not path.is_symlink(), "unexpected package symlink"
            assert path.stat().st_mode == mode, name
            if stat.S_ISREG(mode):
                assert digest(path) == expected_digest, name
                files += 1
            else:
                assert stat.S_ISDIR(mode), name
        binary = extracted / "usr/bin/hunkra"
        assert not "not found" in command(["ldd", str(binary)])
        environment = {key: value for key, value in os.environ.items() if key not in ("DISPLAY", "WAYLAND_DISPLAY") and not key.startswith("GIT_")}
        environment.update(HOME=str(work / "version-home"), XDG_CONFIG_HOME=str(work / "version-config"))
        assert command([str(binary), "--version"], cwd=work, env=environment).strip() == f"Hunkra {metadata['version']}"
        command(["desktop-file-validate", str(extracted / "usr/share/applications/hunkra.desktop")])
        installed_info = json.loads((extracted / "usr/share/hunkra/build-info.json").read_text())
        assert installed_info["version"] == metadata["version"] and installed_info["input_binary_sha256"] == metadata["input_binary_sha256"]
        notice_root = extracted / "usr/share/licenses/hunkra/third-party"
        notices = json.loads((extracted / "usr/share/licenses/hunkra/third-party.json").read_text())
        assert len(notices["packages"]) == metadata["notice_packages"]
        for package in notices["packages"]:
            assert package["declared_license"]
            for notice in package["available_notice_files"]:
                assert digest(notice_root / notice["path"]) == notice["sha256"]
        runs = {}
        if native:
            environment = os.environ | {"HUNKRA_TEST_BINARY": str(binary)}
            for name, script in (("staging-commit", "smoke.py"), ("operations", "operations-smoke.py"), ("conflicts", "conflicts-smoke.py"), ("history", "history-smoke.py"), ("network", "network-smoke.py"), ("load", "load-smoke.py"), ("ux", "ux-smoke.py"), ("editor", "editor-smoke.py")):
                destination = work / f"{name}.json"
                print(f"Packaged native scenarios: {name}", flush=True)
                result = subprocess.run([sys.executable, str(ROOT / "scripts" / script), str(destination)], cwd=work, env=environment, capture_output=True, text=True, timeout=300)
                if result.returncode:
                    raise RuntimeError(f"{name}: {result.stdout}{result.stderr}")
                data = json.loads(destination.read_text())
                assert data["ok"]
                runs[name] = data
        report = {"ok": True, "version": metadata["version"], "rpm_sha256": digest(rpm), "input_binary_sha256": digest(input_binary), "packaged_binary_sha256": digest(binary), "rpm_bytes": rpm.stat().st_size, "payload_files_checked": files, "reproducible_assembly": True, "reproducibility_scope": metadata["reproducibility_scope"], "requirements": requirements, "installed_providers": providers, "notice_packages": len(notices["packages"]), "native": runs, "scope": "two actual RPM assemblies, every payload mode/digest/notice, installed provider queries and extracted binary; system package installation, independent clean-host rebuild and remote release chain unverified"}
        report_path.parent.mkdir(parents=True, exist_ok=True)
        report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        print(f"RPM payload/dependencies/reproducibility passed; {len(runs)} native scenario groups")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("rpm", type=Path)
    parser.add_argument("report", type=Path)
    parser.add_argument("--native", action="store_true")
    options = parser.parse_args()
    assert options.rpm.is_absolute() and options.report.is_absolute()
    check(options.rpm, options.report, options.native)
