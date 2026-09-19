"""docker/build.sh and docker/run.sh, with no engine and no network (docs/docker.md).

    python3 -m unittest discover -s docker/tests -p 'test_*.py' -v

Each test runs the real script under a PATH that holds nothing but a sandbox
of symlinks to the handful of coreutils the scripts use, plus recording stubs
named docker, podman, curl, wget and xauth. So what is asserted is the exact
argv the engine would have been given — and a bug here can never reach a real
container, a real registry or github.com.
"""

import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BUILD = ROOT / "docker" / "build.sh"
RUN = ROOT / "docker" / "run.sh"

# Everything the two scripts and lib.sh call that is not a shell builtin.
COREUTILS = ("dirname", "sed", "awk", "grep", "mkdir", "tr", "cut", "cat", "rm", "chmod")

STUB = """#!{python}
import json, os, sys
name = os.path.basename(sys.argv[0])
args = sys.argv[1:]
stdin = ""
if name == "xauth" and "nmerge" in args:
    stdin = sys.stdin.read()
    open(args[args.index("-f") + 1], "w").write(stdin)
with open(os.environ["STUB_LOG"], "a") as log:
    log.write(json.dumps({{"name": name, "args": args, "stdin": stdin}}) + "\\n")
env = os.environ.get
if name == "curl":
    print(env("STUB_LATEST_URL", ""), end="")
    sys.exit(int(env("STUB_CURL_EXIT", "0")))
if name == "wget":
    for hop in env("STUB_WGET_HOPS", "").split():
        print("  Location: " + hop, file=sys.stderr)
    sys.exit(int(env("STUB_WGET_EXIT", "0")))
if name == "xauth":
    if "nlist" in args:
        print(env("STUB_XAUTH_NLIST", ""))
    sys.exit(0)
if name == "selinuxenabled":
    sys.exit(0)
# docker / podman
if args[:2] == ["image", "inspect"]:
    sys.exit(0 if env("STUB_IMAGE", "1") == "1" else 1)
if args[:2] == ["container", "inspect"]:
    sys.exit(0 if env("STUB_CONTAINER", "0") == "1" else 1)
if args[:1] == ["run"]:
    sys.exit(int(env("STUB_RUN_EXIT", "0")))
if args[:1] == ["build"]:
    sys.exit(int(env("STUB_BUILD_EXIT", "0")))
sys.exit(0)
"""

LATEST = "https://github.com/linuztx/alter-zero/releases/tag/v9.8.7"


class ScriptCase(unittest.TestCase):
    #: The stubs a test's sandbox holds. Leaving one out is how "not installed" is said.
    stubs = ("docker", "podman", "curl")

    def setUp(self):
        scratch = tempfile.TemporaryDirectory(prefix="alter-zero-docker-test-")
        self.addCleanup(scratch.cleanup)
        self.dir = Path(scratch.name).resolve()
        self.bin = self.dir / "bin"
        self.bin.mkdir()
        self.home = self.dir / "home"
        self.home.mkdir()
        self.log = self.dir / "calls.jsonl"
        for tool in COREUTILS:
            real = shutil.which(tool)
            self.assertIsNotNone(real, f"this machine has no {tool}")
            (self.bin / tool).symlink_to(real)
        for stub in self.stubs:
            self.add_stub(stub)

    def add_stub(self, name):
        path = self.bin / name
        path.write_text(STUB.format(python=sys.executable))
        path.chmod(0o755)

    def script(self, script, *args, cwd=None, **env):
        self.log.unlink(missing_ok=True)
        environment = {
            "PATH": str(self.bin),
            "HOME": str(self.home),
            "STUB_LOG": str(self.log),
            "STUB_LATEST_URL": LATEST,
        }
        environment.update(env)
        result = subprocess.run(
            ["/bin/sh", str(script), *args], cwd=cwd or self.dir, env=environment,
            capture_output=True, text=True, timeout=60,
        )
        calls = []
        if self.log.exists():
            calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        return result, calls

    def engine_calls(self, calls, verb):
        return [c for c in calls if c["name"] in ("docker", "podman") and c["args"][:1] == [verb]]

    def one(self, calls, verb):
        found = self.engine_calls(calls, verb)
        self.assertEqual(len(found), 1, f"expected one `{verb}`, got {calls}")
        return found[0]

    def assertRefused(self, result, calls, *verbs, saying=None):
        self.assertNotEqual(result.returncode, 0, result.stdout)
        for verb in verbs or ("build", "run"):
            self.assertEqual(self.engine_calls(calls, verb), [], f"`{verb}` ran anyway")
        if saying:
            self.assertIn(saying, result.stderr)


