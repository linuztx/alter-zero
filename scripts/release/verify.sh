#!/usr/bin/env bash
# scripts/release/verify.sh — prove the assets in dist/ are what a release may
# carry, before anything is uploaded.
#
#   scripts/release.sh verify                      # dist/, for Cargo.toml's version
#   scripts/release.sh verify out/ --version 0.1.0
#
# For every archive: its name is {name}-v{version}-{target}.{tar.gz,zip} for THIS
# version; its .sha256 names it and matches; it holds exactly one top-level
# directory of the same stem with the binary, LICENSE, README.md and
# CHANGELOG.md and nothing else; the binary is executable and `file` reports
# the target's object format and CPU (so a cross build that produced a host
# binary cannot ship); and when this machine can run it, `--version` prints
# `{name} {version}`. `SHA256SUMS`, when present, must agree with the
# per-asset files and cover every archive. Nothing else may sit in dist/ —
# a stray file there is a packaging mistake, even though `publish` uploads
# only the archives and SHA256SUMS. Reports every problem, then fails.
set -euo pipefail
# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

dist=""
version=""
while [ $# -gt 0 ]; do
	case "$1" in
	--version)
		version="$2"
		shift 2
		;;
	-h | --help)
		sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d' | sed 's/^# \{0,1\}//'
		exit 0
		;;
	-*) die "verify: unknown option $1" ;;
	*)
		[ -z "$dist" ] || die "verify: one dist directory (got '$dist' and '$1')"
		dist="$1"
		shift
		;;
	esac
done
[ -n "$dist" ] || dist="$RELEASE_ROOT/dist"
[ -d "$dist" ] || die "verify: no directory at $dist — run scripts/release.sh build first"
[ -n "$version" ] || version="$(manifest_version)"
name="$(manifest_name)"
host="$(host_target)"

shopt -s nullglob
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

count=0
for archive in "$dist"/*.tar.gz "$dist"/*.zip; do
	base="$(basename "$archive")"
	if ! target="$(asset_target "$base" "$version")"; then
		fail "$base: not a supported asset of $name $version (unknown target, extension, or version)"
		continue
	fi

	stem="$(asset_stem "$version" "$target")"
	ext="$(archive_ext "$target")"
	binary="$(asset_binary "$target")"
	if ! check_sha256 "$archive"; then
		fail "$base: .sha256 is missing, names another file, or does not match"
		continue
	fi

	if [ "$ext" = tar.gz ]; then
		if ! entries="$(tar -tzf "$archive" | sed 's#/$##' | sort)"; then
			fail "$base: cannot read archive layout"
			continue
		fi
		expected="$(printf '%s\n' "$stem"
			for f in $(asset_files "$target"); do printf '%s/%s\n' "$stem" "$f"; done)"
		expected="$(printf '%s\n' "$expected" | sort)"
		if [ "$entries" != "$expected" ]; then
			fail "$base: unexpected layout —
    have: $(printf '%s' "$entries" | tr '\n' ' ')
    want: $(printf '%s' "$expected" | tr '\n' ' ')"
			continue
		fi
	fi

	rm -rf "$scratch/x"
	mkdir -p "$scratch/x"
	if ! extract_asset "$target" "$archive" "$stem" "$scratch/x"; then
		fail "$base: unsafe or invalid archive layout"
		continue
	fi
	bin="$scratch/x/$stem/$binary"
	if [ ! -f "$bin" ] || [ -L "$bin" ] || { [ "$ext" = tar.gz ] && [ ! -x "$bin" ]; }; then
		fail "$base: $stem/$binary is not a regular executable file"
		continue
	fi

	if [ "$target" = x86_64-pc-windows-msvc ]; then
		if ! windows_archive pe "$bin"; then
			fail "$base: invalid Windows x64 PE executable"
			continue
		fi
	elif words="$(binary_format_words "$target")"; then
		desc="$(file -b "$bin")"
		for w in $(printf '%s' "$words" | tr '|' ' '); do
			case "$desc" in
			*"$w"* | *"$(printf '%s' "$w" | tr '[:upper:]' '[:lower:]')"*) ;;
			*) fail "$base: binary is not a $target build — file says: $desc" ;;
			esac
		done
	else
		fail "$base: no format rule for $target"
		continue
	fi

	if [ "$target" = "$host" ]; then
		if out="$("$bin" --version 2>&1)" && [ "$out" = "$name $version" ]; then
			ok "$base: $(platform_label "$target") — \`$name --version\` → $out"
		else
			fail "$base: \`$name --version\` printed '${out:-<nothing>}', expected '$name $version'"
		fi
	else
		ok "$base: $(platform_label "$target") — checksum, layout and CPU (not run: host is $host)"
	fi
	count=$((count + 1))
done
[ "$count" -gt 0 ] || fail "no supported $name-v$version archives (.tar.gz or .zip) in $dist"

# Every checksum file belongs to an archive; SHA256SUMS agrees with them all.
for sums in "$dist"/*.sha256; do
	[ -f "${sums%.sha256}" ] || fail "$(basename "$sums"): no archive beside it"
done
if [ -f "$dist/SHA256SUMS" ]; then
	listed=0
	while read -r hash file; do
		[ -n "$hash" ] || continue
		if ! [[ "$hash" =~ ^[0-9a-f]{64}$ ]] || ! asset_target "$file" "$version" >/dev/null; then
			fail "SHA256SUMS: invalid checksum or unsupported asset name '$file'"
			continue
		fi
		if [ ! -f "$dist/$file" ]; then
			fail "SHA256SUMS names $file, which is not in $dist"
		elif [ "$hash" != "$(sha256_of "$dist/$file")" ]; then
			fail "SHA256SUMS: $file's hash does not match the file"
		fi
		listed=$((listed + 1))
	done <"$dist/SHA256SUMS"
	archives=0
	for archive in "$dist"/*.tar.gz "$dist"/*.zip; do
		archives=$((archives + 1))
		matches="$(awk -v n="$(basename "$archive")" '$2 == n { count++ } END { print count+0 }' "$dist/SHA256SUMS")"
		[ "$matches" -eq 1 ] || fail "SHA256SUMS must list $(basename "$archive") exactly once"
	done
	if [ "$listed" -eq "$archives" ]; then
		ok "SHA256SUMS covers all $archives archive(s)"
	else
		fail "SHA256SUMS lists $listed lines for $archives archive(s)"
	fi
fi

# Nothing a release should not carry.
for f in "$dist"/*; do
	case "$(basename "$f")" in
	*.tar.gz | *.tar.gz.sha256 | *.zip | *.zip.sha256 | SHA256SUMS) ;;
	*) fail "$(basename "$f"): stray file in $dist — publish uploads everything here" ;;
	esac
done

finish "verified $count asset(s) for $name $version in $dist"
