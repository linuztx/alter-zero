#!/usr/bin/env bash
# Phase 106b — the STATUS LINE itself

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Phase 106b: the STATUS LINE itself. Past the preview slot `live_layout`
# starves the strip, and the spinner row was simply not painted — the elapsed
# and the token tally gone with it. It freezes into scrollback now
# ("freeze the status indicator if it's not visible in the active window").
# A wedged backend (ALTER_ZERO_STALL_MS) holds the turn open so the check is
# not a race.
S106B="${S}_stripstatus"
APP_STALL="env $CFG_ENV_NOHIST ALTER_ZERO_STALL_MS=60000 $BIN"
tmux new-session -d -s "$S106B" -x 80 -y 4 "$APP_STALL"
sleep 0.8
submit "$S106B" "stall please"
sleep 2
sf_st_pane="$(tmux capture-pane -t "$S106B" -p)"
sf_st_full="$(tmux capture-pane -t "$S106B" -p -S -40)"
echo "==== Phase 106b: a 4-row terminal — the status line is off-screen ===="
printf '%s\n' "$sf_st_pane"
expect_lacks "$sf_st_pane" -F "esc to interrupt" "the region fits the status line; the fixture must starve the strip"
echo "==== Phase 106b: …but frozen in scrollback ===="
printf '%s\n' "$sf_st_full"
expect_has "$sf_st_full" -F "esc to interrupt" "the starved status line vanished instead of freezing into scrollback"
tmux kill-session -t "$S106B" 2>/dev/null
