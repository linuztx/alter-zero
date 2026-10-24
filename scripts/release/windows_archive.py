#!/usr/bin/env python3
"""Deterministic Windows ZIPs, allowlisted extraction, numeric PE validation.

Called by lib.sh; no compiler, external unzip, or locale-dependent file output.
Unix packaging deliberately stays on its existing tar/gzip byte path.
"""
import os
from pathlib import Path
import shutil
import stat
import struct
import sys
import time
import zipfile


def pack(directory, stem, output):
    root = Path(directory) / stem
    # ZIP has no timezone and only two-second resolution. UTC and clamping
    # avoid local timezone differences, and pre-1980 fixtures remain usable.
    stamp = max(315532800, min(int(root.stat().st_mtime), 4354819198))
    date = time.gmtime(stamp)[:6]
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for name in [f"{stem}/", *[f"{stem}/{p.name}" for p in sorted(root.iterdir())]]:
            path = Path(directory) / name
            is_dir = name.endswith("/")
            info = zipfile.ZipInfo(name, date)
            info.create_system = 3
            mode = (stat.S_IFDIR | 0o755) if is_dir else (stat.S_IFREG | (0o755 if path.suffix == ".exe" else 0o644))
            info.external_attr = (mode << 16) | (0x10 if is_dir else 0)
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, b"" if is_dir else path.read_bytes(), compresslevel=9)


def extract(archive_path, stem, binary, destination):
    expected = {f"{stem}/", *[f"{stem}/{name}" for name in (binary, "LICENSE", "README.md", "CHANGELOG.md")]}
    with zipfile.ZipFile(archive_path) as archive:
        entries = archive.infolist()
        names = [entry.filename for entry in entries]
        if len(names) != len(expected) or set(names) != expected:
            raise ValueError("unexpected layout: ZIP must contain only the stem directory and four release files")
        for entry in entries:
            name = entry.filename
            # Exact names above also disallow traversal, drive/UNC paths,
            # alternate streams, backslashes and case-insensitive aliases.
            if entry.orig_filename != name:
                raise ValueError("unexpected layout: ZIP entry contains a NUL or normalized path")
            kind = stat.S_IFMT(entry.external_attr >> 16)
            if kind not in (0, stat.S_IFDIR if entry.is_dir() else stat.S_IFREG):
                raise ValueError(f"unexpected layout: ZIP link or special file: {name}")
            if entry.flag_bits & 1 or entry.file_size > 512 * 1024 * 1024:
                raise ValueError(f"unexpected layout: encrypted or oversized ZIP entry: {name}")
        # Validate every name and type before creating anything. Never use
        # extractall: write only the already allowlisted files as regular files.
        root = Path(destination) / stem
        root.mkdir(parents=True, exist_ok=False)
        for entry in entries:
            if entry.is_dir():
                continue
            path = root / entry.filename.split("/")[1]
            with archive.open(entry) as source, path.open("xb") as target:
                shutil.copyfileobj(source, target)
            path.chmod(0o755 if path.name == binary else 0o644)


def validate_pe(path):
    with open(path, "rb") as image:
        size = os.fstat(image.fileno()).st_size

        def read(offset, length):
            if offset < 0 or offset + length > size:
                raise ValueError("PE header or section is truncated/out of bounds")
            image.seek(offset)
            data = image.read(length)
            if len(data) != length:
                raise ValueError("PE image was truncated while reading")
            return data

        dos = read(0, 64)
        if dos[:2] != b"MZ":
            raise ValueError("PE image lacks the MZ DOS signature")
        offset = struct.unpack_from("<I", dos, 0x3C)[0]
        if offset < 64:
            raise ValueError("PE header offset overlaps the DOS header")
        coff = read(offset, 24)
        if coff[:4] != b"PE\0\0":
            raise ValueError("PE signature is not PE\\0\\0")
        machine, sections, _, _, _, optional_size, flags = struct.unpack_from("<HHIIIHH", coff, 4)
        if machine != 0x8664:
            raise ValueError(f"PE machine must be x64 (0x8664), got 0x{machine:04x}")
        if not flags & 0x0002 or flags & 0x2000:
            raise ValueError("PE must be an executable image, not a DLL")
        if not 1 <= sections <= 96 or not 112 <= optional_size <= 4096:
            raise ValueError("PE section count or optional header size is invalid")
        optional = read(offset + 24, optional_size)
        if struct.unpack_from("<H", optional)[0] != 0x20B:
            raise ValueError("PE optional header must be PE32+ (0x020b)")
        if struct.unpack_from("<H", optional, 68)[0] != 3:
            raise ValueError("PE subsystem must be Windows console (3)")
        directories = struct.unpack_from("<I", optional, 108)[0]
        if directories > (optional_size - 112) // 8:
            raise ValueError("PE data-directory table is truncated")
        table_offset = offset + 24 + optional_size
        table = read(table_offset, sections * 40)
        headers_size = struct.unpack_from("<I", optional, 60)[0]
        if not table_offset + len(table) <= headers_size <= size:
            raise ValueError("PE SizeOfHeaders does not cover the section table")
        for section in range(sections):
            raw_size, raw_offset = struct.unpack_from("<II", table, section * 40 + 16)
            if raw_size and (raw_offset < headers_size or raw_offset + raw_size > size):
                raise ValueError("PE raw section is truncated/out of bounds")


def main():
    try:
        mode, *args = sys.argv[1:]
        if mode == "pack" and len(args) == 3:
            pack(*args)
        elif mode == "extract" and len(args) == 4:
            extract(*args)
        elif mode == "pe" and len(args) == 1:
            validate_pe(*args)
        else:
            raise ValueError("usage: windows_archive.py pack DIR STEM OUTPUT | extract ZIP STEM BINARY DEST | pe EXE")
    except (ValueError, OSError, RuntimeError, zipfile.BadZipFile, NotImplementedError) as error:
        print(f"Windows archive/PE: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
