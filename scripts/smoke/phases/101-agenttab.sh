#!/usr/bin/env bash
# Phase 101 — TAB in an agent session view queues a follow-up turn for THAT AGENT

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# TAB in an agent session view queues a follow-up turn for THAT
# AGENT (docs/queue.md) — the main session's Tab, one level down. Before this
# the Tab arm read the *lead's* `is_streaming()` and pushed onto the *lead's*
# queue, so a message typed into a subagent ran as a follow-up turn of the main
# conversation once the lead's turn ended: the reported "it sends the msg to
# main agent TUI". Open the demo subagent's session while it works, Tab two
# messages — both wait inset above the box, blank-divided — and each then runs
# as its OWN continuation turn on that agent's transcript, in order, once its
# loop settles. The main conversation must never see either.
S101="${S}_agenttab"
tmux new-session -d -s "$S101" -x 100 -y 34 "$APP"
sleep 0.7
tmux send-keys -t "$S101" -l "launch a subagent that streams a table"
sleep 0.3
tmux send-keys -t "$S101" Enter
wait_for 20 "$S101" -F "Stream a comparison table" # the background launch puts its row on the roster
# ↓ ↓ Enter opens the agent's session view (Phase 96's walk).
tmux send-keys -t "$S101" Down
sleep 0.15
tmux send-keys -t "$S101" Down
sleep 0.15
tmux send-keys -t "$S101" Enter
sleep 0.3
tmux send-keys -t "$S101" -l "first follow up"
sleep 0.2
tmux send-keys -t "$S101" Tab
sleep 0.2
tmux send-keys -t "$S101" -l "second follow up"
sleep 0.2
tmux send-keys -t "$S101" Tab
sleep 0.3
# Alt+Up there edits **that agent's** last follow-up, never the main session's
# backlog (which is what it used to reach, alongside Tab): the second row goes
# and its text comes back as the draft. Tab re-queues it for the run below.
tmux send-keys -t "$S101" M-Up
sleep 0.4
agenttab_recall="$(tmux capture-pane -t "$S101" -p)"
if ! printf '%s' "$agenttab_recall" | grep -qE '^❯ second follow up'; then
	fail "Alt+Up did not pull the agent's last follow-up back into the composer"
	echo "---- the frame ----" >&2
	printf '%s\n' "$agenttab_recall" >&2
fi
expect_lacks "$agenttab_recall" -F "  ❯ second follow up" "Alt+Up left the pending row up; it must go with the pull-back"
expect_has "$agenttab_recall" -F "  ❯ first follow up" "Alt+Up took more than the last follow-up; the earlier one must stay queued"
echo "==== Phase 101: Alt+Up edits the agent's own last follow-up ===="
tmux send-keys -t "$S101" Tab
agenttab_pending=""
for _ in $(seq 1 40); do # both wait inset above the box, neither committed
	frame="$(tmux capture-pane -t "$S101" -p)"
	if printf '%s' "$frame" | grep -qF "  ❯ first follow up" &&
		printf '%s' "$frame" | grep -qF "  ❯ second follow up"; then
		agenttab_pending="$frame"
		break
	fi
	sleep 0.1
done
if [ -z "$agenttab_pending" ]; then
	fail "Tab in an agent session view did not queue the drafts on that agent (no inset '  ❯ first follow up' / '  ❯ second follow up' rows)"
	echo "---- the last frame seen ----" >&2
	tmux capture-pane -t "$S101" -p >&2
else
	echo "==== Phase 101: both Tab follow-ups pending on the agent's own queue ===="
	printf '%s\n' "$agenttab_pending" | grep -F "❯ first follow up"
	printf '%s\n' "$agenttab_pending" | grep -F "❯ second follow up"
fi
# Its loop settles, then each follow-up runs as its own continuation turn: the
# first commits at column 0 on the AGENT's transcript while the second is still
# waiting inset behind it.
agenttab_first=""
for _ in $(seq 1 400); do
	frame="$(tmux capture-pane -t "$S101" -p -S -200)"
	if printf '%s' "$frame" | grep -qE '^❯ first follow up$'; then
		agenttab_first="$frame"
		break
	fi
	sleep 0.1
done
if [ -z "$agenttab_first" ]; then
	fail "the agent never ran its first Tab follow-up as a turn of its own"
	echo "---- the last frame seen ----" >&2
	tmux capture-pane -t "$S101" -p -S -200 >&2
else
	echo "==== Phase 101: the first follow-up ran on the agent's own transcript ===="
	printf '%s\n' "$agenttab_first" | grep -E '^❯ first follow up$'
	# It lands under the agent's launch prompt, not the main conversation's.
	expect_has "$agenttab_first" -F "Compare four languages" "the follow-up is not on the agent's own transcript (its launch prompt is gone from the view)"
fi
agenttab_second=""
for _ in $(seq 1 400); do
	frame="$(tmux capture-pane -t "$S101" -p -S -200)"
	if printf '%s' "$frame" | grep -qE '^❯ second follow up$'; then
		agenttab_second="$frame"
		break
	fi
	sleep 0.1
done
if [ -z "$agenttab_second" ]; then
	fail "the second Tab follow-up never ran; the queue must drain one entry per settle"
else
	echo "==== Phase 101: the second follow-up ran after it, its own turn ===="
	printf '%s\n' "$agenttab_second" | grep -E '^❯ second follow up$'
fi
# Esc back to the main conversation: its purge-rebuild must show no trace of
# either — they belonged to the agent's conversation, and the main session's
# own queue was never touched.
tmux send-keys -t "$S101" Escape
sleep 0.8
agenttab_main="$(tmux capture-pane -t "$S101" -p -S -200)"
if printf '%s' "$agenttab_main" | grep -qF "follow up"; then
	fail "a message typed into the subagent leaked into the main conversation"
	echo "---- the main view ----" >&2
	printf '%s\n' "$agenttab_main" >&2
fi
tmux kill-session -t "$S101" 2>/dev/null
echo "==== Phase 101: Tab in a subagent's session queues that agent's own follow-up turns ===="
