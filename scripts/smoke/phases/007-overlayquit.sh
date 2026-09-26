#!/usr/bin/env bash
# Phase 7 — finishing a turn *while the Ctrl+O overlay is open*, then quitting with Ctrl+C

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# finishing a turn *while the Ctrl+O overlay is open*, then quitting
# with Ctrl+C, must leave the RESTORED screen showing the committed turn
# summary — not the stale live "( ● ) {verb}… (… tokens)" status strip. Commits
# made under the overlay queue on the viewport (invariant 4) and a normal Ctrl+O
# return flushes them, but the quit path used to skip that and exit_overlay
# straight onto the stale strip. Run the binary *inside a shell* so the pane
# survives the app exiting and we can capture the restored terminal afterward.
S4="${S}_overlayquit"
tmux new-session -d -s "$S4" -x 80 -y 24
sleep 0.3
tmux send-keys -t "$S4" -l "$CFG_ENV ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"
tmux send-keys -t "$S4" Enter
sleep 0.6
submit "$S4" "hello there"
# Open the overlay *mid-stream*: wait until the reply is visibly streaming (the
# turn is active) before pressing Ctrl+O.
wait_for 4 "$S4" -F "Happy" # up to ~4s
tmux send-keys -t "$S4" C-o
# Wait until the turn FINISHES while the overlay is up — the transcript gains the
# summary. This is the precondition for the bug (the turn ended with
# scrollback commits deferred).
overlay_done="$(wait_pane 20.1 "$S4" -F "$SUMMARY_TURN1")" # up to ~20s
echo "==== captured pane (turn finished inside the Ctrl+O overlay) ===="
printf '%s\n' "$overlay_done"
# Now quit with Ctrl+C from inside the overlay and capture the restored terminal.
tmux send-keys -t "$S4" C-c
sleep 0.5
post_quit="$(tmux capture-pane -t "$S4" -p -S -40)"
echo "==== captured pane (restored terminal after quitting from the overlay) ===="
printf '%s\n' "$post_quit"
tmux kill-session -t "$S4" 2>/dev/null

# Phase 7: a turn that finished while the Ctrl+O overlay was open must, on quit,
# leave the restored screen showing the committed summary — not the stale live
# status line whose commits were still queued when the overlay went up (the
# quit's return flushes them; it used to skip that and exit onto the strip).
# The strip's precise marker is its 'esc to interrupt' hint — the committed
# turn now legitimately contains 'tokens' (the flushed 'Thought for … tokens'
# cell), which the old broader pattern misread as the strip.
if ! printf '%s' "$overlay_done" | grep -qF "$SUMMARY_TURN1"; then
	fail "the turn never finished inside the Ctrl+O overlay (Phase 7 precondition not met — retune the timing)"
else
	expect_has "$post_quit" -F "$SUMMARY_TURN1" "after quitting (Ctrl+C) from the overlay, the committed '$SUMMARY_TURN1 Ns' summary was not restored to the screen — the quit path left the stale live status strip behind"
	expect_lacks "$post_quit" -F "esc to interrupt" "after quitting (Ctrl+C) from the overlay, the stale live status line ('… esc to interrupt') was still on screen instead of the settled conversation"
fi
