#!/usr/bin/env bash
# Focused Red→Green probe for the cursor-hide fix (docs: the term.rs frame hides
# the hardware cursor for the whole redraw so kitty's cursor-trail can't streak
# from the top on a resize / reflow). Drive the real binary in tmux, finish a
# turn, then RECORD THE RAW OUTPUT STREAM (pipe-pane, like smoke.sh Phase 15)
# across a terminal resize — which runs the Purge reflow that homes the cursor to
# the top (ESC[H) and clears the screen (ESC[2J/ESC[3J). Assert the reflow HIDES
# the cursor (ESC[?25l) before that clear and SHOWS it (ESC[?25h) after.
#
# Pre-fix: the reflow only ever Showed the cursor at the end, so no ESC[?25l ever
# rode the resize frame → this probe FAILS (Red). Post-fix: the frame opens with
# a Hide → this probe PASSES (Green).
#
# Usage: scripts/cursor_hide_check.sh [path/to/binary]
set -uo pipefail

BIN="${1:-target/debug/inline-tui}"
if [ ! -x "$BIN" ]; then
	echo "FAIL: binary not found at $BIN (run: cargo build)" >&2
	exit 1
fi

S="curhide_$$"
CFG="$(mktemp -d)"
RAW="$(mktemp)"
cleanup() {
	tmux kill-session -t "$S" 2>/dev/null
	rm -rf "$CFG" 2>/dev/null
	rm -f "$RAW" 2>/dev/null
}
trap cleanup EXIT

APP="env INLINE_TUI_CONFIG_DIR=$CFG INLINE_TUI_STARTUP_DELAY_MS=200 $BIN"

tmux new-session -d -s "$S" -x 80 -y 24 "$APP"
sleep 0.5
# Finish one turn so there is real conversation content on screen and the box is
# seated at the bottom — the state where a resize reflow has the most cursor to
# move (worst case for the trail).
tmux send-keys -t "$S" -l "hello there"
sleep 0.2
tmux send-keys -t "$S" Enter
done_seen=0
for _ in $(seq 1 80); do # up to ~8s for the committed summary
	if tmux capture-pane -t "$S" -p | grep -qE "^Done for [0-9]+s"; then
		done_seen=1
		break
	fi
	sleep 0.1
done
if [ "$done_seen" -ne 1 ]; then
	echo "FAIL: precondition — the turn never finished, cannot probe the resize reflow" >&2
	exit 1
fi

# Start recording the RAW app output, then resize (width AND height change → the
# Purge reflow rebuilds from history, homing the cursor to the top first).
tmux pipe-pane -t "$S" -o "cat > $RAW"
sleep 0.2
tmux resize-window -t "$S" -x 100 -y 30
sleep 0.6
tmux resize-window -t "$S" -x 72 -y 20
sleep 0.6
tmux pipe-pane -t "$S" # stop recording

# Tokenise the raw stream: each cursor Hide/Show and each screen clear/home onto
# its own line (NR order = stream order), then reason about the ordering.
read -r result < <(sed \
	-e $'s/\x1b\[?25l/\\\n@HIDE@\\\n/g' \
	-e $'s/\x1b\[?25h/\\\n@SHOW@\\\n/g' \
	-e $'s/\x1b\[2J/\\\n@CLR@\\\n/g' \
	-e $'s/\x1b\[3J/\\\n@CLR@\\\n/g' \
	-e $'s/\x1b\[H/\\\n@HOME@\\\n/g' \
	"$RAW" | awk '
	/@HIDE@/ { hide++; if (first_hide == 0) first_hide = NR; next }
	/@SHOW@/ { show++; if (first_show == 0) first_show = NR; last_show = NR; next }
	/@CLR@/  { clr++;  if (first_clr  == 0) first_clr  = NR; next }
	/@HOME@/ { home++; if (first_home == 0) first_home = NR; next }
	END {
		printf "hide=%d show=%d clr=%d home=%d first_hide=%d first_clr=%d first_home=%d last_show=%d",
			hide+0, show+0, clr+0, home+0,
			first_hide+0, first_clr+0, first_home+0, last_show+0
	}')

echo "resize reflow raw-stream tokens: $result"
eval "$result"

status=0
if [ "${clr:-0}" -eq 0 ] && [ "${home:-0}" -eq 0 ]; then
	echo "FAIL: precondition — the resize produced no clear/home in the captured stream (did the reflow run?)" >&2
	status=1
fi
if [ "${hide:-0}" -eq 0 ]; then
	echo "FAIL: the resize reflow never HID the cursor (no ESC[?25l) — kitty's cursor-trail will streak from the top" >&2
	status=1
fi
# The cursor must be hidden BEFORE the screen is homed/cleared, so it is never a
# visible cursor that the terminal drags to the top.
earliest_move=0
if [ "${first_clr:-0}" -ne 0 ]; then earliest_move="$first_clr"; fi
if [ "${first_home:-0}" -ne 0 ] && { [ "$earliest_move" -eq 0 ] || [ "$first_home" -lt "$earliest_move" ]; }; then
	earliest_move="$first_home"
fi
if [ "${hide:-0}" -ne 0 ] && [ "$earliest_move" -ne 0 ] && [ "${first_hide:-0}" -gt "$earliest_move" ]; then
	echo "FAIL: the cursor was hidden ($first_hide) only AFTER the screen was homed/cleared ($earliest_move) — the trail already fired" >&2
	status=1
fi
# And it must be shown again at the end (its final prompt seat), not left hidden.
if [ "${show:-0}" -eq 0 ]; then
	echo "FAIL: the resize reflow never SHOWED the cursor again (ESC[?25h) — the composer cursor would stay hidden" >&2
	status=1
fi
if [ "${hide:-0}" -ne 0 ] && [ "${last_show:-0}" -ne 0 ] && [ "${first_hide:-0}" -gt "${last_show:-0}" ]; then
	echo "FAIL: the final Show came before the Hide — the cursor is left hidden after the reflow" >&2
	status=1
fi

if [ "$status" -eq 0 ]; then
	echo "PASS: the resize reflow hides the cursor before homing/clearing the screen and reshows it at the end (no cursor-trail streak)."
fi
exit "$status"
