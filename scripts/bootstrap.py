#!/usr/bin/env python3
"""Install verified local developer tools and optional Fedora linker files."""
import hashlib
import io
import os
from pathlib import Path
import platform
import subprocess
import sys
import tarfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
TOOLS = {
    "lefthook": ("https://github.com/evilmartians/lefthook/releases/download/v2.1.17/lefthook_2.1.17_Linux_x86_64", "d947ea744483586ac4d856393df212f238d20a468dd0777389737566cf451767"),
    "cog": ("https://github.com/cocogitto/cocogitto/releases/download/7.0.0/cocogitto-7.0.0-x86_64-unknown-linux-musl.tar.gz", "e03938ff2c4c86d71c00c0f3284dbbe95c5ca76fe34a51f33e945c23010d59bb"),
    "release-plz": ("https://github.com/release-plz/release-plz/releases/download/release-plz-v0.3.169/release-plz-x86_64-unknown-linux-musl.tar.gz", "ed709642b7f5b5fda4d47309884e65f84ca097cf9176cfd9793e8e66e28b48ad"),
}


def tool(name, url, digest):
    """Download pinned assets and verify release SHA-256 before extracting one executable."""
    destination = ROOT / ".tools/bin" / name
    archive = ROOT / ".tools/downloads" / (name + ".asset")
    if archive.exists():
        data = archive.read_bytes()
    else:
        data = urllib.request.urlopen(url, timeout=60).read()
    if hashlib.sha256(data).hexdigest() != digest:
        raise RuntimeError(f"SHA-256 mismatch for {name}")
    archive.parent.mkdir(parents=True, exist_ok=True)
    archive.write_bytes(data)
    if url.endswith(".tar.gz"):
        with tarfile.open(fileobj=io.BytesIO(data)) as package:
            entry = next(entry for entry in package.getmembers() if entry.isfile() and Path(entry.name).name == name)
            data = package.extractfile(entry).read()
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(data)
    destination.chmod(0o755)


def native_fedora():
    """Extract Fedora development RPMs into a local sysroot without privilege changes."""
    if not Path("/etc/os-release").exists() or 'ID=fedora\n' not in Path("/etc/os-release").read_text():
        return
    if Path("/usr/lib64/libfontconfig.so").exists() and Path("/usr/lib64/libxkbcommon-x11.so").exists():
        return
    packages = ROOT / ".tools/rpms"
    packages.mkdir(parents=True, exist_ok=True)
    subprocess.run(["dnf5", "download", "--arch=x86_64", f"--destdir={packages}", "fontconfig-devel", "freetype-devel", "libxcb-devel", "libxkbcommon-x11-devel"], check=True)
    sysroot = ROOT / ".local-sysroot"
    sysroot.mkdir(exist_ok=True)
    for package in packages.glob("*.x86_64.rpm"):
        subprocess.run(["rpmkeys", "--checksig", str(package)], check=True)
        with subprocess.Popen(["rpm2cpio", str(package)], stdout=subprocess.PIPE) as process:
            subprocess.run(["cpio", "-idmu", "--quiet"], stdin=process.stdout, cwd=sysroot, check=True)
            process.stdout.close()
            if process.wait() != 0:
                raise RuntimeError("rpm2cpio failed")
    for library in (sysroot / "usr/lib64").glob("*.so"):
        if library.is_symlink() and not library.exists():
            target = Path("/usr/lib64") / library.readlink().name
            if target.exists():
                library.unlink()
                library.symlink_to(target)


if __name__ == "__main__":
    if platform.system() != "Linux" or platform.machine() != "x86_64":
        raise SystemExit("Pinned tool assets currently support Linux x86_64 only")
    subprocess.run(["rustup", "toolchain", "install", "1.99.0", "--profile", "minimal", "--component", "rustfmt", "--component", "clippy"], check=True)
    for name, (url, digest) in TOOLS.items():
        tool(name, url, digest)
    if "--no-native" not in sys.argv:
        native_fedora()
    subprocess.run(["cargo", "fetch", "--locked"], cwd=ROOT, check=True)
    environment = os.environ | {"PATH": str(ROOT / ".tools/bin") + os.pathsep + os.environ["PATH"]}
    subprocess.run([str(ROOT / ".tools/bin/lefthook"), "install"], cwd=ROOT, env=environment, check=True)
    print("Bootstrap complete; tools and native linker files are project-local")
