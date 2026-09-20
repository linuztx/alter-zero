#!/usr/bin/env bash
# Phase 92 — a VERY LONG `!` output shows WHOLE inline, and Ctrl+O agrees row for row

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# a VERY LONG `!` output shows WHOLE inline (docs/shell-command.md). The `!`
# shell cell never folds: the user ran the command to read its output, so a
# 750-char line — a minified blob, a `curl` JSON body — paints every row it
# needs and nothing waits behind a `… +N lines (ctrl+o to expand)` hint (the
# fold is the model's `bash` cell's alone — docs/long-lines.md, unit-tested,
# and on screen in Phase 38's settled `Bash(ping …)` cells). At 80 columns
# the `  ⎿  ` gutter leaves 75, so 750 chars is exactly 10 rows, all inline;
# Ctrl+O holds the same ten.
S92="${S}_longline"
launch "$S92" 80 24
submit "$S92" "!printf 'x%.0s' \$(seq 1 750)"
ll_pane="$(wait_settled 8 "$S92" -S -40 -- -F 'xxxxxxxx')" # up to ~8s
echo "==== Phase 92: captured pane (a 750-char single line, shown whole) ===="
printf '%s\n' "$ll_pane"
ll_rows="$(printf '%s' "$ll_pane" | grep -c 'xxxxxxxx')"
if [ "$ll_rows" -ne 10 ]; then
	fail "the 750-char line painted $ll_rows rows inline, not all 10"
fi
# Nothing is cut and nothing is folded: no `…` on any row, no hint under them.
expect_lacks "$ll_pane" -E 'x…$' "a row carries a … marker; the shell cell shows the line whole"
expect_lacks "$ll_pane" -F "ctrl+o to expand" "the shell cell folded behind a '+N lines (ctrl+o to expand)' hint; it must show its output whole"
# Ctrl+O: the same ten rows — the transcript and the inline cell agree.
tmux send-keys -t "$S92" C-o
sleep 0.5
ll_view="$(tmux capture-pane -t "$S92" -p)"
echo "==== Phase 92: captured pane (Ctrl+O — the line whole) ===="
printf '%s\n' "$ll_view"
ll_full="$(printf '%s' "$ll_view" | grep -c 'xxxxxxxx')"
if [ "$ll_full" -lt 10 ]; then
	fail "the transcript shows only $ll_full of the line's 10 rows"
fi
expect_lacks "$ll_view" -E 'x…$' "the expansion clipped the line, leaving the text nowhere"
tmux send-keys -t "$S92" C-o
sleep 0.3
tmux kill-session -t "$S92" 2>/dev/null

# FOUR wrapping lines — a `curl | grep` of a web page: four 150-char lines are
# 2 rows each at the 75-column gutter, 8 rows, and all eight show inline —
# the third and fourth lines included, which a fold would have hidden.
S92B="${S}_peekrows"
launch "$S92B" 80 24
submit "$S92B" "!for c in a b c d; do printf \"\$c%.0s\" \$(seq 1 150); echo; done"
pr_pane="$(wait_settled 8 "$S92B" -S -40 -- -F 'dddddddd')" # up to ~8s
echo "==== Phase 92: captured pane (four wrapping lines, every row inline) ===="
printf '%s\n' "$pr_pane"
pr_rows="$(printf '%s' "$pr_pane" | grep -cE 'aaaaaaaa|bbbbbbbb|cccccccc|dddddddd')"
if [ "$pr_rows" -ne 8 ]; then
	fail "the four-line output painted $pr_rows rows inline, not all 8"
fi
expect_has "$pr_pane" -F "cccccccc" "the third line is missing inline: the cell is folding"
expect_lacks "$pr_pane" -F "ctrl+o to expand" "the four-line output folded behind a hint; the shell cell must show it whole"
# Ctrl+O holds the same eight rows.
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

# BLANK lines are output too: with no fold there is no first block to prefer,
# so `\n\nfirst\nsecond\n\nhidden` shows exactly as printed — the two leading
# blank rows (the first under the `⎿` corner), `first`, `second`, the blank,
# and `hidden` — the rows the Ctrl+O view paints, nothing hinted.
S92C="${S}_peekblank"
launch "$S92C" 80 24
submit "$S92C" "!printf '\n\nfirst\nsecond\n\nhidden\n'"
pb_pane="$(wait_settled 8 "$S92C" -S -40 -- -E '^ +hidden$')" # up to ~8s
echo "==== Phase 92: captured pane (blank lines kept, nothing folded) ===="
printf '%s\n' "$pb_pane"
# Anchored to gutter rows: the echoed `! printf …` header names them all too.
expect_has "$pb_pane" -E '^ +⎿ *$' "the leading blank line the command printed is not on the corner row"
expect_has "$pb_pane" -E '^ +first$' "the first non-blank line is missing"
expect_has "$pb_pane" -E '^ +second$' "the first block's second line is missing"
expect_has "$pb_pane" -E '^ +hidden$' "the block after the blank line is missing inline: the cell is folding at the blank"
expect_lacks "$pb_pane" -F "ctrl+o to expand" "the blank-shaped output folded behind a hint; the shell cell must show it whole"
# Ctrl+O holds the same rows.
tmux send-keys -t "$S92C" C-o
sleep 0.5
pb_view="$(tmux capture-pane -t "$S92C" -p)"
echo "==== Phase 92: captured pane (Ctrl+O — the whole output, blanks included) ===="
printf '%s\n' "$pb_view"
expect_has "$pb_view" -E '^ +hidden$' "the transcript dropped the block after the blank line"
tmux send-keys -t "$S92C" C-o
sleep 0.3
tmux kill-session -t "$S92C" 2>/dev/null
