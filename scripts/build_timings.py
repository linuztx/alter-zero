#!/usr/bin/env python3
"""Rank the compile units of a `cargo build --timings` run (docs/build-time.md).

    cargo build --release --timings
    scripts/build_timings.py                # the newest target/cargo-timings/*.html
    scripts/build_timings.py report.html 30 # a given report, the top 30 units

Cargo's HTML report embeds every unit's start and duration as JSON; this
prints them ranked by duration, so the crates worth trimming are visible at a
glance, then the serial sum (roughly CPU seconds) against the wall time so the
parallelism the machine reached is a number too. A `[build]` suffix marks a
build script's own run (a C compile, mostly) rather than the crate's rustc.
"""

import glob
import json
import os
import re
import sys


def main() -> int:
    args = [a for a in sys.argv[1:] if not a.isdigit()]
    top = next((int(a) for a in sys.argv[1:] if a.isdigit()), 25)
    if args:
        path = args[0]
    else:
        reports = sorted(
            glob.glob("target/cargo-timings/cargo-timing-*.html"),
            key=os.path.getmtime,
        )
        if not reports:
            print("no target/cargo-timings/*.html — run `cargo build --timings` first", file=sys.stderr)
            return 1
        path = reports[-1]
    html = open(path, encoding="utf-8").read()
    match = re.search(r"const UNIT_DATA = (\[.*?\]);", html, re.S)
    if match is None:
        print(f"{path}: no UNIT_DATA block — not a cargo timing report?", file=sys.stderr)
        return 1
    units = json.loads(match.group(1))
    if not units:
        print("nothing was compiled (everything was fresh)")
        return 0
    serial = sum(u["duration"] for u in units)
    wall = max(u["start"] + u["duration"] for u in units)
    units.sort(key=lambda u: -u["duration"])
    print(f"{'dur(s)':>7} {'start':>7}  unit")
    for u in units[:top]:
        mode = "" if u["mode"] == "todo" else " [build]"
        target = f" ({u['target']})" if u.get("target") else ""
        print(f"{u['duration']:7.1f} {u['start']:7.1f}  {u['name']} v{u['version']}{mode}{target}")
    print(
        f"\n{len(units)} units · serial sum {serial:.0f}s · wall {wall:.0f}s"
        f" · parallelism {serial / wall:.2f}x   ({path})"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
