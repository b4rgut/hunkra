"""Finite loopback SSH acceptance; private keys/socket/host trust and all child processes belong to this fixture."""
from pathlib import Path
import getpass
import os
import socket
import shutil
import subprocess
import sys
import time


def run():
    """Authenticate through a real private ssh-agent, invoke one Rust child test, then reap agent/server."""
    root = Path(sys.argv[2])
    environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_") and key not in ("SSH_AUTH_SOCK", "SSH_AGENT_PID")}
    environment.update(HOME=str(root), GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1", GIT_ATTR_NOSYSTEM="1")
    for name in ("host", "client"):
        subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(root / name)], env=environment, check=True, capture_output=True, timeout=10)
    (root / "authorized").write_bytes((root / "client.pub").read_bytes())
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    config = root / "sshd_config"
    config.write_text(f"Port {port}\nListenAddress 127.0.0.1\nHostKey {root}/host\nPidFile {root}/pid\nAuthorizedKeysFile {root}/authorized\nStrictModes no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nUsePAM no\nAllowUsers {getpass.getuser()}\nLogLevel ERROR\n")
    known = root / "known_hosts"
    known.write_text(f"[127.0.0.1]:{port} " + (root / "host.pub").read_text())
    agent_socket = root / "agent.sock"
    environment["SSH_AUTH_SOCK"] = str(agent_socket)
    agent = subprocess.Popen(["ssh-agent", "-D", "-a", str(agent_socket)], env=environment, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    server = None
    try:
        for _ in range(100):
            if agent_socket.exists() or agent.poll() is not None:
                break
            time.sleep(0.02)
        assert agent.poll() is None and agent_socket.exists(), "fixture agent did not start"
        subprocess.run(["ssh-add", str(root / "client")], env=environment, check=True, capture_output=True, timeout=10)
        sshd = shutil.which("sshd") or "/usr/sbin/sshd"
        server = subprocess.Popen([sshd, "-D", "-e", "-f", str(config)], env=environment, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for _ in range(100):
            assert server.poll() is None, "fixture sshd did not start"
            try:
                with socket.create_connection(("127.0.0.1", port), timeout=0.1):
                    break
            except OSError:
                time.sleep(0.02)
        else:
            raise AssertionError("fixture sshd did not listen")
        environment.update(HUNKRA_REAL_SSH_FIXTURE=str(root), HUNKRA_REAL_SSH_SOURCE=sys.argv[3], HUNKRA_REAL_SSH_PORT=str(port), HUNKRA_REAL_SSH_USER=getpass.getuser(), GIT_SSH_VARIANT="ssh", GIT_SSH_COMMAND=f"ssh -F /dev/null -o BatchMode=yes -o IdentityFile=none -o StrictHostKeyChecking=yes -o UserKnownHostsFile='{known}'")
        return subprocess.run([sys.argv[1], "--exact", "real_ssh_agent_clone_fetch_push_pull_are_isolated", "--nocapture"], env=environment, timeout=40).returncode
    finally:
        for child in (server, agent):
            if child is not None:
                child.terminate()
                child.wait(timeout=5)


if __name__ == "__main__":
    sys.exit(run())
