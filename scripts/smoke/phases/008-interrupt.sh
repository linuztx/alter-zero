#!/usr/bin/env bash
# Phase 8 — Esc INTERRUPTS a streaming turn (codex-style) instead of quitting

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Esc INTERRUPTS a streaming turn (codex-style) instead of quitting.
# Mid-stream Esc must stop the generation promptly: the partial reply stays on
# screen, the red "Conversation interrupted" notice commits, the live status
# strip clears (no "tokens" line), and NO turn summary appears. The app keeps
# running — a follow-up message must stream and finish normally (its summary is
# the only one on screen). Esc when idle now arms the Esc-Esc
# backtrack once user messages exist (Phase 30; quitting is Ctrl+C — Phase 4).
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
submit "$S5" "again please"
after_interrupt="$(wait_pane 20.1 "$S5" -S -30 -- -E "$SUMMARY_RE")" # up to ~20s: wait for the follow-up turn's summary
echo "==== captured pane (follow-up turn after the interrupt) ===="
printf '%s\n' "$after_interrupt"
tmux kill-session -t "$S5" 2>/dev/null

expect_has "$interrupted" -F "Conversation interrupted" "Esc mid-stream did not commit the 'Conversation interrupted' notice (did the app quit instead?)"
expect_has "$interrupted" -F "Happy" "the partial reply was not kept on screen after the interrupt"
expect_lacks "$interrupted" -F "tokens" "the live status line ('… tokens') is still showing after the interrupt"
expect_lacks "$interrupted" -E "$SUMMARY_RE" "an interrupted turn must not commit a turn summary (the notice is its terminal state)"
expect_has "$after_interrupt" -E "$SUMMARY_RE" "the app did not complete a follow-up turn after the interrupt — the loop or backend channel is wedged"
