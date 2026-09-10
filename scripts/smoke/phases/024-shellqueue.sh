#!/usr/bin/env bash
# Phase 24 — a `!` command typed WHILE A TURN STREAMS queues as its own STANDALONE shell en

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# a `!` command typed WHILE A TURN STREAMS queues as its own
# STANDALONE shell entry and runs LOCALLY as a separate turn after it
# (docs/queue.md, docs/shell-command.md) — codex's action-tagged queued shell
# command, NOT the old v1 behaviour of sending "!echo …" to the backend as
# literal text. Submit "hello there", then mid-stream queue "world" (Enter, a
# text message) and "!echo smoke_queue_ok" (Enter in shell mode, a standalone
# shell turn). Both show inset above the box ("  ❯ world", "  ! echo
# smoke_queue_ok"); then the running turn reads "world" at its round boundary
# and the command runs LOCALLY as the next turn, committing an exec cell
# ("! echo …" header + "⎿ smoke_queue_ok" output) — never a "❯ !echo …" user
# message.
S21="${S}_shellqueue"
launch "$S21" 80 24
submit "$S21" "hello there"
wait_for 4 "$S21" -F "Happy" # up to ~4s: wait until turn 1 is visibly streaming
submit "$S21" "world" # streaming → into the turn already running
submit "$S21" "!echo smoke_queue_ok" # streaming → a STANDALONE shell entry (local, next turn)
shellqueue_band=""
for _ in $(seq 1 20); do # up to ~3s: both queued entries show inset above the box
	shellqueue_band="$(tmux capture-pane -t "$S21" -p)"
	if printf '%s' "$shellqueue_band" | grep -qF "  ❯ world" &&
		printf '%s' "$shellqueue_band" | grep -qF "  ! echo smoke_queue_ok"; then
		break
	fi
	sleep 0.15
done
echo "==== captured pane (text + shell command queued above the box) ===="
printf '%s\n' "$shellqueue_band"
shellqueue="$(wait_pane 45 "$S21" -S -100 -- -F "⎿  smoke_queue_ok")" # up to ~45s: turn 1 (reading "world"), then the LOCAL shell turn
echo "==== captured pane (the queued !command ran locally as its own turn) ===="
printf '%s\n' "$shellqueue"
tmux kill-session -t "$S21" 2>/dev/null

# Phase 24: a !command queued mid-turn runs LOCALLY as its own turn (docs/queue.md).
# While turn 1 streams, the text "world" (❯, a model turn) and the command
# "! echo …" (the red shell prompt) both show inset above the box; then the
# command commits an exec cell (⎿ output), proving it ran locally — NOT a
# "❯ !echo …" user message sent to the backend (the old v1 limitation).
expect_has "$shellqueue_band" -F "  ❯ world" "the Enter-queued 'world' was not shown inset above the box while turn 1 streamed"
expect_has "$shellqueue_band" -F "  ! echo smoke_queue_ok" "the mid-turn !command did not queue as an inset '! echo …' shell entry (the red bang prompt)"
expect_has "$shellqueue" -E "^! echo smoke_queue_ok" "the queued !command did not commit its '! echo …' exec-cell header — did it run locally?"
expect_has "$shellqueue" -F "⎿  smoke_queue_ok" "the queued !command produced no '⎿' output cell — it was not run locally as its own turn"
expect_lacks "$shellqueue" -F "❯ !echo smoke_queue_ok" "the queued !command was sent to the backend as literal text ('❯ !echo …') — the old v1 limitation, not run locally"
