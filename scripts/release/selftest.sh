#!/usr/bin/env bash
# scripts/release/selftest.sh — the release tooling's own tests.
#
#   scripts/release.sh selftest
#
# Every rule in lib.sh and every step is driven against a FIXTURE tree under
# a temp dir (RELEASE_ROOT points the steps at it), never the checkout: the
# readers over a manifest whose dependencies carry their own `version =`
# lines, the changelog grammar over a file with two releases, `check` against
# a consistent tree and then each way of breaking it, `package_dist` +
# `verify` over a real (tiny, C) binary so the CPU and `--version` checks run
# for real, `notes` over the changelog, `prepare` rolling a fixture forward,
# `publish --dry-run`, `download` from a stand-in github.com (release_server.py),
# and the npm packages staged, published to a stand-in
# registry (npm_registry.py) and installed back with `npm install -g`. Needs
# only bash, awk, sed, tar, gzip, file and a C compiler (the binary cases are
# skipped without one), plus python3 for the stand-in servers and node + npm
# for the npm cases (each skipped without them). Runs in a few seconds; CI
# runs it on every push and the release workflow before it builds.
set -uo pipefail
# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"
set +e

STEPS="$RELEASE_LIB_DIR"
CHECKOUT="$(cd "$RELEASE_LIB_DIR/../.." && pwd)"

# The fixtures are tagless unless a case says otherwise, so the runner's own
# ref must not leak into them: `check` falls back to GITHUB_REF_TYPE /
# GITHUB_REF_NAME when given no tag, and the release workflow runs this ON a
# tag — so every fixture check inherited v{the release} and compared it
# against the fixture's own version (0.4.2, 0.5.0), failing four cases in
# exactly the run that matters most. The cases that DO want a tag set these
# per command, which overrides the unset.
unset GITHUB_REF_TYPE GITHUB_REF_NAME
T="$(mktemp -d)"
trap 'stop_release; stop_registry; rm -rf "$T"' EXIT

# serve_release DIST TAG — the stand-in github.com (release_server.py) over
# DIST, on an ephemeral port printed on stdout; stop_release takes it down.
# Callers read the port as $(serve_release …), a subshell, so the server's
# PID goes to a file: a variable set in here would die with the subshell and
# leave the server running past the selftest.
serve_release() {
	python3 "$STEPS/release_server.py" "$1" "$2" >"$T/port" 2>"$T/server.err" &
	printf '%s\n' "$!" >"$T/server.pid"
	local i=0
	while [ ! -s "$T/port" ] && [ "$i" -lt 100 ]; do
		sleep 0.05
		i=$((i + 1))
	done
	cat "$T/port"
}
stop_release() {
	if [ -s "$T/server.pid" ]; then
		kill "$(cat "$T/server.pid")" 2>/dev/null
		rm -f "$T/server.pid"
	fi
	rm -f "$T/port"
}
# serve_registry DIR — the stand-in registry.npmjs.org (npm_registry.py) over
# DIR, its port on stdout; stop_registry takes it down. The PID file is
# serve_release's, for the same reason.
serve_registry() {
	python3 "$STEPS/npm_registry.py" "$1" >"$T/registry-port" 2>"$T/registry.err" &
	printf '%s\n' "$!" >"$T/registry.pid"
	local i=0
	while [ ! -s "$T/registry-port" ] && [ "$i" -lt 100 ]; do
		sleep 0.05
		i=$((i + 1))
	done
	cat "$T/registry-port"
}
stop_registry() {
	if [ -s "$T/registry.pid" ]; then
		kill "$(cat "$T/registry.pid")" 2>/dev/null
		rm -f "$T/registry.pid"
	fi
	rm -f "$T/registry-port"
}
PASS=0
FAILED=0

pass() {
	PASS=$((PASS + 1))
	printf '  %sok%s  %s\n' "$_c_green" "$_c_off" "$*"
}
flunk() {
	FAILED=$((FAILED + 1))
	printf '  %sFAIL%s %s\n' "$_c_red" "$_c_off" "$*"
}
# expect_eq DESCRIPTION WANT GOT
expect_eq() {
	if [ "$2" = "$3" ]; then pass "$1"; else flunk "$1 — want '$2', got '$3'"; fi
}
# expect_ok DESCRIPTION CMD… — the command exits 0 (its output is kept in $OUT)
expect_ok() {
	local d="$1"
	shift
	if OUT="$("$@" 2>&1)"; then pass "$d"; else flunk "$d — exit $? — $OUT"; fi
}
# expect_fail DESCRIPTION CMD… — the command exits non-zero
expect_fail() {
	local d="$1"
	shift
	if OUT="$("$@" 2>&1)"; then flunk "$d — unexpectedly succeeded: $OUT"; else pass "$d"; fi
}
expect_contains() {
	case "$2" in
	*"$3"*) pass "$1" ;;
	*) flunk "$1 — '$3' not found in: $2" ;;
	esac
}
expect_lacks() {
	case "$2" in
	*"$3"*) flunk "$1 — '$3' unexpectedly found in: $2" ;;
	*) pass "$1" ;;
	esac
}
section() { printf '%s%s%s\n' "$_c_dim" "$*" "$_c_off"; }

# craft_header TARGET FILE — an executable whose first bytes file(1) reads as
# TARGET's (an ELF or Mach-O header and zeros): a binary `verify` accepts for
# a target this machine cannot build, since it runs only the host's.
craft_header() {
	case "$1" in
	x86_64-unknown-linux-gnu) printf '\177ELF\002\001\001\000\000\000\000\000\000\000\000\000\002\000\076\000\001\000\000\000' ;;
	aarch64-unknown-linux-gnu) printf '\177ELF\002\001\001\000\000\000\000\000\000\000\000\000\002\000\267\000\001\000\000\000' ;;
	x86_64-apple-darwin) printf '\317\372\355\376\007\000\000\001\003\000\000\000\002\000\000\000' ;;
	aarch64-apple-darwin) printf '\317\372\355\376\014\000\000\001\000\000\000\000\002\000\000\000' ;;
	esac >"$2"
	head -c 64 /dev/zero >>"$2"
	chmod 755 "$2"
}
# json FILE EXPR — EXPR evaluated by node over FILE's JSON, bound to `m`.
json() {
	node -p "const m = JSON.parse(require('fs').readFileSync(process.argv[1], 'utf8')); $2" "$1"
}
# packed DIR — the paths `npm pack` would put in DIR's tarball, sorted.
packed() {
	(cd "$1" && npm pack --dry-run --json 2>/dev/null) |
		node -e 'let s = ""; process.stdin.on("data", (d) => (s += d)).on("end", () => console.log(JSON.parse(s)[0].files.map((f) => f.path).sort().join(" ")))'
}

