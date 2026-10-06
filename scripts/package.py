#!/usr/bin/env python3
"""Assemble a Fedora RPM from a locked binary in a private buildroot, without installation or publication."""
from pathlib import Path
import argparse
import gzip
import hashlib
import io
import json
import os
import platform
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def command(arguments, combined=False, **kwargs):
    """Run bounded build/inspection commands and return actionable failure diagnostics."""
    result = subprocess.run(arguments, capture_output=True, text=True, timeout=180, **kwargs)
    if result.returncode:
        raise RuntimeError(f"{arguments[0]} failed: {result.stdout}{result.stderr}")
    return result.stdout + (result.stderr if combined else "")


def git(*arguments):
    """Read build identity only from this checkout, ignoring inherited repository/configuration redirects."""
    environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    environment.update(GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1")
    return command(["git", *arguments], cwd=ROOT, env=environment)


def digest(path):
    """Identify the exact locked input and generated artifact bytes."""
    return hashlib.sha256(path.read_bytes()).hexdigest()


def notices(payload):
    """Collect exact Linux normal/build dependency notices and unmodified MPL source files."""
    target = "x86_64-unknown-linux-gnu"
    tree = command([
        "cargo", "tree", "--locked", "-p", "hunkra", "--features", "gui",
        "--edges", "normal,build", "--prefix", "none", "--format", "{p}", "--target", target,
    ], cwd=ROOT)
    wanted = set()
    for line in tree.splitlines():
        fields = line.removesuffix(" (*)").split()
        if len(fields) >= 2:
            wanted.add((fields[0], fields[1].removeprefix("v")))
    metadata = json.loads(command([
        "cargo", "metadata", "--locked", "--format-version", "1",
        "--features", "hunkra/gui", "--filter-platform", target,
    ], cwd=ROOT))
    packages = sorted(
        (item for item in metadata["packages"] if item["source"] and (item["name"], item["version"]) in wanted),
        key=lambda item: (item["name"], item["version"]),
    )
    entries = []
    for package in packages:
        name = f"{package['name']}-{package['version']}"
        assert re.fullmatch(r"[A-Za-z0-9_.+-]+", name)
        source = Path(package["manifest_path"]).parent
        files = []
        declared = package.get("license_file")
        mpl = "MPL-2.0" in (package["license"] or "")
        for path in sorted(source.rglob("*")):
            relative = path.relative_to(source)
            if path.is_symlink() or not path.is_file():
                continue
            notice = path.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE", "AUTHORS", "COPYRIGHT"))
            explicit_license = declared and path.resolve() == (source / declared).resolve()
            # Include unmodified MPL source along with its license. This does not
            # select or change the separate license of Hunkra's own source.
            mpl_source = mpl and (relative.parts[0] == "src" or path.name in ("Cargo.toml", "Cargo.toml.orig", "README.md"))
            if not (notice or explicit_license or mpl_source):
                continue
            data = path.read_bytes()
            assert len(data) < 4 * 1024 * 1024, "license/source file bound"
            destination = payload / "third-party" / name / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(data)
            files.append({"path": str(Path(name) / relative), "sha256": hashlib.sha256(data).hexdigest()})
        entries.append({
            "name": package["name"], "version": package["version"],
            "declared_license": package["license"], "repository": package["repository"],
            "available_notice_files": files, "included_mpl_source": mpl,
        })
    (payload / "third-party").mkdir(exist_ok=True)
    notice_info = {
        "scope": "locked Linux x86_64 normal/build graph; available exact source notices, no product license choice; declarations without packaged texts remain explicit",
        "packages": entries,
    }
    (payload / "third-party.json").write_text(json.dumps(notice_info, ensure_ascii=False, indent=2) + "\n")
    return len(entries)


def archive(payload, destination, epoch):
    """Produce fixed payload bytes without source owners/mtimes or gzip timestamps."""
    with destination.open("wb") as output:
        with gzip.GzipFile(filename="", mode="wb", fileobj=output, mtime=epoch) as zipped:
            with tarfile.open(fileobj=zipped, mode="w", format=tarfile.PAX_FORMAT) as tar:
                for path in [payload, *sorted(payload.rglob("*"))]:
                    info = tarfile.TarInfo(str(Path("hunkra-payload") / path.relative_to(payload)))
                    info.uid = info.gid = 0
                    info.uname = info.gname = "root"
                    info.mtime = epoch
                    executable = path.name == "hunkra" and path.parent == payload
                    info.mode = 0o755 if path.is_dir() or executable else 0o644
                    if path.is_dir():
                        info.type = tarfile.DIRTYPE
                        tar.addfile(info)
                    else:
                        data = path.read_bytes()
                        info.size = len(data)
                        tar.addfile(info, io.BytesIO(data))


def rpm_environment(work, builder, local, epoch):
    """Initialize complete project-local RPM configuration before macros load and isolate personal RPM settings."""
    settings = os.environ | {"SOURCE_DATE_EPOCH": str(epoch), "RPM_BUILD_NCPUS": "2"}
    # --define _rpmconfigdir alone rebinds helpers but loses their attribute
    # macros when RPM parses a spec. RPM_CONFIGDIR selects them at startup.
    if Path(builder) == local:
        configuration = work / "rpm-config"
        configuration.mkdir()
        for origin in (Path("/usr/lib/rpm"), local.parent.parent / "lib/rpm"):
            for source in origin.iterdir():
                link = configuration / source.name
                if link.is_symlink():
                    link.unlink()
                link.symlink_to(source)
        settings["RPM_CONFIGDIR"] = str(configuration)
    rpm_home = work / "rpm-home"
    rpm_home.mkdir()
    settings["HOME"] = str(rpm_home)
    return settings


def package(binary, output, epoch=None):
    """Assemble Fedora/x86_64 with automatic ELF requirements and preserve caller files."""
    release = Path("/etc/os-release").read_text()
    if 'ID=fedora\n' not in release or platform.machine() != "x86_64":
        raise RuntimeError("Fedora x86_64 packaging target required")
    if output.is_symlink() or output.exists() and (not output.is_dir() or any(output.iterdir())):
        raise RuntimeError("output must be a new or empty directory")
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    assert re.fullmatch(r"\d+\.\d+\.\d+", version)
    assert command([str(binary), "--version"], cwd=ROOT).strip() == f"Hunkra {version}", "binary/workspace version mismatch"
    revision = git("rev-parse", "HEAD").strip()
    if epoch is None:
        epoch = int(git("show", "-s", "--format=%ct", "HEAD").strip())
    local = ROOT / ".tools/rpm-build/usr/bin/rpmbuild"
    builder = shutil.which("rpmbuild")
    if builder is None and local.is_file():
        builder = str(local)
    if builder is None:
        raise RuntimeError("rpmbuild unavailable; use scripts/dev bootstrap --packaging on Fedora")
    builder_version = command([builder, "--version"]).strip()
    with tempfile.TemporaryDirectory(prefix="hunkra-rpm-") as temporary:
        work = Path(temporary)
        for directory in ("SOURCES", "SPECS", "BUILD", "BUILDROOT", "RPMS", "SRPMS"):
            (work / directory).mkdir()
        payload = work / "hunkra-payload"
        payload.mkdir()
        shutil.copyfile(binary, payload / "hunkra")
        (payload / "hunkra").chmod(0o755)
        for filename in ("hunkra.desktop", "hunkra.svg", "LICENSE-STATUS"):
            shutil.copyfile(ROOT / "packaging" / filename, payload / filename)
        shutil.copyfile(ROOT / "packaging/README.md", payload / "README.md")
        count = notices(payload)
        info = {
            "version": version, "revision": revision,
            "dirty": bool(git("status", "--porcelain")),
            "source_date_epoch": epoch, "rust": command(["rustc", "--version"], cwd=ROOT).strip(),
            "rpm": builder_version, "target": "Fedora x86_64",
            "input_binary_sha256": digest(binary), "lock_sha256": digest(ROOT / "Cargo.lock"),
            "notice_packages": count,
            "reproducibility_scope": "same binary/payload bytes and source epoch; independent complete Rust-build equivalence not claimed",
        }
        (payload / "build-info.json").write_text(json.dumps(info, indent=2) + "\n")
        archive(payload, work / "SOURCES/hunkra-payload.tar.gz", epoch)
        spec = work / "SPECS/hunkra.spec"
        shutil.copyfile(ROOT / "packaging/hunkra.spec", spec)
        args = [
            builder, "-bb", "--define", f"_topdir {work}", "--define", f"hunkra_version {version}",
            "--define", "_buildhost hunkra.local", str(spec),
        ]
        log = command(args, cwd=work, env=rpm_environment(work, builder, local, epoch), combined=True)
        rpms = list((work / "RPMS").rglob("*.rpm"))
        assert len(rpms) == 1
        destination = output / rpms[0].name
        if destination.exists():
            raise RuntimeError(f"artifact already exists: {destination}; use a new output directory")
        requirements = command(["rpm", "-qp", "--requires", str(rpms[0])]).splitlines()
        if not any(item.startswith("libc.so.6") for item in requirements):
            raise RuntimeError(f"Automatic ELF requirements missing: {requirements}\n{log[-8000:]}")
        assert "git-core" in requirements and "openssh-clients" in requirements
        provides = command(["rpm", "-qp", "--provides", str(rpms[0])]).splitlines()
        assert "application(hunkra.desktop)" in provides, "desktop application provide missing"
        assert "not found" not in command(["ldd", str(binary)])
        output.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(rpms[0], destination)
        info.update(rpm_sha256=digest(destination), requirements=requirements)
        (output / "build-info.json").write_text(json.dumps(info, indent=2) + "\n")
        (output / "SHA256SUMS").write_text(f"{info['rpm_sha256']}  {destination.name}\n")
        print(destination, flush=True)
        return destination


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/hunkra")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--epoch", type=int)
    options = parser.parse_args()
    assert options.binary.is_absolute() and options.output.is_absolute()
    package(options.binary, options.output, options.epoch)
