#!/usr/bin/env bash
# scripts/release/publish.sh — create the GitHub release for a version from
# the assets in dist/.
#
#   scripts/release.sh publish 0.1.0                     # dist/, notes from CHANGELOG.md
#   scripts/release.sh publish 0.1.0 out/ --notes notes.md
#   scripts/release.sh publish 0.1.0 --dry-run           # print the gh commands, touch nothing
#
# Needs `gh` signed in (a workflow hands it GH_TOKEN) and the tag already on
# the remote — `--verify-tag` refuses to mint one. The order is what makes a
# half-finished upload harmless: the assets are verified again, SHA256SUMS is
# assembled from the per-asset files, the release is created as a DRAFT with
# its notes and every asset in one call, and only then flipped to published
# (marked latest unless the version is a pre-release). A published release
# for the tag is never touched — releases are immutable here, a fix is a new
# version — while a leftover draft (a run that died mid-upload) is replaced.
set -euo pipefail
# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

version=""
dist=""
notes=""
dry_run=0
while [ $# -gt 0 ]; do
	case "$1" in
	--notes)
		notes="$2"
		shift 2
		;;
	--dry-run)
		dry_run=1
		shift
		;;
	-h | --help)
		sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d' | sed 's/^# \{0,1\}//'
		exit 0
		;;
	-*) die "publish: unknown option $1" ;;
	*)
		if [ -z "$version" ]; then
			version="$1"
		elif [ -z "$dist" ]; then
			dist="$1"
		else
			die "publish: unexpected argument $1"
		fi
		shift
		;;
	esac
done
[ -n "$version" ] || die "usage: scripts/release.sh publish VERSION [DIST] [--notes FILE] [--dry-run]"
is_semver "$version" || die "publish: '$version' is not a semver version"
[ -n "$dist" ] || dist="$RELEASE_ROOT/dist"
[ -d "$dist" ] || die "publish: no directory at $dist"
tag="$(tag_of "$version")"
slug="$(repo_slug)"

# The assets are proven before anything leaves the machine.
bash "$RELEASE_LIB_DIR/verify.sh" "$dist" --version "$version"

shopt -s nullglob
cat "$dist"/*.tar.gz.sha256 | sort -k 2 >"$dist/SHA256SUMS"
info "wrote $dist/SHA256SUMS"

if [ -z "$notes" ]; then
	notes="$(mktemp)"
	bash "$RELEASE_LIB_DIR/notes.sh" "$version" "$dist" >"$notes"
	info "rendered the notes from CHANGELOG.md into $notes"
fi
[ -s "$notes" ] || die "publish: notes file $notes is empty"

assets=("$dist"/*.tar.gz "$dist"/*.tar.gz.sha256 "$dist/SHA256SUMS")
create=(gh release create "$tag" -R "$slug" --draft --verify-tag --title "$tag" --notes-file "$notes")
if is_prerelease "$version"; then
	create+=(--prerelease)
	edit=(gh release edit "$tag" -R "$slug" --draft=false)
else
	edit=(gh release edit "$tag" -R "$slug" --draft=false --latest)
fi

if [ "$dry_run" -eq 1 ]; then
	info "dry run — would run:"
	{
		printf '  '
		printf '%q ' "${create[@]}" "${assets[@]}"
		printf '\n  '
		printf '%q ' "${edit[@]}"
		printf '\n'
	} >&2
	ok "dry run complete: $tag with ${#assets[@]} files (nothing published)"
	exit 0
fi

command -v gh >/dev/null 2>&1 || die "publish: gh (the GitHub CLI) is not installed"
if existing="$(gh release view "$tag" -R "$slug" --json isDraft --jq .isDraft 2>/dev/null)"; then
	if [ "$existing" = "false" ]; then
		die "publish: $slug already has a published release $tag — releases are immutable; cut a new version instead"
	fi
	warn "replacing the leftover draft release $tag"
	gh release delete "$tag" -R "$slug" --yes
fi

info "creating $tag as a draft with ${#assets[@]} files"
"${create[@]}" "${assets[@]}"
info "publishing $tag"
"${edit[@]}"
url="$(gh release view "$tag" -R "$slug" --json url --jq .url)"
ok "published $url"
