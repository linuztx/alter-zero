#!/usr/bin/env bash
# Phase 79 — the `$` SKILL PICKER

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the `$` SKILL PICKER (docs/input/skill-mentions.md). Codex's skill
# mentions as a fourth band: typing `$` in the composer lists the discovered
# skills (name + description columns), five at a time over a `(n/total)`
# counter with the highlight riding the middle row as ↑/↓ scroll (the `/`
# palette's window), typing narrows on the name, Tab completes the mention IN PLACE —
# `$alpha-skill ` stays in the draft, sigil kept — and submitting a message
# that carries a mention plays the skill load (offline: the dummy's skills
# demo; live: the Skill tool description makes the model call the `skill`
# tool).
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
# Five more, so the list outgrows the band's five rows. Discovery sorts by
# folder name, so alpha-skill and beta-skill open the list and golf-skill
# closes it.
for filler in charlie delta echo foxtrot golf; do
	mkdir -p "$SM_DIR/$filler-skill"
	printf -- '---\nname: %s-skill\ndescription: Fills the band past its window, for the smoke suite only\n---\nFiller body.\n' \
		"$filler" >"$SM_DIR/$filler-skill/SKILL.md"
done
# A skill row: the marker or its inset, a name, then the description column.
skill_rows() { printf '%s\n' "$1" | grep -cE '^(→ |  )[a-z]+-skill +[A-Z]' || true; }
# The band's counter as `n/total`, empty when there is none.
skill_counter() { printf '%s\n' "$1" | sed -nE 's/^  \(([0-9]+)\/([0-9]+)\)$/\1\/\2/p' | tail -1; }
APP_SM="env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$SM_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_SKILLS_DIR=$SM_DIR ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
launch "$S79" 90 32 "$APP_SM"
# A bare `$` lists the discovered skills with their descriptions.
tmux send-keys -t "$S79" -l '$'
sleep 0.4
mention_all="$(tmux capture-pane -t "$S79" -p)"
echo "==== Phase 79: the band on a bare \$ ===="
printf '%s\n' "$mention_all"
for expect in "alpha-skill" "The first demo skill" "beta-skill" "The second demo skill"; do
	expect_has "$mention_all" -F "$expect" "the \$ band is missing '$expect'"
done
# Seven skills, five rows: the first five over the counter, the last below
# the window.
if [ "$(skill_rows "$mention_all")" -ne 5 ]; then
	fail "the \$ band lists $(skill_rows "$mention_all") skills, not the five it is capped at"
fi
expect_lacks "$mention_all" -F "golf-skill" "the \$ band shows its seventh skill in its first window"
expect_eq "$(skill_counter "$mention_all")" "1/7" "the \$ band's counter on a bare \$"
# ↑ from the first skill wraps to the last: the window follows the
# highlight down, the first skills scroll out, and the counter reads the
# last of them all.
tmux send-keys -t "$S79" Up
sleep 0.4
mention_last="$(tmux capture-pane -t "$S79" -p)"
echo "==== Phase 79: the band wrapped to its last skill ===="
printf '%s\n' "$mention_last"
expect_has "$mention_last" -E '^→ golf-skill ' "↑ from the first skill did not wrap the highlight to the last"
expect_lacks "$mention_last" -F "alpha-skill" "the \$ band's window did not follow the highlight to the last skill"
expect_eq "$(skill_counter "$mention_last")" "7/7" "the \$ band's counter after wrapping to the last skill"
# ↓ wraps back to the first skill, and three more steps put the highlight on
# the middle row, the `/` palette's window: the list scrolls under it, so
# alpha-skill leaves the top while foxtrot-skill comes in below. (The
# edge-pinned window kept alpha-skill on screen and the arrow on the fourth
# row.)
tmux send-keys -t "$S79" Down
wait_for 3 "$S79" -E '^  \(1/7\)$' || fail "↓ from the last skill did not wrap the \$ band back to the first"
tmux send-keys -t "$S79" Down Down Down
mention_mid="$(wait_pane 3 "$S79" -E '^  \(4/7\)$')"
echo "==== Phase 79: the band with its highlight centered ===="
printf '%s\n' "$mention_mid"
mid_rows="$(printf '%s\n' "$mention_mid" | grep -E '^(→ |  )[a-z]+-skill +[A-Z]' || true)"
expect_has "$(printf '%s\n' "$mid_rows" | sed -n 3p)" -E '^→ delta-skill ' "the \$ band's highlight is not on its middle row"
expect_has "$(printf '%s\n' "$mid_rows" | sed -n 1p)" -E '^  beta-skill ' "the \$ band's window did not scroll to keep its highlight centered"
expect_lacks "$mention_mid" -F "alpha-skill" "the \$ band kept its first skill in view with the highlight past the middle"
expect_has "$mention_mid" -E '^  foxtrot-skill ' "the \$ band did not scroll the next skill in under its centered highlight"
# Typing narrows on the name…
tmux send-keys -t "$S79" -l "alp"
sleep 0.4
mention_narrow="$(tmux capture-pane -t "$S79" -p)"
echo "==== Phase 79: the band narrowed to \$alp ===="
printf '%s\n' "$mention_narrow"
expect_has "$mention_narrow" -F "alpha-skill" "the narrowed band lost the match"
expect_lacks "$mention_narrow" -F "beta-skill" "'alp' still lists beta-skill"
expect_eq "$(skill_counter "$mention_narrow")" "1/1" "the narrowed \$ band's counter"
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
wait_for 24 "$S79" -S -80 -- -E "$SUMMARY_RE"
mention_turn="$(tmux capture-pane -t "$S79" -p -S -80)"
echo "==== Phase 79: the submitted mention's turn ===="
printf '%s\n' "$mention_turn"
expect_has "$mention_turn" -E "● Skill\(dataviz\)" "the submitted mention played no skill load"
expect_has "$mention_turn" -F "Successfully loaded skill" "the skill cell is missing its loaded row"
tmux kill-session -t "$S79" 2>/dev/null
rm -rf "$SM_CFG" "$SM_DIR"
