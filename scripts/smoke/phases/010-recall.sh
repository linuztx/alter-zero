#!/usr/bin/env bash
# Phase 10 — ↑ recalls the last sent message into the input box, ↓ past the newest clears i

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# ↑ recalls the last sent message into the input box, ↓ past the
# newest clears it, and ↑ + Enter RESUBMITS it (docs/input-history.md). The
# committed user line and the input prompt share the "❯ " glyph, so the
# assertions count occurrences: recall adds one (box + scrollback), the ↓ clear
# removes it, and the resubmit commits a second scrollback copy plus a second
# turn summary.
S7="${S}_recall"
RECALL_MSG="history one"
launch "$S7" 80 24
submit "$S7" "$RECALL_MSG"
wait_summaries 20.1 "$S7" 1 >/dev/null # up to ~20s: wait for turn 1 to finish
tmux send-keys -t "$S7" Up
sleep 0.4
recalled="$(tmux capture-pane -t "$S7" -p -S -200)"
echo "==== captured pane (last message recalled with Up) ===="
printf '%s\n' "$recalled"
recall_up_count=$(printf '%s\n' "$recalled" | grep -cF "❯ $RECALL_MSG")
tmux send-keys -t "$S7" Down
sleep 0.4
recall_down_count=$(tmux capture-pane -t "$S7" -p -S -200 | grep -cF "❯ $RECALL_MSG")
tmux send-keys -t "$S7" Up # recall again …
sleep 0.3
tmux send-keys -t "$S7" Enter # … and resubmit it
resubmitted="$(wait_summaries 20.1 "$S7" 2 -S -200)" # up to ~20s: wait for turn 2's summary
echo "==== captured pane (recalled message resubmitted) ===="
printf '%s\n' "$resubmitted"
recall_resubmit_count=$(printf '%s\n' "$resubmitted" | grep -cF "❯ $RECALL_MSG")
echo "==== Phase 10: '❯ $RECALL_MSG' lines — after Up=$recall_up_count, after Down=$recall_down_count, after resubmit=$recall_resubmit_count ===="
tmux kill-session -t "$S7" 2>/dev/null

# Phase 10: ↑/↓ input-history recall (docs/input-history.md). After turn 1 the
# committed user line is the only "❯ $RECALL_MSG"; ↑ adds the recalled copy in
# the input box, ↓ clears it again, and ↑ + Enter commits a second copy.
if [ "${recall_up_count:-0}" -lt 2 ]; then
	fail "Up did not recall the sent message into the input box (saw $recall_up_count '❯ $RECALL_MSG' lines, expected the committed one plus the recalled draft)"
fi
if [ "${recall_down_count:-99}" -ge "${recall_up_count:-0}" ]; then
	fail "Down past the newest entry did not clear the recalled draft (still $recall_down_count '❯ $RECALL_MSG' lines)"
fi
if [ "$(count_summaries "$resubmitted")" -lt 2 ]; then
	fail "resubmitting the recalled message (Up + Enter) never finished a second turn"
fi
if [ "${recall_resubmit_count:-0}" -lt 2 ]; then
	fail "the recalled message was not resubmitted — expected a second committed '❯ $RECALL_MSG' line (saw $recall_resubmit_count)"
fi
