#!/usr/bin/env bash
# Phase 32 — an Esc interrupt stays PROMPT even when the backend is slow to observe the can

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# an Esc interrupt stays PROMPT even when the backend is slow to
# observe the cancel (docs/interrupt.md — the interrupt-lag fix). ALTER_ZERO_STALL_MS
# selects a test backend that ignores the cancel for N ms, modelling a real
# network backend wedged in a blocking read during the pre-first-token pause. The
# old loop did `cancel + join`, which blocks the single-threaded loop until the
# thread unwinds — freezing the whole UI for ~N ms; the fix detaches the thread
# and swaps the reply channel, so Esc settles within a frame. The stall backend
# streams NOTHING before the stall, so Esc UNDOES the turn (req 1: no output yet)
# — no `Conversation interrupted` notice — and the signal that the loop stayed
# responsive is the status line (its `esc to interrupt` hint) clearing promptly:
# a join()ing loop keeps it up ~N ms, a detaching loop clears it in tens of ms.
STALL_S="${S}_stall"
STALL_MS=3000
launch "$STALL_S" 80 24 "env $CFG_ENV ALTER_ZERO_STALL_MS=$STALL_MS $BIN"
submit "$STALL_S" "hello there"
# Wait for the pre-first-token status window (the stall backend sends nothing yet).
stall_streaming=0
for _ in $(seq 1 40); do # up to ~2s
	if tmux capture-pane -t "$STALL_S" -p | grep -qF "esc to interrupt"; then
		stall_streaming=1
		break
	fi
	sleep 0.05
done
# Esc, then measure how long the live status line takes to clear (the undo's
# prompt settle — no notice commits for a no-output interrupt).
stall_t0="$(date +%s.%N)"
tmux send-keys -t "$STALL_S" Escape
stall_gap=""
for _ in $(seq 1 250); do # up to ~5s (well past the 3s stall)
	if ! tmux capture-pane -t "$STALL_S" -p | grep -qF "esc to interrupt"; then
		stall_gap="$(awk "BEGIN{printf \"%.3f\", $(date +%s.%N) - $stall_t0}")"
		break
	fi
	sleep 0.02
done
echo "==== Phase 32: interrupt latency under a ${STALL_MS}ms stalled backend ===="
echo "stall_streaming=$stall_streaming stall_gap=${stall_gap:-none}"
stall_after="$(tmux capture-pane -t "$STALL_S" -p -S -30)"
printf '%s\n' "$stall_after"
tmux kill-session -t "$STALL_S" 2>/dev/null

# Phase 32: an Esc interrupt is prompt even when the backend is slow to observe
# the cancel — the loop detaches the thread instead of join()ing it, so the UI
# never freezes (docs/interrupt.md, the interrupt-lag fix). The stall backend
# streams nothing, so Esc UNDOES the turn (req 1) — the promptness signal is the
# status line clearing, and the undo restores the message with no notice.
if [ "$stall_streaming" -ne 1 ]; then
	fail "never reached the streaming status line under the stalled backend"
fi
if [ -z "$stall_gap" ]; then
	fail "status line never cleared after Esc under the stalled backend"
elif awk "BEGIN{exit !($stall_gap > 1.5)}"; then
	# 1.5s is a generous ceiling — half the 3s stall. A join()ing loop lands near
	# 3s; the detaching fix lands in tens of ms. Anything over 1.5s means the loop
	# is blocking on the backend thread again (the interrupt-lag regression).
	fail "Esc took ${stall_gap}s to settle (>1.5s) — the loop is blocking on the backend (join(), not detach)"
fi
# The no-output interrupt undoes the submission: the message returns to the
# composer and NO `Conversation interrupted` notice is committed (req 1).
expect_has "$stall_after" -F "hello there" "undo did not restore 'hello there' to the composer"
expect_lacks "$stall_after" -F "Conversation interrupted" "committed a 'Conversation interrupted' notice — a no-output interrupt must undo, not notify (req 1)"
