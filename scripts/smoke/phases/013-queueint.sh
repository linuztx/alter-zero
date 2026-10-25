#!/usr/bin/env bash
# Phase 13 — Esc with a queued message interrupts the current turn AND sends the queued one

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Esc with a queued message interrupts the current turn AND sends
# the queued one right away (the user's spec; codex's steer-after-interrupt). Submit
# "hello there", queue "world" mid-stream, then Esc: the red "Conversation
# interrupted" notice commits for turn 1 with its receipt under it, and "world"
# is sent immediately as turn 2 ("❯ world" + its summary — the second on
# screen, turn 1's receipt being the first).
S10="${S}_queueint"
launch "$S10" 80 24
submit "$S10" "hello there"
wait_for 4 "$S10" -F "Happy" # up to ~4s: wait until turn 1 is visibly streaming
submit "$S10" "world" # queued while streaming
sleep 0.3
tmux send-keys -t "$S10" Escape # interrupt turn 1 → send "world" right away
queueint="$(wait_summaries 20.1 "$S10" 2 -S -200)" # up to ~20s: the flushed "world" turn finishes
echo "==== captured pane (Esc interrupted turn 1 and sent the queued 'world') ===="
printf '%s\n' "$queueint"
tmux kill-session -t "$S10" 2>/dev/null

# Phase 13: Esc with a queued message interrupts the current turn and sends the
# queued one right away (the user's spec; codex's steer-after-interrupt).
expect_has "$queueint" -F "Conversation interrupted" "Esc with a queued message did not interrupt the current turn"
expect_has "$queueint" -F "❯ world" "Esc did not send the queued 'world' right away ('❯ world' missing)"
if [ "$(count_summaries "$queueint")" -lt 2 ]; then
	fail "the queued message sent on interrupt never finished its turn"
fi
# Turn 1's receipt is committed before "world" goes out: it closes turn 1.
receipt_row="$(printf '%s\n' "$queueint" | grep -nE "^$SUMMARY_RE" | head -1 | cut -d: -f1)"
world_row="$(printf '%s\n' "$queueint" | grep -nF "❯ world" | head -1 | cut -d: -f1)"
if ! [ "${receipt_row:-0}" -gt 0 ] || ! [ "${world_row:-0}" -gt "${receipt_row:-0}" ]; then
	fail "turn 1's receipt does not sit above the queued 'world' it sent (receipt row=$receipt_row world row=$world_row)"
fi
