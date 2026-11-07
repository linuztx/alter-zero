#!/usr/bin/env bash
# Phase 153 — a settled thought holds the box still until the next cell lands, and leaves no blank rows

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# THE THOUGHT HAND-OFF (docs/thought-handoff.md). The backend settles a
# thinking phase at the round's first tool-call piece, but the call has a cell
# only once its batch is announced. Collapsing the `● Thinking…` block at once
# lifted the box by the block's rows for as long as the call took to generate,
# then dropped it back when the cell landed — the flicker reported between a
# quick think and a `skill` call. The block now settles IN PLACE: its header
# becomes the `Thought for …` line, the thought stays under it, and the
# batch's cells take those rows the frame they land.
#
# The first fix held the rows blank instead, and that is the second half of
# this phase: blank rows held between rounds — after a command's tall live
# tail settled into its peek — sat as three empty lines over the status line
# for as long as the next request took (the reported "it should be 1 new
# line"). Mid-turn the status line has exactly ONE blank row above it.
#
# The default dummy turn: a paragraph, a two-line thought, then a
# Read+Edit+Bash batch generated over about a second. Sampled ~20 times a
# second: the footer never moves up while the turn runs, the settled line is
# seen with the thought still under it, and no sample shows two blank rows
# directly above the status line — each candidate confirmed against a second
# capture a beat later, Phase 121's rule (a torn frame lasts microseconds, a
# hop or a held gap as long as the call takes to generate). The turn's byte
# stream carries no purge rebuild, and the thought's line and every cell land
# exactly once, the thought's own text never in scrollback.
S153="${S}_thoughthandoff"
RAW153="$SMOKE_TMP/phase153.raw"
THOUGHT153="Let me read the file first."

# The blank rows directly above the status line in a capture, or nothing when
# no status line is on screen.
gap_above_status() {
	printf '%s\n' "$1" | awk '
		/esc to interrupt/ { print blank; exit }
		/^[[:space:]]*$/ { blank++; next }
		{ blank = 0 }'
}

launch "$S153" 80 24
tmux pipe-pane -t "$S153" -o "cat > $RAW153"
submit "$S153" "$USER_MSG"

samples=0
hops=0
gaps=0
held_seen=0
footer_prev=0
seen_status=0
for _ in $(seq 1 600); do # up to ~30s
	vis="$(tmux capture-pane -t "$S153" -p)"
	samples=$((samples + 1))
	if printf '%s' "$vis" | grep -qF "esc to interrupt"; then
		seen_status=1
	elif [ "$seen_status" -eq 1 ]; then
		# The turn is over: its end collapses the strip into the summary,
		# which is a settle rather than a hop.
		break
	fi
	# The held view: the settled line on screen with the thought still under
	# it, which only the strip ever shows (the thought never commits).
	if printf '%s' "$vis" | grep -qE '^Thought for [0-9]+s' &&
		printf '%s' "$vis" | grep -qF "$THOUGHT153"; then
		held_seen=1
	fi
	footer_now=$(printf '%s\n' "$vis" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)
	if [ "${footer_now:-0}" -gt 0 ] && [ "$footer_now" -lt "$footer_prev" ]; then
		sleep 0.03
		again="$(tmux capture-pane -t "$S153" -p)"
		footer_again=$(printf '%s\n' "$again" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)
		if printf '%s' "$again" | grep -qF "esc to interrupt" &&
			[ "${footer_again:-0}" -gt 0 ] && [ "$footer_again" -lt "$footer_prev" ]; then
			hops=$((hops + 1))
			if [ "$hops" -le 2 ]; then
				dump "the box hopped up from row $footer_prev to row $footer_again (sample $samples)" "$again"
			fi
		else
			footer_now="${footer_again:-$footer_now}"
		fi
	fi
	footer_prev="${footer_now:-$footer_prev}"
	gap="$(gap_above_status "$vis")"
	if [ "${gap:-1}" -gt 1 ]; then
		sleep 0.03
		again="$(tmux capture-pane -t "$S153" -p)"
		gap_again="$(gap_above_status "$again")"
		if [ "${gap_again:-1}" -gt 1 ]; then
			gaps=$((gaps + 1))
			if [ "$gaps" -le 2 ]; then
				dump "$gap_again blank rows above the status line (sample $samples)" "$again"
			fi
		fi
	fi
	sleep 0.05
done
note "sampled $samples times; box hops=$hops; held gaps=$gaps; held thought seen=$held_seen"
[ "$seen_status" -eq 1 ] || fail "the turn never showed its status line"
wait_summaries 20 "$S153" 1 >/dev/null || fail "the default turn never finished"
tmux pipe-pane -t "$S153" # close the recording before quitting

final="$(pane "$S153" -S -200)"
dump "captured pane + scrollback (the settled turn)" "$final"
# Liveness: the turn reached the hand-off, so a quiet watch is a real pass.
expect_eq "$(grep -cE '^Thought for [0-9]+s' <<<"$final")" "1" \
	"the thought did not settle into exactly one 'Thought for' line"
expect_lacks "$final" -F "$THOUGHT153" "the thought's own text reached the conversation: only its line may commit"
expect_eq "$(grep -cF '● Read(about.py)' <<<"$final")" "1" "the read's cell is not there exactly once"
expect_eq "$(grep -cF '● Edit(about.py)' <<<"$final")" "1" "the edit's cell is not there exactly once"
expect_eq "$(grep -cF '● Bash(python3 about.py)' <<<"$final")" "1" "the run's cell is not there exactly once"
if [ "$samples" -lt 20 ]; then
	fail "only $samples samples of the turn: the watch saw too little to mean anything"
fi
if [ "$held_seen" -ne 1 ]; then
	fail "the settled thought never held its rows: no sample showed its line with the thought still under it"
fi
if [ "$hops" -ne 0 ]; then
	fail "the box hopped up $hops time(s) mid-turn: the thought gave its rows up before the next cell could take them (see the frames above)"
fi
if [ "$gaps" -ne 0 ]; then
	fail "$gaps sample(s) held more than one blank row above the status line: rows were held blank instead of collapsing to one spacer (see the frames above)"
fi
purges=$(grep -c $'\x1b\\[3J' "$RAW153" 2>/dev/null || true)
expect_eq "${purges:-0}" "0" "the turn purge-rebuilt the screen"

keys "$S153" C-c
wait_gone 10 "$S153" || fail "the app did not quit on Ctrl+C"
