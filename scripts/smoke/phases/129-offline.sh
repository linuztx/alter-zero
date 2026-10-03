#!/usr/bin/env bash
# Phase 129 — waiting out a lost connection

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Waiting out a lost connection (docs/offline.md). The dummy's `offline` demo
# plays what the real backend does when a request cannot leave the machine:
# two failed checks at the real cadence (1 s, then 2 s), the reconnect, then
# the reply. The whole wait wears the offline look — the signal bars,
# `Waiting for internet…`, an `offline Ns · retrying in Ns` countdown, and
# never a `retrying n/N` counter, since a lost connection spends no retry
# budget — and the line goes back to the turn's own spinner the moment the
# provider answers, under a `Back online after Ns offline` toast. Then an Esc
# during the wait: nothing has streamed, so the interrupt undoes the
# submission and hands the prompt back (docs/interrupt.md).
S129="${S}_offline"
OFFLINE_PROMPT="show me what happens offline"
# The status line's own row: the reply narrates the look in prose, so every
# check about the line reads this row alone, never the whole pane.
status_row() { printf '%s\n' "$1" | grep -F "esc to interrupt)" | tail -1; }
# lacks_status SESSION — no status line left in the pane.
lacks_status() { ! pane_has "$1" -F "esc to interrupt"; }
# composer_row CONTENT — the composer's own row: the last non-blank rows are
# it, the box's bottom rule and the footer. (The first turn's `❯` message in
# scrollback reads the same, so the pane as a whole proves nothing.)
composer_row() { printf '%s\n' "$1" | grep -v '^[[:space:]]*$' | tail -3 | head -1; }

launch "$S129" 110 24
submit "$S129" "$OFFLINE_PROMPT"

note "the offline look while the request waits"
frame="$(wait_pane 10 "$S129" -F "Waiting for internet…")"
dump "offline, first check" "$frame"
row="$(status_row "$frame")"
expect_has "$row" -E '^▂ ▄ ▆ █ Waiting for internet… \([0-9]+s · ' \
	"the signal bars and the verb do not open the status line"
expect_has "$row" -E '· offline [0-9]+s · retrying in 1s · esc to interrupt\)$' \
	"the first check does not count down from one second"
expect_lacks "$frame" -E 'retrying [0-9]+/[0-9]+' "a lost connection spent the retry budget"

note "the second check counts down from two seconds"
frame="$(wait_pane 10 "$S129" -F "retrying in 2s")"
dump "offline, second check" "$frame"
expect_has "$(status_row "$frame")" -E '· offline [1-9]s · retrying in 2s · ' \
	"the second wait is not the cadence's two seconds"

note "reconnected: the toast, the working line, then the reply"
frame="$(wait_pane 10 "$S129" -E 'Back online after [0-9]+s offline')"
dump "reconnected" "$frame"
expect_has "$frame" -E '^  Back online after [0-9]+s offline$' "no toast said the connection came back"
row="$(status_row "$frame")"
expect_lacks "$row" -F "Waiting for internet" "the offline look outlived the reconnect"
expect_has "$row" -F "Working…" "the turn's own line did not come back"
settled="$(wait_settled 20 "$S129" -F "$SETTLED_REPLY")"
dump "settled" "$settled"
expect_has "$settled" -F "Back online — that pause was the network" "the reply never streamed"
expect_has "$settled" -E "^$SUMMARY_RE" "the turn never committed its summary"
expect_lacks "$settled" -F "esc to interrupt" "the status line outlived the turn"

note "esc during the wait hands the prompt back"
submit "$S129" "$OFFLINE_PROMPT"
frame="$(wait_pane 10 "$S129" -F "retrying in")"
expect_has "$frame" -F "Waiting for internet…" "the second turn never went offline"
keys "$S129" Escape
if ! poll 5 lacks_status "$S129"; then
	fail "Esc did not stop the offline wait"
fi
frame="$(pane "$S129")"
dump "after esc" "$frame"
expect_eq "$(composer_row "$frame")" "❯ $OFFLINE_PROMPT" "the prompt is not back in the composer"
expect_lacks "$frame" -F "Conversation interrupted" "an undone turn left an interrupt notice"
expect_lacks "$frame" -E '^  Back online after' "a stopped wait still toasted a reconnect"
