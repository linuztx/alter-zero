#!/usr/bin/env bash
# Phase 34 — the hardware cursor is HIDDEN for the whole of a redraw so a terminal cursor-t

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the hardware cursor is HIDDEN for the whole of a redraw so a
# terminal cursor-trail animation (kitty) can't streak across the screen when a
# reflow drags the cursor around — the fix for "the cursor animation starts on
# top" on a resize (term.rs hides the cursor right after opening the frame's
# synchronized update and reshows it only at the prompt seat). RECORD THE RAW
# OUTPUT STREAM (pipe-pane, like Phase 15) across a resize: the Purge reflow
# homes the cursor to the top (ESC[H) and clears the screen (ESC[2J/ESC[3J), so
# the frame must emit a Hide (ESC[?25l) BEFORE that home/clear and a Show
# (ESC[?25h) after. Pre-fix the reflow only ever Showed the cursor at the end, so
# no ESC[?25l ever rode the resize frame → the visible cursor got dragged to the
# top → the trail.
S34="${S}_curhide"
RAW34="$(mktemp)"
launch "$S34" 80 24
submit "$S34" "hello there"
curhide_done=0
for _ in $(seq 1 200); do # up to ~20s for the committed summary (a seated box)
	if tmux capture-pane -t "$S34" -p | grep -qE "^$SUMMARY_TURN1 [0-9]+s"; then
		curhide_done=1
		break
	fi
	sleep 0.1
done
# Record raw app output, then resize twice (width AND height → the Purge reflow
# rebuilds from history, homing the cursor to the top first).
tmux pipe-pane -t "$S34" -o "cat > $RAW34"
sleep 0.2
tmux resize-window -t "$S34" -x 100 -y 30
sleep 0.6
tmux resize-window -t "$S34" -x 72 -y 20
sleep 0.6
tmux pipe-pane -t "$S34" # stop recording
# Tokenise each cursor Hide/Show and each screen home/clear onto its own line
# (NR = stream order), then reason about the ordering: at least one Hide, the
# first Hide before the first home/clear, and a Show after the Hide.
cursor_hide_tokens=$(sed \
	-e $'s/\x1b\[?25l/\\\n@HIDE@\\\n/g' \
	-e $'s/\x1b\[?25h/\\\n@SHOW@\\\n/g' \
	-e $'s/\x1b\[2J/\\\n@CLR@\\\n/g' \
	-e $'s/\x1b\[3J/\\\n@CLR@\\\n/g' \
	-e $'s/\x1b\[H/\\\n@HOME@\\\n/g' \
	"$RAW34" | awk '
	/@HIDE@/ { hide++; if (first_hide == 0) first_hide = NR; next }
	/@SHOW@/ { show++; last_show = NR; next }
	/@CLR@/  { clr++;  if (first_clr  == 0) first_clr  = NR; next }
	/@HOME@/ { home++; if (first_home == 0) first_home = NR; next }
	END {
		earliest = first_clr
		if (first_home > 0 && (earliest == 0 || first_home < earliest)) earliest = first_home
		before = (hide > 0 && earliest > 0 && first_hide < earliest) ? 1 : 0
		shown  = (show > 0 && (hide == 0 || first_hide < last_show)) ? 1 : 0
		printf "ch_hide=%d ch_show=%d ch_clrhome=%d ch_before=%d ch_shown=%d",
			hide + 0, show + 0, (clr + home) + 0, before, shown
	}')
rm -f "$RAW34"
tmux kill-session -t "$S34" 2>/dev/null
echo "==== Phase 34: resize reflow raw-stream cursor tokens — $cursor_hide_tokens ===="
eval "$cursor_hide_tokens"

# Phase 34: a resize reflow hides the hardware cursor before homing/clearing the
# screen (so kitty's cursor-trail can't streak from the top) and reshows it at
# the prompt seat. Read from the RAW output stream recorded across the resize.
if [ "${curhide_done:-0}" -ne 1 ]; then
	fail "precondition — the turn never finished, the resize reflow was never probed"
fi
if [ "${ch_clrhome:-0}" -eq 0 ]; then
	fail "precondition — the resize produced no screen home/clear in the raw stream (did the reflow run?)"
fi
if [ "${ch_hide:-0}" -eq 0 ]; then
	fail "the resize reflow never HID the cursor (no ESC[?25l) — kitty's cursor-trail streaks from the top"
fi
if [ "${ch_before:-0}" -ne 1 ]; then
	fail "the cursor was hidden only AFTER the screen was homed/cleared — the cursor-trail already fired"
fi
if [ "${ch_shown:-0}" -ne 1 ]; then
	fail "the cursor was not reshown (ESC[?25h) at its prompt seat after the reflow — it would stay hidden"
fi
