#!/usr/bin/env bash
# Phase 85 — the VIEW FLOW

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the VIEW FLOW (docs/view-flow.md). A framed view's page taller
# than the terminal — here a /hooks detail whose command wraps to dozens of box
# rows — must not clip at the bottom: the paint bottom-anchors (the hint and
# the closing rule stay on screen) and the skipped top FLOWS into the
# terminal's real scrollback, so the whole page reads via the terminal's own
# scrolling. Navigating back purge-rebuilds — no stale flowed row survives —
# and closing the menu restores the composer.
S85="${S}_viewflow"
FL_CFG="$(mktemp -d)"
FL_CMD="echo FLOW_TOP_OF_COMMAND $(printf 'lorem-ipsum-filler-%03d ' $(seq 1 220))FLOW_DEEP_MARKER_85 done"
printf '{ "hooks": { "PreToolUse": [ { "matcher": "Bash", "hooks": [ { "type": "command", "command": "%s" } ] } ] } }' "$FL_CMD" >"$FL_CFG/hooks.json"
APP_FL="env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$FL_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
launch "$S85" 100 24 "$APP_FL"
tmux send-keys -t "$S85" -l "/hooks"
sleep 0.4
tmux send-keys -t "$S85" Enter
sleep 0.5
tmux send-keys -t "$S85" Enter # PreToolUse -> matchers
sleep 0.4
tmux send-keys -t "$S85" Enter # matcher -> hooks
sleep 0.4
tmux send-keys -t "$S85" Enter # hook -> the screen-tall detail page
sleep 0.8
flow_pane="$(tmux capture-pane -t "$S85" -p)"
flow_full="$(tmux capture-pane -t "$S85" -p -S -200)"
echo "==== Phase 85: the screen-tall detail page (visible pane) ===="
printf '%s\n' "$flow_pane"
# The visible screen keeps the page's TAIL: the hint and the closing rule.
expect_has "$flow_pane" -F "Esc to go back" "the detail tail (Esc to go back) is not on screen"
if ! printf '%s\n' "$flow_pane" | awk 'END { exit ($0 ~ /──/) ? 0 : 1 }'; then
	fail "the bottom rule is not the last screen row"
fi
# …and genuinely overflowed: the page top is NOT on the visible screen…
expect_lacks "$flow_pane" -F "Hook details" "the page fits the pane; the fixture must overflow for this phase to test the flow"
# …but IS in the terminal's real scrollback, whole: the title, the field
# block, and the boxed command's first words all reachable by scrolling up.
for expect in "Hook details" "Event:    PreToolUse" "FLOW_TOP_OF_COMMAND" "FLOW_DEEP_MARKER_85"; do
	expect_has "$flow_full" -F "$expect" "the flowed page is missing '$expect' from scrollback+screen"
done
# Esc back to the hooks level: the shrink purge-rebuilds, so the flowed rows
# vanish from scrollback (the deep marker can't hide in the list's one-row
# truncated command) and the small page paints in place.
tmux send-keys -t "$S85" Escape
sleep 0.6
flow_back="$(tmux capture-pane -t "$S85" -p -S -400)"
echo "==== Phase 85: back at the hooks level after the flow ===="
printf '%s\n' "$flow_back" | tail -24
expect_has "$flow_back" -F "PreToolUse - Matcher: Bash" "Esc from the flowed detail did not return to the hooks level"
expect_lacks "$flow_back" -F "FLOW_DEEP_MARKER_85" "stale flowed rows survived the back-navigation purge"
# Esc the rest of the way out: the composer and its footer return, and no
# trace of the menu or the flowed page is left anywhere in the terminal.
tmux send-keys -t "$S85" Escape
sleep 0.2
tmux send-keys -t "$S85" Escape
sleep 0.2
tmux send-keys -t "$S85" Escape
sleep 0.6
flow_closed="$(tmux capture-pane -t "$S85" -p -S -400)"
echo "==== Phase 85: closed back to the composer ===="
printf '%s\n' "$flow_closed" | tail -12
expect_has "$flow_closed" -F "dummy_model_name" "the composer (and its footer) did not come back"
for stale in "Hook details" "FLOW_TOP_OF_COMMAND" "This menu is read-only"; do
	expect_lacks "$flow_closed" -F "$stale" "'$stale' survived the close (the purge should have wiped it)"
done
tmux kill-session -t "$S85" 2>/dev/null || true
rm -rf "$FL_CFG"
