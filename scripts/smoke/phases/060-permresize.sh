#!/usr/bin/env bash
# Phase 60 — a RESIZE while a permission prompt is open, then the answer

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# a RESIZE while a permission prompt is open, then the answer
# (docs/permissions.md). The resize purge-rebuilds the screen, reseating the
# prompt below the rebuilt tail — a one-way move the plain collapse cannot
# undo, so it used to strand the box above a band of blank rows (the
# "newlines under the composer after a resized prompt" bug). The reflow notes
# it (term's modal-scrolled flag) and the close purge-rebuilds: box flush at
# the bottom, each message committed exactly once. The answer is a reject,
# whose few committed rows can't mask a leftover hole by walking the box back
# down.
S60="${S}_permresize"
launch "$S60" 80 44
for permresize_msg in "hello there" "tell me more about it" "and a little more"; do
	submit "$S60" "$permresize_msg"
	# Wait for the turn to START before waiting for it to end: the settle loop
	# below breaks on the ABSENCE of the status line, so on a loaded machine —
	# eight workers on four cores — it can sample before the first frame paints,
	# break at once and race the turn it was meant to wait out. Phase 62 failed
	# exactly that way in a full parallel run while passing alone.
	wait_for 20 "$S60" -F "esc to interrupt"
	sleep 0.6
	for _ in $(seq 1 300); do
		cap="$(tmux capture-pane -t "$S60" -p)"
		if ! printf '%s' "$cap" | grep -qF "esc to interrupt"; then
			break
		fi
		sleep 0.1
	done
	sleep 0.3
done
submit "$S60" "permission demo please"
permresize_prompt=""
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S60" -p)"
	if printf '%s' "$cap" | grep -qF "Do you want to create hello.py?"; then
		permresize_prompt="$cap"
		break
	fi
	sleep 0.05
done
# Shrink the pane while the prompt is up — the purge-rebuild path — and let
# the redraw settle before answering.
tmux resize-window -t "$S60" -x 76 -y 44
sleep 0.8
permresize_resized="$(tmux capture-pane -t "$S60" -p)"
echo "==== Phase 60: pane after the mid-prompt resize ===="
printf '%s\n' "$permresize_resized"
tmux send-keys -t "$S60" -l "3"
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S60" -p)"
	if printf '%s' "$cap" | grep -qF "left the file alone" &&
		! printf '%s' "$cap" | grep -qF "esc to interrupt"; then
		break
	fi
	sleep 0.05
done
sleep 0.6
permresize_final="$(tmux capture-pane -t "$S60" -p)"
echo "==== Phase 60: final pane (the box must be back flush at the bottom) ===="
printf '%s\n' "$permresize_final"
permresize_footer=$(printf '%s\n' "$permresize_final" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)
permresize_hist="$(tmux capture-pane -t "$S60" -p -S -200)"
permresize_dupes=$(printf '%s\n' "$permresize_hist" | grep -cF '❯ permission demo please')
tmux kill-session -t "$S60" 2>/dev/null
echo "==== Phase 60: a resized prompt's close still lands the box flush at the bottom ===="
if [ -z "$permresize_prompt" ]; then
	fail "the permission prompt never showed"
fi
expect_has "$permresize_resized" -F "Do you want to create hello.py?" "the prompt did not survive the resize"
if [ "${permresize_footer:-0}" != "44" ]; then
	fail "after answering the resized prompt the footer sits on row ${permresize_footer:-none} of the 44-row pane: the box is floating above a band of blank rows"
fi
if [ "${permresize_dupes:-0}" != "1" ]; then
	fail "'❯ permission demo please' appears $permresize_dupes times in scrollback+screen"
fi
