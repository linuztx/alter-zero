#!/usr/bin/env bash
# Phase 17 — a terminal RESIZE re-presents the conversation at the new size

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# a terminal RESIZE re-presents the conversation at the new size —
# HEIGHT-ONLY changes included. codex redraws everything from source on every
# resize (and re-clamps its viewport into the new screen); the pre-fix bug here
# reflowed only on a *width* change, so a height-only resize repainted the box at
# a stale viewport row while the emulator had already moved the screen contents —
# leaving phantom input boxes on screen and pushing the conversation out of view.
# Finish a turn at 80x24, shrink to 80x12 (height only), grow back to 80x24, then
# shrink the height again MID-STREAM: after each step the visible screen must
# hold exactly ONE input box (one bare `❯` prompt row, two rules, one footer)
# with the conversation tail above it.
S14="${S}_resize"
launch "$S14" 80 24
submit "$S14" "hello there"
wait_for 20 "$S14" -E "^$SUMMARY_TURN1 [0-9]+s" # up to ~20s: wait for the turn's committed summary
tmux resize-window -t "$S14" -x 80 -y 12
sleep 0.6
resize_shrunk="$(tmux capture-pane -t "$S14" -p)"
echo "==== captured visible screen (after height-only shrink to 80x12) ===="
printf '%s\n' "$resize_shrunk"
tmux resize-window -t "$S14" -x 80 -y 24
sleep 0.6
resize_regrown="$(tmux capture-pane -t "$S14" -p -S -60)"
echo "==== captured pane (+scrollback) after height grow back to 80x24 ===="
printf '%s\n' "$resize_regrown"
# A WIDTH change is the classic duplication trigger: the emulator re-wraps the
# on-screen lines, and the old in-place overwrite left that reflowed copy behind
# (the TUI-text duplication this fix targets). The purge-mode resize clears the
# screen + scrollback and rebuilds the whole conversation from history, so the
# user message stays SINGLE. Shrink the width, capture the FULL pane (-S), then
# restore to 80x24 so the mid-stream section below starts where it expects.
tmux resize-window -t "$S14" -x 50 -y 24
sleep 0.6
resize_narrow_full="$(tmux capture-pane -t "$S14" -p -S -200)"
echo "==== captured full pane (-S) after width shrink to 50x24 — no duplication ===="
printf '%s\n' "$resize_narrow_full"
tmux resize-window -t "$S14" -x 80 -y 24
sleep 0.6
# A height shrink MID-STREAM must recover the same way: the repaint resets the
# committed count, so the in-flight reply re-commits itself at the new size as
# the remaining chunks flow ($SUMMARY_TURN2 is turn 2's summary).
submit "$S14" "again please"
sleep 0.7 # mid-stream: the first text segment is flowing
tmux resize-window -t "$S14" -x 80 -y 14
resize_mid="$(wait_pane 20.1 "$S14" -E "^$SUMMARY_TURN2 [0-9]+s")" # up to ~20s: wait for the resized turn's summary
echo "==== captured visible screen (height shrunk mid-stream, turn finished at 80x14) ===="
printf '%s\n' "$resize_mid"
tmux kill-session -t "$S14" 2>/dev/null

# Phase 17: every resize re-presents the conversation at the new size. Each
# captured screen must hold exactly one input box — phantom boxes (extra bare
# prompts / rules / footers) are the stale-viewport-row bug — with the
# conversation tail (the turn's committed summary) still in view, and no stale
# streaming strip ("esc to interrupt" rides the live status line only).
for step in shrunk regrown mid; do
	case "$step" in
	shrunk)
		cap="$resize_shrunk"
		label="height-only shrink to 80x12"
		tail_marker="$SUMMARY_TURN1"
		;;
	regrown)
		cap="$resize_regrown"
		label="height grow back to 80x24"
		tail_marker="$SUMMARY_TURN1"
		;;
	mid)
		cap="$resize_mid"
		label="mid-stream height shrink to 80x14"
		tail_marker="$SUMMARY_TURN2"
		;;
	esac
	prompts="$(count_bare_prompts "$cap")"
	rules="$(count_rules "$cap")"
	footers="$(count_footers "$cap")"
	if [ "$prompts" != "1" ] || [ "$rules" != "2" ] || [ "$footers" != "1" ]; then
		fail "after the $label the screen does not hold exactly one input box (bare prompts=$prompts, rules=$rules, footers=$footers)"
	fi
	expect_has "$cap" -F "$tail_marker" "after the $label the conversation tail ('$tail_marker') is not in view"
	expect_lacks "$cap" -F "esc to interrupt" "after the $label a stale streaming status line is still on screen"
done
# The whole conversation is back once the screen regrows: the repaint must
# rebuild the *whole* tail from history, not just the rows the shrunken screen
# showed. Read with scrollback (the resize reflow purges it and rebuilds from
# history, so what `-S` holds is exactly what this repaint wrote) — a demo turn
# is taller than 24 rows, so "rebuilt" and "on the visible screen" stopped
# meaning the same thing.
expect_has "$resize_regrown" -F "❯ $USER_MSG" "after growing back to 80x24 the user message did not return to view"
expect_has "$resize_regrown" -F "$EXPECT_REPLY" "after growing back to 80x24 the reply did not return to view"
# No duplication: after a WIDTH resize the whole pane (visible + scrollback) must
# hold the user message EXACTLY once. The old in-place overwrite left the
# emulator's own reflowed copy behind on a width change — the TUI-text
# duplication this fix targets; the purge-mode rebuild keeps it single.
resize_dup="$(printf '%s\n' "$resize_narrow_full" | grep -cF "❯ $USER_MSG")"
if [ "$resize_dup" != "1" ]; then
	fail "after a width resize the conversation is duplicated ('❯ $USER_MSG' ×$resize_dup, expected 1) — the reflowed copy was not cleared"
fi
