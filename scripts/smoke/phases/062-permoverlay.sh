#!/usr/bin/env bash
# Phase 62 — a permission request that arrives UNDER the Ctrl+O overlay, answered after the

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# a permission request that arrives UNDER the Ctrl+O overlay,
# answered after the return (docs/permissions.md). The overlay return's reflow
# rebuilds the screen with the prompt already open — a one-way reseat whose
# plain collapse used to strand the box above a band of blank rows (the
# "newlines at the bottom after answering, but only when Ctrl+O was opened
# first" bug — the resize twin Phase 60 guards, reached through the overlay
# instead). Any rebuild under an open prompt notes itself (term's
# modal-scrolled flag) and the close purge-rebuilds: box flush at the bottom,
# each message committed exactly once. The startup delay is stretched so
# Ctrl+O reliably lands in the pre-stream pause, BEFORE the request fires.
S62="${S}_permoverlay"
PERMOVERLAY_APP="env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=1200 $BIN"
launch "$S62" 80 44 "$PERMOVERLAY_APP"
# Fill the screen so the composer sits flush at the bottom (the bug needs the
# stranded band to be visible under a full screen).
for permoverlay_msg in "hello there" "tell me more about it" "and a little more"; do
	submit "$S62" "$permoverlay_msg"
	sleep 0.6
	for _ in $(seq 1 300); do
		cap="$(tmux capture-pane -t "$S62" -p)"
		if ! printf '%s' "$cap" | grep -qF "esc to interrupt"; then
			break
		fi
		sleep 0.1
	done
	sleep 0.3
done
permoverlay_before="$(tmux capture-pane -t "$S62" -p)"
permoverlay_before_row=$(printf '%s\n' "$permoverlay_before" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)
submit "$S62" "permission demo please"
sleep 0.2
# Into the transcript overlay while the dummy still pauses — the permission
# request then lands with the alternate screen up.
tmux send-keys -t "$S62" C-o
permoverlay_overlay=""
for _ in $(seq 1 200); do
	cap="$(tmux capture-pane -t "$S62" -p)"
	if printf '%s' "$cap" | grep -qF "Write(hello.py)"; then
		permoverlay_overlay="$cap"
		break
	fi
	sleep 0.05
done
sleep 0.3
echo "==== Phase 62: overlay up with the request pending underneath ===="
printf '%s\n' "$permoverlay_overlay" | tail -8
# Back to the inline view: the return repaint must show the waiting prompt.
tmux send-keys -t "$S62" C-o
permoverlay_prompt=""
for _ in $(seq 1 100); do
	cap="$(tmux capture-pane -t "$S62" -p)"
	if printf '%s' "$cap" | grep -qF "Do you want to create hello.py?"; then
		permoverlay_prompt="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 62: the prompt after the overlay return ===="
printf '%s\n' "$permoverlay_prompt"
tmux send-keys -t "$S62" -l "1"
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S62" -p)"
	if printf '%s' "$cap" | grep -qF "the file is written" &&
		! printf '%s' "$cap" | grep -qF "esc to interrupt"; then
		break
	fi
	sleep 0.05
done
sleep 0.6
permoverlay_final="$(tmux capture-pane -t "$S62" -p)"
echo "==== Phase 62: final pane (the box must be back flush at the bottom) ===="
printf '%s\n' "$permoverlay_final"
permoverlay_footer=$(printf '%s\n' "$permoverlay_final" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)
permoverlay_hist="$(tmux capture-pane -t "$S62" -p -S -200)"
permoverlay_dupes=$(printf '%s\n' "$permoverlay_hist" | grep -cF '❯ permission demo please')
tmux kill-session -t "$S62" 2>/dev/null
echo "==== Phase 62: a prompt raised under the overlay still closes flush ===="
if [ "${permoverlay_before_row:-0}" != "44" ]; then
	fail "the box was not flush at the bottom before the turn (footer on row ${permoverlay_before_row:-none} of 44), so the check proves nothing"
fi
if [ -z "$permoverlay_overlay" ]; then
	fail "the pending Write call never showed inside the overlay (the request did not land under it)"
fi
if [ -z "$permoverlay_prompt" ]; then
	fail "the permission prompt never showed after the overlay return"
fi
if [ "${permoverlay_footer:-0}" != "44" ]; then
	fail "after answering, the footer sits on row ${permoverlay_footer:-none} of the 44-row pane: the box is floating above the blank band the collapsed prompt left (the Ctrl+O-first newlines bug)"
fi
if [ "${permoverlay_dupes:-0}" != "1" ]; then
	fail "'❯ permission demo please' appears $permoverlay_dupes times in scrollback+screen (the close rebuild lost or duplicated rows)"
fi
