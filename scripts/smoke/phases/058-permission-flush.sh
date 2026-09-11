#!/usr/bin/env bash
# Phase 58 — the conversation stays REACHABLE while a permission prompt is open

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the conversation stays REACHABLE while a permission prompt is
# open (docs/permissions.md). The prompt grows like any other region — the
# chat above it scrolls into the terminal's real scrollback, so the user can
# scroll up and re-read what the model said before answering (the covering
# modal used to hold the newest screenful in NO buffer: not on screen, not in
# scrollback — the "terminal scroll is disabled while it asks" bug, worst in
# kitty). The close then purge-rebuilds off the viewport's modal-scrolled
# note, so the box still comes back flush at the bottom with the conversation
# whole and committed exactly once — the invariant this phase has always
# guarded, now reached by scrolling instead of covering.
S58="${S}_permission_flush"
launch "$S58" 80 30
# One real turn: its reply is what the user will want to scroll back to while
# the (screen-tall) prompt is up.
submit "$S58" "hello there"
sleep 0.6
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S58" -p)"
	if ! printf '%s' "$cap" | grep -qF "esc to interrupt"; then
		break
	fi
	sleep 0.1
done
sleep 0.3
submit "$S58" "permission demo please"
permflush_prompt=""
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S58" -p)"
	if printf '%s' "$cap" | grep -qF "Do you want to create hello.py?"; then
		permflush_prompt="$cap"
		break
	fi
	sleep 0.05
done
sleep 0.3
echo "==== Phase 58: the prompt open (screen-tall on a 30-row pane) ===="
printf '%s\n' "$permflush_prompt"
# THE point of this phase: while the prompt is up, the earlier reply must be
# somewhere the user can scroll to — the screen or the terminal's scrollback
# (capture -S reads both) — and exactly once (no replay double-paint).
permflush_reach="$(tmux capture-pane -t "$S58" -p -S -300)"
permflush_reach_count=$(printf '%s\n' "$permflush_reach" | grep -cF "Happy to help")
echo "==== Phase 58: 'Happy to help' reachable while the prompt is open: $permflush_reach_count time(s) ===="
tmux send-keys -t "$S58" -l "1"
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S58" -p)"
	if printf '%s' "$cap" | grep -qF "the file is written" &&
		! printf '%s' "$cap" | grep -qF "esc to interrupt"; then
		break
	fi
	sleep 0.05
done
sleep 0.5
permflush_after="$(tmux capture-pane -t "$S58" -p)"
echo "==== Phase 58: pane after answering (the box must be back flush at the bottom) ===="
printf '%s\n' "$permflush_after"
permflush_after_row=$(printf '%s\n' "$permflush_after" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)
# …and the conversation is whole and committed EXACTLY once: the close's purge
# rebuild replaces screen and scrollback together, so nothing the prompt's
# growth scrolled away can come back a second time.
permflush_hist="$(tmux capture-pane -t "$S58" -p -S -300)"
permflush_dupes=$(printf '%s\n' "$permflush_hist" | grep -cF '❯ permission demo please')
permflush_reply_dupes=$(printf '%s\n' "$permflush_hist" | grep -cF "Happy to help")
tmux kill-session -t "$S58" 2>/dev/null
echo "==== Phase 58: a prompt scrolls the chat into real scrollback and closes flush ===="
if [ -z "$permflush_prompt" ]; then
	fail "the permission prompt never showed"
fi
if [ "${permflush_reach_count:-0}" != "1" ]; then
	fail "while the prompt is open the earlier reply appears $permflush_reach_count times in screen+scrollback: the user cannot scroll up to it (the covered rows live in no buffer)"
fi
if [ "${permflush_after_row:-0}" != "30" ]; then
	fail "after answering, the footer sits on row ${permflush_after_row:-none} of the 30-row pane: the box is floating above a band of blank rows"
fi
if [ "${permflush_dupes:-0}" != "1" ]; then
	fail "'❯ permission demo please' appears $permflush_dupes times in scrollback+screen after the close"
fi
if [ "${permflush_reply_dupes:-0}" != "1" ]; then
	fail "the earlier reply appears $permflush_reply_dupes times in scrollback+screen after the close (the purge rebuild lost or doubled it)"
fi
