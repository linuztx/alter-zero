#!/usr/bin/env bash
# scripts/release/notes.sh — the release notes for one version, as markdown
# on stdout.
#
#   scripts/release.sh notes 0.1.0             # the changelog section alone
#   scripts/release.sh notes 0.1.0 dist/       # …plus the assets table
#
# The body is CHANGELOG.md's `## [X.Y.Z]` section verbatim — the changelog
# is the single place a release is described, so the notes cannot drift
# from it. With a dist directory, an **Assets** table follows (each archive
# linked to its download URL with its platform and SHA-256, read from the
# `.sha256` beside it), then a verify-and-install snippet, and last the
# compare link to the previous release (or the tag's commit list for the
# first). The workflow writes this to the release and to its job summary.
#
# The install snippet below is literal shell for the READER — its ${tag}
# and ${asset} are meant to stay unexpanded, hence the SC2016 waiver.
# shellcheck disable=SC2016
set -euo pipefail
# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

version="${1:-}"
dist="${2:-}"
[ -n "$version" ] || die "usage: scripts/release.sh notes VERSION [DIST]"
is_semver "$version" || die "notes: '$version' is not a semver version"
repo="$(manifest_repository)"
name="$(manifest_name)"
tag="$(tag_of "$version")"

body="$(changelog_section "$version")"
[ -n "$body" ] || die "notes: CHANGELOG.md has no [$version] section (or it is empty)"
printf '%s\n' "$body"

if [ -n "$dist" ]; then
	[ -d "$dist" ] || die "notes: no directory at $dist"
	shopt -s nullglob
	first=""
	rows=""
	for archive in "$dist"/*.tar.gz; do
		base="$(basename "$archive")"
		target="$(asset_target "$base" "$version")" || die "notes: $base is not an asset of $name $version"
		if [ -f "$archive.sha256" ]; then
			read -r hash _ <"$archive.sha256"
		else
			hash="$(sha256_of "$archive")"
		fi
		[ -n "$first" ] || first="$target"
		rows="$rows$(printf '| [`%s`](%s/releases/download/%s/%s) | %s | `%s` |' "$base" "$repo" "$tag" "$base" "$(platform_label "$target")" "$hash")
"
	done
	if [ -n "$rows" ]; then
		printf '\n## Assets\n\n| Asset | Platform | SHA-256 |\n| --- | --- | --- |\n%s' "$rows"
		printf '\nEach archive unpacks to a directory of the same name holding the `%s` binary, `LICENSE`, `README.md` and this release'"'"'s `CHANGELOG.md`; `SHA256SUMS` lists every checksum above in `sha256sum -c` form.\n' "$name"
		printf '\n### Verify and install\n\n'
		printf '```bash\n'
		printf 'tag=%s\n' "$tag"
		printf 'asset=%s-${tag}-%s   # pick your platform'"'"'s name from the table\n' "$name" "$first"
		printf 'curl -fsSLO "%s/releases/download/${tag}/${asset}.tar.gz"\n' "$repo"
		printf 'curl -fsSLO "%s/releases/download/${tag}/${asset}.tar.gz.sha256"\n' "$repo"
		printf 'sha256sum -c "${asset}.tar.gz.sha256"   # macOS: shasum -a 256 -c "${asset}.tar.gz.sha256"\n'
		printf 'tar -xzf "${asset}.tar.gz"\n'
		printf 'mkdir -p ~/.local/bin && install -m 755 "${asset}/%s" ~/.local/bin/   # or anywhere on your PATH\n' "$name"
		printf '%s --version\n' "$name"
		printf '```\n'
	fi
fi

prev="$(changelog_previous "$version")"
if [ -n "$prev" ]; then
	printf '\n**Full changelog**: %s/compare/v%s...%s\n' "$repo" "$prev" "$tag"
else
	printf '\n**Full changelog**: %s/commits/%s\n' "$repo" "$tag"
fi