# A release-ready fixture: version 0.4.2 everywhere, two releases in the
# changelog, an empty [Unreleased].
fixture() {
	local root="$1"
	mkdir -p "$root"
	cat >"$root/Cargo.toml" <<'EOF'
[package]
name = "alter-zero"
version = "0.4.2"
edition = "2024"
repository = "https://github.com/example/alter-zero.git"

[dependencies]
serde = { version = "1", features = ["derive"] }
other = "9.9.9"

[dev-dependencies]
version = "8.8.8"
EOF
	cat >"$root/Cargo.lock" <<'EOF'
# This file is automatically @generated by Cargo.
version = 4

[[package]]
name = "aho-corasick"
version = "1.1.3"

[[package]]
name = "alter-zero"
version = "0.4.2"
dependencies = [
 "aho-corasick",
 "version",
]

[[package]]
name = "version"
version = "8.8.8"
EOF
	cat >"$root/README.md" <<'EOF'
# Fixture

<a href="Cargo.toml"><img src="https://img.shields.io/badge/Version-0.4.2-89DCEB?style=for-the-badge&amp;logo=github" alt="Version: 0.4.2" height="32"></a>
<a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache--2.0-A6E3A1?style=for-the-badge" alt="License: Apache 2.0" height="32"></a>
EOF
	cat >"$root/CHANGELOG.md" <<'EOF'
# Changelog

## [Unreleased]

## [0.4.2] - 2026-09-12

**A fixed thing**

### Fixed

- A thing that was broken.

## [0.4.1] - 2026-09-01

### Added

- The thing.

[Unreleased]: https://github.com/example/alter-zero/compare/v0.4.2...HEAD
[0.4.2]: https://github.com/example/alter-zero/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/example/alter-zero/releases/tag/v0.4.1
EOF
	printf 'Apache License\n' >"$root/LICENSE"
	# The npm launcher package: the checkout's own, renamed and at the
	# fixture's version — derived rather than written out, so the readers
	# are proven against the real file's layout and a reformat fails here.
	mkdir -p "$root/npm"
	sed -e 's#"@linuztx/alter-zero#"@example/alter-zero#' \
		-e 's/"[0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*[^"]*"/"0.4.2"/' \
		"$CHECKOUT/npm/package.json" >"$root/npm/package.json"
	cp -R "$CHECKOUT/npm/bin" "$CHECKOUT/npm/scripts" "$CHECKOUT/npm/README.md" "$root/npm/"
}

# ---------------------------------------------------------------------------
section "version grammar"
for v in 0.1.0 1.2.3 10.20.30 0.2.0-rc.1 1.0.0-beta+exp.sha.5114f85; do
	if is_semver "$v"; then pass "semver accepts $v"; else flunk "semver rejects $v"; fi
done
for v in v0.1.0 0.1 01.0.0 1.2.3.4 "" 1.2.3-; do
	if is_semver "$v"; then flunk "semver accepts '$v'"; else pass "semver rejects '$v'"; fi
done
expect_eq "tag_of" "v0.4.2" "$(tag_of 0.4.2)"
expect_eq "version_of_tag" "0.4.2" "$(version_of_tag v0.4.2)"
expect_fail "version_of_tag refuses a bare version" version_of_tag 0.4.2
expect_fail "version_of_tag refuses a non-semver tag" version_of_tag v0.4
if is_prerelease 0.2.0-rc.1; then pass "0.2.0-rc.1 is a pre-release"; else flunk "0.2.0-rc.1 not seen as a pre-release"; fi
if is_prerelease 0.2.0; then flunk "0.2.0 seen as a pre-release"; else pass "0.2.0 is not a pre-release"; fi

# ---------------------------------------------------------------------------
section "readers"
fixture "$T/fx"
export RELEASE_ROOT="$T/fx"
expect_eq "manifest version is [package]'s, not a dependency's" "0.4.2" "$(manifest_version)"
expect_eq "manifest name" "alter-zero" "$(manifest_name)"
expect_eq "manifest repository drops .git" "https://github.com/example/alter-zero" "$(manifest_repository)"
expect_eq "repo slug" "example/alter-zero" "$(repo_slug)"
expect_eq "lock version of the crate (not the crate named 'version')" "0.4.2" "$(lock_version alter-zero)"
expect_eq "lock version of another package" "8.8.8" "$(lock_version version)"
expect_eq "README badge label" "0.4.2" "$(readme_badge_version)"
expect_eq "README alt text" "0.4.2" "$(readme_alt_version)"
mkdir -p "$T/pre"
sed -e 's/Version-0\.4\.2-/Version-0.5.0--rc.1-/' -e 's/Version: 0\.4\.2/Version: 0.5.0-rc.1/' "$T/fx/README.md" >"$T/pre/README.md"
RELEASE_ROOT="$T/pre" expect_eq "README badge unescapes -- to -" "0.5.0-rc.1" "$(RELEASE_ROOT="$T/pre" readme_badge_version)"
expect_eq "badge_escape" "0.5.0--rc.1" "$(badge_escape 0.5.0-rc.1)"

# ---------------------------------------------------------------------------
section "npm readers"
expect_eq "npm package name" "@example/alter-zero" "$(npm_name)"
expect_eq "npm package version (not a dependency's, nor engines')" "0.4.2" "$(npm_version)"
expect_eq "npm pins, one per platform package" "@example/alter-zero-darwin-arm64 0.4.2
@example/alter-zero-darwin-x64 0.4.2
@example/alter-zero-linux-arm64 0.4.2
@example/alter-zero-linux-x64 0.4.2" "$(npm_pins)"
expect_eq "npm platform of a Linux target" "linux-arm64" "$(npm_platform aarch64-unknown-linux-gnu)"
expect_eq "npm platform of a macOS target" "darwin-x64" "$(npm_platform x86_64-apple-darwin)"
expect_fail "npm_platform refuses a target the release does not build" npm_platform x86_64-unknown-linux-musl
expect_eq "npm package of a target" "@example/alter-zero-linux-x64" "$(npm_package_of x86_64-unknown-linux-gnu)"
expect_eq "a release is published as latest" "latest" "$(npm_dist_tag 0.4.2)"
expect_eq "a pre-release as next" "next" "$(npm_dist_tag 0.5.0-rc.1)"
expect_eq "every target the release workflow builds has a platform package" \
	"$(for t in $NPM_TARGETS; do printf '%s\n' "$t"; done | sort)" \
	"$(sed -n 's/^ *- target: //p' "$CHECKOUT/.github/workflows/release.yml" | sort)"
if command -v node >/dev/null 2>&1; then
	expect_eq "the launcher looks each platform package up by lib.sh's name for it" \
		"$(for t in $NPM_TARGETS; do printf '%s %s\n' "$(npm_platform "$t")" "$t"; done | sort)" \
		"$(node -e 'const p = require(process.argv[1]).PLATFORMS; for (const k of Object.keys(p).sort()) console.log(k, p[k])' "$CHECKOUT/npm/bin/alter-zero.js")"
