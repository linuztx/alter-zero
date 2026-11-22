#!/usr/bin/env bash
# Phase 166 — a release reaches the built-ins an older build seeded

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# docs/builtin-updates.md: an install an older build made holds built-in files
# this release changed. The config home is planted the way that build left
# it, three times, for the three ways a release can first be launched.
#
# Interactive: the unedited explore.md is replaced with this release's version
# — prompts/shipped.sha256 lists the older bytes, so the launch knows nobody
# edited them — and a toast says so. The edited skill-creator/SKILL.md is
# kept, this release's version written beside it as SKILL.md.new, and an
# `Edited built-ins` card committed under the banner — once: a second launch,
# the .new deleted after merging, is quiet and writes nothing back.
#
# Headless first (`-p`): the untouched file is brought up to date all the
# same, but the offer, made once, waits for a launch with a screen — the
# interactive launch after it makes it.
#
# A bare `--resume` first: the card waits behind the picker, whose pick
# rebuilds the screen, and lands once the conversation is back on it.
#
# And an ALTER_ZERO_AGENTS_DIR override, somebody else's directory, is only
# seeded: nothing in it is rewritten, and no record is left there.
S166="${S}_builtinupdate"
BU_SESSIONS="$(mktemp -d)"
# What the older build recorded for the edited file: the version it started
# from, a development version of skill-creator/SKILL.md the history lists.
BU_OLD_BASE=d6b0ec10ef9db48a98e16e572bfa017437bc4470d535fd08cc4498439ee7695a

# plant_older_install HOME — the config home under HOME as the older build
# left it.
plant_older_install() {
	local cfg="$1/.alter-zero"
	mkdir -p "$cfg/agents" "$cfg/skills/skill-creator"
	# explore.md byte for byte as a development build of 2026-08-27 seeded
	# it — one of the versions prompts/shipped.sha256 lists for it.
	cat >"$cfg/agents/explore.md" <<'EOF'
---
name: explore
description: Read-only search agent for broad fan-out searches. Use it when answering means sweeping many files, directories or naming conventions and you want the conclusion, not the file dumps — it locates code and reports what it found, and never edits.
# `model:` — a model id to run this type on, or `inherit` for the session's
#   own. See general-purpose.md for the full grammar of every key here.
model: inherit
# `tools:` — the read-only set: no `Write`, no `Edit`. Drop the key entirely to
#   offer every tool instead.
tools: Bash, Read, Skill, mcp__*
---

You are a read-only search agent. Find what the task asks about and report it —
never modify a file, and never suggest that you did.

- Search widely before you read deeply: `rg`/`grep`/`find` to locate candidates,
  then `read` only the parts that decide the question.
- Report file paths with line numbers (`src/app/mod.rs:412`), quote only the
  lines that matter, and say plainly when something does not exist.
- Your final message is the whole result the caller sees: lead with the answer,
  then the evidence. No preamble, no "I will now…".
EOF
	cp "$SMOKE_ROOT/prompts/agents/general-purpose.md" "$cfg/agents/general-purpose.md"
	# skill-creator's SKILL.md as the user rewrote it.
	printf '%s\n' "---" "name: skill-creator" "description: MINE-NOT-YOURS." "---" \
		>"$cfg/skills/skill-creator/SKILL.md"
	printf '{"files": {"skill-creator/SKILL.md": {"base": "%s"}}}\n' "$BU_OLD_BASE" \
		>"$cfg/skills/.alter-zero-builtins.json"
}

# bu_env HOME — the environment every launch below runs in, on that home.
bu_env() {
	printf '%s' "env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_UPDATE_CHECK=0 ALTER_ZERO_TIPS=0 ALTER_ZERO_MASCOT_ANIMATION=0 ALTER_ZERO_PROJECT_CONFIG=0 HOME=$1 ALTER_ZERO_CONFIG_DIR=$1/.alter-zero ALTER_ZERO_SESSIONS_DIR=$BU_SESSIONS ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS"
}

