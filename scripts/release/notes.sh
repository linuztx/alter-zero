#!/usr/bin/env bash
# scripts/release/notes.sh — the release notes for one version, as markdown
# on stdout.
#
#   scripts/release.sh notes 0.1.0
#
# The body is CHANGELOG.md's `## [X.Y.Z]` section verbatim — the changelog
# is the single place a release is described, so the notes cannot drift
# from it — including the bold line a section may open on, which names the
# release (`**Alter Zero Initial Release**`). That line is left exactly as
# written rather than promoted to a heading: the release is named by its
# tag alone, so this is where a reader meets its name, and bold text sits
# over the entries without out-shouting the `###` sections under it. Then
# the compare link to the previous release (or the tag's commit list for
# the first).
#
# Nothing else, because the release page answers the rest better itself: it
# lists the uploaded assets (SHA256SUMS among them) right under the notes,
# where an assets table read as the same facts twice; the installer is the
# README's first command; and GitHub renders a **Contributors** block of
# its own — avatars, read off the release's commits — between the body and
# those assets, so a line naming them here is that block again in plain
# text, directly above the real one. The workflow writes this to the
# release and to its job summary.
set -euo pipefail
# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

version="${1:-}"
if [ $# -ne 1 ] || [ -z "$version" ]; then
	die "usage: scripts/release.sh notes VERSION"
fi
is_semver "$version" || die "notes: '$version' is not a semver version"
repo="$(manifest_repository)"
tag="$(tag_of "$version")"

body="$(changelog_section "$version")"
[ -n "$body" ] || die "notes: CHANGELOG.md has no [$version] section (or it is empty)"
printf '%s\n' "$body"

prev="$(changelog_previous "$version")"
if [ -n "$prev" ]; then
	printf '\n**Full changelog**: %s/compare/v%s...%s\n' "$repo" "$prev" "$tag"
else
	printf '\n**Full changelog**: %s/commits/%s\n' "$repo" "$tag"
fi