else
	warn "no node — skipping the launcher's platform table case"
fi

# ---------------------------------------------------------------------------
section "changelog"
expect_eq "released versions, newest first" "0.4.2 0.4.1" "$(changelog_versions | tr '\n' ' ' | sed 's/ $//')"
expect_eq "heading date" "2026-09-12" "$(changelog_date 0.4.2)"
expect_eq "no date for an unknown version" "" "$(changelog_date 0.9.9)"
expect_eq "previous of 0.4.2" "0.4.1" "$(changelog_previous 0.4.2)"
expect_eq "previous of the first release" "" "$(changelog_previous 0.4.1)"
expect_eq "section body, trimmed" "**A fixed thing**

### Fixed

- A thing that was broken." "$(changelog_section 0.4.2)"
expect_eq "last section stops before the link block" "### Added

- The thing." "$(changelog_section 0.4.1)"
expect_eq "empty [Unreleased]" "" "$(changelog_section Unreleased)"
expect_eq "link reference" "https://github.com/example/alter-zero/releases/tag/v0.4.1" "$(changelog_link 0.4.1)"

# ---------------------------------------------------------------------------
section "assets"
expect_eq "asset archive name" "alter-zero-v0.4.2-x86_64-unknown-linux-gnu.tar.gz" "$(asset_archive 0.4.2 x86_64-unknown-linux-gnu)"
expect_eq "asset_target parses the triple back" "aarch64-apple-darwin" "$(asset_target alter-zero-v0.4.2-aarch64-apple-darwin.tar.gz 0.4.2)"
expect_fail "asset_target refuses another version" asset_target alter-zero-v0.4.1-aarch64-apple-darwin.tar.gz 0.4.2
expect_fail "asset_target refuses a foreign name" asset_target notes.tar.gz 0.4.2
expect_eq "platform label" "macOS Apple silicon" "$(platform_label aarch64-apple-darwin)"
expect_eq "unknown platform falls back to the triple" "riscv64gc-unknown-linux-gnu" "$(platform_label riscv64gc-unknown-linux-gnu)"
expect_eq "format words" "ELF 64-bit|aarch64" "$(binary_format_words aarch64-unknown-linux-gnu)"
case "$(host_target)" in
*-unknown-linux-gnu | *-apple-darwin) pass "host target is $(host_target)" ;;
*) flunk "host target is $(host_target)" ;;
esac

# ---------------------------------------------------------------------------
section "check"
expect_ok "a consistent tree passes" bash "$STEPS/check.sh"
expect_ok "…and with its tag" bash "$STEPS/check.sh" v0.4.2
expect_fail "a tag for another version fails" bash "$STEPS/check.sh" v0.4.3
expect_contains "…naming the expected tag" "$OUT" "expected v0.4.2"
expect_fail "a malformed tag fails" bash "$STEPS/check.sh" 0.4.2
GITHUB_REF_TYPE=tag GITHUB_REF_NAME=v0.4.9 expect_fail "the workflow's tag variables are read" bash "$STEPS/check.sh"
GITHUB_REF_TYPE=branch GITHUB_REF_NAME=main expect_ok "a branch ref is not a tag" bash "$STEPS/check.sh"

fixture "$T/bad-lock" && sed 's/^version = "0.4.2"$/version = "0.4.1"/' "$T/fx/Cargo.lock" >"$T/bad-lock/Cargo.lock"
RELEASE_ROOT="$T/bad-lock" expect_fail "a stale Cargo.lock fails" bash "$STEPS/check.sh"
expect_contains "…and says so" "$OUT" "Cargo.lock records alter-zero 0.4.1"

fixture "$T/bad-readme" && sed 's/0\.4\.2/0.4.1/g' "$T/fx/README.md" >"$T/bad-readme/README.md"
RELEASE_ROOT="$T/bad-readme" expect_fail "a stale README badge fails" bash "$STEPS/check.sh"

fixture "$T/no-section" && sed '/^## \[0.4.2\]/,/^## \[0.4.1\]/{/^## \[0.4.1\]/!d;}' "$T/fx/CHANGELOG.md" >"$T/no-section/CHANGELOG.md"
RELEASE_ROOT="$T/no-section" expect_fail "a missing changelog section fails" bash "$STEPS/check.sh"
expect_contains "…pointing at prepare" "$OUT" "prepare"

fixture "$T/no-link" && grep -v '^\[0.4.2\]: ' "$T/fx/CHANGELOG.md" >"$T/no-link/CHANGELOG.md"
RELEASE_ROOT="$T/no-link" expect_fail "a missing link reference fails" bash "$STEPS/check.sh"

fixture "$T/no-changelog" && rm "$T/no-changelog/CHANGELOG.md"
RELEASE_ROOT="$T/no-changelog" expect_fail "a missing CHANGELOG.md fails" bash "$STEPS/check.sh"

fixture "$T/pending" && awk '/^## \[Unreleased\]$/ { print; print ""; print "### Added"; print ""; print "- Not yet rolled."; next } { print }' "$T/fx/CHANGELOG.md" >"$T/pending/CHANGELOG.md"
RELEASE_ROOT="$T/pending" expect_ok "pending [Unreleased] entries pass without a tag" bash "$STEPS/check.sh"
RELEASE_ROOT="$T/pending" expect_fail "…but fail on the release's tag" bash "$STEPS/check.sh" v0.4.2
expect_contains "…saying they were not rolled" "$OUT" "roll them into"

fixture "$T/npm-stale" && sed 's/^  "version": "0.4.2",$/  "version": "0.4.1",/' "$T/fx/npm/package.json" >"$T/npm-stale/npm/package.json"
RELEASE_ROOT="$T/npm-stale" expect_fail "a stale npm/package.json version fails" bash "$STEPS/check.sh"
expect_contains "…naming the file" "$OUT" "npm/package.json says 0.4.1"

fixture "$T/npm-pin" && sed 's#"@example/alter-zero-linux-arm64": "0.4.2"#"@example/alter-zero-linux-arm64": "0.4.1"#' "$T/fx/npm/package.json" >"$T/npm-pin/npm/package.json"
RELEASE_ROOT="$T/npm-pin" expect_fail "a platform package pinned at another version fails" bash "$STEPS/check.sh"
expect_contains "…naming the package" "$OUT" "pins @example/alter-zero-linux-arm64 at 0.4.1"

fixture "$T/npm-unpinned" && grep -v 'alter-zero-darwin-x64' "$T/fx/npm/package.json" >"$T/npm-unpinned/npm/package.json"
RELEASE_ROOT="$T/npm-unpinned" expect_fail "a platform package missing from optionalDependencies fails" bash "$STEPS/check.sh"
expect_contains "…naming it" "$OUT" "@example/alter-zero-darwin-x64"

