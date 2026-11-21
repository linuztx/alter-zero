#!/usr/bin/env bash
# The legacy table of built-in files (docs/builtin-updates.md): the SHA-256 of
# every version of every built-in skill file and agent definition that a
# commit reachable from REV ever carried, as `sha256sum` lines over the path
# the file is installed at (`skills/<name>/…`, `agents/<name>.md`).
#
#   scripts/builtin-legacy.sh [REV] > prompts/builtin-legacy.sha256
#   scripts/builtin-legacy.sh --check [TABLE]
#
# It answers one question for an install made before the manifest existed:
# is the copy on disk one this project shipped, untouched, or the user's own
# edit? A release that keeps a manifest never needs a line added here, so the
# table is regenerated only until the first release that carries the feature.
#
# `--check`, which CI runs, holds the table to that. A release tag (`v*`)
# whose tree has no prompts/builtin-legacy.sha256 was made before the
# feature, and every built-in file it installs must have its
# `<digest>  <path>` line in TABLE (by default, that same file): a version
# missing there is one whose untouched installs would be taken for the
# user's own edits and never updated again. Each such line is printed as
# `missing at vX.Y.Z: <digest>  <path>` and the check exits 1; otherwise it
# prints how many tags and files it checked and exits 0. It disables itself:
# a release that carries the table is not checked, and the tags before it
# never change, so it keeps passing with no upkeep. A clone that cannot
# answer (shallow, or without the release tags) exits 2, since a pass there
# would have checked nothing.
#
# It reads history, so it needs all of it: `git fetch --unshallow` first on a
# shallow clone, which would otherwise list only the versions it happens to
# hold and say nothing about the rest. `--check` reads the release tags too.

set -euo pipefail
# One order on every machine: the table is diffed, so `sort` must not follow
# the caller's locale.
export LC_ALL=C

# Where the table is committed. A release whose tree carries it was made with
# the manifest, so `--check` never asks anything of it.
table_path=prompts/builtin-legacy.sha256

