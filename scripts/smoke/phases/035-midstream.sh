#!/usr/bin/env bash
# Phase 35 — a Ctrl+O round trip MID-STREAM keeps the already-streamed partial reply on the

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# a Ctrl+O round trip MID-STREAM keeps the already-streamed partial
# reply on the restored screen. The dummy pauses ~1.2s in its thinking phase right
# after streaming the first half of the reply — a deterministic window where the
# streaming buffer is non-empty and NO chunk will arrive to repair the screen.
# Pre-fix, the return repainted from history alone, so the partial's committed
# rows vanished until the next chunk re-inserted the whole partial from scratch —
# the disappear-then-flicker bug (and, on long replies, duplicated rows already
# scrolled into the terminal's kept scrollback).
S35="${S}_midstream"
launch "$S35" 80 24
submit "$S35" "hello there"
# Wait for the thinking pause (the status gains "Thinking for") — the first half
# of the reply has streamed, and its completed rows are committed, by then.
midstream_thinking=0
for _ in $(seq 1 60); do # up to ~6s
	if tmux capture-pane -t "$S35" -p | grep -qF "Thinking for"; then
		midstream_thinking=1
		break
	fi
	sleep 0.1
done
tmux send-keys -t "$S35" C-o
sleep 0.15
tmux send-keys -t "$S35" C-o # straight back, well inside the thinking pause
sleep 0.15
midstream_returned="$(tmux capture-pane -t "$S35" -p -S -60)"
echo "==== Phase 35: returned from Ctrl+O mid-stream (thinking pause still open) ===="
printf '%s\n' "$midstream_returned"
# Let the turn finish, then check the reply committed exactly ONCE — the return's
# catch-up must not re-insert rows the screen already holds.
midstream_done="$(wait_pane 20 "$S35" -S -80 -- -E "^Done for [0-9]+s")" # up to ~20s
midstream_dupes=$(printf '%s\n' "$midstream_done" | grep -cF "Happy to help")
tmux kill-session -t "$S35" 2>/dev/null

# Phase 35: a mid-stream Ctrl+O round trip keeps the streamed partial visible.
if [ "${midstream_thinking:-0}" -ne 1 ]; then
	fail "precondition — the thinking pause was never observed, so the mid-stream return was not probed (retune the timing)"
fi
expect_has "$midstream_returned" -F "esc to interrupt" "precondition — the turn was no longer in flight when the overlay returned (too slow to probe the bug)"
expect_has "$midstream_returned" -F "Happy to help" "the streamed partial reply vanished from the restored screen after a mid-stream Ctrl+O round trip (the disappear-then-flicker bug)"
if [ "${midstream_dupes:-0}" != "1" ]; then
	fail "the reply text appears ${midstream_dupes} times after the turn settled (expected exactly 1 — the overlay catch-up re-inserted rows the screen already had)"
fi
