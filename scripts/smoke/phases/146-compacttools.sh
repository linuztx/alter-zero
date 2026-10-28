#!/usr/bin/env bash
# Phase 146 — the compact tool display: read-only calls fold into one line

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The compact tool display (docs/compact-tools.md): with the `/settings`
# **Verbose output** row off — `ALTER_ZERO_VERBOSE=0` for a run — the
# dummy's "explore" demo (a listing, a read, then a search, one call a round,
# a thinking phase between the last two) shows ONE blinking summary in the
# strip that grows a clause per call — `Listing 1 directory…`, `Reading 1
# file, listing 1 directory…`, `Searching for 1 pattern, reading 1 file,
# listing 1 directory…` — and, once the reply begins, commits the one dim
# `Searched for 1 pattern, read 1 file, listed 1 directory (ctrl+o to
# expand)` line in their place: no `● Bash(ls -la)`, no `● Read(…)` cell in
# scrollback, the `Thought for …` line of the thought between them still its
# own row above the summary. Ctrl+O still shows every cell whole, and both a
# resize (a purge rebuild from history) and the overlay's return keep exactly
# one summary — the rebuild walk and the commit path agree. Verbose output —
# the default — is untouched: the same demo is three ordinary cells, and
# turning the row off afterwards folds them on the rebuilt screen.
S146="${S}_compact"
S146V="${S}_verbose"
EXPLORE="explore the project layout"
SETTLED="Searched for 1 pattern, read 1 file, listed 1 directory (ctrl+o to expand)"
# The demo's closing sentence — the reply quotes the summary's words, so only
# the full settled line (its expand hint included) is unambiguous.
HANDOFF="Two commands away from the real thing"

launch "$S146" 100 30 "env ALTER_ZERO_VERBOSE=0 $APP"
submit "$S146" "$EXPLORE"
# The summary grows a clause per call while the group is open.
wait_for 15 "$S146" -F "Listing 1 directory" ||
	fail "the first read-only call never showed the compact summary"
wait_for 15 "$S146" -F "Reading 1 file, listing 1 directory" ||
	fail "the read did not join the open group's summary"
wait_for 15 "$S146" -F "Searching for 1 pattern, reading 1 file, listing 1 directory" ||
	fail "the search did not join the group across the thought between them"
compact_done="$(wait_pane 20 "$S146" -S -200 -- -F "$HANDOFF")" ||
	fail "the explore demo never finished"
dump "captured pane (compact — one line for three calls)" "$compact_done"
expect_has "$compact_done" -F "$SETTLED" "the settled summary never committed"
expect_eq "$(grep -cF "$SETTLED" <<<"$compact_done")" "1" "the summary committed more than once"
expect_lacks "$compact_done" -F "● Bash(ls -la)" "the listing committed its own cell"
expect_lacks "$compact_done" -F "● Read(" "the read committed its own cell"
expect_lacks "$compact_done" -F "● Bash(grep" "the search committed its own cell"
expect_lacks "$compact_done" -F "Listing 1 directory…" "the live summary outlived the group"
expect_has "$compact_done" -F "Thought for" "the thought between the members lost its line"
thought_at="$(grep -nF "Thought for" <<<"$compact_done" | head -1 | cut -d: -f1)"
settled_at="$(grep -nF "$SETTLED" <<<"$compact_done" | head -1 | cut -d: -f1)"
[ "$thought_at" -lt "$settled_at" ] ||
	fail "the summary landed above the thought it read through (thought at $thought_at, summary at $settled_at)"

# Ctrl+O expands every member back into its own cell — the pager opens at
# its foot, so Home shows the head where the first two sit — and the return
# keeps the one summary.
keys "$S146" C-o
wait_for 5 "$S146" -F "T R A N S C R I P T" || fail "Ctrl+O did not open the transcript"
keys "$S146" Home
transcript="$(wait_pane 5 "$S146" -F "● Bash(ls -la)")" ||
	fail "Ctrl+O did not show the listing's cell"
dump "captured transcript (every member whole)" "$transcript"
expect_has "$transcript" -F "● Read(mathutil.py)" "the transcript lacks the read's cell"
keys "$S146" End
expect_has "$(pane "$S146")" -F "● Bash(grep -rn 'def ' mathutil.py)" "the transcript lacks the search's cell"
keys "$S146" C-o
returned="$(wait_pane 5 "$S146" -S -200 -- -F "$HANDOFF")" ||
	fail "the overlay did not return to the conversation"
expect_eq "$(grep -cF "$SETTLED" <<<"$returned")" "1" "the overlay return changed the summary count"
expect_lacks "$returned" -F "● Bash(ls -la)" "the overlay return painted a member's cell"

# A resize purge-rebuilds from history: the walk folds the same run.
tmux resize-window -t "$S146" -x 90 -y 30
sleep 0.8
resized="$(pane "$S146" -S -200)"
dump "captured pane (after a width change)" "$resized"
expect_eq "$(grep -cF "$SETTLED" <<<"$resized")" "1" "the rebuild did not paint exactly one summary"
expect_lacks "$resized" -F "● Bash(ls -la)" "the rebuild painted a member's cell"
keys "$S146" C-c
wait_gone 10 "$S146" || fail "the compact session did not quit on Ctrl+C"

# Verbose output (the default): the same demo is three ordinary cells…
launch "$S146V" 100 30 "$APP"
submit "$S146V" "$EXPLORE"
verbose_done="$(wait_pane 25 "$S146V" -S -200 -- -F "$HANDOFF")" ||
	fail "the verbose explore demo never finished"
dump "captured pane (verbose — a cell per call)" "$verbose_done"
expect_has "$verbose_done" -F "● Bash(ls -la)" "verbose output lost the listing's cell"
expect_has "$verbose_done" -F "● Read(mathutil.py)" "verbose output lost the read's cell"
expect_lacks "$verbose_done" -F "$SETTLED" "verbose output folded the calls"
# …until the `/settings` row is turned off, which folds them on the rebuilt
# screen.
submit "$S146V" "/settings"
wait_for 5 "$S146V" -F "Type to search" || fail "/settings did not open"
type_text "$S146V" "verbose"
sleep 0.4
keys "$S146V" Enter
wait_for 5 "$S146V" -F "Verbose output: false" || fail "the row did not cycle off"
# Esc clears the search first, then closes the menu.
keys "$S146V" Escape
keys "$S146V" Escape
sleep 0.6
folded="$(pane "$S146V" -S -200)"
dump "captured pane (verbose output turned off: the rebuild folds the cells)" "$folded"
expect_has "$folded" -F "$SETTLED" "turning verbose output off did not fold the committed cells"
expect_lacks "$folded" -F "● Bash(ls -la)" "a member's cell survived the fold"
keys "$S146V" C-c
wait_gone 10 "$S146V" || fail "the verbose session did not quit on Ctrl+C"
