#!/usr/bin/env bash
# Phase 41 — a streamed TABLE keeps the box flush at the bottom

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# a streamed TABLE keeps the box flush at the bottom. The dummy's
# "table" reply streams a 10-row GFM grid with prose after it, so the whole
# block commits in ONE flush at its close while the strip collapses from the
# tall forming-table preview to a single row (docs/table-streaming.md). That
# flush must sync to the collapsed height first — pre-fix it reserved the stale
# taller strip below the grid, so the box rose off the bottom and a blank band
# was left beneath it (the reported bug).
S41="${S}_tableflush"
launch "$S41" 80 24
submit "$S41" "table demo"
tableflush_pane=""
for _ in $(seq 1 200); do # ~20s cap; the table reply streams in about eight
	tableflush_pane="$(tmux capture-pane -t "$S41" -p)"
	if printf '%s' "$tableflush_pane" | grep -qF "properly in Markdown." \
		&& printf '%s' "$tableflush_pane" | grep -qE "^$SUMMARY_RE"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 41: pane after the streamed-table turn ended ===="
printf '%s\n' "$tableflush_pane"
# capture-pane trims trailing blank rows, so judge the footer's ABSOLUTE row
# against the 24-row pane: flush-at-bottom puts it on the last row; the bug
# left it floating ~a strip-height higher with blank rows beneath.
tableflush_footer_row=$(printf '%s\n' "$tableflush_pane" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)
# The EMOJI half of the table bug (docs/table-streaming.md): the demo table's
# Description cells open with a two-column ✅/❌, and every grid row on screen
# must still be exactly as wide as the border rows. Pre-fix the draw paths also
# printed the blank filler cell ratatui reserves *after* a wide grapheme, so an
# emoji row came out one column wider per emoji — the right border stepped out
# of line and, on a table that fills the width, wrapped onto the next row.
# Measured in awk's byte mode (the suite runs in a POSIX locale): map every
# 3-byte box-drawing glyph to ONE ascii byte and each 3-byte emoji to TWO, then
# a byte count is the display width.
grid_row_widths() { # → the distinct display widths of a pane's grid rows
	printf '%s\n' "$1" |
		grep -E '^[[:space:]]*(│|┌|├|└)' |
		sed 's/^[[:space:]]*//' |
		awk '{
			line = $0
			gsub(/│|┌|┐|└|┘|├|┤|┬|┴|┼|─/, "#", line)
			gsub(/✅|❌/, "##", line)
			print length(line)
		}' | sort -u | tr '\n' ' '
}
tableflush_row_widths="$(grid_row_widths "$tableflush_pane")"
tableflush_emoji_rows=$(printf '%s\n' "$tableflush_pane" | grep -cE '^[[:space:]]*│.*(✅|❌)')
# …and the SAME must hold in the Ctrl+O transcript overlay, which paints the
# alternate screen through its own full-cell emitter (`draw_overlay`). Fixing
# only the inline emitters left the overlay — the one view you open to read a
# table in full — still drifting a column per emoji.
tmux send-keys -t "$S41" C-o
sleep 1.2
tmux send-keys -t "$S41" End
sleep 0.8
tableflush_overlay="$(tmux capture-pane -t "$S41" -p)"
echo "==== Phase 41: Ctrl+O transcript overlay over the emoji table ===="
printf '%s\n' "$tableflush_overlay"
tableflush_overlay_widths="$(grid_row_widths "$tableflush_overlay")"
tableflush_overlay_emoji=$(printf '%s\n' "$tableflush_overlay" | grep -cE '^[[:space:]]*│.*(✅|❌)')
tmux kill-session -t "$S41" 2>/dev/null

# Phase 41: a streamed table's close-flush keeps the box flush at the bottom —
# the footer ends the turn on the pane's last row, not floating above a blank
# band (docs/table-streaming.md; the flush syncs to the collapsed strip height).
expect_has "$tableflush_pane" -F "│ 10 │ Regex" "the streamed table's grid rows never reached the screen"
if [ "${tableflush_footer_row:-0}" -lt 23 ]; then
	fail "after the table turn the footer sits on row ${tableflush_footer_row:-none} of the 24-row pane: the box rose off the bottom, leaving a blank band beneath it"
fi
# Every grid row the same width, emoji rows included — in the inline view AND in
# the Ctrl+O overlay, the two independent full-cell emitters (`term::visible_cells`).
if [ "${tableflush_emoji_rows:-0}" -lt 1 ]; then
	fail "no emoji grid row reached the screen, so the wide-glyph alignment check proved nothing"
elif [ "$(printf '%s' "$tableflush_row_widths" | wc -w)" -ne 1 ]; then
	fail "the streamed table's grid rows are not all the same width (${tableflush_row_widths}): a wide glyph (✅/❌) is costing an extra terminal column, so the right border steps out of line"
fi
if [ "${tableflush_overlay_emoji:-0}" -lt 1 ]; then
	fail "no emoji grid row reached the Ctrl+O overlay, so its wide-glyph alignment check proved nothing"
elif [ "$(printf '%s' "$tableflush_overlay_widths" | wc -w)" -ne 1 ]; then
	fail "the Ctrl+O transcript's grid rows are not all the same width (${tableflush_overlay_widths}): draw_overlay is emitting the cells shadowed by a wide glyph, so the table tears in the overlay even though the inline view is fine"
fi
