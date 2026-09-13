#!/usr/bin/env bash
# scripts/release/lib.sh — the shared half of the release tooling.
#
# Every step under scripts/release/ sources this, and scripts/release.sh
# routes to the steps. Nothing here runs cargo, gh or the network: this file
# owns the RULES — the version grammar, the readers for every file a version
# lives in (Cargo.toml, Cargo.lock, the README's badge, CHANGELOG.md), the
# asset naming, the archive layout, the checksum vocabulary — as pure
# functions of their arguments and the tree under $RELEASE_ROOT, which is
# what lets scripts/release/selftest.sh drive each rule against a fixture
# tree instead of the checkout. See docs/release.md.
#
# Portability: GitHub's macOS runners may hand a script bash 3.2, so nothing
# here needs bash 4 (no `${var,,}`, no associative arrays, no mapfile), no
# `sed -i` (GNU and BSD disagree on its argument), no gawk-only awk.

[ -n "${RELEASE_LIB_LOADED:-}" ] && return 0
RELEASE_LIB_LOADED=1

set -euo pipefail

RELEASE_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# The tree the readers look at: the checkout by default, a fixture under the
# selftest.
RELEASE_ROOT="${RELEASE_ROOT:-$(cd "$RELEASE_LIB_DIR/../.." && pwd)}"

# ---------------------------------------------------------------------------
# Logging — all of it on stderr, so a step's stdout stays machine-readable
# (`version` prints one line, `notes` prints markdown).
# ---------------------------------------------------------------------------
if [ -t 2 ] && [ -z "${NO_COLOR:-}" ]; then
	_c_red=$'\033[31m' _c_green=$'\033[32m' _c_yellow=$'\033[33m' _c_dim=$'\033[2m' _c_off=$'\033[0m'
else
	_c_red='' _c_green='' _c_yellow='' _c_dim='' _c_off=''
fi
info() { printf '%s· %s%s\n' "$_c_dim" "$*" "$_c_off" >&2; }
ok() { printf '%s✔ %s%s\n' "$_c_green" "$*" "$_c_off" >&2; }
warn() { printf '%s! %s%s\n' "$_c_yellow" "$*" "$_c_off" >&2; }
die() {
	printf '%s✘ %s%s\n' "$_c_red" "$*" "$_c_off" >&2
	exit 1
}

# A check that should report EVERY mismatch before failing (so one run of
# `check` names all of them) records with `fail` and closes with `finish`.
FAILS=0
fail() {
	printf '%s✘ %s%s\n' "$_c_red" "$*" "$_c_off" >&2
	FAILS=$((FAILS + 1))
}
finish() {
	if [ "$FAILS" -gt 0 ]; then
		printf '%s%s check(s) failed%s\n' "$_c_red" "$FAILS" "$_c_off" >&2
		exit 1
	fi
	ok "${1:-all checks passed}"
}

# ---------------------------------------------------------------------------
# The version grammar: semver, and the `v`-prefixed tag that names it.
# ---------------------------------------------------------------------------
SEMVER_RE='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$'
is_semver() { [[ "$1" =~ $SEMVER_RE ]]; }
# A hyphenated version (`0.2.0-rc.1`) is a pre-release: published flagged as
# one and never marked the latest.
is_prerelease() { [[ "$1" == *-* ]]; }
tag_of() { printf 'v%s\n' "$1"; }
# `v1.2.3` → `1.2.3`; anything else is not a release tag.
version_of_tag() {
	local tag="$1"
	[[ "$tag" == v* ]] || return 1
	is_semver "${tag#v}" || return 1
	printf '%s\n' "${tag#v}"
}
# A regex-safe form of a version for the sed/awk patterns below.
version_re() { printf '%s\n' "$1" | sed 's/[][\.*^$/+]/\\&/g'; }

