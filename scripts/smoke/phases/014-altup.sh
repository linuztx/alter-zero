#!/usr/bin/env bash
# Phase 14 — Alt+Up pulls only the LAST queued batch back into the composer

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Alt+Up pulls only the LAST queued batch back into the composer
# (docs/queue.md): queue "world" with Enter (batch 1) and "again" with Tab
# (batch 2 — a separate turn) mid-stream, press Alt+Up — only "again" returns to
# the box as the draft ("❯ again"), while the earlier "world" batch stays queued
# (its "  ❯ world" inset row remains) and the pulled "  ❯ again" inset row is gone.
S11="${S}_altup"
launch "$S11" 80 24
submit "$S11" "hello there"
wait_for 4 "$S11" -F "Happy" # up to ~4s: wait until turn 1 is visibly streaming
submit "$S11" "world" # batch 1 = [world]
tmux send-keys -t "$S11" -l "again"
sleep 0.2
tmux send-keys -t "$S11" Tab # batch 2 = [again] — a separate follow-up turn
wait_for 3 "$S11" -F "  ❯ again" # both queued rows visible before the restore
tmux send-keys -t "$S11" M-Up
sleep 0.4
altup="$(tmux capture-pane -t "$S11" -p)"
echo "==== captured pane (Alt+Up restored only the last batch into the composer) ===="
printf '%s\n' "$altup"
tmux kill-session -t "$S11" 2>/dev/null

# Phase 14: Alt+Up restores only the LAST batch into the composer. The box shows
# "❯ again" (the Tab batch pulled back), the earlier "world" batch stays queued
# (its "  ❯ world" inset row remains), and the pulled "again" inset row is gone.
expect_has "$altup" -F "❯ again" "Alt+Up did not restore the last batch into the composer ('❯ again' draft line missing)"
expect_has "$altup" -F "  ❯ world" "Alt+Up pulled the earlier 'world' batch too — it should have stayed queued ('  ❯ world' inset row missing)"
expect_lacks "$altup" -F "  ❯ again" "the pulled 'again' batch is still shown queued after Alt+Up restored it into the composer"
