#!/usr/bin/env bash
# scripts/release.sh — cut, package, verify and publish a release
# (docs/release.md). One entry point over the steps under scripts/release/,
# the smoke suite's runner-over-phases shape: each step is its own file that
# sources scripts/release/lib.sh, and .github/workflows/release.yml is a
# thin caller of these same commands, so a release can be rehearsed — and
# every piece of it verified — on a laptop with no tag pushed.
#
#   scripts/release.sh version                   # the version Cargo.toml names
#   scripts/release.sh check [TAG]               # Cargo.toml, Cargo.lock, the README badge and CHANGELOG.md agree (and with TAG)
#   scripts/release.sh build [TARGET]            # a packaged binary + checksum into dist/ (default: the host target)
#   scripts/release.sh verify [DIST]             # every asset in dist/: checksums, layout, CPU, `--version`
#   scripts/release.sh notes VERSION [DIST]      # the release notes, from CHANGELOG.md + the assets
#   scripts/release.sh publish VERSION [DIST]    # the GitHub release (gh), or --dry-run to see the commands
#   scripts/release.sh prepare VERSION           # bump the version everywhere, roll the changelog, ready to tag
#   scripts/release.sh selftest                  # the tooling's own fixture-driven tests
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

usage() {
	sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d' | sed 's/^# \{0,1\}//'
}

cmd="${1:-help}"
[ $# -gt 0 ] && shift
case "$cmd" in
version)
	# shellcheck source-path=SCRIPTDIR
	# shellcheck source=release/lib.sh
	. "$HERE/release/lib.sh"
	manifest_version
	;;
check | build | verify | notes | publish | prepare | selftest)
	exec bash "$HERE/release/$cmd.sh" "$@"
	;;
-h | --help | help)
	usage
	;;
*)
	usage >&2
	echo >&2
	echo "release.sh: unknown command '$cmd'" >&2
	exit 2
	;;
esac
