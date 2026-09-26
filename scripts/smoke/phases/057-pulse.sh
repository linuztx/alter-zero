#!/usr/bin/env bash
# Phase 57 — the running bullet's BLINK

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the running bullet's BLINK (docs/tool-pulse.md). A tool in
# flight no longer shows a blue `●` — it shows the permission prompt's grey,
# and in the live region that bullet BLINKS: shown for half a second, hidden
# behind blanks of its own width for the next, the header text keeping its
# column meanwhile — Claude Code's running dot. Colour is only half of it: the
# point is that it MOVES, which no unit test can see. Sample the painted pane
# across frames while a batch runs: the running call's header must be seen
# both WITH its bullet and WITHOUT it, no frame may hide more than one bullet
# (only the running call blinks — its `⎿ Waiting…` siblings hold their `●`),
# a shown bullet must wear the one resting grey and never the retired breath's
# darker shade, and the resolved cell must still land green.
S57="${S}_pulse"
launch "$S57" 100 34
# The dummy's `parallel` turn: three long Bash(ping …) calls, so one is
# running while the others wait — both states on screen at once.
submit "$S57" "parallel"
pulse_ready=""
for _ in $(seq 1 250); do
	cap="$(tmux capture-pane -t "$S57" -p)"
	if printf '%s' "$cap" | grep -qF "Waiting…"; then
		pulse_ready="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 57: captured pane (a batch mid-flight) ===="
printf '%s\n' "$pulse_ready"
# Per frame: how many `Bash(ping …)` headers wear their `●`, and how many
# stand on two blanks instead — `  Bash(ping …)`, the hidden half of the
# blink, the text still in its column. A second read of the frame with
# `capture-pane -e` keeps the SGR sequences, so the shown bullets' colours
# ride along: each bullet is `ESC[1m ESC[38;2;R;G;Bm ●`.
pulse_samples=""
pulse_colours=""
for _ in $(seq 1 30); do
	frame="$(tmux capture-pane -t "$S57" -p 2>/dev/null)"
	shown="$(printf '%s\n' "$frame" | grep -cE '^● Bash\(ping ')"
	hidden="$(printf '%s\n' "$frame" | grep -cE '^  Bash\(ping ')"
	pulse_samples="$pulse_samples$shown,$hidden
"
	colours="$(tmux capture-pane -t "$S57" -p -e 2>/dev/null |
		grep -oE $'\x1b\\[1m\x1b\\[38;2;[0-9]+;[0-9]+;[0-9]+m●' |
		grep -oE '[0-9]+;[0-9]+;[0-9]+m' | tr -d 'm' | paste -sd, -)"
	pulse_colours="$pulse_colours$colours
"
	sleep 0.07
done
echo "==== Phase 57: sampled Bash(ping …) headers per frame (shown,hidden) ===="
printf '%s' "$pulse_samples"
echo "==== Phase 57: sampled bullet colours (one frame per line) ===="
printf '%s' "$pulse_colours"
# Across the frames: how many caught the running bullet hidden, how many
# caught every header shown, and whether any frame hid more than one header
# at once (a `⎿ Waiting…` sibling blinking, which it must not).
pulse_stats="$(printf '%s' "$pulse_samples" | awk -F, '
	NF {
		if ($2 + 0 >= 1) hidden_frames++
		if ($2 + 0 == 0 && $1 + 0 >= 1) shown_frames++
		if ($2 + 0 > 1) doubles++
	}
	END { print hidden_frames + 0, shown_frames + 0, doubles + 0 }')"
pulse_hidden="${pulse_stats%% *}"
pulse_rest="${pulse_stats#* }"
pulse_shown="${pulse_rest%% *}"
pulse_doubles="${pulse_rest#* }"
# The one grey a bullet may wear: the default theme's `tool_running_color()`
# (= `tool_waiting_color()`), Catppuccin Mocha's `dim` #7F849C = 127;132;156
# (docs/theme.md; `ui/palette.rs`). A "grey" here is that grey or darker on
# EVERY channel — the text colour, the green and the red all exceed it on
# some channel — so a darker grey can only be a second shade of the bullet.
pulse_second_grey="$(printf '%s' "$pulse_colours" | tr ',' '\n' | awk -F';' '
	NF == 3 && $1 + 0 <= 127 && $2 + 0 <= 132 && $3 + 0 <= 156 && $0 != "127;132;156" { print; exit }')"
echo "==== Phase 57: frames with the running bullet hidden: $pulse_hidden · every header shown: $pulse_shown · more than one hidden: $pulse_doubles ===="
# Let the turn finish so the resolved colour can be checked.
pulse_done=""
for _ in $(seq 1 400); do
	cap="$(tmux capture-pane -t "$S57" -p -e 2>/dev/null)"
	if printf '%s' "$cap" | grep -qE "$SUMMARY_RE"; then
		pulse_done="$cap"
		break
	fi
	sleep 0.1
done
tmux kill-session -t "$S57" 2>/dev/null
echo "==== Phase 57: running bullet blinks, waiting stays put, resolved lands green ===="
if [ -z "$pulse_ready" ]; then
	fail "the parallel batch never showed a running + waiting pair"
fi
# No blue bullet anywhere: #61AFEF is 97;175;239.
expect_lacks "$pulse_colours" "97;175;239" "a bullet is still painted the old blue"
# The blink: the running header was caught without its bullet…
if [ "${pulse_hidden:-0}" -lt 1 ]; then
	fail "the running bullet never hid (the blink is not animating)"
fi
# …and with it — on a frame where every header wore one.
if [ "${pulse_shown:-0}" -lt 1 ]; then
	fail "no frame showed every header's bullet (the running bullet never came back)"
fi
# Only the running call blinks: a queued sibling keeps its `●` on every frame.
if [ "${pulse_doubles:-0}" -ne 0 ]; then
	fail "a frame hid more than one bullet (a ⎿ Waiting… sibling blinked; only the running call may)"
fi
# One grey, never two: the blink hides the bullet, it does not dim it. The
# retired breath's dark end (Mocha's `pulse_dim` #585B70 = 88;91;112) or any
# other shade below the resting grey is the bug this phase exists to catch.
if [ -n "$pulse_second_grey" ]; then
	fail "a bullet wore a second grey ($pulse_second_grey): the blink must hide the bullet, never dim it"
fi
expect_has "$pulse_colours" "127;132;156" "no bullet wore the resting grey"
if [ -z "$pulse_done" ]; then
	fail "the batch never finished"
fi
# Green — the default theme's `tool_ok_color()`, Mocha's #A6E3A1 = 166;227;161 —
# still lands when a call resolves.
expect_has "$pulse_done" "166;227;161" "a resolved cell lost its green bullet"
