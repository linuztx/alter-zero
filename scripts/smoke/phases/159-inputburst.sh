#!/usr/bin/env bash
# Phase 159 — a typed burst over 1 KB all reaches the composer

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# More than a kilobyte of plain typed input arriving in one piece — a paste
# into a terminal without bracketed paste, a dictation tool's text, typeahead a
# stalled connection delivers at once. The terminal library read 1024 bytes of
# it and went back to waiting on a readiness event that fires when NEW input
# arrives, not while input remains, so the rest sat unread until the next key:
# the composer showed the first kilobyte and the burst's end turned up one
# keypress late (docs/input-latency.md, vendor/README.md).
S159="${S}_burst"
launch "$S159" 100 30
burst="$(awk 'BEGIN { for (i = 0; i < 400; i++) printf "word%03d ", i }')ENDBURST"
type_text "$S159" "$burst"
if burst_pane="$(wait_pane 10 "$S159" -F "ENDBURST")"; then :; else
	fail "a ${#burst}-byte typed burst never reached the composer whole"
fi
dump "the pane after the burst" "$burst_pane"
# Whole and in order: the composer ends on the burst's own last words.
expect_has "$burst_pane" -F "word399 ENDBURST" "the composer does not end on the burst's last words"
# …and nothing was left for the next key to shake loose: a key typed after it
# lands after the burst, not in front of a tail that was still unread.
type_text "$S159" "!"
after_pane="$(wait_pane 5 "$S159" -F "ENDBURST!")"
dump "the pane after one more key" "$after_pane"
expect_has "$after_pane" -F "word399 ENDBURST!" "a key typed after the burst did not land right behind it"

smoke_finish
