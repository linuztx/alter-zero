#!/usr/bin/env python3
"""What the built image promises, checked from inside it (docs/docker.md).

Run by docker/tests/smoke.sh, which pipes this file into a throwaway container:

    docker run --rm -i --network=none alter-zero:kali python3 - < docker/tests/smoke_image.py

`--network=none` is the point, not a convenience. The last check launches the
real CLI to prove telemetry is still ON in this image, and with no route out
of the container the only collector it can reach is the stub below — so a
test run can never land in the production numbers.

Standard library only: this runs on the image's own python3.
"""

import errno
import fcntl
import http.server
import json
import os
import pty
import re
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
from pathlib import Path

# Every tool docker/README.md says is in the image — the binary names, not the
# package names: `rg` is ripgrep, `nc` netcat-openbsd, `dig` bind9-dnsutils.
TOOLS = (
    "alter-zero", "bash", "git", "ssh", "curl", "wget", "jq", "rg", "file", "less",
    "nano", "tree", "xxd", "unzip", "python3", "ps", "tini", "getcap",
    "ifconfig", "netstat", "ip", "ss", "ping", "traceroute",
    "nmap", "whois", "nc", "socat", "dig", "nslookup", "openssl",
)
# Forwarding `-e TERM` from these terminals must not break less/nano/git.
TERMINALS = ("xterm-256color", "tmux-256color", "xterm-kitty", "alacritty", "foot", "wezterm")
# Every Python tool runs out of this virtualenv (docs/docker.md *Python*).
VENV = "/opt/az-venv"
# pip's cache, deliberately outside the /root volume (docs/docker.md *Python*).
PIP_CACHE = "/var/cache/pip"
# Setting any of these in the image would change what the app reports home.
TELEMETRY_KEYS = ("ALTER_ZERO_TELEMETRY", "DO_NOT_TRACK", "ALTER_ZERO_TELEMETRY_URL")


def run(*args, **kwargs):
    return subprocess.run(args, capture_output=True, text=True, timeout=60, **kwargs)


def check_identity():
    assert (os.getuid(), os.getgid()) == (0, 0), "the image must run as root"
    assert os.environ.get("HOME") == "/root", f"HOME is {os.environ.get('HOME')!r}"
    assert Path.cwd() == Path("/workspace"), f"the working directory is {Path.cwd()}"
    probe = Path("/workspace/.smoke-write")
    probe.write_text("ok")
    probe.unlink()
    passwd = Path("/etc/passwd").read_text()
    humans = [line.split(":")[0] for line in passwd.splitlines()
              if line and 1000 <= int(line.split(":")[2]) < 65000]
    assert not humans, f"the image adds no user, but has: {humans}"
    os_release = Path("/etc/os-release").read_text()
    assert re.search(r"^ID=kali$", os_release, re.M), "telemetry reads ID= from here"


def check_telemetry_is_left_alone():
    for key in TELEMETRY_KEYS:
        assert key not in os.environ, f"the image sets {key}; telemetry must keep its defaults"
    assert Path("/usr/share/doc/alter-zero/TELEMETRY.md").is_file(), "the data statement ships too"


def check_tools():
    missing = [tool for tool in TOOLS if not shutil.which(tool)]
    assert not missing, f"missing from the image: {missing}"
    for terminal in TERMINALS:
        assert run("infocmp", terminal).returncode == 0, f"no terminfo entry for {terminal}"
    version = run("alter-zero", "--version").stdout.strip()
    assert re.fullmatch(r"alter-zero \d+\.\d+\.\d+(?:[-+][\w.-]+)?", version), version
    return version.removeprefix("alter-zero ")


def check_venv():
    """The virtualenv is active for *every* process, not just interactive bash.

    The agent runs its own `bash` tool through `sh -c`, and this image's /bin/sh
    is dash — so a .bashrc activation would reach a human's shell and miss the
    agent entirely. The image sets VIRTUAL_ENV and PATH instead, which every
    process inherits however it was started."""
    assert os.environ.get("VIRTUAL_ENV") == VENV, os.environ.get("VIRTUAL_ENV")
    # This interpreter was found on PATH, so it is the venv's own.
    assert sys.prefix == VENV, sys.prefix
    for tool in ("python", "python3", "pip", "pip3"):
        assert shutil.which(tool) == f"{VENV}/bin/{tool}", f"{tool} -> {shutil.which(tool)}"
    assert run("pip", "--version").stdout.startswith(f"pip "), "the venv has no working pip"
    assert f"{VENV}/lib" in run("pip", "--version").stdout, run("pip", "--version").stdout

    # Kali marks its system Python externally managed (PEP 668), which is what
    # makes a `pip install` there fail; the venv is the supported way in.
    assert list(Path("/usr/lib").glob("python3*/EXTERNALLY-MANAGED")), "no PEP 668 marker"
    assert Path("/usr/bin/python3").is_file(), "the system interpreter is still there"

    # The two shells that matter: dash, which the agent's tool calls use, and
    # an interactive bash, which is what a person gets from `exec -it … bash`.
    for shell in (["sh", "-c"], ["bash", "-c"], ["bash", "-ic"]):
        seen = run(*shell, "command -v python3; echo $VIRTUAL_ENV").stdout.split()
        assert seen == [f"{VENV}/bin/python3", VENV], f"{shell}: {seen}"
    # …and PATH carries it exactly once, so sourcing activate did not stack.
    entries = run("bash", "-ic", "printf %s \"$PATH\"").stdout.split(":")
    assert entries.count(f"{VENV}/bin") == 1, entries
    # The prompt names it. Assert the *rendered* prompt (bash's ${PS1@P})
    # rather than a literal prefix: Kali's own .bashrc draws $VIRTUAL_ENV into
    # its ┌──(az-venv)(root㉿host) line and sets VIRTUAL_ENV_DISABLE_PROMPT=1
    # so activate does not also prepend one. Either mechanism, same thing seen.
    prompt = run("bash", "-ic", 'printf "%s" "${PS1@P}"').stdout
    assert "az-venv" in prompt, f"the prompt does not name the venv: {prompt!r}"
    assert run("bash", "-ic", "type -t deactivate").stdout.strip() == "function", "no deactivate"


