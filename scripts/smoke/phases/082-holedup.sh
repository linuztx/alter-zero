#!/usr/bin/env bash
# Phase 82 — mid-stream overlay round-trips neither lose NOR DOUBLE rows, and a resize unde

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# mid-stream overlay round-trips neither lose NOR DOUBLE rows,
# and a resize under the overlay still purge-rebuilds cleanly. The return
# flushes exactly the not-yet-flushed queue; bouncing through the overlay
# three times while the reply streams must leave every committed row in the
# terminal exactly once, and the resize-under-overlay return (the one case
# that still rebuilds — the emulator reflowed the main screen underneath)
# must not duplicate them either.
S82="${S}_holedup"
launch "$S82" 80 24
submit "$S82" "$USER_MSG"
wait_for 4 "$S82" -F "$EXPECT_REPLY" # wait for the stream to begin
for _ in 1 2 3; do # bounce while it streams
	tmux send-keys -t "$S82" C-o
	sleep 0.6
	tmux send-keys -t "$S82" C-o
	sleep 0.4
done
tmux send-keys -t "$S82" C-o # a resize lands UNDER the overlay…
sleep 0.3
tmux resize-window -t "$S82" -x 70 -y 20 2>/dev/null
sleep 0.4
tmux send-keys -t "$S82" C-o # …so this return purge-rebuilds
sleep 0.5
for _ in $(seq 1 100); do
	if tmux capture-pane -t "$S82" -p -S - | grep -qF "Done for"; then
		break
	fi
	sleep 0.15
done
sleep 0.5
dedup="$(tmux capture-pane -t "$S82" -p -S -)"
echo "==== Phase 82: after three mid-stream round-trips + a resize under the overlay ===="
printf '%s\n' "$dedup" | tail -40
for marker in "❯ $USER_MSG" "$EXPECT_REPLY" "Read(about.py)" "Edit(about.py)" "Bash(python3 about.py)" "$SETTLED_REPLY" "Done for"; do
	count=$(printf '%s\n' "$dedup" | grep -cF "$marker")
	if [ "$count" -ne 1 ]; then
		fail "'$marker' appears $count times after overlay round-trips (expected exactly 1)"
	fi
done
tmux kill-session -t "$S82" 2>/dev/null
