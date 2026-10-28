#!/usr/bin/env bash
# Phase 146 — collapsed read/search groups, and Verbose output unfolding them

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The collapsed read/search group (docs/tool-groups.md). Every other phase
# runs with ALTER_ZERO_VERBOSE=1 exported, so its per-call cells stay; this
# launch drops it, so the session comes up in the DEFAULT, where a run of
# reads, searches and listings is one cell. The offline `explore` demo lists
# the folder, thinks, searches and reads in one parallel round, then runs the
# script — the one call that is no read, and so the one that closes the group:
#   - while the reads run, the group's live cell — `Listing 1 directory…
#     (ctrl+o to expand)` over `⎿  $ ls -la` — held open through the thought
#     between the rounds, which streams under it;
#   - once the script runs, ONE settled line above the script's own cell —
#     `Searched for 1 pattern, read 2 files, listed 1 directory (ctrl+o to
#     expand)` — and no cell for any read, nor the thought between them;
#   - Ctrl+O still shows every call whole;
#   - a resize's rebuild paints the same one line (it is derived from history);
#   - /settings → Verbose output → true rebuilds the conversation with a cell
#     per call, and the row persists to settings.json.
S146="${S}_toolgroups"
APP146="env ${CFG_ENV_NOHIST/ ALTER_ZERO_VERBOSE=1/} ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
GROUP146="Searched for 1 pattern, read 2 files, listed 1 directory (ctrl+o to expand)"
case "$APP146" in
*ALTER_ZERO_VERBOSE*) fail "the launch string still sets ALTER_ZERO_VERBOSE: $APP146" ;;
esac

launch "$S146" 100 44 "env -u ALTER_ZERO_VERBOSE ${APP146#env }"
submit "$S146" "explore the project for me"

# The live cell, caught while the thought between the rounds streams under
# it. Its bullet blinks, so the rows are matched without it.
live="$(wait_pane 15 "$S146" -F "Thinking…")" ||
	fail "the explore demo never reached its thought between the rounds"
dump "captured pane (the open group over the streaming thought)" "$live"
expect_has "$live" -F "Listing 1 directory… (ctrl+o to expand)" "the open group's live cell is not up while the model thinks"
expect_has "$live" -F "  ⎿  \$ ls -la" "the live group cell does not name the command it ran"
expect_lacks "$live" -F "Bash(ls -la)" "the listing got a cell of its own"
# The thought sits UNDER the group's cell, not above it.
group_row="$(grep -nF "Listing 1 directory…" <<<"$live" | head -1 | cut -d: -f1)"
thought_row="$(grep -nF "Thinking…" <<<"$live" | head -1 | cut -d: -f1)"
if [ -n "$group_row" ] && [ -n "$thought_row" ] && [ "$thought_row" -le "$group_row" ]; then
	fail "the thought streams above the open group (thought row $thought_row, group row $group_row)"
fi

# The reads join the group as they run: the parallel round grows its clauses.
grown="$(wait_pane 15 "$S146" -F "Searching for 1 pattern, reading 2 files, listing 1 directory…")" ||
	fail "the parallel round's search and reads never joined the open group"
dump "captured pane (the group after the parallel round)" "$grown"

wait_summaries 25 "$S146" 1 >/dev/null || fail "the explore demo never finished"
settled="$(pane "$S146" -S -400)"
dump "captured pane + scrollback (the group closed by the script run)" "$settled"
expect_eq "$(grep -cF "$GROUP146" <<<"$settled")" "1" "the closed group is not exactly one settled line"
for cell in "Bash(ls -la)" "Read(about.py)" "Bash(grep -rn 'card()' .)" "Bash(head -n 20 README.md)"; do
	expect_lacks "$settled" -F "$cell" "a read inside the group kept a cell of its own: $cell"
done
expect_lacks "$settled" -F "Listing 1 directory" "the open group's live cell was left behind in scrollback"
expect_lacks "$settled" -F "Thought for" "the thought between the reads was not absorbed into the group"
expect_has "$settled" -F "Bash(python3 about.py)" "the script run lost its own cell"
group_line="$(grep -nF "$GROUP146" <<<"$settled" | head -1 | cut -d: -f1)"
run_line="$(grep -nF "Bash(python3 about.py)" <<<"$settled" | head -1 | cut -d: -f1)"
if [ -z "$group_line" ] || [ -z "$run_line" ] || [ "$group_line" -ge "$run_line" ]; then
	fail "the group's line is not above the call that closed it (group row ${group_line:-none}, run row ${run_line:-none})"
fi
# The settled line wears no bullet: what is left is a fact about the turn.
expect_lacks "$settled" -F "● $GROUP146" "the settled group line kept a bullet"

# Ctrl+O shows every call whole — the group is only what the conversation shows.
keys "$S146" C-o
wait_for 5 "$S146" -F "T R A N S C R I P T" || fail "Ctrl+O did not open the transcript"
keys "$S146" Home
sleep 0.4
top="$(pane "$S146")"
keys "$S146" End
sleep 0.4
bottom="$(pane "$S146")"
dump "the transcript, top" "$top"
dump "the transcript, bottom" "$bottom"
for cell in "Bash(ls -la)" "Read(about.py)" "Bash(grep -rn 'card()' .)" "Bash(head -n 20 README.md)" "Bash(python3 about.py)"; do
	expect_has "$top
$bottom" -F "$cell" "Ctrl+O does not expand $cell"
done
keys "$S146" q
wait_for 5 "$S146" -E '^❯' || fail "q did not close the transcript"

# A resize rebuilds the conversation from history — the same one line.
tmux resize-window -t "$S146" -x 90 -y 44
sleep 1.0
resized="$(pane "$S146" -S -400)"
dump "captured pane + scrollback (after a resize rebuild)" "$resized"
expect_eq "$(grep -cF "$GROUP146" <<<"$resized")" "1" "the resize rebuild did not paint the group as its one line"
expect_lacks "$resized" -F "Read(about.py)" "the resize rebuild unfolded the group"

# /settings → Verbose output → true: a cell per call again, at once.
type_text "$S146" "/settings"
sleep 0.3
keys "$S146" Enter
wait_for 3 "$S146" -F "Type to search" || fail "/settings did not open"
type_text "$S146" "verbose"
menu="$(wait_pane 3 "$S146" -E "→ Verbose output +false")" ||
	fail "the Verbose output row does not read false in a session that never set it"
dump "/settings filtered to the Verbose output row" "$menu"
keys "$S146" Enter
wait_for 3 "$S146" -F "Verbose output: true" || fail "no 'Verbose output: true' toast after Enter"
keys "$S146" Escape
sleep 0.3
keys "$S146" Escape
sleep 1.0
verbose="$(pane "$S146" -S -400)"
dump "captured pane + scrollback (Verbose output on)" "$verbose"
for cell in "Bash(ls -la)" "Read(about.py)" "Bash(grep -rn 'card()' .)" "Bash(head -n 20 README.md)" "Bash(python3 about.py)"; do
	expect_has "$verbose" -F "$cell" "Verbose output did not give $cell its own cell back"
done
expect_lacks "$verbose" -F "$GROUP146" "Verbose output left the collapsed line on screen"
wait_file 5 "$SMOKE_CFG/settings.json" -E '"verbose": *true' ||
	fail "the Verbose output row did not persist to settings.json: $(cat "$SMOKE_CFG/settings.json" 2>/dev/null)"

keys "$S146" C-c
wait_gone 10 "$S146" || fail "the app did not quit on Ctrl+C"
