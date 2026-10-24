"""Offline release fixtures: python3 -m unittest discover -s scripts/release -p 'test_*.py'."""
import hashlib
import io
import os
from pathlib import Path
import struct
import subprocess
import tarfile
import tempfile
import unittest
import urllib.error
import urllib.request
import warnings
import zipfile


ROOT = Path(__file__).resolve().parents[2]
LIB = ROOT / "scripts/release/lib.sh"
VERIFY = ROOT / "scripts/release/verify.sh"
TARGET = "x86_64-pc-windows-msvc"
VERSION = "0.15.0"
STEM = f"alter-zero-v{VERSION}-{TARGET}"


def pe_image():
    """Small numeric PE32+ fixture, not a runnable Windows program."""
    image = bytearray(1024)
    image[:2] = b"MZ"
    struct.pack_into("<I", image, 0x3C, 0x80)
    image[0x80:0x84] = b"PE\0\0"
    struct.pack_into("<HHIIIHH", image, 0x84, 0x8664, 1, 0, 0, 0, 240, 0x22)
    struct.pack_into("<H", image, 0x98, 0x20B)
    struct.pack_into("<I", image, 0x98 + 60, 512)  # SizeOfHeaders
    struct.pack_into("<H", image, 0x98 + 68, 3)  # IMAGE_SUBSYSTEM_WINDOWS_CUI
    image[0x188:0x190] = b".text\0\0\0"
    struct.pack_into("<II", image, 0x188 + 16, 512, 512)
    return image


class WindowsReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="release-windows-")
        self.addCleanup(self.temp.cleanup)
        self.work = Path(self.temp.name)
        self.root = self.work / "root"
        self.root.mkdir()
        (self.root / "Cargo.toml").write_text(
            '[package]\nname = "alter-zero"\nversion = "0.15.0"\n', encoding="utf-8"
        )
        for name in ("LICENSE", "README.md", "CHANGELOG.md"):
            (self.root / name).write_text(f"fixture {name}\n", encoding="utf-8")
        self.env = {**os.environ, "RELEASE_ROOT": str(self.root)}

    def bash(self, script, *args):
        return subprocess.run(
            ["bash", "-c", f'. "$1"; shift; {script}', "_", str(LIB), *map(str, args)],
            env=self.env, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            check=False,
        )

    def verify(self, dist):
        return subprocess.run(
            ["bash", str(VERIFY), str(dist), "--version", VERSION],
            env=self.env, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            check=False,
        )

    def zip_dist(self, image=None):
        dist = self.work / "dist"
        dist.mkdir(exist_ok=True)
        archive = dist / f"{STEM}.zip"
        with zipfile.ZipFile(archive, "w") as out:
            out.writestr(f"{STEM}/", b"")
            out.writestr(f"{STEM}/alter-zero.exe", pe_image() if image is None else image)
            for name in ("LICENSE", "README.md", "CHANGELOG.md"):
                out.writestr(f"{STEM}/{name}", (self.root / name).read_bytes())
        self.checksum(archive)
        return dist

    @staticmethod
    def checksum(archive):
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        Path(f"{archive}.sha256").write_text(f"{digest}  {archive.name}\n", encoding="ascii")

    def test_windows_target_maps_archive_binary_and_files(self):
        result = self.bash('asset_archive 0.15.0 "$1"; asset_files "$1"', TARGET)
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertEqual(result.stdout.splitlines(), [
            f"{STEM}.zip", "alter-zero.exe", "LICENSE", "README.md", "CHANGELOG.md",
        ])

    def test_valid_windows_zip_verifies_on_linux_without_running_it(self):
        result = self.verify(self.zip_dist())
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("verified 1 asset(s)", result.stdout)

    def test_packaging_zip_is_reproducible_with_exact_layout(self):
        binary = self.work / "fixture.exe"
        binary.write_bytes(pe_image())
        os.utime(binary, (1700000001, 1700000001))
        a = self.bash('package_dist "$1" 0.15.0 "$2" "$3"', binary, TARGET, self.work / "a")
        b = self.bash('package_dist "$1" 0.15.0 "$2" "$3"', binary, TARGET, self.work / "b")
        self.assertEqual(a.returncode, 0, a.stdout)
        self.assertEqual(b.returncode, 0, b.stdout)
        archive = self.work / "a" / f"{STEM}.zip"
        self.assertTrue(archive.exists(), a.stdout)
        self.assertEqual(archive.read_bytes(), (self.work / "b" / archive.name).read_bytes())
        with zipfile.ZipFile(archive) as out:
            self.assertEqual(sorted(out.namelist()), sorted([
                f"{STEM}/", f"{STEM}/alter-zero.exe", f"{STEM}/LICENSE",
                f"{STEM}/README.md", f"{STEM}/CHANGELOG.md",
            ]))

    def test_four_unix_tarballs_keep_the_original_packaging_bytes(self):
        binary = self.work / "unix-bin"
        binary.write_bytes(b"unchanged Unix binary fixture\n")
        os.utime(binary, (1700000001, 1700000001))
        for target in ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu",
                       "x86_64-apple-darwin", "aarch64-apple-darwin"):
            with self.subTest(target=target):
                stage = self.work / "reference" / f"alter-zero-v{VERSION}-{target}"
                stage.mkdir(parents=True)
                for name in ("alter-zero", "LICENSE", "README.md", "CHANGELOG.md"):
                    contents = binary.read_bytes() if name == "alter-zero" else (self.root / name).read_bytes()
                    path = stage / name
                    path.write_bytes(contents)
                    path.chmod(0o755 if name == "alter-zero" else (self.root / name).stat().st_mode & 0o777)
                    os.utime(path, (1700000001, 1700000001))
                os.utime(stage, (1700000001, 1700000001))
                original = self.work / f"reference-{target}.tar.gz"
                result = self.bash('tar_reproducible "$1" "$2" "$3"', stage.parent, stage.name, original)
                self.assertEqual(result.returncode, 0, result.stdout)
                result = self.bash('package_dist "$1" 0.15.0 "$2" "$3"', binary, target, self.work / "unix")
                self.assertEqual(result.returncode, 0, result.stdout)
                self.assertEqual(original.read_bytes(), (self.work / "unix" / f"{stage.name}.tar.gz").read_bytes())

    def test_pe_validation_is_numeric_not_file_output(self):
        mutations = {
            "not_mz": (0, b"ZZ"), "not_pe": (0x80, b"PX\0\0"),
            "x86": (0x84, struct.pack("<H", 0x14C)),
            "arm64": (0x84, struct.pack("<H", 0xAA64)),
            "pe32": (0x98, struct.pack("<H", 0x10B)),
            "dll": (0x96, struct.pack("<H", 0x2022)),
            "not_executable": (0x96, struct.pack("<H", 0x20)),
            "bad_pe_offset": (0x3C, struct.pack("<I", 0xFFFFFFF0)),
            "optional_header_short": (0x94, struct.pack("<H", 2)),
            "gui_subsystem": (0x98 + 68, struct.pack("<H", 2)),
            "unknown_subsystem": (0x98 + 68, struct.pack("<H", 0)),
        }
        for name, (offset, data) in mutations.items():
            with self.subTest(name=name):
                image = pe_image()
                image[offset:offset + len(data)] = data
                result = self.verify(self.zip_dist(image))
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertIn("PE", result.stdout)
        result = self.verify(self.zip_dist(pe_image()[:150]))
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("PE", result.stdout)

    def test_zip_rejects_unsafe_duplicate_and_link_entries_before_extraction(self):
        for name in ("../escaped", "/absolute", "C:/drive", "C:\\drive", "..\\escaped",
                     f"{STEM}/../escaped", f"{STEM}/README.md:stream", f"{STEM}/README.md"):
            with self.subTest(name=name):
                dist = self.zip_dist()
                archive = dist / f"{STEM}.zip"
                with warnings.catch_warnings(), zipfile.ZipFile(archive, "a") as out:
                    warnings.simplefilter("ignore", UserWarning)  # deliberately duplicated malicious member
                    out.writestr(name, "evil")
                self.checksum(archive)
                result = self.verify(dist)
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertIn("layout", result.stdout)
        dist = self.zip_dist()
        archive = dist / f"{STEM}.zip"
        with zipfile.ZipFile(archive, "a") as out:
            link = zipfile.ZipInfo(f"{STEM}/link")
            link.create_system = 3
            link.external_attr = 0o120777 << 16
            out.writestr(link, "../../escaped")
        self.checksum(archive)
        self.assertNotEqual(self.verify(dist).returncode, 0)
        self.assertFalse((self.work / "escaped").exists())

    def test_zip_rejects_nul_names_not_normalized_into_the_allowlist(self):
        dist = self.work / "dist"
        dist.mkdir()
        archive = dist / f"{STEM}.zip"
        with zipfile.ZipFile(archive, "w") as out:
            out.writestr(f"{STEM}/", b"")
            out.writestr(f"{STEM}/alter-zero.exe", pe_image())
            for name in ("LICENSE", "README.md~x", "CHANGELOG.md"):
                out.writestr(f"{STEM}/{name}", b"fixture")
        archive.write_bytes(archive.read_bytes().replace(b"README.md~x", b"README.md\0x"))
        self.checksum(archive)
        result = self.verify(dist)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("layout", result.stdout)

    def test_download_rejects_windows_paths_before_creating_a_directory(self):
        tools = self.work / "tools"
        tools.mkdir()
        curl = tools / "curl"
        curl.write_text('#!/bin/sh\nprintf "%s  %s\\n" "' + ('0' * 64) + '" "$LISTING_NAME"\n', encoding="ascii")
        curl.chmod(0o755)
        for name in (r"..\escape.zip", r"C:\escape.zip", "C:relative.zip", "../escape.zip"):
            with self.subTest(name=name):
                dest = self.work / "download"
                result = subprocess.run(["bash", str(ROOT / "scripts/release/download.sh"), VERSION, str(dest),
                                         "--from", "http://127.0.0.1:9"],
                                        env={**self.env, "PATH": f"{tools}{os.pathsep}{os.environ['PATH']}", "LISTING_NAME": name},
                                        text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=False)
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertFalse(dest.exists(), result.stdout)

    def test_posix_installer_redirects_native_windows_to_powershell(self):
        tools = self.work / "tools"
        tools.mkdir()
        uname = tools / "uname"
        uname.write_text('#!/bin/sh\ncase "$1" in -s) echo MINGW64_NT-10.0 ;; -m) echo x86_64 ;; esac\n', encoding="ascii")
        uname.chmod(0o755)
        result = subprocess.run(["sh", str(ROOT / "install.sh")],
                                env={**self.env, "PATH": f"{tools}{os.pathsep}{os.environ['PATH']}"},
                                text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("install.ps1", result.stdout)
        self.assertNotIn("Windows is not supported", result.stdout)

    def test_powershell_installer_exposes_existing_install_environment_contract(self):
        installer = ROOT / "install.ps1"
        self.assertTrue(installer.is_file(), "native installer entry point is missing")
        script = installer.read_text(encoding="utf-8")
        for contract in ("ALTER_ZERO_VERSION", "ALTER_ZERO_INSTALL_DIR", "ALTER_ZERO_INSTALL_BASE_URL",
                         "LOCALAPPDATA", "SHA256SUMS", "Get-FileHash", "FileShare", "PE32+"):
            self.assertIn(contract, script)
        self.assertNotIn("Set-ExecutionPolicy", script)

    def test_release_server_serves_powershell_installer(self):
        installer = self.work / "install.ps1"
        installer.write_text("# offline PowerShell installer\n", encoding="utf-8")
        dist = self.zip_dist()
        with subprocess.Popen(
            ["python3", str(ROOT / "scripts/release/release_server.py"), str(dist),
             f"v{VERSION}", str(ROOT / "install.sh"), str(installer)],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        ) as server:
            try:
                port = server.stdout.readline().strip()
                try:
                    with urllib.request.urlopen(f"http://127.0.0.1:{port}/install.ps1", timeout=5) as response:
                        self.assertEqual(response.read(), installer.read_bytes())
                except urllib.error.HTTPError as error:
                    self.fail(f"PowerShell installer was not served: HTTP {error.code}")
            finally:
                server.terminate()
                server.wait(timeout=5)

    def test_unknown_target_is_not_a_warning_and_success(self):
        dist = self.work / "dist"
        dist.mkdir()
        stem = f"alter-zero-v{VERSION}-riscv64gc-unknown-linux-gnu"
        archive = dist / f"{stem}.tar.gz"
        with tarfile.open(archive, "w:gz") as out:
            directory = tarfile.TarInfo(stem)
            directory.type = tarfile.DIRTYPE
            directory.mode = 0o755
            out.addfile(directory)
            for name in ("alter-zero", "LICENSE", "README.md", "CHANGELOG.md"):
                contents = b"#!/bin/sh\nexit 0\n"
                info = tarfile.TarInfo(f"{stem}/{name}")
                info.mode = 0o755
                info.size = len(contents)
                out.addfile(info, io.BytesIO(contents))
        self.checksum(archive)
        result = self.verify(dist)
        self.assertNotEqual(result.returncode, 0, result.stdout)


if __name__ == "__main__":
    unittest.main()
