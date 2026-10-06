#!/usr/bin/env python3
"""Install the generated RPM in a rootless Fedora container with private storage, excluding optional documentation."""
from pathlib import Path
import argparse
import hashlib
import json
import subprocess
import uuid

ROOT = Path(__file__).resolve().parents[1]


def command(arguments, **kwargs):
    """Run bounded container/metadata inspections and preserve actionable failure output."""
    result = subprocess.run(arguments, capture_output=True, text=True, timeout=600, **kwargs)
    if result.returncode:
        raise RuntimeError(f"{arguments[0]} failed: {result.stdout}{result.stderr}")
    return result.stdout


def check(rpm, destination):
    """Verify the real installed package identity/digests/notices without accessing host installation or container storage."""
    rpm = rpm.resolve(strict=True)
    identity = command(["rpm", "-qp", "--qf", "%{NAME}\t%{VERSION}\t%{LICENSE}", str(rpm)]).split("\t")
    assert identity[0] == "hunkra" and identity[2] == "LicenseRef-Hunkra-Unselected"
    assert not command(["rpm", "-qp", "--scripts", str(rpm)]).strip()
    metadata = json.loads((rpm.parent / "build-info.json").read_text())
    rpm_digest = hashlib.sha256(rpm.read_bytes()).hexdigest()
    assert rpm_digest == metadata["rpm_sha256"] and identity[1] == metadata["version"]
    inventory = command(["rpm", "-qp", "--qf", "[%{FILENAMES}\t%{FILEDIGESTS}\n]", str(rpm)])
    expected_binary = dict(line.split("\t") for line in inventory.splitlines())["/usr/bin/hunkra"]
    cache = ROOT / "target/package-install-container"
    assert not cache.is_symlink()
    cache.mkdir(parents=True, exist_ok=True)
    podman = ["podman", "--root", str(cache / "root"), "--runroot", str(cache / "run"), "--storage-driver", "vfs"]
    assert command([*podman, "info", "--format", "{{.Host.Security.Rootless}}"]).strip() == "true"
    image = (ROOT / "packaging/fedora-image.txt").read_text().strip()
    assert image.startswith("quay.io/fedora/fedora:44@sha256:")
    name = "hunkra-package-check-" + uuid.uuid4().hex
    script = '''set -eu
dnf -y --setopt=tsflags=nodocs install /tmp/hunkra.rpm > /tmp/hunkra-install.log 2>&1 || { tail -50 /tmp/hunkra-install.log; exit 1; }
rpm -V hunkra
test -s /usr/share/hunkra/build-info.json
test -s /usr/share/licenses/hunkra/third-party.json
test -s /usr/share/licenses/hunkra/third-party/option-ext-0.2.0/src/lib.rs
test -s /usr/share/licenses/hunkra/third-party/option-ext-0.2.0/LICENSE.txt
test -s /usr/share/applications/hunkra.desktop
test -s /usr/share/icons/hicolor/scalable/apps/hunkra.svg
test ! -f /usr/share/doc/hunkra/README.md
/usr/bin/hunkra --version
sha256sum /usr/bin/hunkra
cat /usr/share/hunkra/build-info.json
'''
    try:
        output = command([
            *podman, "run", "--rm", "--name", name, "--security-opt", "label=disable",
            "--volume", f"{rpm}:/tmp/hunkra.rpm:ro", image, "sh", "-ec", script,
        ])
    finally:
        # Remove only this uniquely named private test container after timeouts;
        # the base-image cache stays under the project's ignored target folder.
        subprocess.run([*podman, "rm", "--force", name], capture_output=True, timeout=30)
    version, checksum, installed_json = output.split("\n", 2)
    assert version == f"Hunkra {metadata['version']}"
    assert checksum.split()[0] == expected_binary
    installed = json.loads(installed_json)
    assert installed["version"] == metadata["version"] and installed["revision"] == metadata["revision"]
    report = {
        "ok": True, "version": metadata["version"], "image": image,
        "rpm_sha256": rpm_digest, "installed_binary_sha256": expected_binary,
        "rootless": True, "optional_docs_excluded": True, "rpm_verify": "passed",
        "mandatory_notices_source_launcher_icon_identity": "passed",
        "scope": "real fresh Fedora container installation and headless version launch; host RPM database/configuration untouched; display/GPU and independent Rust rebuild not exercised here",
    }
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(json.dumps(report, indent=2) + "\n")
    print("Rootless clean Fedora RPM installation, nodocs payload, digests and version passed")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("rpm", type=Path)
    parser.add_argument("report", type=Path)
    options = parser.parse_args()
    assert options.rpm.is_absolute() and options.report.is_absolute()
    check(options.rpm, options.report)
