#!/usr/bin/env bash
# Phase 8 — Esc INTERRUPTS a streaming turn (codex-style) instead of quitting

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Esc INTERRUPTS a streaming turn (codex-style) instead of quitting.
# Mid-stream Esc must stop the generation promptly: the partial reply stays on
# screen, the red "Conversation interrupted" notice commits, the live status
# strip clears, and the turn's receipt — the summary a finished turn ends on,
# "Worked for Ns · ≈N tokens" — commits UNDER the notice (docs/interrupt.md,
# "The receipt an interrupted turn keeps"). The dummy reports no usage, so the
# whole tally is the tokenizer's and the total wears the `≈` mark. The app
# keeps running — a follow-up message must stream and finish normally, its
# own summary the second on screen. Esc when idle now arms the Esc-Esc
# backtrack once user messages exist (Phase 30; quitting is Ctrl+C — Phase 4).
S5="${S}_interrupt"
launch "$S5" 80 24
submit "$S5" "hello there"
wait_for 4 "$S5" -F "Happy" # up to ~4s: wait until the reply is visibly streaming
tmux send-keys -t "$S5" Escape
interrupted="$(wait_settled 5 "$S5" -- -F "Conversation interrupted")"
echo "==== captured pane (turn interrupted with Esc) ===="
printf '%s\n' "$interrupted"
# The loop must survive the interrupt: a follow-up turn streams and finishes.
# The interrupted turn's receipt is already a summary on screen, so the
# follow-up's is the SECOND.
submit "$S5" "again please"
after_interrupt="$(wait_summaries 20.1 "$S5" 2 -S -200)" # up to ~20s: wait for the follow-up turn's summary
echo "==== captured pane (follow-up turn after the interrupt) ===="
printf '%s\n' "$after_interrupt"
tmux kill-session -t "$S5" 2>/dev/null

expect_has "$interrupted" -F "Conversation interrupted" "Esc mid-stream did not commit the 'Conversation interrupted' notice (did the app quit instead?)"
expect_has "$interrupted" -F "Happy" "the partial reply was not kept on screen after the interrupt"
expect_lacks "$interrupted" -F "esc to interrupt" "the live status line is still showing after the interrupt"
expect_has "$interrupted" -E "^${SUMMARY_RE}[0-9]*s · ≈[0-9][0-9.]*k? tokens$" "the interrupted turn did not commit its receipt ('Worked for Ns · ≈N tokens', estimated with no usage reported)"
receipts=$(count_summaries "$interrupted")
expect_eq "${receipts:-0}" 1 "the interrupted turn commits exactly one receipt"
notice_row="$(printf '%s\n' "$interrupted" | grep -nF "Conversation interrupted" | head -1 | cut -d: -f1)"
receipt_row="$(printf '%s\n' "$interrupted" | grep -nE "^$SUMMARY_RE" | head -1 | cut -d: -f1)"
if ! [ "${notice_row:-0}" -gt 0 ] || ! [ "${receipt_row:-0}" -gt "${notice_row:-0}" ]; then
	fail "the receipt does not sit under the notice (notice row=$notice_row receipt row=$receipt_row)"
fi
if [ "$(count_summaries "$after_interrupt")" -lt 2 ]; then
	fail "the app did not complete a follow-up turn after the interrupt — the loop or backend channel is wedged"
fi
