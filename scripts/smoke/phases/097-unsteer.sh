#!/usr/bin/env bash
# Phase 97 — Alt+Up pulls back a message the running turn has NOT read yet

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Alt+Up pulls back a message the running turn has NOT read yet
# (docs/queue.md). Submit "hello there", Enter "oops typo" mid-stream — it waits
# inset above the box — then Alt+Up: only the shared queue knows whether the
# loop has taken it, so the boundary pops it and hands it back. The row goes and
# the text returns to the composer as an editable draft, with the turn still
# running. (The drive presses Alt+Up well before the turn's first tool
# resolution, which is the only point that could have taken it.)
S97="${S}_unsteer"
launch "$S97" 80 24
submit "$S97" "hello there"
wait_for 4 "$S97" -F "Happy" # up to ~4s: wait until turn 1 is visibly streaming
submit "$S97" "oops typo"
unsteer_pending="$(wait_pane 3 "$S97" -F "  ❯ oops typo")" # the message waits inset above the box
tmux send-keys -t "$S97" M-Up
sleep 0.5
unsteer_after="$(tmux capture-pane -t "$S97" -p)"
echo "==== Phase 97: pulled back into the composer ===="
printf '%s\n' "$unsteer_after"
expect_has "$unsteer_pending" -F "  ❯ oops typo" "the message never showed pending above the box"
expect_lacks "$unsteer_after" -F "  ❯ oops typo" "Alt+Up left the pending row up; it must go with the pull-back"
expect_has "$unsteer_after" -E '^❯ oops typo' "Alt+Up did not return the message to the composer as a draft"
expect_has "$unsteer_after" -F "esc to interrupt" "the turn stopped: Alt+Up must take the message back without touching it"
tmux kill-session -t "$S97" 2>/dev/null
echo "==== Phase 97: an unread message comes back on Alt+Up ===="