fixture "$T/npm-extra" && awk '{ print } /alter-zero-darwin-x64"/ { print "    \"@example/alter-zero-linux-riscv64\": \"0.4.2\"," }' "$T/fx/npm/package.json" >"$T/npm-extra/npm/package.json"
RELEASE_ROOT="$T/npm-extra" expect_fail "a pin for a platform no release target builds fails" bash "$STEPS/check.sh"
expect_contains "…naming it" "$OUT" "@example/alter-zero-linux-riscv64"

fixture "$T/npm-missing" && rm "$T/npm-missing/npm/package.json"
RELEASE_ROOT="$T/npm-missing" expect_fail "a missing npm/package.json fails" bash "$STEPS/check.sh"
expect_contains "…saying so" "$OUT" "npm/package.json is missing"

# ---------------------------------------------------------------------------
section "package + verify"
host="$(host_target)"
if command -v cc >/dev/null 2>&1; then
	cat >"$T/fake.c" <<'EOF'
#include <stdio.h>
#include <string.h>
int main(int argc, char **argv) {
	if (argc > 1 && strcmp(argv[1], "--version") == 0) { puts("alter-zero 0.4.2"); return 0; }
	return 1;
}
EOF
	if cc -o "$T/fake-bin" "$T/fake.c" 2>"$T/cc.log"; then
		archive="$(package_dist "$T/fake-bin" 0.4.2 "$host" "$T/dist")"
		expect_eq "package_dist names the archive" "$T/dist/alter-zero-v0.4.2-$host.tar.gz" "$archive"
		if [ -f "$archive.sha256" ]; then pass "…beside its .sha256"; else flunk "no .sha256 beside $archive"; fi
		expected_entries="$(printf '%s\n' \
			"alter-zero-v0.4.2-$host" \
			"alter-zero-v0.4.2-$host/CHANGELOG.md" \
			"alter-zero-v0.4.2-$host/LICENSE" \
			"alter-zero-v0.4.2-$host/README.md" \
			"alter-zero-v0.4.2-$host/alter-zero" | sort)"
		expect_eq "the archive holds exactly the stem dir and four files" "$expected_entries" "$(tar -tzf "$archive" | sed 's#/$##' | sort)"
		sleep 1 # straddle a second boundary: the copies' own mtimes must not leak into the archive
		second="$(package_dist "$T/fake-bin" 0.4.2 "$host" "$T/dist2")"
		expect_eq "packaging is reproducible across time" "$(sha256_of "$archive")" "$(sha256_of "$second")"
		expect_ok "verify passes a good dist" bash "$STEPS/verify.sh" "$T/dist"
		expect_contains "…running --version on the host's asset" "$OUT" "alter-zero --version"
		expect_ok "verify takes an explicit version" bash "$STEPS/verify.sh" "$T/dist" --version 0.4.2
		expect_fail "verify refuses another version" bash "$STEPS/verify.sh" "$T/dist" --version 0.4.3
		expect_fail "verify refuses an empty dist" bash "$STEPS/verify.sh" "$T/empty-dist" 2>/dev/null || true
		mkdir -p "$T/empty-dist"
		expect_fail "verify refuses a dist with no archives" bash "$STEPS/verify.sh" "$T/empty-dist"

		cp -R "$T/dist" "$T/dist-badsum"
		printf '%s  %s\n' "0000000000000000000000000000000000000000000000000000000000000000" "$(basename "$archive")" >"$T/dist-badsum/$(basename "$archive").sha256"
		# …and in SHA256SUMS too, which is what a downloader actually reads.
		write_sha256sums "$T/dist-badsum" >/dev/null
		expect_fail "a wrong .sha256 fails" bash "$STEPS/verify.sh" "$T/dist-badsum"
		expect_contains "…naming the checksum" "$OUT" ".sha256"

		cp -R "$T/dist" "$T/dist-stray" && printf 'oops\n' >"$T/dist-stray/notes.txt"
		expect_fail "a stray file in dist fails" bash "$STEPS/verify.sh" "$T/dist-stray"
		expect_contains "…naming it" "$OUT" "notes.txt"

		cp -R "$T/dist" "$T/dist-sums" && cat "$T/dist-sums"/*.sha256 >"$T/dist-sums/SHA256SUMS"
		expect_ok "a SHA256SUMS agreeing with the per-asset files passes" bash "$STEPS/verify.sh" "$T/dist-sums"
		printf '%s  missing.tar.gz\n' "0000000000000000000000000000000000000000000000000000000000000000" >>"$T/dist-sums/SHA256SUMS"
		expect_fail "a SHA256SUMS naming a missing file fails" bash "$STEPS/verify.sh" "$T/dist-sums"

		# A release as published — its archives and SHA256SUMS, never the
		# per-asset files — is `download`'s to put back in a built dist's shape
		# (below); verify holds every archive to the .sha256 beside it.
		mkdir -p "$T/dist-as-published"
		cp "$T/dist"/*.tar.gz "$T/dist"/*.tar.gz.sha256 "$T/dist-as-published/"
		write_sha256sums "$T/dist-as-published" >/dev/null
		rm -f "$T/dist-as-published"/*.tar.gz.sha256
		expect_fail "verify refuses an archive with no .sha256 beside it, SHA256SUMS or not" bash "$STEPS/verify.sh" "$T/dist-as-published"
		expect_contains "…saying so" "$OUT" ".sha256 is missing"
		# shellcheck disable=SC2016 # $1 and $2 are bash -c's own arguments
		expect_fail "write_sha256sums refuses a dist with no per-asset files to gather" bash -c '. "$1/lib.sh"; write_sha256sums "$2"' _ "$STEPS" "$T/dist-as-published"
		expect_contains "…naming what is missing" "$OUT" "no per-asset .sha256 files"
		expect_eq "…leaving the SHA256SUMS there as it was" "1" "$(grep -c "$(basename "$archive")" "$T/dist-as-published/SHA256SUMS")"

		mkdir -p "$T/dist-layout/x/alter-zero-v0.4.2-$host"
		cp "$T/fake-bin" "$T/dist-layout/x/alter-zero-v0.4.2-$host/alter-zero"
		cp "$T/fx/LICENSE" "$T/dist-layout/x/alter-zero-v0.4.2-$host/"
		tar_reproducible "$T/dist-layout/x" "alter-zero-v0.4.2-$host" "$T/dist-layout/alter-zero-v0.4.2-$host.tar.gz"
		write_sha256 "$T/dist-layout/alter-zero-v0.4.2-$host.tar.gz"
		rm -rf "$T/dist-layout/x"
		expect_fail "an archive missing README/CHANGELOG fails" bash "$STEPS/verify.sh" "$T/dist-layout"
		expect_contains "…as a layout problem" "$OUT" "unexpected layout"

		case "$host" in
		x86_64-*) other="${host/x86_64/aarch64}" ;;
		*) other="${host/aarch64/x86_64}" ;;
		esac
		package_dist "$T/fake-bin" 0.4.2 "$other" "$T/dist-cpu" >/dev/null
		expect_fail "a host binary labelled as $other fails the CPU check" bash "$STEPS/verify.sh" "$T/dist-cpu"
		expect_contains "…quoting file(1)" "$OUT" "file says"

		section "publish --dry-run"
		expect_ok "a dry run needs no gh" env PATH="/nonexistent:$PATH" bash "$STEPS/publish.sh" 0.4.2 "$T/dist" --dry-run
		expect_contains "…printing the draft create" "$OUT" "gh release create v0.4.2 -R example/alter-zero --draft --verify-tag"
		expect_contains "…with the notes" "$OUT" "--notes-file"
		expect_contains "…titled by the tag alone" "$OUT" "--title v0.4.2 --notes-file"
		expect_lacks "…never the tag plus the changelog's title" "$OUT" "A\\ fixed\\ thing"
		expect_contains "…uploading the archive" "$OUT" "alter-zero-v0.4.2-$host.tar.gz"
		expect_contains "…and SHA256SUMS" "$OUT" "SHA256SUMS"
		expect_lacks "…but no per-asset checksum files" "$OUT" ".tar.gz.sha256"
		expect_contains "…and the publish flip" "$OUT" "gh release edit v0.4.2 -R example/alter-zero --draft=false --latest"
		if [ -f "$T/dist/SHA256SUMS" ]; then pass "…after writing SHA256SUMS"; else flunk "no SHA256SUMS written"; fi
		expect_ok "the dist still verifies with SHA256SUMS" bash "$STEPS/verify.sh" "$T/dist"
		expect_fail "publish refuses a dist that fails verify" bash "$STEPS/publish.sh" 0.4.2 "$T/dist-badsum" --dry-run

		section "install.sh"
		if command -v python3 >/dev/null 2>&1; then
			base="http://127.0.0.1:$(serve_release "$T/dist" v0.4.2)"
			expect_ok "install.sh installs the latest release from the stand-in server" env ALTER_ZERO_INSTALL_BASE_URL="$base" ALTER_ZERO_INSTALL_DIR="$T/home/bin" sh "$CHECKOUT/install.sh"
			expect_contains "…reading the tag off the /releases/latest redirect" "$OUT" "v0.4.2 · latest"
			expect_contains "…verifying the checksum" "$OUT" "matches the published value"
			expect_contains "…and reporting the install" "$OUT" "Installed"
			expect_contains "…with the PATH hint for a directory not on it" "$OUT" "is not on your PATH"
			expect_lacks "…and no colour codes when piped" "$OUT" "$(printf '\033')"
			if [ -x "$T/home/bin/alter-zero" ]; then pass "…the binary is executable"; else flunk "no executable at $T/home/bin/alter-zero"; fi
			expect_eq "…and runs" "alter-zero 0.4.2" "$("$T/home/bin/alter-zero" --version 2>&1)"
			# A minimal Debian server ships wget and no curl. The tag must still
			# come off the redirect: GitHub's API is rate-limited per address and
			# knows nothing of ALTER_ZERO_INSTALL_BASE_URL, so a fork — or this
			# stand-in — would be answered with the real repository's newest tag.
			# A PATH of links to everything the installer runs, minus curl.
			mkdir -p "$T/nocurl"
			for tool in sh uname ldd wget tar gzip sha256sum shasum openssl awk sed mktemp rm wc tr cut paste head cp chmod mv mkdir basename; do
				tool_path=$(command -v "$tool" 2>/dev/null) && ln -s "$tool_path" "$T/nocurl/$tool"
			done
			if [ -x "$T/nocurl/wget" ]; then
				expect_ok "install.sh resolves and downloads with wget where there is no curl" env PATH="$T/nocurl" ALTER_ZERO_INSTALL_BASE_URL="$base" ALTER_ZERO_INSTALL_DIR="$T/home/bin-wget" sh "$CHECKOUT/install.sh"
				expect_contains "…reading the tag off the same redirect" "$OUT" "v0.4.2 · latest"
				expect_contains "…verifying the checksum" "$OUT" "matches the published value"
				expect_eq "…and installing a binary that runs" "alter-zero 0.4.2" "$("$T/home/bin-wget/alter-zero" --version 2>&1)"
			else
				warn "no wget — skipping the curl-less install.sh case"
			fi
			stop_release
			mkdir -p "$T/dist-published"
			cp "$T/dist"/*.tar.gz "$T/dist"/*.tar.gz.sha256 "$T/dist-published/"
			write_sha256sums "$T/dist-published" >/dev/null
			rm -f "$T/dist-published"/*.tar.gz.sha256
			base="http://127.0.0.1:$(serve_release "$T/dist-published" v0.4.2)"
			expect_ok "install.sh verifies against SHA256SUMS, with no per-asset file published" env ALTER_ZERO_INSTALL_BASE_URL="$base" ALTER_ZERO_INSTALL_DIR="$T/home/bin-pub" sh "$CHECKOUT/install.sh"
			expect_contains "…reporting the match" "$OUT" "matches the published value"
			if [ -x "$T/home/bin-pub/alter-zero" ]; then pass "…and installing the binary"; else flunk "no executable at $T/home/bin-pub/alter-zero"; fi
			stop_release
			base="http://127.0.0.1:$(serve_release "$T/dist" v0.4.2)"
			expect_ok "install.sh takes --version and --dir" env ALTER_ZERO_INSTALL_BASE_URL="$base" sh "$CHECKOUT/install.sh" --version 0.4.2 --dir "$T/home/bin2"
			expect_contains "…as a pinned release" "$OUT" "v0.4.2 · pinned"
			expect_ok "…installing over itself again" env ALTER_ZERO_INSTALL_BASE_URL="$base" ALTER_ZERO_INSTALL_DIR="$T/home/bin2" sh "$CHECKOUT/install.sh"
			expect_fail "install.sh refuses a release with no asset for the machine" env ALTER_ZERO_INSTALL_BASE_URL="$base" ALTER_ZERO_VERSION=v9.9.9 ALTER_ZERO_INSTALL_DIR="$T/home/bin3" sh "$CHECKOUT/install.sh"
			expect_contains "…saying so" "$OUT" "no release asset"
			if [ ! -e "$T/home/bin3/alter-zero" ]; then pass "…and installing nothing"; else flunk "…but left a binary behind"; fi
			stop_release
			base="http://127.0.0.1:$(serve_release "$T/dist-badsum" v0.4.2)"
			expect_fail "install.sh refuses a download whose checksum does not match" env ALTER_ZERO_INSTALL_BASE_URL="$base" ALTER_ZERO_INSTALL_DIR="$T/home/bin4" sh "$CHECKOUT/install.sh"
			expect_contains "…naming the mismatch" "$OUT" "checksum mismatch"
			expect_contains "…and that nothing was installed" "$OUT" "Nothing was installed"
			if [ ! -e "$T/home/bin4/alter-zero" ]; then pass "…truly"; else flunk "…but left a binary behind"; fi
			stop_release

			section "download"
			base="http://127.0.0.1:$(serve_release "$T/dist-published" v0.4.2)"
			expect_ok "download fetches a published release and verifies it" bash "$STEPS/download.sh" 0.4.2 "$T/dl" --from "$base"
			expect_contains "…verifying what it fetched" "$OUT" "verified 1 asset(s) for alter-zero 0.4.2"
			expect_eq "…in the shape build leaves: each archive beside its .sha256, and SHA256SUMS" \
				"SHA256SUMS alter-zero-v0.4.2-$host.tar.gz alter-zero-v0.4.2-$host.tar.gz.sha256" "$(cd "$T/dl" && printf '%s\n' * | LC_ALL=C sort | tr '\n' ' ' | sed 's/ $//')"
			expect_eq "…the archive byte for byte" "$(sha256_of "$archive")" "$(sha256_of "$T/dl/$(basename "$archive")")"
			expect_eq "…and its .sha256 the one build wrote" "$(cat "$archive.sha256")" "$(cat "$T/dl/$(basename "$archive").sha256")"
			expect_ok "…a dist publish and npm take as built" bash "$STEPS/verify.sh" "$T/dl"
			expect_fail "download refuses a directory that is not empty" bash "$STEPS/download.sh" 0.4.2 "$T/dl" --from "$base"
			expect_contains "…saying so" "$OUT" "not empty"
			expect_fail "download refuses a version that is not published" bash "$STEPS/download.sh" 0.4.3 "$T/dl-unpublished" --from "$base"
			expect_contains "…naming the tag" "$OUT" "is v0.4.3 published?"
			if [ ! -e "$T/dl-unpublished" ]; then pass "…creating no directory"; else flunk "…but created $T/dl-unpublished"; fi
			expect_fail "download refuses a version that is not semver" bash "$STEPS/download.sh" 0.4 "$T/dl-bad-version" --from "$base"
			# A PATH with every tool there is but curl, as on a minimal server.
			mkdir -p "$T/no-curl-bin"
			(
				IFS=:
				for dir in $PATH; do
					[ -d "$dir" ] && ln -s "$dir"/* "$T/no-curl-bin/" 2>/dev/null
				done
			)
			rm -f "$T/no-curl-bin/curl"
			if [ -e "$T/no-curl-bin/wget" ]; then
				expect_ok "download fetches with wget where there is no curl" env PATH="$T/no-curl-bin" "$BASH" "$STEPS/download.sh" 0.4.2 "$T/dl-wget" --from "$base"
				expect_eq "…the same files" "$(cd "$T/dl" && sha256_of SHA256SUMS)" "$(cd "$T/dl-wget" && sha256_of SHA256SUMS)"
			else
				warn "no wget — skipping download's curl-less case"
			fi
			stop_release
			base="http://127.0.0.1:$(serve_release "$T/dist-badsum" v0.4.2)"
			expect_fail "download refuses a release whose archive does not match its SHA256SUMS" bash "$STEPS/download.sh" 0.4.2 "$T/dl-badsum" --from "$base"
			expect_contains "…as verify does" "$OUT" "does not match"
			stop_release
			mkdir -p "$T/dist-evil"
			printf '%s  ../evil.tar.gz\n' "$(sha256_of "$archive")" >"$T/dist-evil/SHA256SUMS"
			base="http://127.0.0.1:$(serve_release "$T/dist-evil" v0.4.2)"
			expect_fail "download refuses a SHA256SUMS naming a path rather than an asset" bash "$STEPS/download.sh" 0.4.2 "$T/dl-evil/inner" --from "$base"
			expect_contains "…naming it" "$OUT" "../evil.tar.gz"
			if [ ! -e "$T/dl-evil" ]; then pass "…writing nothing at all"; else flunk "…but wrote into $T/dl-evil"; fi
			stop_release
			mkdir -p "$T/dist-empty-sums" && : >"$T/dist-empty-sums/SHA256SUMS"
			base="http://127.0.0.1:$(serve_release "$T/dist-empty-sums" v0.4.2)"
			expect_fail "download refuses a SHA256SUMS that lists nothing" bash "$STEPS/download.sh" 0.4.2 "$T/dl-empty" --from "$base"
			expect_contains "…saying so" "$OUT" "lists no assets"
			stop_release

			expect_ok "install.sh --help prints the usage" sh "$CHECKOUT/install.sh" --help
			expect_contains "…naming the one-liner" "$OUT" "curl -fsSL"
			expect_fail "install.sh refuses an unknown option" sh "$CHECKOUT/install.sh" --bogus
			expect_ok "release_server.py parses" python3 -c 'import ast, sys; ast.parse(open(sys.argv[1]).read(), sys.argv[1])' "$STEPS/release_server.py"
		else
			warn "no python3 — skipping the install.sh cases"
		fi

		section "npm"
		if ! command -v node >/dev/null 2>&1 || ! command -v npm >/dev/null 2>&1; then
			warn "no node/npm — skipping the npm cases"
		elif ! command -v python3 >/dev/null 2>&1; then
			warn "no python3 — skipping the npm cases"
		else
			# A whole release: the host's real (fake) binary, and for every
			# other target a header file(1) reads as that target's.
			for target in $NPM_TARGETS; do
				bin="$T/fake-bin"
				if [ "$target" != "$host" ]; then
					bin="$T/header-$target"
					craft_header "$target" "$bin"
				fi
				package_dist "$bin" 0.4.2 "$target" "$T/npm-dist" >/dev/null
			done
			expect_ok "a four-target fixture release verifies" bash "$STEPS/verify.sh" "$T/npm-dist"

			expect_ok "npm --dry-run stages every package" bash "$STEPS/npm.sh" 0.4.2 "$T/npm-dist" --out "$T/npm-out" --dry-run
			expect_contains "…publishing nothing" "$OUT" "nothing published"
			expect_contains "…naming each command it would run" "$OUT" "npm publish $T/npm-out/alter-zero-linux-x64 --tag latest"
			for target in $NPM_TARGETS; do
				key="$(npm_platform "$target")"
				dir="$T/npm-out/alter-zero-$key"
				libc=""
				[ "${key%-*}" = linux ] && libc=glibc
				expect_eq "…$key: name, version, os, cpu, libc and files" \
					"@example/alter-zero-$key 0.4.2 ${key%-*} ${key#*-} $libc bin/alter-zero" \
					"$(json "$dir/package.json" '[m.name, m.version, m.os.join(), m.cpu.join(), (m.libc || []).join(), m.files.join()].join(" ")')"
				expect_eq "…$key: packing the binary, its README and the LICENSE" "LICENSE README.md bin/alter-zero package.json" "$(packed "$dir")"
				if [ -x "$dir/bin/alter-zero" ]; then pass "…$key: the binary is executable"; else flunk "$dir/bin/alter-zero is not executable"; fi
			done
			expect_eq "…the host's package holds the host's binary" "alter-zero 0.4.2" "$("$T/npm-out/alter-zero-$(npm_platform "$host")/bin/alter-zero" --version 2>&1)"
			expect_contains "…its README pointing at the launcher" "$(cat "$T/npm-out/alter-zero-linux-x64/README.md")" "npm install -g @example/alter-zero"
			expect_eq "…the launcher: npm/package.json without its scripts" \
				"$(json "$T/fx/npm/package.json" 'delete m.scripts; JSON.stringify(m)')" \
				"$(json "$T/npm-out/alter-zero/package.json" 'JSON.stringify(m)')"
			expect_eq "…packing the launcher, its README and the LICENSE" "LICENSE README.md bin/alter-zero.js package.json" "$(packed "$T/npm-out/alter-zero")"

			cp -R "$T/npm-dist" "$T/npm-dist-3"
			rm -f "$T/npm-dist-3"/*aarch64-apple-darwin*
			expect_fail "npm refuses to publish a release missing a platform" bash "$STEPS/npm.sh" 0.4.2 "$T/npm-dist-3" --out "$T/npm-out-3"
			expect_contains "…naming the package every such install would lack" "$OUT" "@example/alter-zero-darwin-arm64"
			expect_ok "…while a dry run stages the rest" bash "$STEPS/npm.sh" 0.4.2 "$T/npm-dist-3" --out "$T/npm-out-3" --dry-run
			expect_contains "…saying what it skipped" "$OUT" "skipping @example/alter-zero-darwin-arm64"
			RELEASE_ROOT="$T/npm-stale" expect_fail "npm refuses an npm/package.json at another version" bash "$STEPS/npm.sh" 0.4.2 "$T/npm-dist" --out "$T/npm-out-stale" --dry-run
			expect_contains "…naming it" "$OUT" "npm/package.json is at 0.4.1"
			expect_fail "npm refuses a dist that fails verify" bash "$STEPS/npm.sh" 0.4.2 "$T/dist-badsum" --out "$T/npm-out-bad" --dry-run

			mkdir -p "$T/registry"
			port="$(serve_registry "$T/registry")"
			printf '//127.0.0.1:%s/:_authToken=selftest\n' "$port" >"$T/npmrc"
			npm_env=(env "NPM_CONFIG_REGISTRY=http://127.0.0.1:$port/" "NPM_CONFIG_USERCONFIG=$T/npmrc" "NPM_CONFIG_CACHE=$T/npm-cache"
				NPM_CONFIG_UPDATE_NOTIFIER=false NPM_CONFIG_AUDIT=false NPM_CONFIG_FUND=false)
			expect_ok "npm publishes the release to the registry" "${npm_env[@]}" bash "$STEPS/npm.sh" 0.4.2 "$T/npm-dist" --out "$T/npm-out"
			expect_eq "…the platform packages first, the launcher last" "@example/alter-zero-linux-x64@0.4.2
@example/alter-zero-linux-arm64@0.4.2
@example/alter-zero-darwin-x64@0.4.2
@example/alter-zero-darwin-arm64@0.4.2
@example/alter-zero@0.4.2" "$(cat "$T/registry/published.log")"
			expect_ok "a second run publishes nothing" "${npm_env[@]}" bash "$STEPS/npm.sh" 0.4.2 "$T/npm-dist" --out "$T/npm-out"
			expect_contains "…finding each version already there" "$OUT" "@example/alter-zero@0.4.2 is already on the registry"
			expect_eq "…and the registry as it was" "5" "$(wc -l <"$T/registry/published.log" | tr -d ' ')"
			expect_ok "npm install -g installs the launcher from the registry" "${npm_env[@]}" npm install -g --prefix "$T/npm-prefix" @example/alter-zero
			expect_eq "…whose alter-zero runs the host's binary" "alter-zero 0.4.2" "$("$T/npm-prefix/bin/alter-zero" --version 2>&1)"
			expect_eq "…the one platform package installed beside it" "alter-zero-$(npm_platform "$host")" "$(ls "$T/npm-prefix/lib/node_modules/@example/alter-zero/node_modules/@example")"
			expect_fail "…and the binary's exit status comes through" "$T/npm-prefix/bin/alter-zero" --unknown-flag
			expect_fail "npm/ itself refuses to be published" "${npm_env[@]}" npm publish "$T/fx/npm"
			expect_contains "…pointing at scripts/release.sh npm" "$OUT" "scripts/release.sh npm"
			expect_ok "npm_registry.py parses" python3 -c 'import ast, sys; ast.parse(open(sys.argv[1]).read(), sys.argv[1])' "$STEPS/npm_registry.py"
			stop_registry
		fi
	else
		flunk "cc could not build the fixture binary: $(cat "$T/cc.log")"
	fi
else
	warn "no C compiler — skipping the package/verify/notes/publish cases"
fi

section "notes"
expect_ok "notes render for a version with a previous release" bash "$STEPS/notes.sh" 0.4.2
notes="$OUT"
expect_eq "…opening on the section's own bold title line" "**A fixed thing**" "$(printf '%s' "$notes" | head -1)"
expect_contains "…over the changelog body" "$notes" "- A thing that was broken."
expect_eq "…promoting it to no heading of its own" "0" "$(printf '%s\n' "$notes" | grep -c '^## ')"
expect_ok "notes render for a version with no title" bash "$STEPS/notes.sh" 0.4.1
expect_eq "…opening straight on the entries" "### Added" "$(printf '%s' "$OUT" | head -1)"
expect_eq "…and inventing no title" "0" "$(printf '%s\n' "$OUT" | grep -c '^\*\*A ')"
expect_contains "…and closing on the compare link" "$notes" "**Full changelog**: https://github.com/example/alter-zero/compare/v0.4.1...v0.4.2"
expect_lacks "…with no assets table" "$notes" "## Assets"
expect_lacks "…and no install section" "$notes" "### Install"
expect_lacks "…nor the one-liner" "$notes" "install.sh | sh"
expect_lacks "…and no contributors line" "$notes" "**Contributors**"
expect_ok "notes render for a first release" bash "$STEPS/notes.sh" 0.4.1
expect_contains "…linking its commits, with nothing to compare against" "$OUT" "**Full changelog**: https://github.com/example/alter-zero/commits/v0.4.1"
expect_fail "notes refuse an unknown version" bash "$STEPS/notes.sh" 0.9.9
expect_fail "notes refuse a second argument" bash "$STEPS/notes.sh" 0.4.2 "$T"
expect_contains "…pointing at the usage" "$OUT" "usage: scripts/release.sh notes VERSION"

# GitHub renders its own Contributors block — avatars, read off the release's
# commits — between the body and the assets, so a line of our own would be the
# same fact twice, in plain text, directly above the real one. The case that
# used to print it is a tree with real history, which is what this fixture is.
section "contributors"
if command -v git >/dev/null 2>&1; then
	fixture "$T/repo"
	(
		cd "$T/repo" || exit 1
		git init -q . && git config user.email a@example.com && git config user.name "Ada Lovelace"
		git add -A && git commit -qm "the thing" --no-gpg-sign
		git tag -a v0.4.1 -m v0.4.1
		printf 'x\n' >x.txt && git add -A
		git -c user.name="Grace Hopper" -c user.email=g@example.com commit -qm "a fix" --no-gpg-sign
		git tag -a v0.4.2 -m v0.4.2
	) >/dev/null 2>&1
	expect_ok "notes render inside a git repo" env RELEASE_ROOT="$T/repo" bash "$STEPS/notes.sh" 0.4.2
	expect_lacks "…crediting nobody in the body" "$OUT" "**Contributors**"
	expect_lacks "…naming no author of its own" "$OUT" "Grace Hopper"
	expect_eq "…and ending on the compare link, with nothing after it" "**Full changelog**: https://github.com/example/alter-zero/compare/v0.4.1...v0.4.2" "$(printf '%s\n' "$OUT" | tail -1)"
else
	warn "no git — skipping the contributors cases"
fi

# ---------------------------------------------------------------------------
section "prepare"
fixture "$T/prep" && cp "$T/pending/CHANGELOG.md" "$T/prep/CHANGELOG.md"
RELEASE_ROOT="$T/prep" RELEASE_DATE=2026-10-01 expect_ok "prepare rolls a fixture forward" bash "$STEPS/prepare.sh" 0.5.0
expect_eq "…Cargo.toml bumped (dependencies untouched)" "0.5.0 9.9.9" "$(RELEASE_ROOT="$T/prep" manifest_version) $(grep -c '"9.9.9"' "$T/prep/Cargo.toml" >/dev/null && echo 9.9.9)"
expect_eq "…Cargo.lock's crate entry bumped" "0.5.0" "$(RELEASE_ROOT="$T/prep" lock_version alter-zero)"
expect_eq "…the other lock entries untouched" "8.8.8" "$(RELEASE_ROOT="$T/prep" lock_version version)"
expect_eq "…README badge bumped" "0.5.0 0.5.0" "$(RELEASE_ROOT="$T/prep" readme_badge_version) $(RELEASE_ROOT="$T/prep" readme_alt_version)"
expect_eq "…the license badge untouched" "1" "$(grep -c 'License-Apache--2.0' "$T/prep/README.md")"
expect_eq "…changelog heading dated" "2026-10-01" "$(RELEASE_ROOT="$T/prep" changelog_date 0.5.0)"
expect_eq "…taking the pending body" "### Added

- Not yet rolled." "$(RELEASE_ROOT="$T/prep" changelog_section 0.5.0)"
expect_eq "…leaving [Unreleased] empty" "" "$(RELEASE_ROOT="$T/prep" changelog_section Unreleased)"
expect_eq "…older sections intact" "**A fixed thing**

### Fixed

- A thing that was broken." "$(RELEASE_ROOT="$T/prep" changelog_section 0.4.2)"
expect_eq "…compare link added" "https://github.com/example/alter-zero/compare/v0.4.2...v0.5.0" "$(RELEASE_ROOT="$T/prep" changelog_link 0.5.0)"
expect_eq "…[Unreleased] re-pointed" "https://github.com/example/alter-zero/compare/v0.5.0...HEAD" "$(RELEASE_ROOT="$T/prep" changelog_link Unreleased)"
expect_eq "…npm/package.json bumped" "0.5.0" "$(RELEASE_ROOT="$T/prep" npm_version)"
expect_eq "…every platform package re-pinned" "0.5.0 0.5.0 0.5.0 0.5.0" "$(RELEASE_ROOT="$T/prep" npm_pins | awk '{ print $2 }' | tr '\n' ' ' | sed 's/ $//')"
expect_eq "…its engines range untouched" "1" "$(grep -c '"node": ">=18"' "$T/prep/npm/package.json")"
expect_contains "…and the commit it prints adds it" "$OUT" "git add Cargo.toml Cargo.lock README.md CHANGELOG.md npm/package.json"
if command -v node >/dev/null 2>&1; then
	expect_ok "…leaving valid JSON" node -e 'JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"))' "$T/prep/npm/package.json"
fi
RELEASE_ROOT="$T/prep" expect_ok "…and the tree checks out for its tag" bash "$STEPS/check.sh" v0.5.0
expect_eq "…heading order: Unreleased, 0.5.0, 0.4.2, 0.4.1" "0.5.0 0.4.2 0.4.1" "$(RELEASE_ROOT="$T/prep" changelog_versions | tr '\n' ' ' | sed 's/ $//')"

RELEASE_ROOT="$T/prep" expect_fail "prepare refuses the current version" bash "$STEPS/prepare.sh" 0.5.0
RELEASE_ROOT="$T/prep" expect_fail "prepare refuses an empty [Unreleased]" bash "$STEPS/prepare.sh" 0.6.0
expect_contains "…and says why" "$OUT" "[Unreleased] section is empty"
RELEASE_ROOT="$T/prep" expect_fail "prepare refuses a non-semver version" bash "$STEPS/prepare.sh" 0.6
RELEASE_ROOT="$T/prep" expect_fail "prepare refuses a version already in the changelog" bash "$STEPS/prepare.sh" 0.4.1

fixture "$T/first"
awk '/^## \[Unreleased\]$/ { print; print ""; print "- First."; next } { print }' <<'EOF' >"$T/first/CHANGELOG.md"
# Changelog

## [Unreleased]

[Unreleased]: https://github.com/example/alter-zero/commits/HEAD
EOF
RELEASE_ROOT="$T/first" RELEASE_DATE=2026-10-01 expect_ok "prepare handles a first release" bash "$STEPS/prepare.sh" 0.5.0
expect_eq "…linking the tag, with nothing to compare against" "https://github.com/example/alter-zero/releases/tag/v0.5.0" "$(RELEASE_ROOT="$T/first" changelog_link 0.5.0)"

# ---------------------------------------------------------------------------
# Every case stops the stand-in it started; one left running would outlive
# the selftest itself (they were, once: started inside $(…), their PIDs went
# with the subshell).
section "stand-in servers"
if command -v pgrep >/dev/null 2>&1; then
	for _ in $(seq 1 40); do
		pgrep -f "(release_server|npm_registry)\.py $T/" >/dev/null || break
		sleep 0.05
	done
	if pgrep -f "(release_server|npm_registry)\.py $T/" >/dev/null; then
		flunk "stand-in servers still running: $(pgrep -f "(release_server|npm_registry)\.py $T/" | tr '\n' ' ')"
	else
		pass "every stand-in server was stopped"
	fi
fi

# ---------------------------------------------------------------------------
printf '\n%d passed, %d failed\n' "$PASS" "$FAILED"
[ "$FAILED" -eq 0 ]
