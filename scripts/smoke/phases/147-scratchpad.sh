#!/usr/bin/env bash
# Phase 147 — scratchpad edits fold into the collapsed group

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# docs/tool-groups.md *Scratchpad edits*: the offline `scratchpad` demo
# writes a game into the session's scratchpad — the `write` generated the way
# a live model generates one, its arguments in token-sized pieces — then
# changes one line of it, and replies. This launch drops the suite's
# ALTER_ZERO_VERBOSE=1, so the session comes up collapsed:
#   - while the write streams, the group's live line counts its lines over
#     the file's path — `● Making 1 scratchpad edit +N…` — and the write has
#     no cell of its own;
#   - once the reply closes the group, one settled line, `Made 2 scratchpad
#     edits +26 -1 (ctrl+o to expand)`, and no cell for either call;
#   - Ctrl+O still shows both calls whole.
# The scratchpad is ALTER_ZERO_SCRATCHPAD_DIR's, off the usual
# `alter-zero-{uid}/{session}/scratchpad` layout, so the group can only know
# it from the session that has it.
S147="${S}_scratchpad"
PAD147="$(mktemp -d)/pad"
APP147="env ${CFG_ENV_NOHIST/ ALTER_ZERO_VERBOSE=1/} ALTER_ZERO_SCRATCHPAD_DIR=$PAD147 ALTER_ZERO_CHUNK_DELAY_MS=15 ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
LINE147="Made 2 scratchpad edits +26 -1 (ctrl+o to expand)"
case "$APP147" in
*ALTER_ZERO_VERBOSE*) fail "the launch string still sets ALTER_ZERO_VERBOSE: $APP147" ;;
esac

launch "$S147" 100 40 "env -u ALTER_ZERO_VERBOSE ${APP147#env }"
submit "$S147" "draft a game in your scratchpad"

# The write streaming into the group's line. Its bullet blinks, so the row is
# matched without it.
live="$(wait_pane 15 "$S147" -E "Making 1 scratchpad edit \+[0-9]+… \(ctrl\+o to expand\)")" ||
	fail "the scratchpad write never streamed into the group's line"
dump "captured pane (the write streaming into the group's line)" "$live"
expect_has "$live" -F "  ⎿  $PAD147/guess_the_number.py" "the live line does not name the file it writes"
expect_lacks "$live" -F "Write(" "the scratchpad write got a cell of its own"
first_count="$(grep -oE "Making 1 scratchpad edit \+[0-9]+" <<<"$live" | head -1 | grep -oE "[0-9]+$")"
if [ -z "$first_count" ] || [ "$first_count" -ge 25 ]; then
	fail "the line did not count the lines as they arrived (first seen +${first_count:-none} of 25)"
fi

wait_summaries 25 "$S147" 1 >/dev/null || fail "the scratchpad demo never finished"
settled="$(pane "$S147" -S -400)"
dump "captured pane + scrollback (the group closed by the reply)" "$settled"
expect_eq "$(grep -cF "$LINE147" <<<"$settled")" "1" "the scratchpad edits did not settle as one line"
expect_lacks "$settled" -F "● $LINE147" "the settled line kept a bullet"
expect_lacks "$settled" -F "guess_the_number.py)" "a scratchpad call kept a cell of its own"
expect_lacks "$settled" -F "Making " "the live line was left behind in scrollback"

# Ctrl+O shows both calls whole — the line is only what the conversation shows.
keys "$S147" C-o
wait_for 5 "$S147" -F "T R A N S C R I P T" || fail "Ctrl+O did not open the transcript"
keys "$S147" Home
sleep 0.4
top="$(pane "$S147")"
keys "$S147" End
sleep 0.4
bottom="$(pane "$S147")"
dump "the transcript, top" "$top"
dump "the transcript, bottom" "$bottom"
for cell in "Write(" "Edit("; do
	expect_has "$top
$bottom" -F "$cell" "Ctrl+O does not expand the scratchpad $cell call"
done
keys "$S147" q
wait_for 5 "$S147" -E '^❯' || fail "q did not close the transcript"

keys "$S147" C-c
wait_gone 10 "$S147" || fail "the app did not quit on Ctrl+C"