def value_of(args, flag):
    """Every value `flag` was given, in order."""
    return [args[i + 1] for i, arg in enumerate(args) if arg == flag]


# ---------------------------------------------------------------------------
class BuildTests(ScriptCase):
    def build(self, *args, **env):
        return self.script(BUILD, *args, **env)

    def test_the_latest_release_is_resolved_on_every_build_and_handed_to_the_engine(self):
        # Resolved here, before the layer cache is consulted: a `latest`
        # resolved inside a cached layer would install the old release forever.
        for tag in ("v9.8.7", "v9.8.8"):
            result, calls = self.build(STUB_LATEST_URL=f"https://github.com/linuztx/alter-zero/releases/tag/{tag}")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual([c["name"] for c in calls], ["curl", "docker"])
            args = self.one(calls, "build")["args"]
            self.assertIn(f"ALTER_ZERO_VERSION={tag}", value_of(args, "--build-arg"))
            self.assertIn(tag, result.stdout)

    def test_docker_is_the_default_and_the_context_is_the_repository_root(self):
        result, calls = self.build()
        self.assertEqual(result.returncode, 0, result.stderr)
        call = self.one(calls, "build")
        self.assertEqual(call["name"], "docker")
        self.assertEqual(call["args"][-1], str(ROOT), "the context comes last")
        self.assertEqual(value_of(call["args"], "-f"), [str(ROOT / "docker" / "Dockerfile")])
        self.assertEqual(value_of(call["args"], "-t"), ["alter-zero:kali"])
        self.assertIn("--pull", call["args"], "a rolling base is refreshed by default")

    def test_engine_podman_builds_with_podman_in_every_spelling(self):
        for args, env in ((["--engine", "podman"], {}), (["--engine=podman"], {}), (["-e", "podman"], {}),
                          ([], {"CONTAINER_ENGINE": "podman"})):
            with self.subTest(args=args, env=env):
                result, calls = self.build(*args, **env)
                self.assertEqual(result.returncode, 0, result.stderr)
                call = self.one(calls, "build")
                self.assertEqual(call["name"], "podman")
                self.assertIn("--pull=always", call["args"])
                self.assertNotIn("--pull", call["args"])
                self.assertIn("docker/run.sh --engine podman", result.stdout, "the next step names the engine")

    def test_the_flag_outranks_the_environment(self):
        _, calls = self.build("--engine", "docker", CONTAINER_ENGINE="podman")
        self.assertEqual(self.one(calls, "build")["name"], "docker")

    def test_a_pinned_version_needs_no_network(self):
        for given in ("1.2.3-rc.1", "v1.2.3-rc.1"):
            result, calls = self.build("--version", given)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual([c["name"] for c in calls], ["docker"], "no curl")
            self.assertIn("ALTER_ZERO_VERSION=v1.2.3-rc.1", value_of(calls[0]["args"], "--build-arg"))

    def test_a_tag_that_is_not_a_version_never_reaches_the_engine(self):
        cases = [
            (["--version", "v1.2.3/../../bad"], {}),
            (["--version", "--build-arg"], {}),
            (["--version", "latest"], {}),
            ([], {"STUB_LATEST_URL": "https://example.com/releases/tag/v1.2.3"}),
            ([], {"STUB_LATEST_URL": "https://github.com/linuztx/alter-zero/releases/latest"}),
            ([], {"STUB_LATEST_URL": "https://github.com/linuztx/alter-zero/releases/tag/v1.2.3;id"}),
            ([], {"STUB_CURL_EXIT": "22"}),
        ]
        for args, env in cases:
            with self.subTest(args=args, env=env):
                result, calls = self.build(*args, **env)
                self.assertRefused(result, calls)

    def test_an_unknown_engine_is_refused_by_name(self):
        result, calls = self.build("--engine", "nerdctl")
        self.assertRefused(result, calls, saying="unknown engine: nerdctl")
        self.assertEqual(calls, [], "not even the release lookup")

    def test_extra_packages_ride_a_build_argument(self):
        result, calls = self.build("--with", "binutils tcpdump", "--with=libfoo2.0-dev")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("EXTRA_PACKAGES=binutils tcpdump libfoo2.0-dev", value_of(self.one(calls, "build")["args"], "--build-arg"))

    def test_without_extra_packages_the_argument_is_not_sent(self):
        _, calls = self.build()
        self.assertFalse([v for v in value_of(self.one(calls, "build")["args"], "--build-arg") if v.startswith("EXTRA_")])

    def test_a_word_that_is_not_a_package_name_is_refused(self):
        # The list is word-split into `apt-get install`: an option-shaped word
        # would be apt's to interpret.
        for words in ("-oAPT::Update::Pre-Invoke::=id", "Binutils", "a;b", "$(id)", "pkg=1.0"):
            with self.subTest(words=words):
                result, calls = self.build("--with", words)
                self.assertRefused(result, calls, saying="not a package name")

    def test_no_pull_and_no_cache(self):
        _, calls = self.build("--no-pull", "--no-cache")
        args = self.one(calls, "build")["args"]
        self.assertNotIn("--pull", args)
        self.assertIn("--no-cache", args)

    def test_arguments_after_a_double_dash_are_the_engines_own(self):
        _, calls = self.build("--", "--platform", "linux/arm64")
        args = self.one(calls, "build")["args"]
        self.assertEqual(args[-3:], ["--platform", "linux/arm64", str(ROOT)], "after ours, before the context")

    def test_a_forks_base_url_resolves_and_downloads_from_the_fork(self):
        fork = "https://github.com/someone/alter-zero"
        result, calls = self.build(ALTER_ZERO_INSTALL_BASE_URL=fork, STUB_LATEST_URL=f"{fork}/releases/tag/v2.0.0")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"{fork}/releases/latest", calls[0]["args"])
        self.assertIn(f"ALTER_ZERO_INSTALL_BASE_URL={fork}", value_of(self.one(calls, "build")["args"], "--build-arg"))
        # And the upstream's redirect is then the unexpected one.
        result, calls = self.build(ALTER_ZERO_INSTALL_BASE_URL=fork)
        self.assertRefused(result, calls)

    def test_a_workspace_folder_is_handed_to_run_sh_after_the_build(self):
        result, calls = self.build("--engine", "podman", "--name", "site", "--replace", "projects/site", STUB_CONTAINER="1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([c["args"][0] for c in calls if c["name"] == "podman"],
                         ["build", "image", "container", "rm", "run"], "built first, then created")
        run = self.one(calls, "run")
        self.assertEqual(value_of(run["args"], "--name"), ["site"])
        self.assertIn(f"{self.dir / 'projects' / 'site'}:/workspace", value_of(run["args"], "--volume"))
        self.assertTrue((self.dir / "projects" / "site").is_dir(), "created for you, as you")

    def test_a_workspace_that_is_a_file_is_refused_before_the_build(self):
        (self.dir / "notes.txt").write_text("x")
        result, calls = self.build("notes.txt")
        self.assertRefused(result, calls, saying="not a folder")

    def test_container_options_without_a_workspace_are_refused(self):
        for option in (["--name", "site"], ["--replace"]):
            result, calls = self.build(*option)
            self.assertRefused(result, calls, saying="only applies with a workspace folder")

    def test_a_failed_build_creates_no_container(self):
        result, calls = self.build("site", STUB_BUILD_EXIT="1")
        self.assertRefused(result, calls, "run")

    def test_help_runs_nothing(self):
        for flag in ("--help", "-h"):
            result, calls = self.build(flag)
            self.assertEqual(result.returncode, 0)
            self.assertIn("docker/build.sh --engine podman", result.stdout)
            self.assertNotIn("set -eu", result.stdout)
            self.assertEqual(calls, [])

    def test_an_unknown_option_is_refused(self):
        result, calls = self.build("--frobnicate")
        self.assertRefused(result, calls, saying="unknown option: --frobnicate")


