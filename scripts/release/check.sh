#!/usr/bin/env bash
# scripts/release/check.sh — everything a version is written in agrees.
#
#   scripts/release.sh check            # the tree is consistent with itself
#   scripts/release.sh check v0.1.0     # …and the tag names that version
#
# Reports EVERY mismatch before failing. The workflow runs this first on a
# tag push (with $GITHUB_REF_NAME), CI runs it on every push without a tag,
# so a commit that bumps Cargo.toml without a changelog entry — or the
# other way round — never reaches a tag. The rules (docs/release.md):
#
#   1. Cargo.toml's [package] version is semver.
#   2. Cargo.lock records the same version for the crate (`--locked` would
#      refuse the build otherwise, but this says so in one line).
#   3. The README's version badge — URL label and alt text — says the same.
#   4. CHANGELOG.md has a `## [X.Y.Z] - YYYY-MM-DD` section for it, with a
#      non-empty body and a `[X.Y.Z]: url` link reference.
#   5. CHANGELOG.md keeps a `## [Unreleased]` section and its link.
#   6. With a TAG: it is `vX.Y.Z` for that version, and the `[Unreleased]`
#      section is empty — an entry left there was written for this release
#      and never rolled into it.
set -euo pipefail
# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

tag="${1:-}"
if [ -z "$tag" ] && [ "${GITHUB_REF_TYPE:-}" = tag ]; then
	tag="${GITHUB_REF_NAME:-}"
fi

version="$(manifest_version)"
name="$(manifest_name)"
[ -n "$version" ] || die "Cargo.toml: no [package] version found under $RELEASE_ROOT"
if is_semver "$version"; then
	ok "Cargo.toml: $name $version"
else
	fail "Cargo.toml: version '$version' is not semver (X.Y.Z, optionally -pre or +build)"
fi

lock="$(lock_version "$name")"
if [ "$lock" = "$version" ]; then
	ok "Cargo.lock: $name $lock"
else
	fail "Cargo.lock records $name ${lock:-<none>}, Cargo.toml says $version — run scripts/release.sh prepare, or cargo update --workspace"
fi

badge="$(readme_badge_version)"
alt="$(readme_alt_version)"
if [ "$badge" = "$version" ] && [ "$alt" = "$version" ]; then
	ok "README.md: version badge $version"
else
	fail "README.md: version badge says '${badge:-<none>}' (alt '${alt:-<none>}'), Cargo.toml says $version"
fi

if [ ! -f "$(changelog_path)" ]; then
	fail "CHANGELOG.md is missing"
else
	date="$(changelog_date "$version")"
	if [ -z "$date" ]; then
		fail "CHANGELOG.md: no '## [$version] - YYYY-MM-DD' section — write the release's entry (scripts/release.sh prepare rolls [Unreleased] into one)"
	elif [ -z "$(changelog_section "$version")" ]; then
		fail "CHANGELOG.md: the [$version] section is empty"
	else
		ok "CHANGELOG.md: [$version] - $date"
	fi
	if [ -z "$(changelog_link "$version")" ]; then
		fail "CHANGELOG.md: no '[$version]: <url>' link reference at the bottom"
	fi
	if ! grep -q '^## \[Unreleased\]$' "$(changelog_path)"; then
		fail "CHANGELOG.md: no '## [Unreleased]' section"
	elif [ -z "$(changelog_link Unreleased)" ]; then
		fail "CHANGELOG.md: no '[Unreleased]: <url>' link reference"
	else
		ok "CHANGELOG.md: [Unreleased] section and link present"
	fi
fi

if [ -n "$tag" ]; then
	if [ "$tag" = "$(tag_of "$version")" ]; then
		ok "tag $tag names $version"
	else
		fail "tag '$tag' does not name Cargo.toml's version $version (expected $(tag_of "$version"))"
	fi
	if [ -f "$(changelog_path)" ] && [ -n "$(changelog_section Unreleased)" ]; then
		fail "CHANGELOG.md: [Unreleased] still has entries — roll them into [$version] before tagging"
	fi
fi

finish "release check passed for $name $version${tag:+ ($tag)}"
