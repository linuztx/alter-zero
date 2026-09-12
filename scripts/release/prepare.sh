#!/usr/bin/env bash
# scripts/release/prepare.sh — bump the version everywhere it is written and
# roll CHANGELOG.md's [Unreleased] section into the release's own.
#
#   scripts/release.sh prepare 0.2.0
#
# Rewrites, in place: Cargo.toml's [package] version, Cargo.lock's entry for
# the crate (exactly what `cargo update --workspace` would write), the
# README's version badge (URL label and alt text), and CHANGELOG.md — the
# `## [Unreleased]` heading stays, empty, over a new `## [X.Y.Z] - <today>`
# heading that takes its body, and the link block gains the version's
# compare link (or the tag link, for a first release) with [Unreleased]
# re-pointed past it. Refuses a version that is not semver, is the current
# one, already has a changelog section or a local tag, or would release an
# empty [Unreleased] — the notes would be empty too. Ends by running `check`
# and printing the commit / tag / push that hands the rest to the workflow.
# RELEASE_DATE=YYYY-MM-DD overrides the date (the selftest pins it).
#
# The awk programs below are quoted whole; their $0 is awk's, not the
# shell's — the SC2016 waiver is for those.
# shellcheck disable=SC2016
set -euo pipefail
# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

version="${1:-}"
[ -n "$version" ] || die "usage: scripts/release.sh prepare VERSION"
is_semver "$version" || die "prepare: '$version' is not a semver version (X.Y.Z, optionally -pre)"
current="$(manifest_version)"
name="$(manifest_name)"
repo="$(manifest_repository)"
tag="$(tag_of "$version")"
date="${RELEASE_DATE:-$(date -u +%Y-%m-%d)}"

[ "$version" != "$current" ] || die "prepare: $version is already the current version"
[ -z "$(changelog_date "$version")" ] || die "prepare: CHANGELOG.md already has a [$version] section"
if git -C "$RELEASE_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1 && [ -n "$(git -C "$RELEASE_ROOT" tag -l "$tag")" ]; then
	die "prepare: the tag $tag already exists"
fi
grep -q '^## \[Unreleased\]$' "$(changelog_path)" || die "prepare: CHANGELOG.md has no '## [Unreleased]' section to roll"
[ -n "$(changelog_section Unreleased)" ] || die "prepare: CHANGELOG.md's [Unreleased] section is empty — write the release's entries first"

# Rewrite FILE through a filter, atomically (no `sed -i`: GNU and BSD differ).
rewrite() {
	local file="$1" tmp
	shift
	tmp="$(mktemp)"
	"$@" <"$file" >"$tmp"
	mv "$tmp" "$file"
}

# Cargo.toml: the [package] version, nothing else's.
rewrite "$RELEASE_ROOT/Cargo.toml" awk -v v="$version" '
	/^\[/ { in_pkg = ($0 == "[package]") }
	in_pkg && !done && /^version[ \t]*=/ { print "version = \"" v "\""; done = 1; next }
	{ print }
'
# Cargo.lock: the crate's own entry.
rewrite "$RELEASE_ROOT/Cargo.lock" awk -v name="$name" -v v="$version" '
	/^\[\[package\]\]/ { n = ""; next_is_pkg = 1 }
	/^name = / { n = $0; sub(/^name = "/, "", n); sub(/"$/, "", n) }
	/^version = / && n == name && !done { print "version = \"" v "\""; done = 1; next }
	{ print }
'
# README.md: the badge's URL label (shields escapes `-` as `--`) and alt text.
rewrite "$RELEASE_ROOT/README.md" sed \
	-e "s|img\.shields\.io/badge/Version-$(badge_escape "$current")-|img.shields.io/badge/Version-$(badge_escape "$version")-|" \
	-e "s|alt=\"Version: $current\"|alt=\"Version: $version\"|"
# CHANGELOG.md: roll [Unreleased] into the release's section and re-point
# the links.
prev="$(changelog_versions | sed -n '1p')"
if [ -n "$prev" ]; then
	link="$repo/compare/v$prev...$tag"
else
	link="$repo/releases/tag/$tag"
fi
rewrite "$(changelog_path)" awk -v v="$version" -v d="$date" -v repo="$repo" -v link="$link" '
	state == 0 && /^## \[Unreleased\]$/ { print; print ""; print "## [" v "] - " d; state = 1; next }
	state == 1 { if ($0 ~ /^[ \t]*$/) next; print ""; state = 2 }
	/^\[Unreleased\]: / { print "[Unreleased]: " repo "/compare/" "v" v "...HEAD"; print "[" v "]: " link; next }
	{ print }
'

ok "$name $current → $version ($date) in Cargo.toml, Cargo.lock, README.md and CHANGELOG.md"
bash "$RELEASE_LIB_DIR/check.sh"
cat >&2 <<EOF

Next, from $RELEASE_ROOT:
  git add Cargo.toml Cargo.lock README.md CHANGELOG.md
  git commit -m "Release $tag"
  git tag -a $tag -m "$name $tag"
  git push origin HEAD $tag          # the tag push runs .github/workflows/release.yml
EOF
