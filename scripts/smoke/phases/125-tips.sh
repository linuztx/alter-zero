#!/usr/bin/env bash
# Phase 125 — the usage tip under the status line: none at first, one a few seconds in, gone with the turn, a new one next turn and after a relaunch
#
# docs/tips.md: a turn opens on the status line alone; TIP_DELAY (5 s) in, a
# dim `⎿  Tip: …` row hangs off it; the row is live-only (gone with the
# status line, never in scrollback); the walk continues — the next turn shows
# the next tip, and a relaunch opens after the last tip shown, which
# `tips.json` in the config home remembers; and ALTER_ZERO_TIPS=0 shows none.
# A 7 s pre-stream pause holds the status line still across the delay, so the
# frame the tip lands on is a frame nothing else moves in.

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

APP_TIPS="env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=7000 $BIN"
S125="${S}_tips"
launch "$S125" 100 30 "$APP_TIPS"
submit "$S125" "$USER_MSG"

# The status line's first frame carries no tip (the Phase 62 rule: wait for
# the turn to start before anything else).
first="$(wait_pane 20 "$S125" -F "esc to interrupt")"
dump "the status line, just submitted" "$first"
expect_has "$first" -F "esc to interrupt" "the turn never started"
expect_lacks "$first" -F "Tip:" "a tip showed the moment the turn started — none is due under TIP_DELAY"

# A few seconds in, the row hangs off the line in the tool gutter.
tipped="$(wait_pane 15 "$S125" -F "⎿  Tip: ")"
dump "the tip row" "$tipped"
expect_has "$tipped" -F "  ⎿  Tip: " "no tip came up after the delay"
under_status="$(printf '%s\n' "$tipped" | grep -A 1 -F "esc to interrupt" | tail -1)"
expect_has "$under_status" -F "  ⎿  Tip: " "the tip row does not hang directly under the status line (got: '$under_status')"
first_tip="$(printf '%s\n' "$tipped" | grep -F "⎿  Tip: " | head -1)"

# The turn settles: the row went with the status line, and it never entered
# scrollback (it is strip chrome, not a record).
settled="$(wait_summaries 40 "$S125" 1)"
dump "settled" "$settled"
expect_has "$settled" -E "^$SUMMARY_RE" "the first turn never settled"
expect_lacks "$settled" -F "Tip:" "the tip row outlived the turn"
expect_lacks "$(pane "$S125" -S -200)" -F "Tip:" "the tip row leaked into scrollback"

# The next turn's tip is the next one in the walk.
submit "$S125" "$USER_MSG"
second="$(wait_pane 20 "$S125" -F "⎿  Tip: ")"
second_tip="$(printf '%s\n' "$second" | grep -F "⎿  Tip: " | head -1)"
dump "the second turn's tip" "$second"
expect_has "$second" -F "⎿  Tip: " "no tip on the second turn"
expect_ne "$second_tip" "$first_tip" "the second turn repeated the first turn's tip"
# With scrollback: the second reply is long enough to push the first
# summary off a 30-row screen.
wait_summaries 40 "$S125" 2 -S -200 >/dev/null || fail "the second turn never settled"

# tips.json remembers the last tip shown, so a relaunch opens after it.
expect_file_has "$SMOKE_CFG/tips.json" -F '"last"' "tips.json was not written to the config home"
keys "$S125" C-c
wait_gone 5 "$S125" || fail "Ctrl+C on an empty composer did not quit"
launch "$S125" 100 30 "$APP_TIPS"
submit "$S125" "$USER_MSG"
third="$(wait_pane 20 "$S125" -F "⎿  Tip: ")"
third_tip="$(printf '%s\n' "$third" | grep -F "⎿  Tip: " | head -1)"
dump "the relaunched session's first tip" "$third"
expect_has "$third" -F "⎿  Tip: " "no tip after the relaunch"
expect_ne "$third_tip" "$first_tip" "the relaunch reopened on the first session's first tip"
expect_ne "$third_tip" "$second_tip" "the relaunch repeated the last tip shown"

# ALTER_ZERO_TIPS=0: the status line, and nothing under it.
S125OFF="${S}_tipsoff"
launch "$S125OFF" 100 30 "env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=7000 ALTER_ZERO_TIPS=0 $BIN"
submit "$S125OFF" "$USER_MSG"
wait_for 20 "$S125OFF" -F "esc to interrupt" || fail "the tips-off turn never started"
if wait_for 8 "$S125OFF" -F "Tip:"; then
	dump "tips off, yet a tip" "$(pane "$S125OFF")"
	fail "ALTER_ZERO_TIPS=0 still showed a tip"
fi
