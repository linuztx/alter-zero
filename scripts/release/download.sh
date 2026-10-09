#!/usr/bin/env bash
# scripts/release/download.sh — fetch a published release back into a dist
# directory, in the shape `build` leaves one, and verify it.
#
#   scripts/release.sh download 0.12.0                # into dist/
#   scripts/release.sh download 0.12.0 out/           # into out/
#   scripts/release.sh download 0.12.0 --from URL     # another repository root, or a stand-in for github.com
#
# A release is what its SHA256SUMS lists: that file first, then every asset
# it names, from {repository}/releases/download/vX.Y.Z/ — Cargo.toml's
# `repository` unless --from names another root — with curl, or wget where
# there is no curl: no gh and no GitHub account. Each asset's own line of
# SHA256SUMS is written beside it as its .sha256, the file `build` writes
# and `verify` holds every archive to, and then `verify` proves the lot:
# checksums, layout, CPU and, for the host's own archive, `--version`. This
# is how a version already on GitHub is published to npm by hand
# (docs/npm.md). The directory must be empty or new, so a download is never
# mixed into another dist, and nothing is written to it until SHA256SUMS has
# been read and every name in it is a plain file name.
set -euo pipefail
# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

version=""
dist=""
base=""
while [ $# -gt 0 ]; do
	case "$1" in
	--from)
		[ $# -ge 2 ] || die "download: --from needs a URL"
		base="$2"
		shift 2
		;;
	-h | --help)
		sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d' | sed 's/^# \{0,1\}//'
		exit 0
		;;
	-*) die "download: unknown option $1" ;;
	*)
		if [ -z "$version" ]; then
			version="$1"
		elif [ -z "$dist" ]; then
			dist="$1"
		else
			die "download: unexpected argument $1"
		fi
		shift
		;;
	esac
done
[ -n "$version" ] || die "usage: scripts/release.sh download VERSION [DIST] [--from URL]"
is_semver "$version" || die "download: '$version' is not a semver version"
[ -n "$dist" ] || dist="$RELEASE_ROOT/dist"
[ -n "$base" ] || base="$(manifest_repository)"
base="${base%/}"
tag="$(tag_of "$version")"
url="$base/releases/download/$tag"

# fetch URL FILE — the body of URL into FILE (`-`: stdout), failing on any
# HTTP error.
if command -v curl >/dev/null 2>&1; then
	fetch() { curl -fsSL --retry 2 -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
	fetch() { wget -q -O "$2" "$1"; }
else
	die "download: neither curl nor wget is installed"
fi

if [ -d "$dist" ] && [ -n "$(ls -A "$dist")" ]; then
	die "download: $dist is not empty — download into an empty or new directory"
fi

listing="$(fetch "$url/SHA256SUMS" -)" || die "download: no SHA256SUMS at $url — is $tag published?"

# Every line is `<hex>  <name>`, and every name a plain file name: a name
# with a slash in it, or a dot first, is a path, and is never fetched.
count=0
while read -r hash name; do
	[ -n "$hash$name" ] || continue
	[[ "$hash" =~ ^[0-9a-f]{64}$ ]] || die "download: SHA256SUMS has a line that is not '<sha256>  <name>': $hash $name"
	case "$name" in
	"" | .* | */*) die "download: SHA256SUMS names '$name', which is not an asset's file name" ;;
	esac
	count=$((count + 1))
done <<<"$listing"
[ "$count" -gt 0 ] || die "download: the SHA256SUMS at $url lists no assets"

mkdir -p "$dist"
printf '%s\n' "$listing" >"$dist/SHA256SUMS"

while read -r hash name; do
	[ -n "$hash$name" ] || continue
	fetch "$url/$name" "$dist/$name" || die "download: could not fetch $url/$name"
	printf '%s  %s\n' "$hash" "$name" >"$dist/$name.sha256"
	ok "$name"
done <"$dist/SHA256SUMS"

bash "$RELEASE_LIB_DIR/verify.sh" "$dist" --version "$version"
