#!/usr/bin/env bash
# Phase 29 — the QUEUE follows into the Ctrl+O overlay

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the QUEUE follows into the Ctrl+O overlay (docs/queue.md). While
# turn 1 streams, a message queued mid-turn shows in the transcript view as the
# inline strip's inset "  ❯ world" row; when the turn reaches its next round
# boundary UNDER the overlay it takes the message right there (the loop keeps
# draining reply events with the overlay up — invariant 4): the overlay gains
# the real column-0 "❯ world" user entry and the turn runs on to its summary
# summary, all without leaving the overlay. The Ctrl+O return then repaints the
# inline conversation with the whole turn.
S29="${S}_queueoverlay"
tmux new-session -d -s "$S29" -x 80 -y 60 "$APP" # 60 rows: two demo turns
sleep 0.4
submit "$S29" "hello there"
wait_for 4 "$S29" -F "Happy" # up to ~4s: wait until turn 1 is visibly streaming
submit "$S29" "world" # streaming → queued, not submitted
tmux send-keys -t "$S29" C-o   # open the transcript view mid-stream
queued_overlay=""
for _ in $(seq 1 20); do # up to ~3s: the queued row shows inside the overlay
	queued_overlay="$(tmux capture-pane -t "$S29" -p)"
	if printf '%s' "$queued_overlay" | grep -qF "T R A N S C R I P T" &&
		printf '%s' "$queued_overlay" | grep -qF "  ❯ world"; then
		break
	fi
	sleep 0.15
done
echo "==== captured overlay (queued message shown while turn 1 streams) ===="
printf '%s\n' "$queued_overlay"
# The turn reaches its round boundary under the overlay → it takes "world"
# right there, which becomes a real transcript user entry, and runs to its
# summary.
overlay_advanced="$(wait_pane 45 "$S29" -E "$SUMMARY_ANY_RE")" # up to ~45s: the turn reads it, then finishes
echo "==== captured overlay (queued message taken under the overlay) ===="
printf '%s\n' "$overlay_advanced"
# The view is pinned to the bottom, so the dispatched "❯ world" user entry has
# scrolled off the visible pane; jump Home and page down (the pager's jump/page
# keys) until it scrolls into view.
tmux send-keys -t "$S29" Home
sleep 0.2
overlay_world=""
for _ in $(seq 1 12); do
	overlay_world="$(tmux capture-pane -t "$S29" -p)"
	if printf '%s' "$overlay_world" | grep -qE '^❯ world'; then
		break
	fi
	tmux send-keys -t "$S29" PageDown
	sleep 0.2
done
echo "==== captured overlay (scrolled to the dispatched user entry) ===="
printf '%s\n' "$overlay_world"
tmux send-keys -t "$S29" C-o # return: the inline view repaints from history
sleep 0.6
queue_overlay_returned="$(tmux capture-pane -t "$S29" -p -S -80)"
echo "==== captured pane (inline view after returning from the overlay) ===="
printf '%s\n' "$queue_overlay_returned"
tmux kill-session -t "$S29" 2>/dev/null

# Phase 29: the queued message follows into the Ctrl+O overlay and is taken
# there at the turn's round boundary — the overlay never hides (or freezes) the
# queue.
expect_has "$queued_overlay" -F "  ❯ world" "the queued message row ('  ❯ world') was missing from the Ctrl+O transcript view"
expect_has "$overlay_advanced" -F "T R A N S C R I P T" "the overlay was not still open when the running turn took the queued message"
expect_has "$overlay_world" -E '^❯ world' "the queued message was never taken into the turn under the overlay (no column-0 '❯ world' user entry found via Home/PageDown)"
expect_has "$overlay_advanced" -E "$SUMMARY_ANY_RE" "the turn that read the queued message never ran to its summary under the overlay"
expect_has "$queue_overlay_returned" -E '^❯ world' "after returning from the overlay the taken 'world' message is missing from the inline view"
expect_has "$queue_overlay_returned" -E "$SUMMARY_ANY_RE" "after returning from the overlay the turn's summary is missing from the inline view"