# ---------------------------------------------------------------------------
# Cargo.toml / Cargo.lock.
# ---------------------------------------------------------------------------
# The first `key = "value"` under [package] — never a dependency's own
# `version = "…"` further down the manifest.
manifest_field() {
	local key="$1"
	awk -v key="$key" '
		/^\[/ { in_pkg = ($0 == "[package]") }
		in_pkg && $0 ~ ("^" key "[ \t]*=") {
			s = $0
			sub(/^[^"]*"/, "", s)
			sub(/".*$/, "", s)
			print s
			exit
		}
	' "$RELEASE_ROOT/Cargo.toml"
}
manifest_version() { manifest_field version; }
manifest_name() { manifest_field name; }
# The `repository` URL without a trailing slash or `.git`.
manifest_repository() {
	local url
	url="$(manifest_field repository)"
	url="${url%/}"
	url="${url%.git}"
	printf '%s\n' "$url"
}
# `owner/repo`, for `gh -R`.
repo_slug() {
	local url
	url="$(manifest_repository)"
	printf '%s\n' "${url#https://github.com/}"
}

# The version Cargo.lock records for one package (the crate's own entry is
# what a bump must keep in step, or `--locked` refuses the build).
lock_version() {
	local name="$1"
	awk -v name="$name" '
		/^\[\[package\]\]/ { blk = 1; n = ""; next }
		blk && /^name = / { n = $0; sub(/^name = "/, "", n); sub(/"$/, "", n) }
		blk && /^version = / && n == name {
			v = $0
			sub(/^version = "/, "", v)
			sub(/"$/, "", v)
			print v
			exit
		}
	' "$RELEASE_ROOT/Cargo.lock"
}

# ---------------------------------------------------------------------------
# The README's version badge. shields.io escapes a `-` in the label as `--`,
# so a pre-release `0.2.0-rc.1` rides the badge URL as `0.2.0--rc.1`; the
# alt text says it plainly. Both are read, and both must agree.
# ---------------------------------------------------------------------------
badge_escape() { printf '%s\n' "${1//-/--}"; }
readme_badge_version() {
	sed -n '/img\.shields\.io\/badge\/Version-/{s/.*img\.shields\.io\/badge\/Version-\(.*\)-[0-9A-Fa-f]\{6\}?style=.*/\1/p;q;}' "$RELEASE_ROOT/README.md" | sed 's/--/-/g'
}
readme_alt_version() {
	sed -n '/alt="Version: /{s/.*alt="Version: \([^"]*\)".*/\1/p;q;}' "$RELEASE_ROOT/README.md"
}

# ---------------------------------------------------------------------------
# CHANGELOG.md — Keep a Changelog: a `## [Unreleased]` section on top, one
# `## [X.Y.Z] - YYYY-MM-DD` section per release, and a block of
# `[X.Y.Z]: url` link references at the bottom.
# ---------------------------------------------------------------------------
changelog_path() { printf '%s/CHANGELOG.md\n' "$RELEASE_ROOT"; }

# The released versions, newest first, in the order the headings appear.
changelog_versions() {
	sed -n 's/^## \[\([0-9][^]]*\)\] - [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]$/\1/p' "$(changelog_path)"
}
# The date on a version's heading — empty when the heading is missing or
# malformed (a heading without a date is not a release).
changelog_date() {
	local re
	re="$(version_re "$1")"
	sed -n "s/^## \[$re\] - \([0-9]\{4\}-[0-9]\{2\}-[0-9]\{2\}\)$/\1/p" "$(changelog_path)"
}
# The version released before this one: the heading after its own.
changelog_previous() {
	changelog_versions | awk -v v="$1" 'found { print; exit } $0 == v { found = 1 }'
}
# A section's body — everything between its heading and the next heading
# (or the link block), blank lines trimmed off both ends. `Unreleased` works
# too.
changelog_section() {
	awk -v version="$1" '
		/^## \[/ {
			if (in_sec) exit
			hdr = $0
			sub(/^## \[/, "", hdr)
			sub(/\].*$/, "", hdr)
			if (hdr == version) { in_sec = 1; next }
		}
		in_sec && /^\[[^]]+\]: / { exit }
		in_sec { lines[n++] = $0 }
		END {
			s = 0; e = n - 1
			while (s <= e && lines[s] ~ /^[ \t]*$/) s++
			while (e >= s && lines[e] ~ /^[ \t]*$/) e--
			for (i = s; i <= e; i++) print lines[i]
		}
	' "$(changelog_path)"
}
# The URL a version's `[X.Y.Z]: url` reference names.
changelog_link() {
	local re
	re="$(version_re "$1")"
	sed -n "s/^\[$re\]: \(.*\)$/\1/p" "$(changelog_path)"
}

# ---------------------------------------------------------------------------
# Assets. One archive per target — `alter-zero-v0.1.0-x86_64-unknown-linux-gnu.tar.gz`
# holding a directory of the same stem with the binary, LICENSE, README.md
# and CHANGELOG.md — beside its `.sha256` in `sha256sum -c` form.
# ---------------------------------------------------------------------------
asset_stem() { printf '%s-v%s-%s\n' "$(manifest_name)" "$1" "$2"; }
asset_archive() { printf '%s.tar.gz\n' "$(asset_stem "$1" "$2")"; }
# The target an asset name carries, given the version it must name; fails on
# a foreign name so a stray file can never pass as an asset.
asset_target() {
	local name="$1" version="$2" prefix
	prefix="$(manifest_name)-v$version-"
	name="${name%.tar.gz}"
	[[ "$name" == "$prefix"* ]] || return 1
	[ -n "${name#"$prefix"}" ] || return 1
	printf '%s\n' "${name#"$prefix"}"
}
# What the archive must contain, relative to its top-level directory.
ASSET_FILES="alter-zero LICENSE README.md CHANGELOG.md"
asset_files() {
	local f
	for f in $ASSET_FILES; do
		if [ "$f" = alter-zero ]; then manifest_name; else printf '%s\n' "$f"; fi
	done
}

# The row a target gets in the release notes' assets table.
platform_label() {
	case "$1" in
	x86_64-unknown-linux-gnu) printf 'Linux x86_64 (glibc)\n' ;;
	aarch64-unknown-linux-gnu) printf 'Linux arm64 (glibc)\n' ;;
	x86_64-unknown-linux-musl) printf 'Linux x86_64 (static, musl)\n' ;;
	aarch64-unknown-linux-musl) printf 'Linux arm64 (static, musl)\n' ;;
	x86_64-apple-darwin) printf 'macOS Intel\n' ;;
	aarch64-apple-darwin) printf 'macOS Apple silicon\n' ;;
	*) printf '%s\n' "$1" ;;
	esac
}