def check_pip_cache():
    """pip's cache must not live under /root.

    /root is a named volume, and under rootless Podman a volume whose host
    directory falls outside the user's subuid range shows up inside the
    container as nobody: container root cannot write it, CAP_DAC_OVERRIDE
    stopping at the namespace boundary. pip then greets every install with
    "The directory '/root/.cache/pip' or its parent directory is not owned or
    is not writable … The cache has been disabled." A cache is the one thing
    here with no business in the volume that holds sign-ins anyway, so it
    lives in the image, where it is always the container root's own."""
    cache = os.environ.get("PIP_CACHE_DIR")
    assert cache == PIP_CACHE, f"PIP_CACHE_DIR is {cache!r}"
    assert not cache.startswith("/root"), f"{cache} is inside the home volume"
    assert Path(cache).is_dir(), f"{cache} does not exist"
    assert os.access(cache, os.W_OK), f"{cache} is not writable"
    # pip agrees, rather than us merely having exported a variable at it.
    assert run("pip", "cache", "dir").stdout.strip() == cache, run("pip", "cache", "dir").stdout
    # And nothing warns on a real install attempt (offline, so it fails at the
    # network — the warning, if any, is printed before that).
    attempt = run("pip", "install", "a-package-that-does-not-exist")
    assert "cache has been disabled" not in attempt.stdout + attempt.stderr, attempt.stdout


def check_scanner_runs():
    """Kali's forced file capabilities make the binary un-exec-able here."""
    binary = "/usr/lib/nmap/nmap"
    assert run("getcap", binary).stdout.strip() == "", "the file capabilities are back"
    result = run("nmap", "--version")
    assert result.returncode == 0, f"nmap does not run: {result.stderr.strip()}"
    # A package run would re-apply them (the postinst does); the dpkg hook
    # must take them off again. Re-applying by hand is that, minus the network.
    assert run("setcap", "cap_net_raw,cap_net_admin,cap_net_bind_service+eip", binary).returncode == 0
    hook = Path("/etc/apt/apt.conf.d/90alter-zero-nmap-caps").read_text()
    command = re.search(r'Post-Invoke \{ "(.+)"; \};', hook)
    assert command, f"the dpkg hook is not where apt reads it: {hook!r}"
    assert run("sh", "-c", command.group(1)).returncode == 0
    assert run("getcap", binary).stdout.strip() == "", "the dpkg hook did not strip them"
    assert run("nmap", "--version").returncode == 0


def check_nothing_listens():
    listeners = run("ss", "-H", "-lntu").stdout.strip()
    assert not listeners, f"the idle image has listeners: {listeners}"


