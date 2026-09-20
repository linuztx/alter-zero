#!/usr/bin/env bash
# Phase 121 — a SLOW token-by-token markdown stream never flickers, duplicates or rewrites a row
# smoke: tags=serial

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# A model streaming a few tokens a second is the inline pipeline's hardest
# customer (docs/slow-stream.md): every state the renderer passes through —
# a half-open `**`, one backtick of a fence, a heading with no text yet, a
# table header before its delimiter, a wrapped code line held whole — is ON
# SCREEN for hundreds of milliseconds instead of one animation frame, and a
# row committed to scrollback has all that time to be contradicted by the
# strip above the box. The dummy's markdown tour streams every block kind in
# token-sized pieces (`stream::tokens`) at the pace `ALTER_ZERO_CHUNK_DELAY_MS`
# sets, and this phase watches the whole turn three ways:
#   1. the terminal's SCROLLBACK is append-only — every sample of it is a
#      prefix of the next (a committed row never changes or moves);
#   2. no content row is on screen TWICE at any settled instant — the strip
#      previews exactly what scrollback does not hold yet, so a row committed
#      while still previewed would show twice until the next token (the
#      slow-stream duplicate-line bug's shape); a duplicate must survive two
#      consecutive samples to count, since a mid-frame capture can read a
#      synchronized update half-parsed;
#   3. the raw byte stream (`tmux pipe-pane`) — every live-region clear sits
#      inside a synchronized update, Phase 15's rule, over a turn a hundred
#      times longer than Phase 15's.
# Then the settled transcript must carry the whole document exactly once, in
# order, with the box flush at the bottom. A second, NARROW session (40x12)
# runs the same turn faster: there the rust signature wraps to three rows,
# the table falls back to records, and the preview slot is a couple of rows,
# so the strip tail-follows and flows while it streams.
S121="${S}_slowstream"
S121N="${S}_slownarrow"
RAW121="$SMOKE_TMP/slow.raw"
SLOW_PROMPT="stream some markdown to me"
# Distinctive phrases, one per content line of the tour, in document order.
# Each must land on screen exactly once (a wrapped line carries its phrase on
# its first row only).
SLOW_MARKERS=(
	"A markdown tour, streamed slowly"
	"I'm the built-in demo backend, and this reply"
	"## Inline styles"
	"Prose with"
	"## Lists"
	"A bullet long enough to wrap"
	"A bullet carrying"
	"A nested bullet under that one"
	"And one level deeper still"
	"An ordered item"
	"A second one, with"
	"A two-digit ordinal"
	"A finished task"
	"An open task"
	"A blockquote spanning"
	"two source lines, both dimmed"
	"## Code"
	"def fibonacci"
	"The n-th Fibonacci number"
	"a, b = 0, 1"
	"for _ in range"
	"a, b = b, a + b"
	"return a  # the blank line above"
	"fn very_long_function_name"
	"format!("
	"## A table"
	"Element"
	"Heading"
	"Fence"
	"Table"
	"### The end"
	"Every block kind, at a pace"
	"Two commands away"
)

MARKERS121="$SMOKE_TMP/markers"
printf '%s\n' "${SLOW_MARKERS[@]}" >"$MARKERS121"
# The markers on screen more than once, as `|marker(count)` — one awk pass.
duplicated_markers() { # CONTENT → the duplicated markers, or nothing
	printf '%s\n' "$1" | awk 'NR == FNR { m[++n] = $0; next }
		{ for (i = 1; i <= n; i++) if (index($0, m[i])) c[i]++ }
		END { for (i = 1; i <= n; i++) if (c[i] > 1) printf "|%s(%d)", m[i], c[i] }' "$MARKERS121" -
}
launch "$S121" 80 24 "env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS ALTER_ZERO_CHUNK_DELAY_MS=${SMOKE_SLOW_CHUNK_MS:-90} $BIN"
launch "$S121N" 40 12 "env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS ALTER_ZERO_CHUNK_DELAY_MS=${SMOKE_NARROW_CHUNK_MS:-45} $BIN"
tmux pipe-pane -t "$S121" -o "cat > $RAW121"
submit "$S121" "$SLOW_PROMPT"
submit "$S121N" "$SLOW_PROMPT"
wait_for 5 "$S121" -F "esc to interrupt" || fail "the slow turn never started"