# What `file` must say about a binary built for a target — the object format
# and the CPU — so a cross build that quietly produced a host binary (a
# stale artifact, a mis-set linker) is caught before it ships. `|`-separated
# words, every one of which must appear (case-insensitively).
binary_format_words() {
	case "$1" in
	x86_64-unknown-linux-*) printf 'ELF 64-bit|x86-64\n' ;;
	aarch64-unknown-linux-*) printf 'ELF 64-bit|aarch64\n' ;;
	x86_64-apple-darwin) printf 'Mach-O 64-bit|x86_64\n' ;;
	aarch64-apple-darwin) printf 'Mach-O 64-bit|arm64\n' ;;
	*) return 1 ;;
	esac
}

# The triple of the machine running the script, from uname — no rustc
# needed, so `verify` works on a box that only downloaded the assets.
host_target() {
	local os arch
	os="$(uname -s)"
	arch="$(uname -m)"
	case "$arch" in
	x86_64 | amd64) arch=x86_64 ;;
	aarch64 | arm64) arch=aarch64 ;;
	esac
	case "$os" in
	Linux) printf '%s-unknown-linux-gnu\n' "$arch" ;;
	Darwin) printf '%s-apple-darwin\n' "$arch" ;;
	*) printf '%s-unknown-%s\n' "$arch" "$(printf '%s' "$os" | tr '[:upper:]' '[:lower:]')" ;;
	esac
}