bu_quit() {
	submit "$S166" "/quit"
	sleep 0.6
	tmux kill-session -t "$S166" 2>/dev/null
}

# ---- interactive first ----
BU_A="$(mktemp -d)"
BU_CFG="$BU_A/.alter-zero"
plant_older_install "$BU_A"
tmux kill-session -t "$S166" 2>/dev/null
launch "$S166" 100 30 "$(bu_env "$BU_A") $BIN_ABS"
bu_first="$(wait_pane 5 "$S166" -F "Updated the built-in explore agent")"
dump "the first launch of the newer release" "$bu_first"
expect_has "$bu_first" -F "Updated the built-in explore agent" \
	"no toast for the agent brought up to date"
expect_has "$bu_first" -F "Edited built-ins" "no card for the edited built-in"
expect_has "$bu_first" -F "~/.alter-zero/skills/skill-creator/SKILL.md.new" \
	"the card does not name the .new it wrote"
bu_quit

if ! cmp -s "$SMOKE_ROOT/prompts/agents/explore.md" "$BU_CFG/agents/explore.md"; then
	fail "the unedited explore.md an older build wrote was not replaced with this release's"
fi
if ! cmp -s "$SMOKE_ROOT/prompts/agents/general-purpose.md" "$BU_CFG/agents/general-purpose.md"; then
	fail "general-purpose.md, already this release's, was changed"
fi
if ! grep -q "MINE-NOT-YOURS" "$BU_CFG/skills/skill-creator/SKILL.md" 2>/dev/null; then
	fail "the launch wrote over an edited SKILL.md"
fi
if ! cmp -s "$SMOKE_ROOT/prompts/skills/skill-creator/SKILL.md" "$BU_CFG/skills/skill-creator/SKILL.md.new"; then
	fail "this release's SKILL.md was not written beside the edited one"
fi
# Every other built-in was missing, so it was seeded as before.
if ! cmp -s "$SMOKE_ROOT/prompts/skills/skill-creator/reference.md" "$BU_CFG/skills/skill-creator/reference.md"; then
	fail "a missing built-in file was not seeded beside the edited one"
fi
for record in "$BU_CFG/skills/.alter-zero-builtins.json" "$BU_CFG/agents/.alter-zero-builtins.json"; do
	expect_file_has "$record" -F '"base"' "the launch wrote no record at $record"
done
expect_file_has "$BU_CFG/skills/.alter-zero-builtins.json" -F '"offered"' \
	"the record does not say the .new is the launch's own"

# The user merges and deletes the .new: the next launch has nothing to say,
# and does not write it again.
rm -f "$BU_CFG/skills/skill-creator/SKILL.md.new"
launch "$S166" 100 30 "$(bu_env "$BU_A") $BIN_ABS"
bu_second="$(pane "$S166")"
dump "the second launch" "$bu_second"
expect_lacks "$bu_second" -F "Edited built-ins" "the card came back on the second launch"
expect_lacks "$bu_second" -F "Updated the built-in" "the second launch announced an update again"
bu_quit
if [ -e "$BU_CFG/skills/skill-creator/SKILL.md.new" ]; then
	fail "the .new the user deleted was written again"
fi
if ! grep -q "MINE-NOT-YOURS" "$BU_CFG/skills/skill-creator/SKILL.md" 2>/dev/null; then
	fail "the second launch wrote over the edited SKILL.md"
fi

# ---- headless first ----
BU_B="$(mktemp -d)"
plant_older_install "$BU_B"
bu_headless="$($(bu_env "$BU_B") ALTER_ZERO_DUMMY=1 ALTER_ZERO_STARTUP_DELAY_MS=0 ALTER_ZERO_CHUNK_DELAY_MS=0 \
	"$BIN_ABS" -p --verbose "$USER_MSG" 2>&1 >/dev/null </dev/null)"