# ---- watch the wide session stream ----------------------------------------
# Captures go through FILES: a `$(…)` strips trailing newlines, and the rows
# under a box that sits above the screen bottom are exactly that — so the
# split below would hand history rows to the "visible" side and read a
# perfectly still scrollback as rewritten.
CAP121="$SMOKE_TMP/cap"
HIST121="$SMOKE_TMP/hist"
HIST121_PREV="$SMOKE_TMP/hist.prev"
: >"$HIST121_PREV"
prev_dupes=""
samples=0
mutations=0
settled_dupes=""
transient_dupes=0
footer_prev=0
hops=0
while :; do
	# ONE capture per sample — history and the visible screen together — then
	# split at the screen height (tmux prints every visible row, blank ones
	# included): two captures could straddle a scroll, and the row that just
	# left the screen would be counted in both.
	tmux capture-pane -t "$S121" -p -S - >"$CAP121"
	head -n -24 "$CAP121" >"$HIST121"
	vis="$(tail -n 24 "$CAP121")"
	samples=$((samples + 1))
	# 1. scrollback is append-only: the previous sample is a prefix of this one.
	prev_n=$(wc -l <"$HIST121_PREV")
	if [ "$prev_n" -gt 0 ] && ! head -n "$prev_n" "$HIST121" | cmp -s - "$HIST121_PREV"; then
		mutations=$((mutations + 1))
		if [ "$mutations" -le 2 ]; then
			note "scrollback rewrote itself (sample $samples) — was:"
			tail -n 5 "$HIST121_PREV"
			note "now:"
			head -n "$prev_n" "$HIST121" | tail -n 5
			note "the visible screen at that sample:"
			printf '%s\n' "$vis"
		fi
	fi
	cp "$HIST121" "$HIST121_PREV"
	# 1b. the box never hops UP while the reply streams: the region is
	# content-anchored, so commits push the footer down (until it reaches the
	# bottom, where it stays) and nothing pulls it back up — a strip that
	# dropped its preview slot between two tokens used to lift the box two
	# rows and drop it again a token later, the bounce a slow model makes
	# visible on every closing fence (docs/slow-stream.md).
	footer_now=$(printf '%s\n' "$vis" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)
	if [ "${footer_now:-0}" -gt 0 ] && [ "$footer_now" -lt "$footer_prev" ]; then
		# A capture can read a frame half-parsed — the screen scrolled, the
		# region not yet repainted, the old footer a row up — so confirm
		# against a second capture a beat later: a real hop lasts until the
		# next token (at least the chunk delay), a torn frame microseconds.
		sleep 0.03
		again="$(tmux capture-pane -t "$S121" -p)"
		footer_again=$(printf '%s\n' "$again" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)
		if [ "${footer_again:-0}" -gt 0 ] && [ "$footer_again" -lt "$footer_prev" ]; then
			hops=$((hops + 1))
			if [ "$hops" -le 2 ]; then
				note "the box hopped up from row $footer_prev to row $footer_again (sample $samples):"
				printf '%s\n' "$again"
			fi
		else
			footer_now="$footer_again"
		fi
	fi
	footer_prev="${footer_now:-$footer_prev}"
	# 2. no content row twice: count each marker across scrollback + screen.
	dupes="$(duplicated_markers "$(cat "$CAP121")")"
	# A few mid-stream frames in the log, so a failure elsewhere has
	# something to read against (every 60th sample, about every 4s).
	if [ $((samples % 60)) -eq 1 ]; then
		dump "mid-stream frame (sample $samples)" "$vis"
	fi
	if [ -n "$dupes" ]; then
		if [ "$dupes" = "$prev_dupes" ]; then
			settled_dupes="$dupes"
			note "a duplicated row survived two samples: $dupes"
			printf '%s\n' "$vis"
		else
			transient_dupes=$((transient_dupes + 1))
		fi
	fi
	prev_dupes="$dupes"
	if printf '%s' "$vis" | grep -qE "^Done for [0-9]"; then
		break
	fi
	if [ "$samples" -gt 2400 ]; then
		fail "the slow turn did not finish within the sampling budget"
		break
	fi
	sleep 0.05
done
tmux pipe-pane -t "$S121" # close the recording before anything else moves
final="$(tmux capture-pane -t "$S121" -p -S -)"
final_vis="$(tmux capture-pane -t "$S121" -p)"
dump "the settled wide session (visible screen)" "$final_vis"
note "sampled $samples times; scrollback mutations=$mutations box hops=$hops transient duplicate samples=$transient_dupes settled duplicates='${settled_dupes}'"

