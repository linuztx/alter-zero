#!/usr/bin/env bash
# Phase 77 — the `/skills` MENU

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the `/skills` MENU (docs/skills.md). The `/settings` menu's
# twin — same frame, same grammar — over the discovered skills, each one
# enable/disable-able. Drive it end to end against a REAL skill directory
# (the rest of the suite runs skill-free, so this phase points
# ALTER_ZERO_SKILLS_DIR at one it plants): open it from the palette, check the
# rows and the value column, disable one, confirm the toast and the flipped
# value, and prove it PERSISTED — a second process against the same config
# home must open the menu already showing it off.
S77="${S}_skillsmenu"
SK_CFG="$(mktemp -d)"
SK_DIR="$(mktemp -d)"
mkdir -p "$SK_DIR/alpha-skill" "$SK_DIR/beta-skill"
cat >"$SK_DIR/alpha-skill/SKILL.md" <<'SKILL'
---
name: alpha-skill
description: The first demo skill, for the smoke suite only
---
Alpha body.
SKILL
cat >"$SK_DIR/beta-skill/SKILL.md" <<'SKILL'
---
name: beta-skill
description: The second demo skill, for the smoke suite only
---
Beta body.
SKILL
APP_SK="env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$SK_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_SKILLS_DIR=$SK_DIR ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
launch "$S77" 90 32 "$APP_SK"
# The palette lists it (a bare `/skil` token filters to it).
tmux send-keys -t "$S77" -l "/skil"
sleep 0.4
skills_palette="$(tmux capture-pane -t "$S77" -p)"
echo "==== Phase 77: the palette filtered to /skills ===="
printf '%s\n' "$skills_palette"
expect_has "$skills_palette" -F "Browse skills and enable or disable each one" "/skills is missing from the slash-command palette"
tmux send-keys -t "$S77" Enter
sleep 0.6
skills_open="$(tmux capture-pane -t "$S77" -p)"
echo "==== Phase 77: the skills menu open ===="
printf '%s\n' "$skills_open"
for expect in "alpha-skill" "beta-skill" "(1/2)" \
	"The first demo skill, for the smoke suite only" \
	"Type to search · Enter/Space to enable/disable · Esc to cancel"; do
	expect_has "$skills_open" -F "$expect" "the skills menu is missing '$expect'"
done
expect_has "$skills_open" -E "→ alpha-skill +enabled" "the first row is not marked with its value in the value column"
# Enter disables the highlighted skill: the row flips and a toast confirms.
tmux send-keys -t "$S77" Enter
sleep 0.6
skills_off="$(tmux capture-pane -t "$S77" -p)"
echo "==== Phase 77: after Enter disabled the highlighted skill ===="
printf '%s\n' "$skills_off"
expect_has "$skills_off" -E "alpha-skill +disabled" "Enter did not flip alpha-skill to disabled"
expect_has "$skills_off" -F "Skill alpha-skill: disabled" "the toggle was not confirmed with a toast above the box"
expect_has "$skills_off" -E "beta-skill +enabled" "the toggle moved a sibling row's value too"
# The menu stays open across toggles; Esc closes back to the composer.
tmux send-keys -t "$S77" Escape
sleep 0.5
skills_closed="$(tmux capture-pane -t "$S77" -p)"
expect_lacks "$skills_closed" -F "Enter/Space to enable/disable" "Esc did not close the skills menu"
expect_has "$skills_closed" -F "dummy_model_name" "the composer (and its footer) did not come back"
tmux kill-session -t "$S77" 2>/dev/null
echo "==== Phase 77: skills.json on disk ===="
cat "$SK_CFG/skills.json" 2>/dev/null || echo "(no file)"
if ! grep -qF "alpha-skill" "$SK_CFG/skills.json" 2>/dev/null; then
	fail "the disabled skill was not persisted to skills.json"
fi
# A SECOND process against the same config home opens the menu already off.
launch "$S77" 90 32 "$APP_SK"
tmux send-keys -t "$S77" -l "/skills"
sleep 0.3
tmux send-keys -t "$S77" Enter
sleep 0.6
skills_reopened="$(tmux capture-pane -t "$S77" -p)"
echo "==== Phase 77: a fresh process shows the persisted state ===="
printf '%s\n' "$skills_reopened"
expect_has "$skills_reopened" -E "alpha-skill +disabled" "the disabled skill did not survive a restart"
expect_has "$skills_reopened" -E "beta-skill +enabled" "the restart lost the ENABLED skill's state"
tmux kill-session -t "$S77" 2>/dev/null
rm -rf "$SK_CFG" "$SK_DIR"
