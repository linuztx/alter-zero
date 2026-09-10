#!/usr/bin/env bash
# Phase 81 — a turn that FINISHES under the Ctrl+O overlay loses nothing

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# a turn that FINISHES under the Ctrl+O overlay loses nothing.
# Commits made while an alternate-screen overlay is up queue on the viewport
# and the return flushes them above the live region (invariant 4) — the
# retired history-window rebuild re-emitted at most one screenful, which
# silently dropped the rest of an overlay-covered turn from the terminal (the
# user-reported "cells hidden until a resize" / "can't scroll back to the
# reply" scrollback hole). Submit, open the overlay before the reply streams,
# let the WHOLE turn (text + Read/Edit/Bash cells + summary) finish under it,
# return, and assert every part reached the terminal's screen+scrollback —
# the user bubble included (it sat on the visible screen when the overlay
# opened, and the old return's window rewrite used to overwrite it).
S81="${S}_holefree"
launch "$S81" 80 24
submit "$S81" "$USER_MSG"
sleep 0.3 # the bubble commits; the reply has not started (startup delay)
tmux send-keys -t "$S81" C-o
sleep 0.3
wait_for 15 "$S81" -F "$SETTLED_REPLY" # the turn finishes while the overlay is up
sleep 0.8 # StreamDone + the Done-for summary land under the overlay
tmux send-keys -t "$S81" C-o
sleep 0.8
hole_free="$(tmux capture-pane -t "$S81" -p -S -)"
echo "==== Phase 81: returned after the turn finished under the overlay ===="
printf '%s\n' "$hole_free" | tail -60
for marker in "❯ $USER_MSG" "$EXPECT_REPLY" "Read(about.py)" "Edit(about.py)" "Bash(python3 about.py)" "$SETTLED_REPLY" "Done for"; do
	expect_has "$hole_free" -F "$marker" "'$marker' never reached the terminal after the overlay return (the scrollback hole)"
done
hole_screen="$(tmux capture-pane -t "$S81" -p)"
expect_has "$hole_screen" "^❯" "the composer is missing from the returned screen"
expect_has "$hole_screen" -E "dummy_model_name · .*manual$" "the session footer is missing from the returned screen"
tmux kill-session -t "$S81" 2>/dev/null