# ---- 3. the byte stream: Phase 15's rule over the whole slow turn ----------
sync_clears=$(sed -e $'s/\x1b\[?2026h/\\\n@SYNC@\\\n/g' \
	-e $'s/\x1b\[?2026l/\\\n@ENDS@\\\n/g' \
	-e $'s/\x1b\[0\{0,1\}J/\\\n@CLRJ@\\\n/g' "$RAW121" | awk '
	/@SYNC@/ { depth = 1; seen = 1; frames++; next }
	/@ENDS@/ { depth = 0; next }
	/@CLRJ@/ { total++; if (seen && depth == 0) bad++ }
	END { printf "frames=%d clears=%d outside=%d", frames + 0, total + 0, bad + 0 }')
raw_bytes=$(wc -c <"$RAW121")
note "raw stream: $raw_bytes bytes, $sync_clears"

# ---- assertions on the wide session ----------------------------------------
if [ "$mutations" -ne 0 ]; then
	fail "scrollback rewrote itself $mutations time(s) during the slow stream: a committed row changed or moved (see the samples above)"
fi
if [ -n "$settled_dupes" ]; then
	fail "a content row was on screen twice for two consecutive samples during the slow stream: $settled_dupes"
fi
if [ "$hops" -ne 0 ]; then
	fail "the box hopped up $hops time(s) during the slow stream: the strip dropped rows between two tokens instead of holding its place (see the frames above)"
fi
case "$sync_clears" in
frames=0*) fail "the recording shows no synchronized frames at all (recording broken?)" ;;
esac
case "$sync_clears" in
*outside=0) ;;
*) fail "live-region clears OUTSIDE a synchronized-update frame during the slow stream ($sync_clears)" ;;
esac
# The live region is never BLANKED and repainted — a commit overwrites the
# box's rows in place and clears only the rows the previous region left
# below it, so a terminal without synchronized output can never present a
# boxless frame either (docs/slow-stream.md).
case "$sync_clears" in
*clears=0\ *) ;;
*) fail "a commit blanked the live region before repainting it ($sync_clears): a terminal without mode 2026 can present the boxless state — the region must be repainted in place" ;;
esac
expect_has "$final_vis" -E "^Done for [0-9]" "the slow turn never settled"
# Every phrase exactly once, in document order, in the settled transcript.
last_at=0
for m in "${SLOW_MARKERS[@]}"; do
	c=$(printf '%s\n' "$final" | grep -cF -- "$m")
	if [ "$c" -ne 1 ]; then
		fail "'$m' appears $c times in the settled transcript (expected exactly once)"
		continue
	fi
	at=$(printf '%s\n' "$final" | grep -nF -- "$m" | head -1 | cut -d: -f1)
	if [ "$at" -le "$last_at" ]; then
		fail "'$m' landed out of document order (row $at, after row $last_at)"
	fi
	last_at=$at
done
# The box is flush at the bottom: the footer on the pane's last row.
footer_row=$(printf '%s\n' "$final_vis" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)
if [ "${footer_row:-0}" -lt 24 ]; then
	fail "after the slow turn the footer sits on row ${footer_row:-none} of the 24-row pane: the box rose off the bottom"
fi
expect_eq "$(count_bare_prompts "$final_vis")" 1 "exactly one composer prompt after the slow turn"
expect_eq "$(count_rules "$final_vis")" 2 "exactly one input box (two rules) after the slow turn"
# The grid rows are all the same width (the emoji cells cost two columns).
grid_widths="$(printf '%s\n' "$final" | grep -E '^[[:space:]]*(│|┌|├|└)' | sed 's/^[[:space:]]*//' | awk '{
	line = $0
	gsub(/│|┌|┐|└|┘|├|┤|┬|┴|┼|─/, "#", line)
	gsub(/✅/, "##", line)
	print length(line)
}' | sort -u | tr '\n' ' ')"
if [ "$(printf '%s' "$grid_widths" | wc -w)" -ne 1 ]; then
	fail "the streamed table's grid rows are not all the same width (${grid_widths})"
fi

# ---- the narrow session -----------------------------------------------------
narrow="$(wait_pane 120 "$S121N" -S - -- -E "^Done for [0-9]")"
narrow_vis="$(tmux capture-pane -t "$S121N" -p)"
dump "the settled narrow session (visible screen)" "$narrow_vis"
expect_has "$narrow_vis" -E "^Done for [0-9]" "the narrow turn never settled"
for m in "A markdown tour" "def fibonacci" "fn very_long_function_name" "Two commands away"; do
	c=$(printf '%s\n' "$narrow" | grep -cF -- "$m")
	if [ "$c" -ne 1 ]; then
		fail "narrow: '$m' appears $c times in the settled transcript (expected exactly once)"
	fi
done
narrow_footer=$(printf '%s\n' "$narrow_vis" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)
if [ "${narrow_footer:-0}" -lt 12 ]; then
	fail "narrow: the footer sits on row ${narrow_footer:-none} of the 12-row pane after the turn"
fi
expect_eq "$(count_bare_prompts "$narrow_vis")" 1 "narrow: exactly one composer prompt after the turn"
tmux kill-session -t "$S121" 2>/dev/null
tmux kill-session -t "$S121N" 2>/dev/null
