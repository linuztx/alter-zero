"""Offline checks for the image's default packages, exposed ports and apt sources."""
import re
import subprocess
import unittest
from pathlib import Path


DOCKERFILE = Path(__file__).resolve().parents[1] / "Dockerfile"
MIRROR_LIST = "/etc/apt/alter-zero-mirrors.txt"
# /etc/apt/sources.list.d/kali.sources as kalilinux/kali-rolling ships it.
KALI_SOURCES = (
    "Types: deb\n"
    "URIs: http://http.kali.org/kali/\n"
    "Suites: kali-rolling\n"
    "Components: main contrib non-free non-free-firmware\n"
    "Signed-By: /usr/share/keyrings/kali-archive-keyring.gpg\n"
)


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
    """http.kali.org redirects each file to a mirror of its choosing, and apt
    retries the mirror it was sent to, never the redirector: one dead mirror
    failed every build that drew it. So Kali's archive is read through apt's
    mirror method (docs/docker.md *A dead mirror costs a retry, not the build*)."""

    def before_first_update(self):
        source = DOCKERFILE.read_text().replace("\\\n", " ")
        return source[:source.index("apt-get update")]

    def test_kalis_cdn_backs_the_redirector(self):
        entries = re.search(
            r"(printf '%s\\tpriority:%s\\n'(?:\s+\S+\s+\d+)+)\s*>>?\s*" + re.escape(MIRROR_LIST),
            self.before_first_update(),
        )
        self.assertIsNotNone(entries, f"no {MIRROR_LIST} is written before the first apt-get update")
        # What the build writes, byte for byte: apt splits a mirror from its
        # metadata at a tab, and a space would make `priority:1` part of the URI.
        written = subprocess.run(["sh", "-c", entries.group(1)], capture_output=True, text=True, check=True)
        self.assertEqual(
            [line.split("\t") for line in written.stdout.splitlines()],
            [["http://http.kali.org/kali/", "priority:1"], ["http://kali.download/kali/", "priority:2"]],
        )

    def test_only_the_uris_line_of_kalis_own_sources_changes(self):
        rewrite = re.search(
            r"sed -i '([^']+)'\s+/etc/apt/sources\.list\.d/kali\.sources", self.before_first_update()
        )
        self.assertIsNotNone(rewrite, "kali.sources is not rewritten before the first apt-get update")
        rewritten = subprocess.run(
            ["sed", "-e", rewrite.group(1)], input=KALI_SOURCES, capture_output=True, text=True, check=True
        )
        self.assertEqual(
            rewritten.stdout,
            KALI_SOURCES.replace("URIs: http://http.kali.org/kali/", f"URIs: mirror+file:{MIRROR_LIST}"),
        )
