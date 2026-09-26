#!/usr/bin/env bash
# Phase 54 — background agents + the roster selection

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# background agents + the roster selection. A "background agents"
# prompt resolves at once with `● 2 background agents launched (↓ to manage · ctrl+o to expand)`;
# the roster keeps the two running rows, ↓ opens the selection (`❯` on
# `● main`, the `↑/↓ to select · Enter to view` hint in the footer slot), a
# second ↓ moves onto an agent (`Enter to view · x to stop`), and `x` stops it
# — the red `Agent "…" was stopped by user` notice commits AND the row stays
# put, red, for its long stopped linger with the hint swapped to
# `x to clear`; the second `x` is what takes it off. Then the selection's
# **memory**: Esc + ↓ comes back to the row the user last picked instead of
# restarting on `● main`, and Enter into an agent's session view is a pick too
# — the ↓ after it lands on that agent (`docs/agent-tool.md`).
S54="${S}_bgagents"
launch "$S54" 100 44
submit "$S54" "call background agents for the weather"
bg_launched=""
for _ in $(seq 1 400); do
	cap="$(tmux capture-pane -t "$S54" -p)"
	if printf '%s' "$cap" | grep -qF "2 background agents launched (↓ to manage · ctrl+o to expand)" &&
		printf '%s' "$cap" | grep -qF "$SUMMARY_TURN1"; then
		bg_launched="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 54: captured pane (background agents launched) ===="
printf '%s\n' "$bg_launched"
tmux send-keys -t "$S54" Down
sleep 0.3
bg_sel_main="$(tmux capture-pane -t "$S54" -p)"
echo "==== Phase 54: captured pane (❯ on ● main + select hint) ===="
printf '%s\n' "$bg_sel_main"
tmux send-keys -t "$S54" Down
sleep 0.3
bg_sel_agent="$(tmux capture-pane -t "$S54" -p)"
# Enter opens that agent's session view, and the ↓ after it must come back to
# the SAME row — the roster remembers the last picked agent.
tmux send-keys -t "$S54" Enter
sleep 0.6
bg_view="$(tmux capture-pane -t "$S54" -p)"
echo "==== Phase 54: captured pane (the agent session view) ===="
printf '%s\n' "$bg_view"
tmux send-keys -t "$S54" Down
sleep 0.4
bg_resumed="$(tmux capture-pane -t "$S54" -p)"
echo "==== Phase 54: captured pane (↓ resumes on the last picked agent) ===="
printf '%s\n' "$bg_resumed"
# Esc back to the main session, then ↓ again — still the remembered row.
tmux send-keys -t "$S54" Escape
sleep 0.3
tmux send-keys -t "$S54" Escape
sleep 0.6
tmux send-keys -t "$S54" Down
sleep 0.4
bg_resumed_main="$(tmux capture-pane -t "$S54" -p)"
echo "==== Phase 54: captured pane (back in the main session, ↓ still resumes) ===="
printf '%s\n' "$bg_resumed_main"
tmux send-keys -t "$S54" -l "x"
bg_stopped=""
for _ in $(seq 1 100); do
	cap="$(tmux capture-pane -t "$S54" -p)"
	if printf '%s' "$cap" | grep -qF "was stopped by user"; then
		bg_stopped="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 54: captured pane (x stopped the agent) ===="
printf '%s\n' "$bg_stopped"
# The stopped row does NOT leave: it lingers red (30s) with the hint swapped
# to the clear, so the user can see what they stopped.
sleep 2
bg_lingering="$(tmux capture-pane -t "$S54" -p)"
echo "==== Phase 54: captured pane (the stopped row lingers · x to clear) ===="
printf '%s\n' "$bg_lingering"
bg_rows_before="$(printf '%s' "$bg_lingering" | grep -cF "general-purpose  Fetch")"
tmux send-keys -t "$S54" -l "x"
sleep 0.6
bg_cleared="$(tmux capture-pane -t "$S54" -p)"
echo "==== Phase 54: captured pane (the second x cleared the row) ===="
printf '%s\n' "$bg_cleared"
bg_rows_after="$(printf '%s' "$bg_cleared" | grep -cF "general-purpose  Fetch")"
tmux kill-session -t "$S54" 2>/dev/null
echo "==== Phase 54: background agents — launch cell, roster selection, x stop ===="
if [ -z "$bg_launched" ]; then
	fail "the background launch cell never committed"
fi
expect_has "$bg_sel_main" -F "❯ ● main" "↓ did not put the ❯ selection on ● main"
expect_has "$bg_sel_main" -F "↑/↓ to select · Enter to view" "the main-row selection hint is missing"
expect_has "$bg_sel_agent" -F "Enter to view · x to stop" "the agent-row selection hint is missing"
expect_has "$bg_sel_agent" -F "❯ ◯ general-purpose" "the ❯ never moved onto the agent row"
if [ -z "$bg_stopped" ]; then
	fail "x did not stop the agent with the red notice"
fi
expect_has "$bg_view" -F "Fetch current weather and time in" "Enter did not open the agent session view"
expect_has "$bg_resumed" -F "❯ ● general-purpose" "↓ inside the view did not resume on the picked agent"
expect_has "$bg_resumed_main" -F "❯ ◯ general-purpose" "↓ in the main session did not resume on the picked agent"
expect_has "$bg_lingering" -F "Enter to view · x to clear" "the stopped row did not swap its hint to the clear"
if [ "$bg_rows_before" != "2" ]; then
	fail "the stopped row left the roster instead of lingering (rows: $bg_rows_before)"
fi
if [ "$bg_rows_after" != "1" ]; then
	fail "the second x did not clear the stopped row (rows: $bg_rows_after)"
fi
