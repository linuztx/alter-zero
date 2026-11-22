#!/usr/bin/env bash
# The shipped history's completeness (docs/builtin-updates.md):
# prompts/shipped.sha256 must list every version of every built-in skill file
# and agent definition that any commit reachable from HEAD carried, since a
# build of that commit could have seeded it into someone's config home. A
# version missing there makes every untouched copy of it read as the user's
# edit, kept for good and never updated again.
#
#   scripts/shipped_history.sh            # the missing lines, ready to append:
#                                         #   scripts/shipped_history.sh >> prompts/shipped.sha256
#   scripts/shipped_history.sh --check    # the same on stderr, and exit 1 if any (CI)
#
# The unit test every_built_in_file_is_in_the_shipped_history holds the
# current bytes to the list at every commit; this holds every earlier
# version to it, the ones from before the list existed included. It reads
# the whole history, so a shallow clone, which holds only some versions and
# would pass having checked nothing, exits 2.

set -euo pipefail
# One sort order on every machine.
export LC_ALL=C

check=false
case "${1:-}" in
--check) check=true ;;
"") ;;
*)
	echo "usage: scripts/shipped_history.sh [--check]" >&2
	exit 2
	;;
esac
cd "$(git rev-parse --show-toplevel)"

if [ "$(git rev-parse --is-shallow-repository)" = true ]; then
	echo "shipped_history: this clone is shallow, so it holds only some versions of the built-in files; run \`git fetch --unshallow\` first" >&2
	exit 2
fi

list=prompts/shipped.sha256
if [ ! -r "$list" ]; then
	echo "shipped_history: cannot read $list" >&2
	exit 2
fi

if command -v sha256sum >/dev/null 2>&1; then
	sha256() { sha256sum | cut -d' ' -f1; }
else
	sha256() { shasum -a 256 | cut -d' ' -f1; }
fi

# Where the built-in files are authored. `agents/` is where the definitions
# lived for the one commit before they moved under `prompts/`.
sources=(prompts/skills prompts/agents agents)

# Every `<blob>\t<key>` a commit touching those directories held, the key
# being the file's name in the list (`prompts/skills/…`, `prompts/agents/…`).
# A file that is never seeded is left out: a script's `test_*` unit tests,
# whatever Python leaves behind, and a link.
pairs="$(
	git log --full-history --format=%H HEAD -- "${sources[@]}" |
		while read -r commit; do
			git ls-tree -r "$commit" -- "${sources[@]}"
		done |
		awk -F'\t' '
			{
				split($1, meta, " ")
				if (meta[2] != "blob" || meta[1] == "120000") next
				path = $2
				n = split(path, parts, "/")
				name = parts[n]
				if (name ~ /^test_/ || name ~ /^\./ || name ~ /\.pyc$/ || path ~ /\/__pycache__\//) next
				if (path ~ /^prompts\/skills\/[^\/]+\/.+/ || path ~ /^prompts\/agents\/[^\/]+\.md$/) key = path
				else if (path ~ /^agents\/[^\/]+\.md$/) key = "prompts/" path
				else next
				print meta[3] "\t" key
			}' |
		sort -u
)"

missing=""
while IFS=$'\t' read -r blob key; do
	[ -n "$blob" ] || continue
	line="$(git cat-file blob "$blob" | sha256)  $key"
	# The list as the binary reads it: blank lines and `#` comments are not
	# entries.
	if ! grep -qxF -- "$line" "$list"; then
		missing+="$line"$'\n'
	fi
done <<<"$pairs"
missing="$(printf '%s' "$missing" | sort -u -k2,2 -k1,1)"

if [ "$check" = false ]; then
	[ -z "$missing" ] || printf '%s\n' "$missing"
	exit 0
fi
if [ -n "$missing" ]; then
	printf '%s\n' "$missing" | sed 's/^/missing: /' >&2
	echo "shipped_history --check: $list lacks versions of built-in files that commits in this history carried, so a build of one of them leaves copies a release takes for the user's edits; append them: scripts/shipped_history.sh >> $list" >&2
	exit 1
fi
count="$(printf '%s\n' "$pairs" | grep -c .)"
echo "shipped_history --check: ok: every one of the $count versions of built-in files in this history is listed in $list"
