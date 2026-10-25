#!/usr/bin/env bash
# Phase 13 — Esc with a queued message interrupts the current turn AND sends the queued one

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Esc with a queued message interrupts the current turn AND sends
# the queued one right away (the user's spec; codex's steer-after-interrupt). Submit
# "hello there", queue "world" mid-stream, then Esc: the red "Conversation
# interrupted" notice commits for turn 1, closed by turn 1's own summary, and
# "world" is sent immediately as turn 2 ("❯ world" + its summary, the second on
# screen — turn 1's comes first, ahead of "❯ world").
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
expect_eq "$(count_summaries "$queueint")" 2 "the queued message sent on interrupt never finished its turn"
# Turn 1's summary closes turn 1: under its notice, before "world" goes out.
notice_row="$(printf '%s\n' "$queueint" | grep -nF "Conversation interrupted" | head -1 | cut -d: -f1)"
first_summary_row="$(printf '%s\n' "$queueint" | grep -nE "^$SUMMARY_RE" | head -1 | cut -d: -f1)"
world_row="$(printf '%s\n' "$queueint" | grep -nF "❯ world" | head -1 | cut -d: -f1)"
if ! [ "${notice_row:-0}" -gt 0 ] || ! [ "${first_summary_row:-0}" -gt "${notice_row:-0}" ] ||
	! [ "${world_row:-0}" -gt "${first_summary_row:-0}" ]; then
	fail "turn 1's summary is not between its notice and the queued 'world' (notice ${notice_row:-none}, summary ${first_summary_row:-none}, world ${world_row:-none})"
fi
