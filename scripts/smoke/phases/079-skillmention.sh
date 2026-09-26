#!/usr/bin/env bash
# Phase 79 — the `$` SKILL PICKER

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the `$` SKILL PICKER (docs/skill-mentions.md). Codex's skill
# mentions as a fourth band: typing `$` in the composer lists the discovered
# skills (name + description columns), typing narrows on the name, Tab
# completes the mention IN PLACE — `$alpha-skill ` stays in the draft, sigil
# kept — and submitting a message that carries a mention plays the skill load
# (offline: the dummy's skills demo; live: the Skill tool description makes
# the model call the `skill` tool).
S79="${S}_skillmention"
SM_CFG="$(mktemp -d)"
SM_DIR="$(mktemp -d)"
mkdir -p "$SM_DIR/alpha-skill" "$SM_DIR/beta-skill"
cat >"$SM_DIR/alpha-skill/SKILL.md" <<'SKILL'
---
name: alpha-skill
description: The first demo skill, for the smoke suite only
---
Alpha body.
SKILL
cat >"$SM_DIR/beta-skill/SKILL.md" <<'SKILL'
---
name: beta-skill
description: The second demo skill, for the smoke suite only
---
Beta body.
SKILL
APP_SM="env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$SM_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_SKILLS_DIR=$SM_DIR ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
launch "$S79" 90 32 "$APP_SM"
# A bare `$` lists every discovered skill with its description.
tmux send-keys -t "$S79" -l '$'
sleep 0.4
mention_all="$(tmux capture-pane -t "$S79" -p)"
echo "==== Phase 79: the band on a bare \$ ===="
printf '%s\n' "$mention_all"
for expect in "alpha-skill" "The first demo skill" "beta-skill" "The second demo skill"; do
	expect_has "$mention_all" -F "$expect" "the \$ band is missing '$expect'"
done
# Typing narrows on the name…
tmux send-keys -t "$S79" -l "alp"
sleep 0.4
mention_narrow="$(tmux capture-pane -t "$S79" -p)"
echo "==== Phase 79: the band narrowed to \$alp ===="
printf '%s\n' "$mention_narrow"
expect_has "$mention_narrow" -F "alpha-skill" "the narrowed band lost the match"
expect_lacks "$mention_narrow" -F "beta-skill" "'alp' still lists beta-skill"
# …and Tab completes the mention in place, sigil kept, ready to keep typing.
tmux send-keys -t "$S79" Tab
sleep 0.4
mention_done="$(tmux capture-pane -t "$S79" -p)"
echo "==== Phase 79: the completed mention ===="
printf '%s\n' "$mention_done"
expect_has "$mention_done" -F '❯ $alpha-skill' "Tab did not complete the mention into the composer"
expect_has "$mention_done" -F "dummy_model_name" "the footer did not come back after the band closed"
# Submitting the mention plays the skill load — the dummy's skills demo
# answers it (the cell, never the body), like a live model calling `skill`.
submit "$S79" "load it please"
wait_for 24 "$S79" -S -80 -- -F "$SUMMARY_TURN1"
mention_turn="$(tmux capture-pane -t "$S79" -p -S -80)"
echo "==== Phase 79: the submitted mention's turn ===="
printf '%s\n' "$mention_turn"
expect_has "$mention_turn" -E "● Skill\(dataviz\)" "the submitted mention played no skill load"
expect_has "$mention_turn" -F "Successfully loaded skill" "the skill cell is missing its loaded row"
tmux kill-session -t "$S79" 2>/dev/null
rm -rf "$SM_CFG" "$SM_DIR"
