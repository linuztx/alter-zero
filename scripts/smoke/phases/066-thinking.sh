#!/usr/bin/env bash
# Phase 66 — the THINKING STREAM

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the THINKING STREAM (docs/thinking-stream.md). The model's
# chain-of-thought used to be counted and thrown away; now it streams live in
# the strip under a `● Thinking…` header over the `⎿` gutter, and COLLAPSES at
# the phase's end into one bullet-less committed
# `Thought for {n} · {t} tokens (ctrl+o to expand)` line — the reasoning text
# itself never reaching scrollback (it expands in Ctrl+O instead). Driven
# against the dummy's canned two-line reasoning: poll for the live block, then
# for the settled cell, and assert the thought's text is nowhere in the
# committed scrollback. Then the same turn with ALTER_ZERO_SHOW_THINKING=0
# must show neither.
S66="${S}_thinking"
launch "$S66" 80 24
submit "$S66" "$USER_MSG"
# The live block: the header goes up at ThinkingStart and the thought trickles
# in after it (≈2s for the canned reasoning), so poll the VISIBLE screen for the
# TEXT — polling the header alone would win on the frame before the first delta
# and then assert against an empty block.
think_live=""
think_live_header=0
for _ in $(seq 1 150); do # up to ~15s
	think_live="$(tmux capture-pane -t "$S66" -p)"
	if printf '%s' "$think_live" | grep -qF "● Thinking…"; then
		think_live_header=1
	fi
	if printf '%s' "$think_live" | grep -qF "Let me read the file first."; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 66: captured pane (the live thinking block) ===="
printf '%s\n' "$think_live"
if [ "$think_live_header" -ne 1 ]; then
	fail "the live '● Thinking…' header never showed while the model thought"
fi
expect_has "$think_live" -F "Let me read the file first." "the chain-of-thought did not stream into the live block"
# Invariant 4, LIVE: the block goes up over a finalised segment, so the header
# sits under a blank row instead of butting against the paragraph that was
# streaming (the reported bug — the flush used to wait for the phase's end, so
# the spacer only appeared when the cell collapsed, jolting it down a row).
# The header is matched with OR without its bullet: a live `●` blinks, so the
# frame that first carried the thought may have caught it hidden — two blanks
# where the bullet was, the label still in its column (docs/tool-pulse.md).
if ! printf '%s' "$think_live" | grep -B 1 -E "^(● |  )Thinking…" | head -1 | grep -qE "^[[:space:]]*$"; then
	fail "the live '● Thinking…' header is not preceded by a blank row: the segment before it was not finalised (invariant 4)"
fi
# …then the collapsed cell, once the phase ends.
think_done="$(wait_pane 20 "$S66" -S -80 -- -E "^Thought for [0-9]+s")" # up to ~20s
echo "==== Phase 66: captured pane + scrollback (the collapsed thought) ===="
printf '%s\n' "$think_done"
expect_has "$think_done" -E "^Thought for [0-9]+s · [0-9]+ tokens \(ctrl\+o to expand\)" "the phase never collapsed into a bullet-less 'Thought for Ns · N tokens (ctrl+o to expand)' line"
# …bullet-less: a settled thought is turn meta, so nothing may prefix it.
expect_lacks "$think_done" -E "^[^[:space:]]+ Thought for " "the settled line carries a bullet; it must read like the turn's '$SUMMARY_TURN1 Ns'"
expect_lacks "$think_done" -F "Then edit it and run it." "the chain-of-thought reached scrollback; only the collapsed cell may commit"
# Invariant 4: a phase that ends MID-REPLY must finalise the assistant text
# before it (the ToolStart dance), so the cell is its own block rather than a
# line spliced into the paragraph that was streaming. The dummy's default turn
# is exactly that shape — text, then thinking — so the row above the cell must
# be blank, and the reply must resume as a fresh `● …` bullet below it.
if ! printf '%s' "$think_done" | grep -B 1 -E "^Thought for" | head -1 | grep -qE "^[[:space:]]*$"; then
	fail "the thought cell was spliced into the streaming reply instead of following a finalised segment (invariant 4)"
fi
# Ctrl+O expands it back.
tmux send-keys -t "$S66" C-o
sleep 0.6
think_overlay="$(tmux capture-pane -t "$S66" -p -S -200)"
if ! printf '%s' "$think_overlay" | grep -qF "Then edit it and run it."; then
	echo "==== Phase 66: captured overlay ===="
	printf '%s\n' "$think_overlay"
	fail "the Ctrl+O transcript did not expand the thought's chain-of-thought"
fi
tmux send-keys -t "$S66" C-o
sleep 0.3
tmux kill-session -t "$S66" 2>/dev/null

# The off switch: same turn, ALTER_ZERO_SHOW_THINKING=0 → neither the live
# block nor the collapsed cell, and the turn still settles normally.
S66B="${S}_nothinking"
APP_NOTHINK="env $CFG_ENV_NOHIST ALTER_ZERO_SHOW_THINKING=0 ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
launch "$S66B" 80 24 "$APP_NOTHINK"
submit "$S66B" "$USER_MSG"
nothink="$(wait_pane 25 "$S66B" -S -80 -- -E "^$SUMMARY_TURN1 [0-9]+s")" # up to ~25s
echo "==== Phase 66: captured pane (ALTER_ZERO_SHOW_THINKING=0) ===="
printf '%s\n' "$nothink"
expect_has "$nothink" -E "^$SUMMARY_TURN1 [0-9]+s" "the turn never settled with the thinking display off"
expect_lacks "$nothink" -E "Thinking…|Thought for" "ALTER_ZERO_SHOW_THINKING=0 still showed the model's thinking"
tmux kill-session -t "$S66B" 2>/dev/null
