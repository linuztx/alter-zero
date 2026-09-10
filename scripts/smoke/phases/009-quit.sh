#!/usr/bin/env bash
# Phase 9 — Ctrl+C clears a typed draft (codex's composer-clear step) and the /quit comman

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Ctrl+C clears a typed draft (codex's composer-clear step) and
# the /quit command exits the app. A first Ctrl+C with text in the box must
# only empty it — the app keeps running — and typing "/quit" + Enter (the
# palette runs the highlighted command) must terminate the process, which ends
# the tmux session.
S6="${S}_quit"
launch "$S6" 80 24
tmux send-keys -t "$S6" -l "a draft the user wants gone"
sleep 0.3
tmux send-keys -t "$S6" C-c
sleep 0.4
after_clear="$(tmux capture-pane -t "$S6" -p)"
echo "==== captured pane (draft cleared by Ctrl+C) ===="
printf '%s\n' "$after_clear"
quit_alive=0
tmux has-session -t "$S6" 2>/dev/null && quit_alive=1
tmux send-keys -t "$S6" -l "/quit"
sleep 0.3
tmux send-keys -t "$S6" Enter
quit_exited=0
for _ in $(seq 1 20); do # up to ~2s for the process to exit
	if ! tmux has-session -t "$S6" 2>/dev/null; then
		quit_exited=1
		break
	fi
	sleep 0.1
done
echo "==== Phase 9: alive after Ctrl+C clear=$quit_alive, exited after /quit=$quit_exited ===="
tmux kill-session -t "$S6" 2>/dev/null

# Phase 9: Ctrl+C clears a non-empty draft (the app keeps running); /quit exits.
expect_lacks "$after_clear" -F "a draft the user" "Ctrl+C did not clear the typed draft from the input box"
if [ "$quit_alive" -ne 1 ]; then
	fail "the app quit on the first Ctrl+C instead of clearing the non-empty input"
fi
if [ "$quit_exited" -ne 1 ]; then
	fail "running /quit did not exit the app"
fi