class Collector(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = json.loads(self.rfile.read(length)) if 0 < length <= 8192 else None
        self.server.pings.append({"path": self.path, "agent": self.headers.get("User-Agent"), "body": body})
        self.send_response(204)
        self.end_headers()

    def log_message(self, *_args):
        pass


class Terminal:
    """The CLI on a real PTY. It asks for the cursor position at startup
    (CLAUDE.md invariant 1), so every query gets an answer."""

    def __init__(self, env, cwd):
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            try:
                os.chdir(cwd)
                os.execvpe("alter-zero", ["alter-zero"], env)
            finally:
                os._exit(127)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
        self.output = bytearray()
        self.answered = 0
        self.status = None

    def pump(self, collector, wait=0.1):
        ready, _, _ = select.select([self.fd, collector], [], [], wait)
        if collector in ready:
            collector.handle_request()
        if self.fd in ready:
            try:
                chunk = os.read(self.fd, 65536)
            except OSError as error:
                if error.errno != errno.EIO:
                    raise
                chunk = b""
            self.output.extend(chunk)
            asked = self.output.count(b"\x1b[6n")
            for _ in range(self.answered, asked):
                os.write(self.fd, b"\x1b[1;1R")
            self.answered = asked
        if self.status is None:
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                self.status = os.waitstatus_to_exitcode(status)

    def text(self):
        return re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", self.output.decode("utf-8", "replace"))

    def close(self, collector):
        if self.status is None:
            os.write(self.fd, b"/quit\r")
            deadline = time.monotonic() + 5
            while self.status is None and time.monotonic() < deadline:
                self.pump(collector)
        if self.status is None:
            os.kill(self.pid, signal.SIGKILL)
            os.waitpid(self.pid, 0)
        os.close(self.fd)


def check_telemetry_is_on(version):
    """The app, launched as a user would launch it, pings — once."""
    with tempfile.TemporaryDirectory(prefix="alter-zero-smoke-") as scratch:
        root = Path(scratch)
        for name in ("config", "sessions", "skills", "agents", "work"):
            (root / name).mkdir()
        with http.server.HTTPServer(("127.0.0.1", 0), Collector) as collector:
            collector.pings = []
            env = dict(os.environ)
            for key in TELEMETRY_KEYS:
                assert key not in env
            env.update({
                # Where it pings, and nothing about WHETHER it does.
                "ALTER_ZERO_TELEMETRY_URL": f"http://127.0.0.1:{collector.server_port}/v1/ping",
                "NO_PROXY": "127.0.0.1", "no_proxy": "127.0.0.1",
                # A scratch config home, so the run is a first install.
                "ALTER_ZERO_CONFIG_DIR": str(root / "config"),
                "ALTER_ZERO_SESSIONS_DIR": str(root / "sessions"),
                "ALTER_ZERO_SKILLS_DIR": str(root / "skills"),
                "ALTER_ZERO_AGENTS_DIR": str(root / "agents"),
                "ALTER_ZERO_HISTORY_FILE": "/dev/null",
                "ALTER_ZERO_UPDATE_CHECK": "0",
                "ALTER_ZERO_STARTUP_DELAY_MS": "0",
                "ALTER_ZERO_DISABLE_KEYBOARD_ENHANCEMENT": "1",
            })
            state_file = root / "config" / "telemetry.json"

            def state():
                try:
                    return json.loads(state_file.read_text())
                except (FileNotFoundError, json.JSONDecodeError):
                    return {}

            first = Terminal(env, root / "work")
            try:
                deadline = time.monotonic() + 20
                while time.monotonic() < deadline:
                    first.pump(collector)
                    assert first.status is None, f"the CLI exited early:\n{first.text()[-1500:]}"
                    screen = first.text()
                    # Telemetry can finish before the first frame reaches the PTY.
                    if (collector.pings and state().get("last_ping_version") == version
                            and "Alter Zero" in screen and "ALTER_ZERO_TELEMETRY=0" in screen):
                        break
                else:
                    raise AssertionError(f"CLI startup or telemetry did not complete:\n{first.text()[-1500:]}")
                screen = first.text()
            finally:
                first.close(collector)

            assert "Alter Zero" in screen, "the banner never rendered"
            assert "ALTER_ZERO_TELEMETRY=0" in screen, "the first-run notice must name the off switch"
            assert len(collector.pings) == 1, collector.pings
            ping = collector.pings[0]
            assert ping["path"] == "/v1/ping" and ping["agent"] == f"alter-zero/{version}", ping
            body = ping["body"]
            assert body["version"] == version and body["os"] == "linux", body
            assert body["distro"] == "kali", f"the container must report itself as kali: {body}"
            kept = state()
            assert kept.get("enabled") is True and kept.get("notice_shown") is True, kept
            assert body["id"] == kept.get("install_id"), "the ping carries the persisted install id"

            # A relaunch on the same config home is the same install, the same
            # day: no second ping, no second notice. This is what a persistent
            # /root volume buys — and what a throwaway home would overcount.
            second = Terminal(env, root / "work")
            try:
                deadline = time.monotonic() + 4
                while time.monotonic() < deadline:
                    second.pump(collector)
                again = second.text()
            finally:
                second.close(collector)
            assert "Alter Zero" in again, "the relaunch never rendered"
            assert "ALTER_ZERO_TELEMETRY=0" not in again, "the notice repeated"
            assert len(collector.pings) == 1, "the relaunch pinged again"
            assert state().get("install_id") == kept["install_id"], "the install id changed"
            return body


def main():
    interfaces = {line.split(":")[0].strip() for line in Path("/proc/net/dev").read_text().splitlines() if ":" in line}
    assert interfaces == {"lo"}, f"run this with --network=none, not with {sorted(interfaces)}"
    check_identity()
    check_telemetry_is_left_alone()
    version = check_tools()
    check_venv()
    check_pip_cache()
    check_scanner_runs()
    check_nothing_listens()
    ping = check_telemetry_is_on(version)
    print(json.dumps({"result": "passed", "alter_zero": version, "ping": ping}, indent=2))


if __name__ == "__main__":
    main()
