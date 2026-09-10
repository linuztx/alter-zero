#!/usr/bin/env bash
# Phase 111 — the agent session view's footer GAUGES THAT AGENT'S CONTEXT

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the agent session view's footer GAUGES THAT AGENT'S CONTEXT
# (docs/agent-context-gauge.md). Under a forced window the footer shows the
# `{used}/{window} ({pct}%)` gauge; inside a subagent's session view that same
# segment used to keep showing the LEAD's context — `23.7k/1M` under a roster
# row saying the agent itself was at 64.9k. The `agent-stream` demo launches
# one background subagent whose round plays on the agent channel with no usage
# frame (the dummy scripts none), so: the view opens on a TRUE ZERO (nothing
# has counted the agent's context yet — never the lead's number), and once the
# agent settles the gauge is the tokenizer estimate over ITS transcript, a
# non-zero count that differs from the main footer's.
S111="${S}_agentgauge"
tmux new-session -d -s "$S111" -x 100 -y 34 "env ALTER_ZERO_CONTEXT_WINDOW=100000 $APP"
sleep 0.7
tmux send-keys -t "$S111" -l "launch a subagent that streams a table"
sleep 0.3
tmux send-keys -t "$S111" Enter
wait_for 20 "$S111" -F "Stream a comparison table" # the background launch puts its row on the roster
# ↓ onto `● main`, ↓ onto the agent, Enter into its session view (Phase 95's
# walk; the demo's pre-roll leaves time for it).
tmux send-keys -t "$S111" Down
sleep 0.2
tmux send-keys -t "$S111" Down
sleep 0.2
tmux send-keys -t "$S111" Enter
sleep 0.3
gauge_fresh="$(tmux capture-pane -t "$S111" -p | grep -E '/100k \(' | tail -1)"
echo "==== Phase 111: the agent view's footer before its first settle ===="
printf '%s\n' "$gauge_fresh"
expect_has "$gauge_fresh" -E '(^|[^0-9.k])0/100k \(0\.0%\)' "a just-opened agent view's gauge is not the agent's true zero (the lead's count leaked in): '$gauge_fresh'"
# The agent's round settles: the table closes and its receipt lands.
wait_for 30 "$S111" -S -200 -- -F "Four rows, one grid."
sleep 0.6
gauge_agent="$(tmux capture-pane -t "$S111" -p | grep -E '/100k \(' | tail -1)"
echo "==== Phase 111: the agent view's footer once the agent settled ===="
printf '%s\n' "$gauge_agent"
agent_used="$(printf '%s' "$gauge_agent" | grep -oE '[0-9.]+k?/100k' | head -1)"
if [ -z "$agent_used" ] || [ "$agent_used" = "0/100k" ]; then
	fail "the settled agent's gauge shows no estimate of its own transcript: '$gauge_agent'"
fi
# Back to the main conversation through the roster (↓ onto the viewed agent's
# row, ↑ onto `● main`, Enter leaves the view): its own gauge comes back — a
# different number, since it measures a different conversation. The agent's
# completion starts a follow-up turn on main; let it settle first.
tmux send-keys -t "$S111" Down
sleep 0.2
tmux send-keys -t "$S111" Up
sleep 0.2
tmux send-keys -t "$S111" Enter
sleep 0.5
for _ in $(seq 1 300); do
	if ! tmux capture-pane -t "$S111" -p | grep -qF "esc to interrupt"; then
		break
	fi
	sleep 0.1
done
sleep 0.3
gauge_main="$(tmux capture-pane -t "$S111" -p | grep -E '/100k \(' | tail -1)"
echo "==== Phase 111: the main footer back in the main view ===="
printf '%s\n' "$gauge_main"
main_used="$(printf '%s' "$gauge_main" | grep -oE '[0-9.]+k?/100k' | head -1)"
if [ -z "$main_used" ] || [ "$main_used" = "0/100k" ]; then
	fail "the main footer lost its gauge after leaving the agent view: '$gauge_main'"
fi
if [ -n "$agent_used" ] && [ "$agent_used" = "$main_used" ]; then
	fail "the agent view's gauge ($agent_used) is the main session's ($main_used): the footer is not following the conversation on screen"
fi
tmux kill-session -t "$S111" 2>/dev/null
echo "==== Phase 111: the agent session view's footer gauges the viewed agent's own context ===="
