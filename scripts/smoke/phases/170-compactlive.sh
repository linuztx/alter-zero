#!/usr/bin/env bash
# Phase 170 — a summary written WHILE the agent works lands mid-turn, and a /compact typed mid-turn is taken

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Compaction no longer waits for the turn to end (docs/compact.md *Compacting
# while the agent works*). The offline `compact-live` demo plays what the live
# engine does: a round of work, the summary announced, a SECOND round made
# while it is being written, the summary landing, a last round on the
# compacted context. On screen that is:
#   - the status line saying `compacting context` while the turn keeps going,
#   - the `● Context compacted` cell committed BETWEEN two tool cells, where
#     the swap happened — never at the turn's end,
#   - a derived context (Ctrl+D) whose bridge is followed by the call made
#     while the summary was being written, word for word, and that no longer
#     holds the call the summary covers.
# And `/compact` typed while a turn runs is no longer refused: it is queued
# with a toast and taken — here, on the dummy, as soon as the turn ends.
S170="${S}_compactlive"
launch "$S170" 100 50
submit "$S170" "compact the context while you work"
live_status="$(wait_pane 20 "$S170" -- -F "compacting context")" # up to ~20s
echo "==== Phase 170: pane while the summary is being written ===="
printf '%s\n' "$live_status"
wait_for 30 "$S170" -S -80 -- -E "^$SUMMARY_RE"
live_done="$(pane "$S170" -S -80)"
echo "==== Phase 170: pane after the turn (the cell sits between two calls) ===="
printf '%s\n' "$live_done"
# The derived context: the bridge, then what was done while it was written.
# The view opens on the window's end; the bridge is the message it opens with.
keys "$S170" C-d
wait_for 5 "$S170" -- -F "to quit"
keys "$S170" Home
live_ctx="$(wait_pane 5 "$S170" -- -F "Another language model")"
echo "==== Phase 170: Ctrl+D after the mid-turn compaction ===="
printf '%s\n' "$live_ctx"
keys "$S170" q
wait_for 5 "$S170" -- -E "^$SUMMARY_RE"

expect_has "$live_status" -F "esc to interrupt" "the turn was not running while the summary was being written"
# The demo's own reply names the cell, so the cell is counted by its bullet.
CELL="● Context compacted"
expect_has "$live_done" -F "$CELL" "the '● Context compacted' cell never committed mid-turn"
expect_has "$live_done" -F "· auto" "the mid-turn compaction's cell is not tagged auto"
expect_lacks "$live_done" -F "Next steps" "the summary's own text rendered into the conversation (it must stay hidden)"
row_of() { printf '%s\n' "$live_done" | grep -nF -- "$1" | head -1 | cut -d: -f1; }
read_row="$(row_of "Read(about.py)")"
cell_row="$(row_of "$CELL")"
run_row="$(row_of "Bash(python3 about.py)")"
if [ -z "$read_row" ] || [ -z "$cell_row" ] || [ -z "$run_row" ]; then
	fail "a cell is missing from the turn (read '$read_row', compacted '$cell_row', run '$run_row')"
elif [ "$read_row" -ge "$cell_row" ] || [ "$cell_row" -ge "$run_row" ]; then
	fail "the compaction cell is not between the call made before it and the call made after (read $read_row, compacted $cell_row, run $run_row)"
fi
expect_eq "$(printf '%s\n' "$live_done" | grep -cF "$CELL")" "1" "the compaction cell committed more than once"
expect_has "$live_ctx" -F "Another language model" "Ctrl+D does not show the summary bridge (the derivation did not compact)"
expect_has "$live_ctx" -F "The task was still in progress" "the bridge of a compaction made mid-turn does not say the task was still in progress"
expect_has "$live_ctx" -F "compact the context while you work" "the user's own message dropped from the compacted context"
expect_has "$live_ctx" -F "about.py" "the call made while the summary was being written is not in the compacted context"
expect_lacks "$live_ctx" -F "ls -la" "a call the summary covers is still in the derived context"

# A /compact typed while a turn runs: queued, not refused — and made.
submit "$S170" "hello there"
wait_for 10 "$S170" -- -F "esc to interrupt"
type_text "$S170" "/compact"
sleep "$SMOKE_TYPE_SETTLE"
keys "$S170" Enter
queued_toast="$(wait_pane 5 "$S170" -- -F "Compacting at the agent's next step")"
echo "==== Phase 170: pane with the mid-turn /compact toast ===="
printf '%s\n' "$queued_toast"
count_cells() { pane "$S170" -S -200 | grep -cF "$CELL"; }
for _ in $(seq 1 "$(smoke_ticks 40)"); do # up to ~40s: the turn, then the summary
	[ "$(count_cells)" -ge 2 ] && break
	sleep "$SMOKE_POLL_INTERVAL"
done
queued_done="$(pane "$S170" -S -200)"
echo "==== Phase 170: pane after the queued /compact ran ===="
printf '%s\n' "$queued_done"
expect_has "$queued_toast" -F "Compacting at the agent's next step" "a mid-turn /compact was not queued with its toast"
expect_lacks "$queued_done" -F "/compact is disabled" "a mid-turn /compact was refused"
expect_eq "$(printf '%s\n' "$queued_done" | grep -cF "$CELL")" "2" "the queued /compact never ran once the turn ended"
tmux kill-session -t "$S170" 2>/dev/null