check=false
if [ "${1:-}" = --check ]; then
	check=true
	if [ $# -gt 2 ]; then
		echo "usage: scripts/builtin-legacy.sh --check [TABLE]" >&2
		exit 2
	fi
	# A TABLE given is a path from where the caller stands, so it is made
	# absolute before the move to the top of the repository; the default is
	# the committed table there.
	shown="${2:-$table_path}"
	case "${2:-}" in
	"") table="$table_path" ;;
	/*) table="$2" ;;
	*) table="$PWD/$2" ;;
	esac
else
	rev="${1:-HEAD}"
fi
cd "$(git rev-parse --show-toplevel)"

# `--check` giving up because it cannot answer: exit 2, never the 1 a missing
# line earns, and never a pass that checked nothing.
refuse() {
	echo "builtin-legacy --check: $1" >&2
	exit 2
}

if [ "$(git rev-parse --is-shallow-repository)" = true ]; then
	if [ "$check" = true ]; then
		refuse "this clone is shallow, so it holds only some releases' files and a pass would mean nothing; run \`git fetch --unshallow --tags\` first"
	fi
	echo "builtin-legacy: this clone is shallow; run \`git fetch --unshallow\` first" >&2
	exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
	sha256() { sha256sum | cut -d' ' -f1; }
else
	sha256() { shasum -a 256 | cut -d' ' -f1; }
fi

# Where the built-in files are authored. `agents/` is where the definitions
# lived for the one commit before they moved under `prompts/`.
sources=(prompts/skills prompts/agents agents)

# The path a source file is installed at, or nothing for a file that is never
# installed: a script's unit tests and anything Python leaves behind.
installed_path() {
	local source="$1" name="${1##*/}"
	case "$name" in
	test_* | .* | *.pyc) return 1 ;;
	esac
	case "$source" in
	*/__pycache__/*) return 1 ;;
	prompts/skills/*/*) printf 'skills/%s' "${source#prompts/skills/}" ;;
	prompts/agents/*.md) printf 'agents/%s' "${source#prompts/agents/}" ;;
	agents/*.md) printf 'agents/%s' "${source#agents/}" ;;
	*) return 1 ;;
	esac
}

# Every built-in file a commit holds, as `<blob>\t<installed path>`. The table
# and its check both read a commit through this, so the two cannot disagree
# about what a release installed.
installed_files() {
	git ls-tree -r "$1" -- "${sources[@]}" |
		while IFS=$'\t' read -r meta source; do
			path="$(installed_path "$source")" || continue
			printf '%s\t%s\n' "${meta##* }" "$path"
		done
}

if [ "$check" = true ]; then
	if [ ! -f "$table" ] || [ ! -r "$table" ]; then
		refuse "cannot read the table $shown"
	fi
	tags="$(git tag -l --sort=version:refname 'v*')"
	if [ -z "$tags" ]; then
		refuse "this clone has no release tags (v*), so there is nothing to check; run \`git fetch --tags\` first"
	fi

	# `<tag>\t<digest>  <path>` for every built-in file each release made
	# before the table installs. Built here, outside any `$(…)` or `||`, so a
	# git failure stops the check instead of dropping a tag's files from it.
	required="" checked=0 carrying=0 files=0 first="" last=""
	while IFS= read -r tag; do
		carried="$(git ls-tree --name-only "refs/tags/$tag" -- "$table_path")"
		if [ -n "$carried" ]; then
			carrying=$((carrying + 1))
			continue
		fi
		checked=$((checked + 1))
		first="${first:-$tag}"
		last="$tag"
		installed="$(installed_files "refs/tags/$tag")"
		while IFS=$'\t' read -r blob path; do
			[ -n "$blob" ] || continue
			digest="$(git cat-file blob "$blob" | sha256)"
			required+="$tag"$'\t'"$digest  $path"$'\n'
			files=$((files + 1))
		done <<<"$installed"
	done <<<"$tags"
	if [ "$checked" -eq 0 ]; then
		refuse "every release tag here carries the table, so the releases made before it are missing from this clone; run \`git fetch --tags\` first"
	fi
	if [ "$files" -eq 0 ]; then
		refuse "the release tags without the table hold no built-in file this script can find, so there is nothing to check"
	fi

	# TABLE is read the way the binary reads it (src/builtin.rs): blank lines
	# and `#` comments are not entries, and trailing whitespace is not part of
	# one.
	status=0
	printf '%s' "$required" | awk '
		BEGIN { FS = "\t" }
		FILENAME == ARGV[1] {
			sub(/[ \t\r]+$/, "")
			if ($0 != "" && substr($0, 1, 1) != "#") listed[$0] = 1
			next
		}
		!($2 in listed) { print "missing at " $1 ": " $2; missing = 1 }
		END { exit (missing ? 1 : 0) }
	' "$table" - || status=$?
	case "$status" in
	0) ;;
	1)
		echo "builtin-legacy --check: $shown lacks files that releases without the table installed, so those installs would take their untouched copies for the user's edits and never update them; regenerate it: scripts/builtin-legacy.sh > $table_path" >&2
		exit 1
		;;
	*) exit 2 ;;
	esac
	printf 'builtin-legacy --check: ok: %d built-in files in %d release tags without the table (%s to %s), all listed in %s; %d tags carry the table and need no check\n' \
		"$files" "$checked" "$first" "$last" "$shown" "$carrying"
	exit 0
fi

# Every (blob, path) a commit that touched those directories held. The whole
# history, not the simplified one: a version that lived only on a branch
# before it merged was still a build someone could run.
pairs="$(
	git log --full-history --format=%H "$rev" -- "${sources[@]}" |
		while read -r commit; do
			installed_files "$commit"
		done |
		sort -u
)"

cat <<'HEADER'
# Every version of every built-in skill file and agent definition shipped
# before installs kept a manifest of them (docs/builtin-updates.md), as
# `sha256sum` lines over the path each is installed at. A file on disk whose
# digest is listed here for its path is a built-in nobody edited.
#
# Generated by scripts/builtin-legacy.sh. Never edited by hand.
HEADER

while IFS=$'\t' read -r blob path; do
	printf '%s  %s\n' "$(git cat-file blob "$blob" | sha256)" "$path"
done <<<"$pairs" | sort -u -k2,2 -k1,1
