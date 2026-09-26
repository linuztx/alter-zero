#!/usr/bin/env bash
# Phase 21 — TAB queues a message as a SEPARATE follow-up turn

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# TAB queues a message as a SEPARATE follow-up turn (docs/queue.md),
# unlike Enter which hands it to the turn already running. Submit "hello there",
# then queue "world" with Enter and "later" with TAB while turn 1 streams: both
# show inset above the box ("  ❯ world", "  ❯ later"), divided by a blank
# boundary. "world" is read by turn 1 itself, and "later" then runs as a
# SEPARATE turn 2 (a second turn summary) — the extra turn Phase 12's
# all-Enter drive never produces.
S18="${S}_tabqueue"
launch "$S18" 80 24
submit "$S18" "hello there"
wait_for 4 "$S18" -F "Happy" # up to ~4s: wait until turn 1 is visibly streaming
submit "$S18" "world" # streaming → batch 1 (the first queue)
tmux send-keys -t "$S18" -l "later"
sleep 0.2
tmux send-keys -t "$S18" Tab # streaming → a NEW batch (a separate follow-up turn)
tabqueue_band=""
for _ in $(seq 1 20); do # up to ~3s: both queued messages show above the box
	tabqueue_band="$(tmux capture-pane -t "$S18" -p)"
	if printf '%s' "$tabqueue_band" | grep -qF "  ❯ world" &&
		printf '%s' "$tabqueue_band" | grep -qF "  ❯ later" &&
		printf '%s' "$tabqueue_band" | grep -qF "tokens"; then
		break
	fi
	sleep 0.15
done
echo "==== captured pane (world via Enter + later via Tab queued above the box) ===="
printf '%s\n' "$tabqueue_band"
tabqueue="$(wait_summaries 45 "$S18" 2 -S -100)" # up to ~45s: turn 1, then the SEPARATE Tab turn 2
echo "==== captured pane (the Tab follow-up ran as a separate second turn) ===="
printf '%s\n' "$tabqueue"
tmux kill-session -t "$S18" 2>/dev/null

# Phase 21: TAB queues a SEPARATE follow-up turn (docs/queue.md). While turn 1
# streams, "world" (Enter) and "later" (Tab) both show inset above the box; then
# turn 1 reads "world" and "later" runs as a SEPARATE turn 2, so a second turn
# summary MUST appear — unlike Phase 12's all-Enter drive, which asserts the
# opposite.
expect_has "$tabqueue_band" -F "  ❯ world" "the Enter-queued 'world' was not shown inset above the box while turn 1 streamed"
expect_has "$tabqueue_band" -F "  ❯ later" "the Tab-queued 'later' was not shown inset above the box — did Tab fail to queue?"
if ! printf '%s' "$tabqueue" | grep -qF "❯ world" ||
	! printf '%s' "$tabqueue" | grep -qF "❯ later"; then
	fail "the queued messages never reached scrollback — '❯ world' and '❯ later' did not both commit"
fi
if [ "$(count_summaries "$tabqueue")" -lt 2 ]; then
	fail "the Tab-queued 'later' did not run as a SEPARATE turn (no second turn summary) — Tab must open a follow-up turn, not go into the running one like Enter"
fi
