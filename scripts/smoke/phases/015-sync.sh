#!/usr/bin/env bash
# Phase 15 — scrollback commits are FLICKER-FREE

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# scrollback commits are FLICKER-FREE — every live-region clear
# rides inside a synchronized-update frame (docs/flicker.md). Record the raw
# byte stream of a whole streaming turn via pipe-pane: each ESC[J region clear
# (a commit blanking the live region before its repaint) must sit between
# ESC[?2026h and ESC[?2026l, so a terminal honouring mode 2026 can never
# present the boxless intermediate state. Before the fix insert_before cleared
# the region OUTSIDE any sync block and the box repaint came on a later flush —
# the streaming blink (the pre-fix stream shows every clear outside). The pipe
# closes before the quit: restore()'s teardown clear is legitimately bare.
S12="${S}_sync"
RAW15="$(mktemp)"
launch "$S12" 80 24
tmux pipe-pane -t "$S12" -o "cat > $RAW15"
submit "$S12" "hello there"
wait_for 20.1 "$S12" -S -60 -- -F "$SUMMARY_TURN1" # up to ~20s: the whole turn (text + thinking + tools)
tmux pipe-pane -t "$S12" # close the recording before quitting
sleep 0.2
tmux send-keys -t "$S12" C-c # quit (idle Esc would arm the backtrack instead)
sleep 0.2
tmux kill-session -t "$S12" 2>/dev/null
# Tokenise the stream: each sync begin/end and each ESC[J / ESC[0J clear onto
# its own line, then walk them in order counting clears outside a block.
sync_clears=$(sed -e $'s/\x1b\[?2026h/\\\n@SYNC@\\\n/g' \
	-e $'s/\x1b\[?2026l/\\\n@ENDS@\\\n/g' \
	-e $'s/\x1b\[0\{0,1\}J/\\\n@CLRJ@\\\n/g' "$RAW15" | awk '
	/@SYNC@/ { depth = 1; seen = 1; frames++; next }
	/@ENDS@/ { depth = 0; next }
	/@CLRJ@/ { total++; if (seen && depth == 0) bad++ }
	END { printf "frames=%d total=%d outside=%d", frames + 0, total + 0, bad + 0 }')
rm -f "$RAW15"
echo "==== Phase 15: live-region clears in the raw output stream — $sync_clears ===="

# Phase 15: flicker-free commits (docs/flicker.md). The recording must hold
# synchronized frames (so the meter saw the turn at all), and every
# live-region clear must sit inside one — a clear outside means a terminal
# could present the boxless state (the streaming blink). A commit no longer
# clears the region at all — it is repainted in place, so `total=0` is the
# design (docs/slow-stream.md; Phase 121 pins that count) and the liveness
# check is on the frames.
case "$sync_clears" in
frames=0*)
	fail "the recorded turn shows no synchronized frames — the draw path did not run (recording broken?)"
	;;
esac
case "$sync_clears" in
*outside=0) ;;
*)
	fail "live-region clears OUTSIDE a synchronized-update frame ($sync_clears) — scrollback commits can flicker the box"
	;;
esac
