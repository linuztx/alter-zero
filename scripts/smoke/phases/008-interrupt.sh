#!/usr/bin/env bash
# Phase 8 — Esc INTERRUPTS a streaming turn (codex-style) instead of quitting

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Esc INTERRUPTS a streaming turn (codex-style) instead of quitting.
# Mid-stream Esc must stop the generation promptly: the partial reply stays on
# screen, the red "Conversation interrupted" notice commits, the live status
# strip clears, and the turn closes on its own summary UNDER the notice — how
# long it worked and the tokens its status line had counted, so the user sees
# what the work cost (docs/interrupt.md). The app keeps running — a follow-up
# message must stream and finish normally, its summary the second on screen.
# Esc when idle now arms the Esc-Esc backtrack once user messages exist
# (Phase 30; quitting is Ctrl+C — Phase 4).
S5="${S}_interrupt"
launch "$S5" 80 24
submit "$S5" "hello there"
wait_for 4 "$S5" -F "Happy" # up to ~4s: wait until the reply is visibly streaming
tmux send-keys -t "$S5" Escape
sleep 0.6
interrupted="$(tmux capture-pane -t "$S5" -p)"
echo "==== captured pane (turn interrupted with Esc) ===="
printf '%s\n' "$interrupted"
# The loop must survive the interrupt: a follow-up turn streams and finishes.
# Its summary is the second one — the interrupted turn's is already on screen.
submit "$S5" "again please"
after_interrupt="$(wait_summaries 20.1 "$S5" 2 -S -200)" # up to ~20s: wait for the follow-up turn's summary
echo "==== captured pane (follow-up turn after the interrupt) ===="
printf '%s\n' "$after_interrupt"
tmux kill-session -t "$S5" 2>/dev/null

expect_has "$interrupted" -F "Conversation interrupted" "Esc mid-stream did not commit the 'Conversation interrupted' notice (did the app quit instead?)"
expect_has "$interrupted" -F "Happy" "the partial reply was not kept on screen after the interrupt"
expect_lacks "$interrupted" -F "esc to interrupt" "the live status line is still showing after the interrupt"
expect_has "$interrupted" -E "^$SUMMARY_RE.* · [0-9.]+[kM]? tokens" "the interrupted turn did not close on its summary with the tokens it counted"
# The summary closes the turn: it sits under the notice, never above it.
notice_row="$(printf '%s\n' "$interrupted" | grep -nF "Conversation interrupted" | head -1 | cut -d: -f1)"
summary_row="$(printf '%s\n' "$interrupted" | grep -nE "^$SUMMARY_RE" | head -1 | cut -d: -f1)"
if ! [ "${notice_row:-0}" -gt 0 ] || ! [ "${summary_row:-0}" -gt "${notice_row:-0}" ]; then
	fail "the interrupted turn's summary is not under its notice (notice row ${notice_row:-none}, summary row ${summary_row:-none})"
fi
expect_eq "$(count_summaries "$after_interrupt")" 2 "the app did not complete a follow-up turn after the interrupt — the loop or backend channel is wedged"