dump "a headless run first (its stderr)" "$bu_headless"
expect_has "$bu_headless" -F "Updated the built-in explore agent" \
	"the headless run did not bring the untouched agent up to date"
if [ -e "$BU_B/.alter-zero/skills/skill-creator/SKILL.md.new" ]; then
	fail "a headless run wrote the offer it has no card to show"
fi
launch "$S166" 100 30 "$(bu_env "$BU_B") $BIN_ABS"
bu_after_headless="$(wait_pane 5 "$S166" -F "Edited built-ins")"
dump "the interactive launch after it" "$bu_after_headless"
expect_has "$bu_after_headless" -F "Edited built-ins" \
	"the launch after a headless run did not make the offer it left"
bu_quit
if ! cmp -s "$SMOKE_ROOT/prompts/skills/skill-creator/SKILL.md" "$BU_B/.alter-zero/skills/skill-creator/SKILL.md.new"; then
	fail "the interactive launch after a headless run did not write the .new"
fi

# ---- a bare --resume first ----
# A conversation to pick, recorded by a launch with nothing to update.
BU_REC="$(mktemp -d)"
launch "$S166" 100 30 "$(bu_env "$BU_REC") $BIN_ABS"
submit "$S166" "$USER_MSG"
wait_summaries 20.1 "$S166" 1 -S -40 >/dev/null
tmux send-keys -t "$S166" C-c
sleep 0.4
tmux kill-session -t "$S166" 2>/dev/null
BU_C="$(mktemp -d)"
plant_older_install "$BU_C"
launch -w "R E S U M E" "$S166" 100 30 "$(bu_env "$BU_C") $BIN_ABS --resume"
tmux send-keys -t "$S166" Enter # pick it: its load rebuilds the screen
bu_resumed="$(wait_pane 8 "$S166" -S -80 -- -F "Edited built-ins")"
dump "a bare --resume, the conversation picked" "$bu_resumed"
expect_has "$bu_resumed" -F "Edited built-ins" "the card was lost behind the --resume picker"
expect_has "$bu_resumed" -F "❯ $USER_MSG" "the picked conversation did not load"
bu_quit

# ---- an ALTER_ZERO_AGENTS_DIR override ----
# Somebody else's directory, a team's committed agents say: what is missing
# is written, as the seed always did, and nothing there is rewritten.
BU_D="$(mktemp -d)"
plant_older_install "$BU_D"
BU_TEAM="$BU_D/team-agents"
mv "$BU_D/.alter-zero/agents" "$BU_TEAM"
rm -f "$BU_TEAM/general-purpose.md"
cp "$BU_TEAM/explore.md" "$BU_D/explore.before"
$(bu_env "$BU_D") ALTER_ZERO_AGENTS_DIR="$BU_TEAM" ALTER_ZERO_DUMMY=1 ALTER_ZERO_STARTUP_DELAY_MS=0 \
	ALTER_ZERO_CHUNK_DELAY_MS=0 "$BIN_ABS" -p "$USER_MSG" >/dev/null 2>&1 </dev/null
if ! cmp -s "$BU_D/explore.before" "$BU_TEAM/explore.md"; then
	fail "an untouched built-in in an ALTER_ZERO_AGENTS_DIR override was rewritten"
fi
if ! cmp -s "$SMOKE_ROOT/prompts/agents/general-purpose.md" "$BU_TEAM/general-purpose.md"; then
	fail "a built-in missing from an ALTER_ZERO_AGENTS_DIR override was not seeded"
fi
if [ -e "$BU_TEAM/.alter-zero-builtins.json" ]; then
	fail "the sync left a record in an ALTER_ZERO_AGENTS_DIR override"
fi

rm -rf "$BU_A" "$BU_B" "$BU_C" "$BU_D" "$BU_REC" "$BU_SESSIONS"
note "an unedited built-in is replaced, an edited one kept with this release's version beside it, once — and the offer waits for a screen; an override is only seeded"
