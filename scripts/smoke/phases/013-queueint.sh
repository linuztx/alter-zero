#!/usr/bin/env bash
# Phase 13 — Esc with a queued message interrupts the current turn AND sends the queued one

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Esc with a queued message interrupts the current turn AND sends
# the queued one right away (the user's spec; codex's steer-after-interrupt). Submit
# "hello there", queue "world" mid-stream, then Esc: the red "Conversation
# interrupted" notice commits for turn 1, and "world" is sent immediately as turn 2
# ("❯ world" + "Finished for").
S10="${S}_queueint"
launch "$S10" 80 24
submit "$S10" "hello there"
wait_for 4 "$S10" -F "Happy" # up to ~4s: wait until turn 1 is visibly streaming
submit "$S10" "world" # queued while streaming
sleep 0.3
tmux send-keys -t "$S10" Escape # interrupt turn 1 → send "world" right away
queueint="$(wait_pane 20.1 "$S10" -S -60 -- -F "Finished for")" # up to ~20s: the flushed "world" turn finishes
echo "==== captured pane (Esc interrupted turn 1 and sent the queued 'world') ===="
printf '%s\n' "$queueint"
tmux kill-session -t "$S10" 2>/dev/null

# Phase 13: Esc with a queued message interrupts the current turn and sends the
# queued one right away (the user's spec; codex's steer-after-interrupt).
expect_has "$queueint" -F "Conversation interrupted" "Esc with a queued message did not interrupt the current turn"
expect_has "$queueint" -F "❯ world" "Esc did not send the queued 'world' right away ('❯ world' missing)"
expect_has "$queueint" -F "Finished for" "the queued message sent on interrupt never finished its turn"