# ---------------------------------------------------------------------------
# Checksums — `sha256sum` where it exists, `shasum -a 256` on macOS.
# ---------------------------------------------------------------------------
sha256_of() {
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | awk '{ print $1 }'
	else
		shasum -a 256 "$1" | awk '{ print $1 }'
	fi
}
# PATH → PATH.sha256 holding `<hex>  <basename>`, the line `sha256sum -c`
# (and `shasum -a 256 -c`) reads back.
write_sha256() {
	local path="$1"
	printf '%s  %s\n' "$(sha256_of "$path")" "$(basename "$path")" >"$path.sha256"
}
# True when PATH.sha256 names PATH by its basename and its hash matches.
check_sha256() {
	local path="$1" want name
	[ -f "$path.sha256" ] || return 1
	read -r want name <"$path.sha256" || return 1
	[ "$name" = "$(basename "$path")" ] || return 1
	[ "$want" = "$(sha256_of "$path")" ]
}

# ---------------------------------------------------------------------------
# Packaging.
# ---------------------------------------------------------------------------
# (dir, member, out.tar.gz): the member directory archived without the
# builder's uid/gid, in name order, gzip without a timestamp — with the
# entries' mtimes pinned by package_dist, the same bytes from the same
# inputs on GNU tar and on BSD tar (macOS, where COPYFILE_DISABLE keeps the
# `._*` AppleDouble entries out).
tar_reproducible() {
	local dir="$1" member="$2" out="$3"
	if tar --version 2>/dev/null | grep -q GNU; then
		COPYFILE_DISABLE=1 tar --owner=0 --group=0 --numeric-owner --sort=name -cf - -C "$dir" "$member" | gzip -n -9 >"$out"
	else
		COPYFILE_DISABLE=1 tar --uid 0 --gid 0 --uname '' --gname '' -cf - -C "$dir" "$member" | gzip -n -9 >"$out"
	fi
}

# Stage and archive one asset: DIST/{stem}.tar.gz holding {stem}/ with the
# binary (mode 755) beside LICENSE, README.md and CHANGELOG.md from
# $RELEASE_ROOT, then its .sha256. Prints the archive's path.
package_dist() {
	local bin="$1" version="$2" target="$3" dist="$4"
	local stem stage archive f
	[ -f "$bin" ] || die "no binary at $bin"
	stem="$(asset_stem "$version" "$target")"
	mkdir -p "$dist"
	stage="$(mktemp -d)"
	mkdir -p "$stage/$stem"
	cp "$bin" "$stage/$stem/$(manifest_name)"
	chmod 755 "$stage/$stem/$(manifest_name)"
	for f in LICENSE README.md CHANGELOG.md; do
		[ -f "$RELEASE_ROOT/$f" ] || die "missing $RELEASE_ROOT/$f — every asset ships it"
		cp "$RELEASE_ROOT/$f" "$stage/$stem/"
	done
	# Every entry takes the binary's own mtime (its build time). `cp` stamps
	# each copy with *now*, so without this two packagings of the same binary
	# a second apart differ by nothing but that — which is exactly what the
	# reproducibility check below caught once it straddled a second.
	touch -r "$bin" "$stage/$stem" "$stage/$stem"/*
	archive="$dist/$stem.tar.gz"
	rm -f "$archive" "$archive.sha256"
	tar_reproducible "$stage" "$stem" "$archive"
	rm -rf "$stage"
	write_sha256 "$archive"
	printf '%s\n' "$archive"
}
# DIST's per-asset checksums gathered into the one `sha256sum -c` file a
# release publishes beside its archives — which is the file `install.sh`
# reads, so anything standing in for a published release needs it and not
# just the per-asset ones `package_dist` writes. It lives here because it
# was three copies of one line in `publish`, the selftest's fixtures and
# the smoke suite's stand-in release, and the copy that did not exist is
# what broke the installer.
write_sha256sums() {
	local dist="$1"
	( shopt -s nullglob; cat "$dist"/*.tar.gz.sha256 ) | sort -k 2 >"$dist/SHA256SUMS"
	printf '%s\n' "$dist/SHA256SUMS"
}
