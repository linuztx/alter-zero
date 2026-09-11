#!/usr/bin/env bash
# Phase 57 — the running bullet's PULSE

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the running bullet's PULSE (docs/tool-pulse.md). A tool in
# flight no longer shows a blue `●` — it shows the permission prompt's grey,
# and in the live region that grey breathes. Colour is only half of it: the
# point is that it MOVES, which no unit test can see. Sample the painted cell
# across frames while a batch runs: the running call's bullet must take more
# than one value, its `⎿ Waiting…` sibling's must not move at all, and the
# resolved cell must still land green.
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
# The bullet colours, newest-frame-last. `capture-pane -e` keeps the SGR
# sequences; each bullet is `ESC[1m ESC[38;2;R;G;Bm ●`. The running call is
# the first `⎿ Running…` cell and the waiting ones follow, so sampling every
# bullet per frame and diffing frame-to-frame tells us what moved.
pulse_samples=""
for _ in $(seq 1 24); do
	frame="$(tmux capture-pane -t "$S57" -p -e 2>/dev/null |
		grep -oE $'\x1b\\[1m\x1b\\[38;2;[0-9]+;[0-9]+;[0-9]+m●' |
		grep -oE '[0-9]+;[0-9]+;[0-9]+m' | tr -d 'm' | paste -sd, -)"
	pulse_samples="$pulse_samples$frame
"
	sleep 0.08
done
echo "==== Phase 57: sampled bullet colours (one frame per line) ===="
printf '%s' "$pulse_samples"
# Per bullet slot, how many distinct **grey** shades did it take? A breathing
# bullet sweeps through many; a `⎿ Waiting…` sibling sits on exactly one (the
# flat resting grey — the default theme's `tool_waiting_color()`, Catppuccin
# Mocha's `dim` #7F849C = 127;132;156, docs/theme.md; `ui/palette.rs`), and a
# resolved one leaves grey entirely for green/red. A "grey" here is the resting
# grey or darker on EVERY channel: the pulse only ever dips from it toward the
# palette's `pulse_dim` (#585B70 = 88;91;112), while the text colour, the green
# and the red all exceed it on some channel — so none of them can count.
# Note the batch runs its calls IN TURN, so over a two-second sample several
# slots take their own turn breathing; that is the feature, not a fault.
# The "held flat" count only looks at the OPENING frames: the batch runs its
# calls in turn, so a sibling that is queued at the start takes its own turn
# breathing later. Early on it is unambiguously waiting.
pulse_stats="$(printf '%s' "$pulse_samples" | awk -F, '
	NF {
		frames++
		for (i = 1; i <= NF; i++) {
			split($i, c, ";")
			if (c[1] + 0 <= 127 && c[2] + 0 <= 132 && c[3] + 0 <= 156) {
				if (!((i "," $i) in seen)) { seen[i "," $i] = 1; greys[i]++ }
				if (frames <= 8) {
					if (!((i "," $i) in early)) { early[i "," $i] = 1; egreys[i]++ }
					eflat[i] = $i
				}
			}
		}
		if (NF > n) n = NF
	}
	END {
		breathing = 0; resting = 0
		for (i = 1; i <= n; i++) {
			if (greys[i] >= 3) breathing++
			if (egreys[i] == 1 && eflat[i] == "127;132;156") resting++
		}
		print breathing, resting
	}')"
pulse_breathing="${pulse_stats% *}"
pulse_resting="${pulse_stats#* }"
echo "==== Phase 57: bullets that breathed: $pulse_breathing · bullets held flat: $pulse_resting ===="
# Let the turn finish so the resolved colour can be checked.
pulse_done=""
for _ in $(seq 1 400); do
	cap="$(tmux capture-pane -t "$S57" -p -e 2>/dev/null)"
	if printf '%s' "$cap" | grep -qF "Done for"; then
		pulse_done="$cap"
		break
	fi
	sleep 0.1
done
tmux kill-session -t "$S57" 2>/dev/null
echo "==== Phase 57: running bullet pulses grey, waiting stays flat, resolved lands green ===="
if [ -z "$pulse_ready" ]; then
	fail "the parallel batch never showed a running + waiting pair"
fi
# No blue bullet anywhere: #61AFEF is 97;175;239.
expect_lacks "$pulse_samples" "97;175;239" "a bullet is still painted the old blue"
if [ "${pulse_breathing:-0}" -lt 1 ]; then
	fail "no bullet swept a range of greys (the pulse is not animating)"
fi
# …and a queued sibling stayed put the whole time it was queued: one grey, the
# flat resting one. Without this the assertion above would also pass if every
# bullet flickered indiscriminately.
if [ "${pulse_resting:-0}" -lt 1 ]; then
	fail "no ⎿ Waiting… sibling held a flat grey (waiting must not animate)"
fi
if [ -z "$pulse_done" ]; then
	fail "the batch never finished"
fi
# Green — the default theme's `tool_ok_color()`, Mocha's #A6E3A1 = 166;227;161 —
# still lands when a call resolves.
expect_has "$pulse_done" "166;227;161" "a resolved cell lost its green bullet"
