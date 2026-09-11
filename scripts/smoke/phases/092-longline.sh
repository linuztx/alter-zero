#!/usr/bin/env bash
# Phase 92 — a VERY LONG output line is bounded and counted honestly

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# a VERY LONG output line is bounded and counted honestly
# (docs/long-lines.md). One 750-char line — a minified blob, a `curl` JSON body
# — used to spend the whole peek ceiling on itself (twelve rows of wrapped
# noise) under a hint claiming ONE line was hidden. The cell folds at
# TOOL_FOLD_ROWS rows now — Claude Code's three, shown as they are, the hint
# right under them saying the rest follows — and the hint counts the display
# ROWS the expansion adds. At 80 columns the `  ⎿  ` gutter leaves 75, so 750
# chars is exactly 10 rows: 3 shown, 7 hidden. Ctrl+O still holds all ten.
S92="${S}_longline"
launch "$S92" 80 24
submit "$S92" "!printf 'x%.0s' \$(seq 1 750)"
ll_pane="$(wait_pane 6 "$S92" -S -40 -- -F "lines (ctrl+o to expand)")" # up to ~6s
echo "==== Phase 92: captured pane (a 750-char single line, clipped and counted) ===="
printf '%s\n' "$ll_pane"
ll_rows="$(printf '%s' "$ll_pane" | grep -c 'xxxxxxxx')"
if [ "$ll_rows" -ne 3 ]; then
	fail "the 750-char line painted $ll_rows rows inline, not the 3-row fold"
fi
# The fold shows the rows as they are: no `…` on the third row — the hint
# under it is what says the line continues (Claude Code's look).
expect_lacks "$ll_pane" -E 'x…$' "the folded row carries a … marker; the hint under it already says the rest follows"
expect_has "$ll_pane" -F "… +7 lines (ctrl+o to expand)" "the hint does not count the 7 hidden display rows (a source-line count would say '+1 lines')"
# Ctrl+O: the expansion is where the whole line lives — all ten rows, unmarked.
tmux send-keys -t "$S92" C-o
sleep 0.5
ll_view="$(tmux capture-pane -t "$S92" -p)"
echo "==== Phase 92: captured pane (Ctrl+O — the line whole) ===="
printf '%s\n' "$ll_view"
ll_full="$(printf '%s' "$ll_view" | grep -c 'xxxxxxxx')"
if [ "$ll_full" -lt 10 ]; then
	fail "the transcript shows only $ll_full of the line's 10 rows"
fi
expect_lacks "$ll_view" -E 'x…$' "the expansion clipped the line too, leaving the text nowhere"
tmux send-keys -t "$S92" C-o
sleep 0.3
tmux kill-session -t "$S92" 2>/dev/null

# The other half of the same rule, and the reported one (docs/long-lines.md
# "Rows, not lines"): FOUR wrapping lines — a `curl | grep` of a web page —
# where every single line was inside its own budget and the CELL was still ten
# rows of noise, because the block was budgeted in source lines. The peek folds
# in display ROWS now (TOOL_FOLD_ROWS = 3), so four 150-char lines (2 rows each
# at the 75-column gutter — 8 rows) show the first three rows and hide five.
S92B="${S}_peekrows"
launch "$S92B" 80 24
submit "$S92B" "!for c in a b c d; do printf \"\$c%.0s\" \$(seq 1 150); echo; done"
pr_pane="$(wait_pane 6 "$S92B" -S -40 -- -F "lines (ctrl+o to expand)")" # up to ~6s
echo "==== Phase 92: captured pane (four wrapping lines, bounded in rows) ===="
printf '%s\n' "$pr_pane"
pr_rows="$(printf '%s' "$pr_pane" | grep -cE 'aaaaaaaa|bbbbbbbb|cccccccc|dddddddd')"
if [ "$pr_rows" -ne 3 ]; then
	fail "the four-line output painted $pr_rows rows inline, not the 3-row fold"
fi
expect_lacks "$pr_pane" -F "cccccccc" "the third line shows inline: the fold is not bounding the cell"
expect_has "$pr_pane" -F "… +5 lines (ctrl+o to expand)" "the hint does not count the 5 hidden display rows"
# Ctrl+O still holds every row — the cell is bounded, the output is not lost.
tmux send-keys -t "$S92B" C-o
sleep 0.5
pr_view="$(tmux capture-pane -t "$S92B" -p)"
echo "==== Phase 92: captured pane (Ctrl+O — all four lines) ===="
printf '%s\n' "$pr_view"
pr_full="$(printf '%s' "$pr_view" | grep -cE 'aaaaaaaa|bbbbbbbb|cccccccc|dddddddd')"
if [ "$pr_full" -lt 8 ]; then
	fail "the transcript shows only $pr_full of the output's 8 rows"
fi
tmux send-keys -t "$S92B" C-o
sleep 0.3
tmux kill-session -t "$S92B" 2>/dev/null

# The third part of the same rule (docs/long-lines.md "The peek is the output's
# first block"): a BLANK line costs a full row of a three-row fold and says
# nothing. Leading blanks are skipped and the first blank after the content
# closes the peek, so an output shaped `\n\nfirst\nsecond\n\nhidden` shows
# exactly `first` + `second` — no empty gutter row above them, and no fragment
# of the next block below — while the hint still counts every hidden row
# (the two leading blanks, the closing blank, and `hidden` = 4).
S92C="${S}_peekblank"
launch "$S92C" 80 24
submit "$S92C" "!printf '\n\nfirst\nsecond\n\nhidden\n'"
pb_pane="$(wait_pane 6 "$S92C" -S -40 -- -F "lines (ctrl+o to expand)")" # up to ~6s
echo "==== Phase 92: captured pane (the peek is the output's first block) ===="
printf '%s\n' "$pb_pane"
expect_has "$pb_pane" -E '⎿ +first' "the peek does not open on the first non-blank line"
expect_has "$pb_pane" -E '^ +second$' "the first block's second line is missing from the peek"
# Anchored to a gutter row: the echoed `! printf …` header names `hidden` too.
expect_lacks "$pb_pane" -E '^ +hidden$' "the peek hopped the blank line into the next block"
expect_has "$pb_pane" -F "… +4 lines (ctrl+o to expand)" "the hint does not count the skipped blank rows"
# Ctrl+O still holds the blanks and the block below them — the cell is a peek,
# not a filter.
tmux send-keys -t "$S92C" C-o
sleep 0.5
pb_view="$(tmux capture-pane -t "$S92C" -p)"
echo "==== Phase 92: captured pane (Ctrl+O — the whole output, blanks included) ===="
printf '%s\n' "$pb_view"
expect_has "$pb_view" -E '^ +hidden$' "the transcript dropped the block the peek hid"
tmux send-keys -t "$S92C" C-o
sleep 0.3
tmux kill-session -t "$S92C" 2>/dev/null
