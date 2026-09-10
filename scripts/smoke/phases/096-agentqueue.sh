#!/usr/bin/env bash
# Phase 96 — the agent session view has the SAME mid-turn queue the main one has

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the agent session view has the SAME mid-turn queue the main one
# has (docs/queue.md) — one mechanism, two levels. Open the demo subagent's
# session while it works, type a message: it waits inset above the box
# ("  ❯ also add Elixir") exactly like the main session's, and the agent's own
# loop takes it at its next ROUND boundary (its parallel `write` batch), where
# it commits at column 0 on **that agent's** transcript while the agent is
# still running. It must never touch the main conversation. Before this the
# view recorded the message the instant it was typed — claiming the agent had
# read something it had not — and showed no pending state at all.
S96="${S}_agentqueue"
tmux new-session -d -s "$S96" -x 100 -y 34 "$APP"
sleep 0.7
tmux send-keys -t "$S96" -l "launch a subagent that streams a table"
sleep 0.3
tmux send-keys -t "$S96" Enter
wait_for 20 "$S96" -F "Stream a comparison table" # the background launch puts its row on the roster
# ↓ opens the roster on `● main`, a second ↓ steps onto the agent, Enter opens
# its session view (the demo's pre-roll leaves time for exactly this).
tmux send-keys -t "$S96" Down
sleep 0.15
tmux send-keys -t "$S96" Down
sleep 0.15
tmux send-keys -t "$S96" Enter
sleep 0.3
submit "$S96" "also add Elixir"
agent_pending=""
for _ in $(seq 1 40); do # the message waits inset above the box, not committed
	frame="$(tmux capture-pane -t "$S96" -p)"
	if printf '%s' "$frame" | grep -qF "  ❯ also add Elixir"; then
		agent_pending="$frame"
		break
	fi
	sleep 0.1
done
if [ -z "$agent_pending" ]; then
	fail "a message typed into a running agent's session never showed pending ('  ❯ also add Elixir' inset above the box)"
	echo "---- the last frame seen ----" >&2
	tmux capture-pane -t "$S96" -p >&2
else
	echo "==== Phase 96: the agent's own queued row ===="
	printf '%s\n' "$agent_pending" | grep -F "❯ also add Elixir"
fi
agent_delivered=""
for _ in $(seq 1 300); do # its loop takes it at the round boundary, still running
	frame="$(tmux capture-pane -t "$S96" -p -S -200)"
	if printf '%s' "$frame" | grep -qE '^❯ also add Elixir$' &&
		printf '%s' "$frame" | grep -qF "esc to interrupt"; then
		agent_delivered="$frame"
		break
	fi
	sleep 0.1
done
if [ -z "$agent_delivered" ]; then
	fail "the agent never took the queued message into its running turn ('❯ also add Elixir' at column 0 while its status line was still up)"
	echo "---- the last frame seen ----" >&2
	tmux capture-pane -t "$S96" -p -S -200 >&2
else
	echo "==== Phase 96: taken into the agent's own turn, after its write batch ===="
	printf '%s\n' "$agent_delivered" | grep -E "Wrote 3 lines to notes/sources.md|^❯ also add Elixir$"
	# It lands on the AGENT's transcript — under its launch prompt, after the
	# batch that ended the round — not on the main conversation's.
	expect_has "$agent_delivered" -F "Compare four languages" "the delivered message is not on the agent's own transcript (its launch prompt is gone from the view)"
fi
# Esc back to the main conversation: its purge-rebuild must show no trace of a
# message that belonged to the agent's conversation.
tmux send-keys -t "$S96" Escape
sleep 0.6
main_after="$(tmux capture-pane -t "$S96" -p -S -200)"
if printf '%s' "$main_after" | grep -qF "also add Elixir"; then
	fail "the agent's message leaked into the MAIN conversation on return"
	printf '%s\n' "$main_after" >&2
fi
tmux kill-session -t "$S96" 2>/dev/null
echo "==== Phase 96: the subagent session's mid-turn queue matches the main one ===="
