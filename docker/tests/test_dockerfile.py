"""Offline checks for the image's default packages and exposed ports."""
import re
import unittest
from pathlib import Path


DOCKERFILE = Path(__file__).resolve().parents[1] / "Dockerfile"


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
