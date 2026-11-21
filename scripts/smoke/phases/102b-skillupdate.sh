#!/usr/bin/env bash
# Phase 102b — built-in skills and agents follow the release, and keep the user's version

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Built-in skills and agent definitions follow the release (docs/builtin-
# updates.md): a file the user has not touched is replaced when the binary
# ships other bytes, a file the user changed is kept and named once by where
# it is, and a file a release no longer ships is removed only when no built-in
# file of its unit is the user's — and named once when it stays for that
# reason.
# Every decision is made against `.builtin.json`, a manifest in the folder it
# describes — so what this phase checks is filesystem behaviour across real
# launches: the manifest a fresh install writes, an install staged one release
# behind being brought forward (an edited script kept while its skill's
# outdated one updates), the next launch saying nothing, a bare `--resume`
# settling nothing its pick would erase unread, a folder a newer app settled
# being left alone, and an override folder only ever being seeded. The pure
# decision table is unit-tested; this is the half that writes, renames and
# unlinks in a real config home.
S102B="${S}_skillupdate"
BU_HOME="$(mktemp -d)"
# The config home is HOME's own `.alter-zero`, as a real user's is: a row
# names a file under its root as the user is shown it (`~/.alter-zero/skills/
# …`), which is also what keeps the rows short enough to read.
BU_CFG="$BU_HOME/.alter-zero"
mkdir -p "$BU_CFG"
BU_OUT="$(mktemp -d)"
BU_SK="$BU_CFG/skills"
BU_AG="$BU_CFG/agents"
BU_SKM="$BU_SK/.builtin.json"
BU_AGM="$BU_AG/.builtin.json"
BU_ZEROS="0000000000000000000000000000000000000000000000000000000000000000"
# NEITHER ALTER_ZERO_SKILLS_DIR NOR ALTER_ZERO_AGENTS_DIR: an override folder
# is somebody else's and is never updated, so the suite's default environment
# would test nothing here. Both roots are the config home's own. HOME moves too
# so `~/.claude/skills` finds nothing on a machine that has real skills.
bu_app() { # CONFIG_HOME [EXTRA_ENV…] → the launch string
	local cfg="$1"
	shift
	printf 'env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_UPDATE_CHECK=0 ALTER_ZERO_PROJECT_CONFIG=0 HOME=%s ALTER_ZERO_CONFIG_DIR=%s ALTER_ZERO_SESSIONS_DIR=%s ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_TIPS=0 ALTER_ZERO_PERMISSION_MODE=manual ALTER_ZERO_VERBOSE=1 ALTER_ZERO_FIRST_VERB=%s ALTER_ZERO_MASCOT_ANIMATION=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=%s %s %s' \
		"$BU_HOME" "$cfg" "$SMOKE_SESSIONS" "$VERB_TURN1" "$SMOKE_STARTUP_MS" "$*" "$BIN"
}
APP_BU="$(bu_app "$BU_CFG")"
note "launch string: $APP_BU"

bu_sha() { # FILE → its SHA-256, hex
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | awk '{ print $1 }'
	else
		shasum -a 256 "$1" | awk '{ print $1 }'
	fi
}
# bu_manifest FILE SCOPE VERSION [PATH DIGEST]… — write a manifest wholesale,
# in the shape the app renders (no jq/python on the CI image's guarantee list).
bu_manifest() {
	local file="$1" scope="$2" version="$3" sep=""
	shift 3
	{
		printf '{\n  "schema": 1,\n  "scope": "%s",\n  "version": "%s",\n  "files": {' "$scope" "$version"
		while [ "$#" -ge 2 ]; do
			printf '%s\n    "%s": "%s"' "$sep" "$1" "$2"
			sep=","
			shift 2
		done
		printf '\n  }\n}\n'
	} >"$file"
}
# bu_record MANIFEST PATH → the digest recorded for PATH, or nothing.
bu_record() {
	grep -F "\"$2\":" "$1" 2>/dev/null | sed 's/.*": *"\([0-9a-f]*\)".*/\1/' | head -n 1
}
# bu_joined SCREEN — the screen's rows as one line: each row's indent
# dropped, the rows joined by single spaces. A startup row word-wraps at the
# width (`ui::startup_paragraph_lines`, breaking only between words), so a
# sentence naming several files spans rows; joined, it reads as written.
bu_joined() {
	printf '%s\n' "$1" | sed 's/^ *//' | tr '\n' ' ' | tr -s ' '
}
# bu_run TITLE [CMD [ROW]] — launch, let the startup rows land, capture the
# screen, quit. Sets BU_SCREEN, and BU_ROWS (bu_joined). The rows commit under
# the banner after the first frame, so the capture waits for a screen that has
# stopped changing — showing ROW, when given — never a fixed nap.
bu_run() {
	local title="$1" cmd="${2:-$APP_BU}" row="${3:-}"
	BU_SCREEN=""
	BU_ROWS=""
	tmux kill-session -t "$S102B" 2>/dev/null
	launch "$S102B" 120 30 "$cmd" || return 1
	if [ -n "$row" ]; then
		wait_settled 10 "$S102B" -F "$row" >/dev/null || fail "$title never showed '$row'"
	else
		wait_settled 10 "$S102B" -E '^❯' >/dev/null
	fi
	BU_SCREEN="$(pane "$S102B" -S -100)"
	BU_ROWS="$(bu_joined "$BU_SCREEN")"
	dump "$title" "$BU_SCREEN"
	submit "$S102B" "/quit"
	wait_gone 10 "$S102B" || fail "the app did not quit after /quit ($title)"
	tmux kill-session -t "$S102B" 2>/dev/null
	return 0
}

BU_VERSION="$("$BIN" --version 2>&1 | awk '{ print $NF }')"
note "binary version: $BU_VERSION"
if [ -z "$BU_VERSION" ]; then
	fail "could not read the binary's version from --version"
fi

# ---------------------------------------------------------------------------
# Launch 1 — a fresh install. Seeds everything, says nothing, and leaves a
# manifest in each folder recording exactly what it wrote: without that
# record the next release could not tell its own file from the user's edit.
# ---------------------------------------------------------------------------
bu_run "launch 1: a fresh install"
# A fresh install is not news: rows here would greet every new user with a
# list of files they never had.
expect_lacks "$BU_SCREEN" -F "built-in" "a fresh install printed a built-in row"
for m in "$BU_SKM:skills" "$BU_AGM:agents"; do
	file="${m%:*}"
	scope="${m##*:}"
	if [ ! -f "$file" ]; then
		fail "the fresh install left no $scope/.builtin.json"
		continue
	fi
	echo "---- $file"
	cat "$file"
	expect_file_has "$file" -E '"schema": *1[,[:space:]]' "$scope/.builtin.json has no \"schema\": 1"
	expect_file_has "$file" -F "\"scope\": \"$scope\"" "$scope/.builtin.json does not name its own scope"
	expect_file_has "$file" -F "\"version\": \"$BU_VERSION\"" "$scope/.builtin.json does not carry the binary's version $BU_VERSION"
done
# The record is the digest of the AUTHORED file, not of whatever ended up on
# disk: a record of the wrong bytes would make the next update read every
# untouched file as the user's and keep it forever.
expect_eq "$(bu_record "$BU_SKM" "web-search/SKILL.md")" \
	"$(bu_sha "$SMOKE_ROOT/prompts/skills/web-search/SKILL.md")" \
	"the skills manifest's record for web-search/SKILL.md is not the authored file's SHA-256"
expect_eq "$(bu_record "$BU_AGM" "explore.md")" \
	"$(bu_sha "$SMOKE_ROOT/prompts/agents/explore.md")" \
	"the agents manifest's record for explore.md is not the authored file's SHA-256"

# ---------------------------------------------------------------------------
# Stage an install one release behind, by hand.
# ---------------------------------------------------------------------------
# outdated and untouched: old bytes the manifest says the app wrote.
printf '# an exa_search.py from an older release\nprint("old")\n' >"$BU_SK/web-search/scripts/exa_search.py"
d_exa="$(bu_sha "$BU_SK/web-search/scripts/exa_search.py")"
# the user's version of a script in that same skill: the decision is per file,
# so this one is kept while exa_search.py beside it still updates.
printf '# my own parallel_search.py\nprint("mine")\n' >"$BU_SK/web-search/scripts/parallel_search.py"
cp "$BU_SK/web-search/scripts/parallel_search.py" "$BU_OUT/parallel-mine.py"
# the user's version: bytes on disk that are neither the record nor shipped.
printf '%s\n' "---" "name: yt-dlp" "description: MY-OWN-YTDLP-MARKER." "---" "my body" >"$BU_SK/yt-dlp/SKILL.md"
cp "$BU_SK/yt-dlp/SKILL.md" "$BU_OUT/ytdlp-mine.md"
# a withdrawn skill, every file untouched.
mkdir -p "$BU_SK/old-skill/scripts"
printf '%s\n' "---" "name: old-skill" "description: A skill an older release shipped." "---" >"$BU_SK/old-skill/SKILL.md"
printf 'print("old skill")\n' >"$BU_SK/old-skill/scripts/run.py"
d_old_md="$(bu_sha "$BU_SK/old-skill/SKILL.md")"
d_old_py="$(bu_sha "$BU_SK/old-skill/scripts/run.py")"
# a withdrawn skill with one file the user changed (its record is not its bytes).
mkdir -p "$BU_SK/kept-skill/scripts"
printf '%s\n' "---" "name: kept-skill" "description: A withdrawn skill with an edited script." "---" >"$BU_SK/kept-skill/SKILL.md"
printf 'print("my edit")\n' >"$BU_SK/kept-skill/scripts/run.py"
d_kept_md="$(bu_sha "$BU_SK/kept-skill/SKILL.md")"
# a retired file in a skill that still ships, untouched.
printf 'notes an older flaredantic shipped\n' >"$BU_SK/flaredantic/old-notes.md"
d_flare_notes="$(bu_sha "$BU_SK/flaredantic/old-notes.md")"
# a retired file in a skill whose SKILL.md is the user's.
printf 'notes an older yt-dlp shipped\n' >"$BU_SK/yt-dlp/old-notes.md"
d_yt_notes="$(bu_sha "$BU_SK/yt-dlp/old-notes.md")"
# a symlink somebody else manages, pointing out of the config home.
mkdir -p "$BU_OUT/linked"
printf '%s\n' "---" "name: jina-reader" "description: MY-LINKED-JINA." "---" >"$BU_OUT/linked/SKILL.md"
cp "$BU_OUT/linked/SKILL.md" "$BU_OUT/linked-copy.md"
rm -f "$BU_SK/jina-reader/SKILL.md"
ln -s "$BU_OUT/linked/SKILL.md" "$BU_SK/jina-reader/SKILL.md"
bu_manifest "$BU_SKM" skills "$BU_VERSION" \
	"web-search/scripts/exa_search.py" "$d_exa" \
	"web-search/scripts/parallel_search.py" "$BU_ZEROS" \
	"yt-dlp/SKILL.md" "$BU_ZEROS" \
	"old-skill/SKILL.md" "$d_old_md" \
	"old-skill/scripts/run.py" "$d_old_py" \
	"kept-skill/SKILL.md" "$d_kept_md" \
	"kept-skill/scripts/run.py" "$BU_ZEROS" \
	"flaredantic/old-notes.md" "$d_flare_notes" \
	"yt-dlp/old-notes.md" "$d_yt_notes"
# the agents folder: an outdated, untouched explore.md.
printf '%s\n' "---" "name: explore" "description: An explore agent from an older release." "---" "old body" >"$BU_AG/explore.md"
bu_manifest "$BU_AGM" agents "$BU_VERSION" "explore.md" "$(bu_sha "$BU_AG/explore.md")"
note "the staged skills manifest"
cat "$BU_SKM"

# ---------------------------------------------------------------------------
# Launch 2 — the update.
# ---------------------------------------------------------------------------
# The agents' row is the last one, so waiting for it waits for them all.
bu_run "launch 2: the update" "$APP_BU" "Updated built-in agent: explore"
# A skill that lost a retired file but still ships was UPDATED, not removed:
# `Removed` must name only what is gone for good.
expect_has "$BU_ROWS" -F "Updated built-in skills: flaredantic, web-search" "launch 2 did not report flaredantic and web-search as updated (sorted, one row)"
expect_has "$BU_ROWS" -F "Removed built-in skill no longer shipped: old-skill" "launch 2 did not report old-skill as removed"
# Each kept file by where it is — the path to type to delete it — sorted.
expect_has "$BU_ROWS" -F "Kept your versions of built-in skill files ~/.alter-zero/skills/web-search/scripts/parallel_search.py, ~/.alter-zero/skills/yt-dlp/SKILL.md (delete one to get the built-in's)" "launch 2 did not name the user's kept parallel_search.py and yt-dlp/SKILL.md by their paths under ~/.alter-zero/skills"
# kept-skill holds an edit and yt-dlp's SKILL.md is the user's, so both units
# are the user's now: their retired files stay, and are named once, last.
expect_has "$BU_ROWS" -F "Kept built-in skill files no longer shipped, since the skills have your changes: ~/.alter-zero/skills/kept-skill/SKILL.md, ~/.alter-zero/skills/kept-skill/scripts/run.py, ~/.alter-zero/skills/yt-dlp/old-notes.md" "launch 2 did not name the retired files left in the units that are the user's"
expect_has "$BU_ROWS" -F "Updated built-in agent: explore" "launch 2 did not report the explore agent as updated"
# A symlink is somebody managing that file, and is left silently.
expect_lacks "$BU_SCREEN" -F "jina-reader" "a symlinked built-in was named on screen"

cmp -s "$SMOKE_ROOT/prompts/skills/web-search/scripts/exa_search.py" "$BU_SK/web-search/scripts/exa_search.py" ||
	fail "the outdated, untouched exa_search.py was not replaced by the shipped one"
cmp -s "$SMOKE_ROOT/prompts/agents/explore.md" "$BU_AG/explore.md" ||
	fail "the outdated, untouched explore.md was not replaced by the shipped one"
# The one promise the whole feature rests on: an edit is never overwritten.
cmp -s "$BU_OUT/ytdlp-mine.md" "$BU_SK/yt-dlp/SKILL.md" ||
	fail "the user's yt-dlp/SKILL.md was overwritten"
cmp -s "$BU_OUT/parallel-mine.py" "$BU_SK/web-search/scripts/parallel_search.py" ||
	fail "the user's web-search/scripts/parallel_search.py was overwritten"
# A kept SKILL.md may still name the file a release retired, so nothing is
# retired out of a skill that holds the user's version.
[ -f "$BU_SK/yt-dlp/old-notes.md" ] ||
	fail "a retired file was removed from a skill whose SKILL.md is the user's"
[ -e "$BU_SK/old-skill" ] && fail "the withdrawn, untouched old-skill folder was not removed"
# Removing only the untouched half would take the SKILL.md out from under the
# script the user changed.
[ -f "$BU_SK/kept-skill/SKILL.md" ] || fail "kept-skill/SKILL.md was removed though its unit holds the user's edit"
[ -f "$BU_SK/kept-skill/scripts/run.py" ] || fail "the user's kept-skill/scripts/run.py was removed"
[ -e "$BU_SK/flaredantic/old-notes.md" ] && fail "the retired, untouched flaredantic/old-notes.md was not removed"
for f in SKILL.md reference.md; do
	cmp -s "$SMOKE_ROOT/prompts/skills/flaredantic/$f" "$BU_SK/flaredantic/$f" ||
		fail "removing flaredantic/old-notes.md disturbed flaredantic/$f"
done
# A link is never written through, nor replaced by a regular file.
[ -L "$BU_SK/jina-reader/SKILL.md" ] || fail "jina-reader/SKILL.md is no longer a symlink"
cmp -s "$BU_OUT/linked-copy.md" "$BU_OUT/linked/SKILL.md" ||
	fail "the update wrote through the jina-reader symlink into its target"
note "the skills manifest after launch 2"
cat "$BU_SKM"
for gone in '"old-skill/' '"kept-skill/' '"flaredantic/old-notes.md"' '"yt-dlp/old-notes.md"'; do
	expect_file_lacks "$BU_SKM" -F "$gone" "the skills manifest still holds a record for $gone after launch 2"
done
expect_eq "$(bu_record "$BU_SKM" "web-search/scripts/exa_search.py")" \
	"$(bu_sha "$SMOKE_ROOT/prompts/skills/web-search/scripts/exa_search.py")" \
	"the record for the updated exa_search.py is not the shipped file's SHA-256"
# Settling a kept file against the shipped version is what makes it told once.
expect_eq "$(bu_record "$BU_SKM" "web-search/scripts/parallel_search.py")" \
	"$(bu_sha "$SMOKE_ROOT/prompts/skills/web-search/scripts/parallel_search.py")" \
	"the kept parallel_search.py was not settled against the shipped file's SHA-256"
# Every write goes through a temp file and a rename; a leftover is a write
# that died, and one left beside an agent could be read later.
leftovers="$(find "$BU_SK" "$BU_AG" -name '*.tmp' 2>/dev/null)"
[ -n "$leftovers" ] && fail "launch 2 left temp files behind: $leftovers"
cp "$BU_SKM" "$BU_OUT/skills-after-2.json"
cp "$BU_AGM" "$BU_OUT/agents-after-2.json"

# ---------------------------------------------------------------------------
# Launch 3 — settled. The kept file was reported once per built-in version,
# so a relaunch says nothing, and an ordinary launch writes nothing at all.
# ---------------------------------------------------------------------------
bu_run "launch 3: settled"
expect_lacks "$BU_SCREEN" -F "built-in" "a launch with nothing to settle still printed a built-in row (a kept file reported on every launch?)"
cmp -s "$BU_OUT/skills-after-2.json" "$BU_SKM" || fail "launch 3 rewrote the skills manifest though nothing changed"
cmp -s "$BU_OUT/agents-after-2.json" "$BU_AGM" || fail "launch 3 rewrote the agents manifest though nothing changed"
cmp -s "$BU_OUT/ytdlp-mine.md" "$BU_SK/yt-dlp/SKILL.md" || fail "launch 3 overwrote the user's yt-dlp/SKILL.md"

# ---------------------------------------------------------------------------
# A bare --resume settles nothing that is told once. It boots into the
# /resume picker, and a pick purge-rebuilds the screen, taking the rows queued
# under the banner with it — so, like a headless run, it brings files up to
# date but leaves a kept file's record for the next launch that can name it.
# Dismissed, the picker's return still shows the rows it queued.
# ---------------------------------------------------------------------------
# the user's general-purpose.md, under a record that is not the shipped digest
# (a release changed it since the folder settled), so it would be named...
printf '%s\n' "---" "name: general-purpose" "description: MY-OWN-GENERAL-PURPOSE." "---" "my body" >"$BU_AG/general-purpose.md"
cp "$BU_AG/general-purpose.md" "$BU_OUT/gp-mine.md"
# ...and an outdated, untouched explore.md, which is updated whatever the launch.
printf '%s\n' "---" "name: explore" "description: An explore agent from an older release." "---" "old body" >"$BU_AG/explore.md"
bu_manifest "$BU_AGM" agents "$BU_VERSION" \
	"explore.md" "$(bu_sha "$BU_AG/explore.md")" \
	"general-purpose.md" "$BU_ZEROS"
tmux kill-session -t "$S102B" 2>/dev/null
# The sessions folder is empty (no launch here has recorded a turn), so the
# picker opens on its empty state.
launch -w "R E S U M E" "$S102B" 120 30 "$APP_BU --resume" || fail "a bare --resume did not boot into the picker"
dump "launch 3b: a bare --resume's picker" "$(pane "$S102B")"
keys "$S102B" Escape
wait_settled 10 "$S102B" -F "Updated built-in agent: explore" >/dev/null ||
	fail "the dismissed picker's return never showed 'Updated built-in agent: explore'"
BU_SCREEN="$(pane "$S102B" -S -100)"
dump "launch 3b: the picker dismissed" "$BU_SCREEN"
expect_lacks "$BU_SCREEN" -F "Kept your version" "a bare --resume named a kept file, which a pick would have erased unread"
submit "$S102B" "/quit"
wait_gone 10 "$S102B" || fail "the app did not quit after /quit (launch 3b)"
tmux kill-session -t "$S102B" 2>/dev/null
note "the agents manifest after a bare --resume"
cat "$BU_AGM"
expect_eq "$(bu_record "$BU_AGM" "general-purpose.md")" "$BU_ZEROS" \
	"a bare --resume settled the kept general-purpose.md (its record moved, so no launch would name it)"
cmp -s "$BU_OUT/gp-mine.md" "$BU_AG/general-purpose.md" || fail "a bare --resume overwrote the user's general-purpose.md"
cmp -s "$SMOKE_ROOT/prompts/agents/explore.md" "$BU_AG/explore.md" ||
	fail "a bare --resume did not update the outdated, untouched explore.md"

# The next launch that can show the rows names the kept file — and only it.
bu_run "launch 3c: the next launch names what the bare --resume kept" "$APP_BU" \
	"general-purpose.md (delete it to get the built-in's)"
expect_has "$BU_ROWS" -F "Kept your version of built-in agent file ~/.alter-zero/agents/general-purpose.md (delete it to get the built-in's)" "the launch after a bare --resume did not name the kept general-purpose.md"
expect_lacks "$BU_SCREEN" -F "Updated built-in agent" "the launch after a bare --resume reported explore's update a second time"
expect_eq "$(bu_record "$BU_AGM" "general-purpose.md")" \
	"$(bu_sha "$SMOKE_ROOT/prompts/agents/general-purpose.md")" \
	"the launch that named the kept general-purpose.md did not settle it against the shipped file's SHA-256"
cmp -s "$BU_OUT/gp-mine.md" "$BU_AG/general-purpose.md" || fail "launch 3c overwrote the user's general-purpose.md"

# ---------------------------------------------------------------------------
# A folder a newer app settled is left alone: two versions sharing a config
# home would otherwise rewrite each other's files on every launch, and an
# older one would re-seed what a newer one retired.
# ---------------------------------------------------------------------------
printf '# an exa_search.py from an older release\nprint("old")\n' >"$BU_SK/web-search/scripts/exa_search.py"
cp "$BU_SK/web-search/scripts/exa_search.py" "$BU_OUT/exa-old.py"
rm -f "$BU_SK/skill-creator/reference.md"
# Same records as launch 2 left, but the outdated script's is its old bytes
# (so this launch WOULD update it, were it in charge) and the version is newer.
sed -e 's/"version": *"[^"]*"/"version": "999.0.0"/' \
	-e "s|\(\"web-search/scripts/exa_search.py\": *\"\)[0-9a-f]*\"|\1$d_exa\"|" \
	"$BU_OUT/skills-after-2.json" >"$BU_SKM"
cp "$BU_SKM" "$BU_OUT/skills-newer.json"
grep -qF '"version": "999.0.0"' "$BU_SKM" || fail "staging the newer manifest failed (no 999.0.0 version)"
expect_eq "$(bu_record "$BU_SKM" "web-search/scripts/exa_search.py")" "$d_exa" "staging the newer manifest failed (exa_search.py record)"
bu_run "launch 4: a folder a newer app settled"
expect_lacks "$BU_SCREEN" -F "built-in" "a folder a newer app settled was touched (a built-in row was printed)"
cmp -s "$BU_OUT/exa-old.py" "$BU_SK/web-search/scripts/exa_search.py" ||
	fail "an older binary updated a file in a folder a newer app settled"
[ -e "$BU_SK/skill-creator/reference.md" ] &&
	fail "an older binary re-seeded a file in a folder a newer app settled"
cmp -s "$BU_OUT/skills-newer.json" "$BU_SKM" || fail "an older binary rewrote a newer app's manifest"

# ---------------------------------------------------------------------------
# An override agents folder is only seeded: a directory a team commits its
# agents to must not be rewritten by whoever runs the newest build.
# ---------------------------------------------------------------------------
BU_OVR="$(mktemp -d)"
BU_CFG2="$(mktemp -d)"
printf '%s\n' "---" "name: explore" "description: OUR-TEAM-EXPLORE." "---" "team body" >"$BU_OVR/explore.md"
cp "$BU_OVR/explore.md" "$BU_OUT/team-explore.md"
bu_run "launch 5: an ALTER_ZERO_AGENTS_DIR override" "$(bu_app "$BU_CFG2" "ALTER_ZERO_AGENTS_DIR=$BU_OVR")"
expect_lacks "$BU_SCREEN" -F "built-in" "a launch over an override agents folder printed a built-in row"
cmp -s "$BU_OUT/team-explore.md" "$BU_OVR/explore.md" || fail "the override folder's own explore.md was rewritten"
[ -f "$BU_OVR/general-purpose.md" ] || fail "the override folder was not seeded with general-purpose.md (the agent tool's default type)"
[ -e "$BU_OVR/.builtin.json" ] && fail "a manifest was left in an ALTER_ZERO_AGENTS_DIR override"

rm -rf "$BU_CFG" "$BU_CFG2" "$BU_HOME" "$BU_OUT" "$BU_OVR"
echo "==== Phase 102b: built-in files follow the release, keep the user's version, retire only untouched units, name what stays, settle nothing a bare --resume cannot show, defer to a newer app, and only seed an override ===="
