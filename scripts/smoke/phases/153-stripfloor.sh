#!/usr/bin/env bash
# Phase 153 — the box holds its place while one live cell hands over to the next

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# THE PREVIEW FLOOR (docs/strip-floor.md). The live region is content-
# anchored, so mid-turn the box only ever moves DOWN: a commit pushes it,
# nothing pulls it back. Two hand-offs between live cells broke that on every
# turn that thinks before it calls a tool — the reported flicker:
#   - the `● Thinking…` block settles into its one `Thought for …` line at the
#     round's first tool-call piece, but the call's cell only arrives with the
#     batch, once the arguments have streamed. The header, the rows of thought
#     and the gap gave way to one line and a spacer, so the box sat that many
#     rows up for as long as the call took to generate — ~80 ms on a fast
#     model, a jolt down-up-down — then dropped back when the cell landed;
#   - the collapsed read group's two-row cell settles into its one `Read 1
#     file` line while the call behind it runs on: a row up, until that call's
#     cell committed.
# The default dummy turn has both (a two-line thought, then a Read+Edit+Bash
# batch generated over about a second), with Verbose output off so the read
# groups. Sampled ~20 times a second, the footer must never move up while the
# turn runs — each candidate confirmed against a second capture a beat later,
# Phase 121's rule: a torn frame lasts microseconds, a hop as long as the call
# takes to generate. The rows the floor holds are blank geometry, never
# content: the turn's byte stream carries no purge rebuild (a strip flow over
# padding would be one), and the thought, the group's line and every cell land
# exactly once.
S153="${S}_stripfloor"
APP153="env ${CFG_ENV_NOHIST/ ALTER_ZERO_VERBOSE=1/} ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
case "$APP153" in
*ALTER_ZERO_VERBOSE*) fail "the launch string still sets ALTER_ZERO_VERBOSE: $APP153" ;;
esac
RAW153="$SMOKE_TMP/phase153.raw"

launch "$S153" 80 24 "env -u ALTER_ZERO_VERBOSE ${APP153#env }"
tmux pipe-pane -t "$S153" -o "cat > $RAW153"
submit "$S153" "$USER_MSG"

samples=0
hops=0
footer_prev=0
seen_status=0
for _ in $(seq 1 600); do # up to ~30s
	vis="$(tmux capture-pane -t "$S153" -p)"
	samples=$((samples + 1))
	if printf '%s' "$vis" | grep -qF "esc to interrupt"; then
		seen_status=1
	elif [ "$seen_status" -eq 1 ]; then
		# The turn is over. Its end collapses the whole strip into the
		# summary, which is a settle rather than a hop — and nothing after it
		# is mid-turn.
		break
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
	sleep 0.05
done
note "sampled $samples times; box hops=$hops"
[ "$seen_status" -eq 1 ] || fail "the turn never showed its status line"
wait_summaries 20 "$S153" 1 >/dev/null || fail "the default turn never finished"
tmux pipe-pane -t "$S153" # close the recording before quitting

final="$(pane "$S153" -S -200)"
dump "captured pane + scrollback (the settled turn)" "$final"
# Liveness: the turn reached both hand-offs, so a quiet watch is a real pass.
expect_eq "$(grep -cE '^Thought for [0-9]+s' <<<"$final")" "1" \
	"the thought did not settle into exactly one 'Thought for' line"
expect_eq "$(grep -cF 'Read 1 file (ctrl+o to expand)' <<<"$final")" "1" \
	"the read did not settle into exactly one group line"
expect_eq "$(grep -cF '● Edit(about.py)' <<<"$final")" "1" "the edit's cell is not there exactly once"
expect_eq "$(grep -cF '● Bash(python3 about.py)' <<<"$final")" "1" "the run's cell is not there exactly once"
if [ "$samples" -lt 20 ]; then
	fail "only $samples samples of the turn: the watch saw too little to mean anything"
fi
if [ "$hops" -ne 0 ]; then
	fail "the box hopped up $hops time(s) mid-turn: a live cell gave way to fewer committed rows than it stood on, and nothing held them (see the frames above)"
fi
purges=$(grep -c $'\x1b\\[3J' "$RAW153" 2>/dev/null || true)
expect_eq "${purges:-0}" "0" "the turn purge-rebuilt the screen: held rows must never flow into scrollback"

keys "$S153" C-c
wait_gone 10 "$S153" || fail "the app did not quit on Ctrl+C"
