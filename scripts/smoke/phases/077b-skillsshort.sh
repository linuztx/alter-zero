#!/usr/bin/env bash
# Phase 77b — the `/skills` menu on a short terminal

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The `/skills` menu on a terminal too short for its whole page
# (docs/skills.md). Bottom-anchoring the whole page there flowed the search
# line and the top of the list into scrollback, so the row Enter would toggle
# could be off screen. A short terminal now gets the compact page: the
# description and the counter yield first, then the header, leaving the
# search line, three skill rows between the `↑`/`↓` overflow slots, and the
# hint. Drive it against six planted skills: open the menu on 14 rows (the
# compact page, its header kept), walk ↓ into the middle of the list (both
# slots name what they hide), resize to 12 rows (the header yields too),
# toggle the highlighted skill there, then resize to 30 rows (the whole page,
# its description and counter back).
S77B="${S}_skillsshort"
SKS_CFG="$(mktemp -d)"
SKS_DIR="$(mktemp -d)"
for name in alpha bravo charlie delta echo foxtrot; do
	mkdir -p "$SKS_DIR/$name-skill"
	cat >"$SKS_DIR/$name-skill/SKILL.md" <<SKILL
---
name: $name-skill
description: The $name demo skill, for the smoke suite only
---
Body.
SKILL
done
APP_SKS="env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$SKS_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_SKILLS_DIR=$SKS_DIR ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
launch "$S77B" 90 14 "$APP_SKS"
tmux send-keys -t "$S77B" -l "/skills"
sleep 0.3
tmux send-keys -t "$S77B" Enter
sleep 0.6
sks_open="$(tmux capture-pane -t "$S77B" -p)"
echo "==== Phase 77b: the menu on a 14-row terminal (the compact page, header kept) ===="
printf '%s\n' "$sks_open"
expect_has "$sks_open" -E "^  Skills *$" "the compact page lost its header's title"
expect_has "$sks_open" -F "6 skills · 6 enabled" "the compact page lost its header's count"
expect_has "$sks_open" -E '^  ❯ *$' "the compact page lost its search line"
expect_has "$sks_open" -E "→ alpha-skill +✔ enabled" "the highlighted skill is not on screen"
expect_has "$sks_open" -F "↓ 3 more below" "the rows under the window are not named"
expect_has "$sks_open" -F "Type to search · Enter/Space to enable/disable · Esc to cancel" "the compact page lost its hint"
expect_lacks "$sks_open" -F "The alpha demo skill" "the compact page still shows the description"
expect_lacks "$sks_open" -F "(1/6)" "the compact page still shows the counter"
if [ "$(printf '%s\n' "$sks_open" | grep -cE '(✔|◯) (enabled|disabled)')" -ne 3 ]; then
	fail "the compact page does not show exactly three skill rows"
fi
# ↓↓↓ walks into the middle of the list: the window rides the selection and
# both slots name what they hide.
tmux send-keys -t "$S77B" Down Down Down
sleep 0.4
sks_middle="$(tmux capture-pane -t "$S77B" -p)"
echo "==== Phase 77b: ↓↓↓ — the window rides the selection ===="
printf '%s\n' "$sks_middle"
expect_has "$sks_middle" -F "↑ 2 more above" "the rows over the window are not named"
expect_has "$sks_middle" -E "→ delta-skill +✔ enabled" "the selection did not follow ↓"
expect_has "$sks_middle" -F "↓ 1 more below" "the rows under the window are not named after the walk"
# 12 rows: the header yields too; the search line and the choices stay.
tmux resize-window -t "$S77B" -x 90 -y 12
sleep 0.6
sks_shorter="$(tmux capture-pane -t "$S77B" -p)"
echo "==== Phase 77b: a 12-row terminal — the header yields too ===="
printf '%s\n' "$sks_shorter"
expect_lacks "$sks_shorter" -E "^  Skills *$" "the header did not yield on 12 rows"
expect_has "$sks_shorter" -E '^  ❯ *$' "the search line did not survive 12 rows"
expect_has "$sks_shorter" -E "→ delta-skill +✔ enabled" "the highlighted skill did not survive 12 rows"
expect_has "$sks_shorter" -F "Type to search" "the hint did not survive 12 rows"
# Enter toggles the highlighted row on the compact page.
tmux send-keys -t "$S77B" Enter
sleep 0.5
sks_toggled="$(tmux capture-pane -t "$S77B" -p)"
echo "==== Phase 77b: Enter on the compact page ===="
printf '%s\n' "$sks_toggled"
expect_has "$sks_toggled" -E "→ delta-skill +◯ disabled" "Enter did not toggle the highlighted row on the compact page"
# 30 rows: the whole page again, its description and counter back.
tmux resize-window -t "$S77B" -x 90 -y 30
sleep 0.6
sks_tall="$(tmux capture-pane -t "$S77B" -p)"
echo "==== Phase 77b: 30 rows — the whole page again ===="
printf '%s\n' "$sks_tall"
for expect in "(4/6)" "The delta demo skill, for the smoke suite only" "6 skills · 5 enabled" \
	"alpha-skill" "foxtrot-skill"; do
	expect_has "$sks_tall" -F -- "$expect" "the whole page is missing '$expect'"
done
expect_lacks "$sks_tall" -F "more below" "the whole page still names hidden rows"
tmux kill-session -t "$S77B" 2>/dev/null
rm -rf "$SKS_CFG" "$SKS_DIR"
