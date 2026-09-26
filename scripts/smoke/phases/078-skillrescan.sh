#!/usr/bin/env bash
# Phase 78 — skills are RE-WALKED every turn

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# skills are RE-WALKED every turn (docs/skills.md). Discovery
# used to run once at startup, so a skill added mid-session — or one the agent
# had just written for you — stayed invisible until a restart. Boot against an
# EMPTY skills dir, plant a SKILL.md while the session is running, take one
# turn, and both surfaces must have it: the `<system-reminder>` listing in the
# derived context (Ctrl+D) and the `/skills` menu. The listing is the sharp
# one — it rides `skills_offered`, which reads the Skills row's availability,
# so a rescan that re-rendered it BEFORE re-deriving availability shipped the
# listing a turn late (it looked exactly like the rescan not working).
S78="${S}_skillrescan"
RS_CFG="$(mktemp -d)"
RS_DIR="$(mktemp -d)"
APP_RS="env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$RS_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_SKILLS_DIR=$RS_DIR ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
launch "$S78" 90 32 "$APP_RS"
# Nothing on disk yet: the menu opens on the where-would-one-go placeholder.
tmux send-keys -t "$S78" -l "/skills"
sleep 0.3
tmux send-keys -t "$S78" Enter
sleep 0.6
rescan_empty="$(tmux capture-pane -t "$S78" -p)"
echo "==== Phase 78: the menu before any skill exists ===="
printf '%s\n' "$rescan_empty"
expect_has "$rescan_empty" -F "No skills found. Add one at:" "an empty skills dir did not open on the placeholder"
tmux send-keys -t "$S78" Escape
sleep 0.4
# …now plant one while the session is running.
mkdir -p "$RS_DIR/late-skill"
cat >"$RS_DIR/late-skill/SKILL.md" <<'SKILL'
---
name: late-skill
description: Planted mid-session by the smoke suite
---
Late body.
SKILL
# One turn re-walks the roots at its start.
submit "$S78" "hello there"
wait_for 24 "$S78" -S -80 -- -F "$SUMMARY_TURN1"
# Ctrl+D: the listing must name it in THIS turn's context, not the next one's.
# The view opens tail-following, and the listing is a LEADING fragment, so jump
# to the top before reading it.
tmux send-keys -t "$S78" C-d
sleep 0.8
tmux send-keys -t "$S78" Home
sleep 0.5
rescan_ctx="$(tmux capture-pane -t "$S78" -p)"
echo "==== Phase 78: the derived context after the rescan ===="
printf '%s\n' "$rescan_ctx" | grep -A4 "skills are available" || true
for expect in "The following skills are available for use with the Skill tool" \
	"late-skill: Planted mid-session by the smoke suite"; do
	expect_has "$rescan_ctx" -F "$expect" "the mid-session skill is missing from the context: '$expect'"
done
tmux send-keys -t "$S78" q
sleep 0.6
# …and the menu lists it without a restart.
tmux send-keys -t "$S78" -l "/skills"
sleep 0.3
tmux send-keys -t "$S78" Enter
sleep 0.6
rescan_menu="$(tmux capture-pane -t "$S78" -p)"
echo "==== Phase 78: the menu after the rescan ===="
printf '%s\n' "$rescan_menu"
expect_has "$rescan_menu" -E "late-skill +enabled" "the mid-session skill never reached the /skills menu"
tmux kill-session -t "$S78" 2>/dev/null
rm -rf "$RS_CFG" "$RS_DIR"