class BuildWithOnlyPodman(ScriptCase):
    stubs = ("podman", "curl")

    def test_podman_is_picked_when_docker_is_not_installed(self):
        result, calls = self.script(BUILD)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.one(calls, "build")["name"], "podman")
        self.assertNotIn("--engine", result.stdout, "an auto-picked engine needs no flag in the hints")

    def test_asking_for_the_missing_engine_says_so(self):
        result, calls = self.script(BUILD, "--engine", "docker")
        self.assertRefused(result, calls, saying="docker is not installed")


class BuildWithNoEngine(ScriptCase):
    stubs = ("curl",)

    def test_no_engine_at_all_is_said_plainly(self):
        result, calls = self.script(BUILD)
        self.assertRefused(result, calls, saying="neither docker nor podman is installed")


class BuildWithWgetOnly(ScriptCase):
    stubs = ("docker", "wget")

    def test_wget_resolves_the_latest_release_where_curl_is_missing(self):
        # A minimal Debian server ships wget and no curl.
        hops = "https://github.com/linuztx/alter-zero/releases/latest https://github.com/linuztx/alter-zero/releases/tag/v3.1.4"
        result, calls = self.script(BUILD, STUB_WGET_HOPS=hops)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("ALTER_ZERO_VERSION=v3.1.4", value_of(self.one(calls, "build")["args"], "--build-arg"))

    def test_a_wget_that_followed_no_redirect_is_a_failure(self):
        result, calls = self.script(BUILD, STUB_WGET_HOPS="")
        self.assertRefused(result, calls, saying="could not resolve the latest release")


