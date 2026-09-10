#!/usr/bin/env bash
# Phase 20 — the dummy AI PAUSES before streaming so the status indicator is visible first

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the dummy AI PAUSES before streaming so the status indicator is
# visible first (docs/status-indicator.md), and the just-sent user message is
# counted into the tally with the ↑ arrow. Launch with a longer startup delay
# (overriding the smoke-wide short one), submit, then capture MID-PAUSE: the
# status line must show with `↑ N tokens` and NO reply text yet — then the
# reply must still stream once the pause elapses.
S17="${S}_delay"
DELAY_MSG="count my input tokens"
# 21 chars → responses[0], which opens with this phrase.
DELAY_REPLY="Sure thing"
launch "$S17" 80 24 "env $CFG_ENV ALTER_ZERO_STARTUP_DELAY_MS=2000 $BIN"
submit "$S17" "$DELAY_MSG"
sleep 0.9 # mid-pause: the 2s startup delay is still running
delay_pause="$(tmux capture-pane -t "$S17" -p)"
echo "==== captured visible screen (mid pre-stream pause — status shows, no reply yet) ===="
printf '%s\n' "$delay_pause"
# The reply must still arrive once the pause elapses (the pause is not a hang).
delay_reply="$(wait_pane 6 "$S17" -S -40 -- -F "$DELAY_REPLY")" # up to ~6s
echo "==== captured pane (after the pause — reply streaming, arrow flipped down) ===="
printf '%s\n' "$delay_reply"
tmux kill-session -t "$S17" 2>/dev/null

# Phase 20: the pre-stream pause shows the status indicator with the input
# counted as ↑ tokens, before any reply text.
expect_has "$delay_pause" -F "esc to interrupt" "the status indicator is not visible during the pre-stream pause"
expect_has "$delay_pause" -E "↑ [0-9]+ tokens" "the just-sent user message is not counted as '↑ N tokens' during the pause"
expect_lacks "$delay_pause" -F "$DELAY_REPLY" "the reply streamed during the pause (the startup delay did not hold)"
expect_has "$delay_reply" -F "$DELAY_REPLY" "the reply never streamed after the pause (a hang, not a delay)"
expect_has "$delay_reply" -E "↓ [0-9]+ tokens" "the arrow did not flip to ↓ once the reply started streaming"
