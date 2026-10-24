"""Offline checks for the image's default packages, exposed ports and apt mirrors."""
import re
import subprocess
import tempfile
import unittest
from pathlib import Path


DOCKERFILE = Path(__file__).resolve().parents[1] / "Dockerfile"
MIRROR_LIST = "/etc/apt/kali-mirrors.txt"
KALI_SOURCES_PATH = "/etc/apt/sources.list.d/kali.sources"
# kali.sources as kalilinux/kali-rolling ships it (October 2026).
KALI_SOURCES = """\
Types: deb
URIs: http://http.kali.org/kali/
Suites: kali-rolling
Components: main contrib non-free non-free-firmware
Signed-By: /usr/share/keyrings/kali-archive-keyring.gpg
"""


def tools_steps():
    """Every `&&` step of the tools stage's RUN instructions, in build order."""
    source = DOCKERFILE.read_text().replace("\\\n", " ")
    stage = source.split(" AS tools\n", 1)[1].split("\nFROM ", 1)[0]
    steps = []
    for command in re.findall(r"^RUN\s+(.+)$", stage, re.M):
        steps.extend(step.strip() for step in command.split(" && "))
    return steps


def written(steps, path):
    """What the steps that redirect into PATH leave there, found by running
    each one with its output sent here instead."""
    text = ""
    for step in steps:
        target = re.search(r"\s(>>?)\s*(\S+)$", step)
        if not target or target.group(2) != path:
            continue
        printed = subprocess.run(["sh", "-c", step[:target.start()]],
                                 capture_output=True, text=True, check=True).stdout
        text = text + printed if target.group(1) == ">>" else printed
    return text


class ImageDefaults(unittest.TestCase):
    def test_nodejs_and_npm_are_default_packages(self):
        source = DOCKERFILE.read_text().replace("\\\n", " ")
        install = re.search(r"apt-get install -y --no-install-recommends\s+(.*?)\s*&&", source)
        self.assertIsNotNone(install)
        packages = install.group(1).split()
        for package in ("nodejs", "npm"):
            with self.subTest(package=package):
                self.assertIn(package, packages)

    def test_exposed_tcp_ports_include_existing_and_requested_ports(self):
        exposed = set()
        for ports in re.findall(r"^EXPOSE\s+(.+)$", DOCKERFILE.read_text(), re.M):
            exposed.update(port.removesuffix("/tcp") for port in ports.split())
        expected = {"22", "80", "3000", "5000", "8000", "8080", "8888"}
        expected.update(str(port) for port in range(9000, 9010))
        self.assertEqual(exposed, expected)


class AptMirrors(unittest.TestCase):
    """Kali's redirector first, and Kali's CDN for any file the mirror the
    redirector picked cannot deliver (docs/docker.md *Mirrors*)."""

    def test_the_list_tries_the_redirector_then_kalis_cdn(self):
        # apt reads a mirror's metadata only after a tab: with a space, the
        # priority silently becomes part of the URI.
        listed = [line.split("\t") for line in written(tools_steps(), MIRROR_LIST).splitlines()
                  if line and not line.startswith("#")]
        self.assertEqual(listed, [["http://http.kali.org/kali/", "priority:1"],
                                  ["http://kali.download/kali/", "priority:2"]])

    def test_kali_sources_names_the_list_and_keeps_the_rest(self):
        rewrite = [step for step in tools_steps() if KALI_SOURCES_PATH in step]
        self.assertEqual(len(rewrite), 1, rewrite)
        script = re.search(r"sed -i '([^']+)'", rewrite[0])
        self.assertIsNotNone(script, rewrite[0])
        # Applied with `sed -e` to a copy: `sed -i` differs between GNU and BSD.
        result = subprocess.run(["sed", "-e", script.group(1)], input=KALI_SOURCES,
                                capture_output=True, text=True, check=True).stdout
        self.assertEqual(result, KALI_SOURCES.replace(
            "URIs: http://http.kali.org/kali/", f"URIs: mirror+file:{MIRROR_LIST}"))

    def test_the_build_installs_through_the_list(self):
        # The final image would name the list either way; only the order
        # decides whether the build's own install can fall back.
        steps = tools_steps()
        update = next(i for i, step in enumerate(steps) if "apt-get update" in step)
        for needle in (MIRROR_LIST, KALI_SOURCES_PATH):
            with self.subTest(needle=needle):
                first = next((i for i, step in enumerate(steps) if needle in step), None)
                self.assertIsNotNone(first, f"nothing in the tools stage mentions {needle}")
                self.assertLess(first, update, steps[first])

    def test_a_base_without_kali_sources_still_builds(self):
        rewrite = next((step for step in tools_steps() if KALI_SOURCES_PATH in step), None)
        self.assertIsNotNone(rewrite, f"nothing in the tools stage rewrites {KALI_SOURCES_PATH}")
        with tempfile.TemporaryDirectory() as scratch:
            missing = f"{scratch}/kali.sources"
            result = subprocess.run(["sh", "-c", rewrite.replace(KALI_SOURCES_PATH, missing)],
                                    capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