# ---------------------------------------------------------------------------
class RunTests(ScriptCase):
    stubs = ("docker", "podman", "xauth")

    def run_sh(self, *args, **env):
        return self.script(RUN, *args, **env)

    def test_the_defaults(self):
        result, calls = self.run_sh()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.one(calls, "run")["args"], [
            "run", "--detach", "--name", "alter-zero-kali", "--hostname", "alter-zero-kali",
            "--security-opt", "no-new-privileges",
            "--volume", "alter-zero-home:/root", "--volume", "alter-zero-workspace:/workspace",
            "--publish", "127.0.0.1:8080:8080", "--publish", "127.0.0.1:8888:8888",
            "--restart", "unless-stopped",
            "alter-zero:kali",
        ])

    def test_it_never_asks_for_privileged_mode_or_another_user(self):
        _, calls = self.run_sh("--clipboard", WAYLAND_DISPLAY="", DISPLAY="")
        _, calls = self.run_sh()
        args = self.one(calls, "run")["args"]
        for flag in ("--privileged", "--user", "-u", "--cap-add", "--network"):
            self.assertNotIn(flag, args)

    def test_it_prints_the_command_that_forwards_the_terminals_identity(self):
        # The five variables are what let the app pick real graphics over
        # half-blocks (docs/images.md); the README shows this exact command.
        for engine in ("docker", "podman"):
            result, _ = self.run_sh("--engine", engine)
            flat = " ".join(result.stdout.replace("\\\n", " ").split())
            self.assertIn(
                f"{engine} exec -it -e TERM -e COLORTERM -e TERM_PROGRAM -e KITTY_WINDOW_ID -e TMUX alter-zero-kali alter-zero",
                flat,
            )

    def test_a_folder_becomes_the_workspace(self):
        (self.dir / "my project").mkdir()
        for given, expected in (("my project", self.dir / "my project"), (str(self.dir / "new" / "deep"), self.dir / "new" / "deep")):
            with self.subTest(given=given):
                result, calls = self.run_sh(given)
                self.assertEqual(result.returncode, 0, result.stderr)
                volumes = value_of(self.one(calls, "run")["args"], "--volume")
                self.assertEqual(volumes, ["alter-zero-home:/root", f"{expected}:/workspace"])
                self.assertTrue(expected.is_dir(), "a missing folder is created here, not by the engine as root")

    def test_a_symlinked_folder_is_mounted_by_its_real_path(self):
        (self.dir / "real").mkdir()
        (self.dir / "link").symlink_to(self.dir / "real")
        _, calls = self.run_sh("link")
        self.assertIn(f"{self.dir / 'real'}:/workspace", value_of(self.one(calls, "run")["args"], "--volume"))

    def test_folders_that_cannot_be_a_workspace(self):
        (self.dir / "a:b").mkdir()
        (self.dir / "file").write_text("x")
        for given, why in (("/", "refusing to mount /"), ("a:b", "':'"), ("file", "not a folder")):
            with self.subTest(given=given):
                result, calls = self.run_sh(given)
                self.assertRefused(result, calls, saying=why)

    def test_two_folders_is_a_mistake(self):
        result, calls = self.run_sh("one", "two")
        self.assertRefused(result, calls, saying="only one workspace folder")

    def test_port_forms(self):
        cases = {
            ("--port", "3000"): ["127.0.0.1:3000:3000"],
            ("--port=9000:80",): ["127.0.0.1:9000:80"],
            ("-p", "5353/udp"): ["127.0.0.1:5353:5353/udp"],
            ("-p", "9000:53/udp"): ["127.0.0.1:9000:53/udp"],
            ("-p", "0.0.0.0:80:8080"): ["0.0.0.0:80:8080"],
            ("-p", "[::1]:80:8080/tcp"): ["[::1]:80:8080/tcp"],
            ("-p", "3000", "-p", "3001"): ["127.0.0.1:3000:3000", "127.0.0.1:3001:3001"],
        }
        for args, expected in cases.items():
            with self.subTest(args=args):
                result, calls = self.run_sh(*args)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(value_of(self.one(calls, "run")["args"], "--publish"), expected)

    def test_bind_applies_wherever_it_appears(self):
        for args in (("--bind", "0.0.0.0", "--port", "3000"), ("--port", "3000", "--bind=0.0.0.0")):
            _, calls = self.run_sh(*args)
            self.assertEqual(value_of(self.one(calls, "run")["args"], "--publish"), ["0.0.0.0:3000:3000"])
        _, calls = self.run_sh("-b", "0.0.0.0")
        self.assertEqual(value_of(self.one(calls, "run")["args"], "--publish"), ["0.0.0.0:8080:8080", "0.0.0.0:8888:8888"])
        _, calls = self.run_sh("--bind", "::1", "--port", "3000")
        self.assertEqual(value_of(self.one(calls, "run")["args"], "--publish"), ["[::1]:3000:3000"])

    def test_no_ports(self):
        result, calls = self.run_sh("--no-ports")
        self.assertEqual(value_of(self.one(calls, "run")["args"], "--publish"), [])
        self.assertIn("none published", result.stdout)

    def test_bad_ports_and_addresses_are_refused(self):
        for args in (("--port", "0"), ("--port", "65536"), ("--port", "http"), ("--port", "80:x"),
                     ("--port", "123456"), ("--bind", "example.com"), ("--no-ports", "--port", "80")):
            with self.subTest(args=args):
                result, calls = self.run_sh(*args)
                self.assertRefused(result, calls)

    def test_a_missing_image_points_at_the_build(self):
        result, calls = self.run_sh("--engine", "podman", STUB_IMAGE="0")
        self.assertRefused(result, calls, saying="docker/build.sh --engine podman")

    def test_an_existing_container_is_never_touched_without_replace(self):
        result, calls = self.run_sh(STUB_CONTAINER="1")
        self.assertRefused(result, calls, "run", "rm", saying="--replace")

    def test_replace_removes_then_creates(self):
        result, calls = self.run_sh("--replace", "--name", "site", STUB_CONTAINER="1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([c["args"][0] for c in calls], ["image", "container", "rm", "run"])
        self.assertEqual(self.one(calls, "rm")["args"], ["rm", "-f", "site"])
        self.assertIn("untouched", result.stdout, "and says what was NOT removed")

    def test_a_container_that_failed_to_start_is_cleaned_up(self):
        # A taken port fails after `create`, and the leftover would block the retry.
        result, calls = self.run_sh(STUB_RUN_EXIT="125")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual([c["args"][0] for c in calls], ["image", "container", "run", "rm"])
        self.assertIn("--port", result.stderr)
        self.assertNotIn("Started", result.stdout)

    def test_names_the_engine_would_misread_are_refused(self):
        for option in ("--name", "--home-volume"):
            for bad in ("-rf", "a b", "a/b", "a,b", ""):
                with self.subTest(option=option, bad=bad):
                    result, calls = self.run_sh(option, bad)
                    self.assertRefused(result, calls)

    def test_a_name_that_is_no_hostname_gets_one(self):
        _, calls = self.run_sh("--name", "my_box.1")
        args = self.one(calls, "run")["args"]
        self.assertEqual((value_of(args, "--name"), value_of(args, "--hostname")), (["my_box.1"], ["my-box-1"]))

    def test_home_volume_and_image_and_passthrough(self):
        _, calls = self.run_sh("--home-volume", "work-home", "--image", "me/az:dev", "--", "--cap-add", "NET_RAW")
        args = self.one(calls, "run")["args"]
        self.assertIn("work-home:/root", value_of(args, "--volume"))
        self.assertEqual(args[-3:], ["--cap-add", "NET_RAW", "me/az:dev"], "after ours, before the image")

    def test_help_runs_nothing(self):
        result, calls = self.run_sh("--help")
        self.assertEqual((result.returncode, calls), (0, []))
        self.assertIn("--clipboard", result.stdout)

    # ----- the clipboard ---------------------------------------------------

    def socket_at(self, path):
        path.parent.mkdir(parents=True, exist_ok=True)
        listener = socket.socket(socket.AF_UNIX)
        listener.bind(str(path))
        self.addCleanup(listener.close)
        return path

    def test_wayland_forwards_the_one_socket_and_never_the_runtime_dir(self):
        runtime = self.dir / "run-user"
        sock = self.socket_at(runtime / "wayland-1")
        for display in ("wayland-1", str(sock)):
            with self.subTest(WAYLAND_DISPLAY=display):
                result, calls = self.run_sh("--clipboard", WAYLAND_DISPLAY=display, XDG_RUNTIME_DIR=str(runtime))
                self.assertEqual(result.returncode, 0, result.stderr)
                args = self.one(calls, "run")["args"]
                self.assertEqual(value_of(args, "--mount"), [f"type=bind,src={sock},dst=/run/alter-zero/wayland-0,ro"])
                self.assertEqual(value_of(args, "--env"), ["XDG_RUNTIME_DIR=/run/alter-zero", "WAYLAND_DISPLAY=wayland-0"])
                self.assertNotIn(str(runtime) + ":", " ".join(args))
                self.assertIn("Wayland forwarded", result.stdout)

    def test_a_clipboard_container_is_not_restarted_at_boot(self):
        # Its sockets belong to a desktop login that does not exist yet then.
        sock = self.socket_at(self.dir / "rt" / "wayland-0")
        _, calls = self.run_sh("--clipboard", WAYLAND_DISPLAY=str(sock))
        self.assertNotIn("--restart", self.one(calls, "run")["args"])

    def test_clipboard_sources_are_mounts_that_fail_when_missing(self):
        # `-v` would CREATE a missing source as a root-owned folder — in the
        # runtime dir, exactly where the compositor's socket has to go.
        sock = self.socket_at(self.dir / "rt" / "wayland-0")
        _, calls = self.run_sh("--clipboard", WAYLAND_DISPLAY=str(sock))
        args = self.one(calls, "run")["args"]
        self.assertFalse([v for v in value_of(args, "--volume") if "wayland" in v])

    def test_x11_hands_over_the_socket_folder_and_a_wildcard_cookie(self):
        x11 = self.dir / "X11-unix"
        self.socket_at(x11 / "X7")
        cookie = "0100 0004 686f7374 0001 37 0012 4d49542d4d414749432d434f4f4b49452d31 0010 00112233445566778899aabbccddeeff"
        result, calls = self.run_sh("--clipboard", DISPLAY=":7.0", ALTER_ZERO_X11_DIR=str(x11), STUB_XAUTH_NLIST=cookie)
        self.assertEqual(result.returncode, 0, result.stderr)
        state = self.home / ".local" / "state" / "alter-zero" / "docker" / "alter-zero-kali"
        args = self.one(calls, "run")["args"]
        self.assertEqual(value_of(args, "--mount"), [
            f"type=bind,src={x11},dst=/tmp/.X11-unix,ro",
            f"type=bind,src={state},dst=/run/alter-zero/x11,ro",
        ])
        self.assertEqual(value_of(args, "--env"), ["DISPLAY=:7", "XAUTHORITY=/run/alter-zero/x11/Xauthority"])
        # The server files its cookie under this machine's hostname, which the
        # container does not share: family 0100 becomes the wildcard, ffff.
        merged = [c for c in calls if c["name"] == "xauth" and "nmerge" in c["args"]]
        self.assertEqual(len(merged), 1)
        self.assertEqual(merged[0]["stdin"].strip(), "ffff" + cookie[4:])
        self.assertEqual((state / "Xauthority").stat().st_mode & 0o777, 0o600)
        self.assertEqual(state.stat().st_mode & 0o777, 0o700)
        self.assertIn("X11 forwarded", result.stdout)

    def test_a_wayland_desktop_forwards_xwayland_too(self):
        # GNOME's compositor has no data-control protocol; its clipboard is
        # served over XWayland, and the app falls back to it on its own.
        sock = self.socket_at(self.dir / "rt" / "wayland-0")
        self.socket_at(self.dir / "X11-unix" / "X0")
        result, calls = self.run_sh("--clipboard", WAYLAND_DISPLAY=str(sock), DISPLAY=":0",
                                    ALTER_ZERO_X11_DIR=str(self.dir / "X11-unix"))
        self.assertEqual(len(value_of(self.one(calls, "run")["args"], "--mount")), 3)
        self.assertIn("Wayland and X11 forwarded", result.stdout)

    def test_a_remote_display_is_not_a_local_socket(self):
        # SSH X forwarding (localhost:10.0) is TCP on the host's loopback.
        result, calls = self.run_sh("--clipboard", DISPLAY="localhost:10.0")
        self.assertRefused(result, calls, saying="found no desktop session")

    def test_no_desktop_session_is_explained_and_nothing_is_created(self):
        result, calls = self.run_sh("--clipboard")
        self.assertRefused(result, calls, saying="WAYLAND_DISPLAY or a local DISPLAY")
        self.assertIn("read it", result.stderr, "and what works over SSH instead")


class RunWithoutXauth(ScriptCase):
    stubs = ("docker",)

    def test_x11_without_xauth_still_forwards_and_says_what_is_missing(self):
        x11 = self.dir / "X11-unix"
        x11.mkdir()
        listener = socket.socket(socket.AF_UNIX)
        listener.bind(str(x11 / "X0"))
        self.addCleanup(listener.close)
        result, calls = self.script(RUN, "--clipboard", DISPLAY=":0", ALTER_ZERO_X11_DIR=str(x11))
        self.assertEqual(result.returncode, 0, result.stderr)
        args = self.one(calls, "run")["args"]
        self.assertEqual(value_of(args, "--env"), ["DISPLAY=:0"])
        self.assertIn("xauth is not installed", result.stdout)


class RunUnderSelinux(ScriptCase):
    stubs = ("docker", "selinuxenabled")

    def test_a_mounted_folder_is_labelled_for_this_container(self):
        _, calls = self.script(RUN, "site")
        self.assertIn(f"{self.dir / 'site'}:/workspace:Z", value_of(self.one(calls, "run")["args"], "--volume"))

    def test_a_named_volume_needs_no_label(self):
        _, calls = self.script(RUN)
        self.assertIn("alter-zero-workspace:/workspace", value_of(self.one(calls, "run")["args"], "--volume"))

    def test_the_home_directory_is_never_relabelled(self):
        result, calls = self.script(RUN, str(self.home))
        self.assertRefused(result, calls, saying="refusing to relabel your home directory")


if __name__ == "__main__":
    unittest.main()
