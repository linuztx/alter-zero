#!/usr/bin/env bash
# Phase 153 — a thought settling ahead of the call it led to never lifts the box

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The STRIP HOLD (docs/strip-hold.md). A reasoning model's round is a thought,
# then the call it decided on — and the call is only announced once the model
# has finished GENERATING it, its arguments streaming meanwhile. The thought
# settles the moment the first argument piece arrives: its four-row live
# block collapses into the one-line `Thought for …` cell, and for the whole
# time the call is still being written there was nothing left in the strip
# to show. The region is content-anchored, so the box rose by the rows the
# block gave back — and dropped again when the call's cell went up. A fast
# model makes that a hop for a frame or two (the reported flicker: a
# thinking model loading a skill); the dummy's default turn generates its
# three calls for ~0.8s, so here it is a hop that holds still long enough to
# sample. The strip now never gives back more rows than it committed while
# the turn runs: the thought's rows are held, blank, and the batch's cells
# land on them.
#
# Watched by sampling the footer's row through the whole turn (Phase 121's
# hop check): the region is content-anchored, so commits push the footer
# down until it reaches the bottom, where it stays, and nothing may pull it
# back up while the status line is on screen. A drop seen once is confirmed
# against a second capture a beat later — a capture can read a frame
# half-parsed, a real hop holds until the next cell. Three times: on a tall
# terminal, where the region has room to rise into; on a short one, where it
# sits at the bottom and a rise strands blank rows under the box; and with
# **Verbose output** off — the setting a user starts with, which the suite
# overrides — where the turn's opening `Read` collapses into a read group
# whose two-row cell settles into its one line under the `Edit` behind it,
# a second hand-off that used to lift the box a row.
S153="${S}_striphold"
S153S="${S}_stripholdshort"
S153V="${S}_stripholdgroup"

# watch_turn SESSION — sample the footer row until the turn's summary
# commits; sets HOPS (confirmed rises while a status line was up) and
# SAMPLES, noting the first two hops' screens.
watch_turn() {
	local sess="$1" vis again prev=0 now n
	HOPS=0
	SAMPLES=0
	for ((n = 0; n < 600; n++)); do # up to ~40s
		vis="$(tmux capture-pane -t "$sess" -p)"
		SAMPLES=$((SAMPLES + 1))
		now="$(printf '%s\n' "$vis" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)"
		now="${now:-0}"
		if [ "$now" -gt 0 ] && [ "$prev" -gt 0 ] && [ "$now" -lt "$prev" ] &&
			printf '%s' "$vis" | grep -qF "esc to interrupt"; then
			sleep 0.03
			again="$(tmux capture-pane -t "$sess" -p)"
			now="$(printf '%s\n' "$again" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)"
			now="${now:-0}"
			if [ "$now" -gt 0 ] && [ "$now" -lt "$prev" ] &&
				printf '%s' "$again" | grep -qF "esc to interrupt"; then
				HOPS=$((HOPS + 1))
				if [ "$HOPS" -le 2 ]; then
					note "$sess: the box rose from row $prev to row $now mid-turn (sample $SAMPLES):"
					printf '%s\n' "$again"
				fi
			fi
		fi
		[ "$now" -gt 0 ] && prev="$now"
		if printf '%s' "$vis" | grep -qE "^$SUMMARY_RE"; then
			return 0
		fi
		sleep 0.05
	done
	fail "$sess: the turn did not finish within the sampling budget"
}

# ---- 1. a tall terminal: the region has room to rise into -------------------
launch "$S153" 100 40
submit "$S153" "$USER_MSG"
watch_turn "$S153"
tall_hops="$HOPS"
tall_samples="$SAMPLES"
tall="$(pane "$S153" -S -60)"
dump "the settled tall session" "$tall"
note "tall: sampled $tall_samples times, $tall_hops hop(s)"
# The turn really passed through the shape the hold is for: a settled thought,
# then the batch it led to.
expect_has "$tall" -E "^Thought for [0-9]+s" "the turn's thought never settled into its line"
for call in "● Read(about.py)" "● Edit(about.py)" "● Bash(python3 about.py)"; do
	expect_has "$tall" -F "$call" "the batch's $call cell never committed"
done
expect_eq "$tall_hops" "0" "the box rose mid-turn on the tall terminal: the settled thought gave back rows it never committed"

# ---- 2. a short terminal: the region sits on the bottom row -----------------
launch "$S153S" 80 16
submit "$S153S" "$USER_MSG"
watch_turn "$S153S"
short_hops="$HOPS"
short="$(pane "$S153S")"
dump "the settled short session" "$short"
note "short: sampled $SAMPLES times, $short_hops hop(s)"
expect_eq "$short_hops" "0" "the box rose off the bottom row mid-turn on the short terminal"
# Settled, the box is flush at the bottom: the held rows went with the turn.
footer_row="$(printf '%s\n' "$short" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)"
expect_eq "${footer_row:-0}" "16" "the settled box is not on the bottom row: held rows outlived the turn"
expect_eq "$(count_footers "$short")" "1" "the settled short session shows the footer more than once"

# ---- 3. Verbose output off: the read group hands over too -----------------
# The fixture exports ALTER_ZERO_VERBOSE=1 and spells it out in every launch
# string; this session takes it out of both.
APP153V="env -u ALTER_ZERO_VERBOSE ${CFG_ENV_NOHIST/ ALTER_ZERO_VERBOSE=1/} ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
case "$APP153V" in
*ALTER_ZERO_VERBOSE=*) fail "the launch string still sets ALTER_ZERO_VERBOSE: $APP153V" ;;
esac
launch "$S153V" 100 30 "$APP153V"
submit "$S153V" "$USER_MSG"
watch_turn "$S153V"
group_hops="$HOPS"
group="$(pane "$S153V" -S -60)"
dump "the settled session with Verbose output off" "$group"
note "verbose off: sampled $SAMPLES times, $group_hops hop(s)"
expect_has "$group" -E "^Read 1 file" "the turn's Read never collapsed into a read group (is Verbose output still on?)"
expect_eq "$group_hops" "0" "the box rose mid-turn with Verbose output off: a hand-off (the thought, or the read group's line) gave back rows it never committed"

