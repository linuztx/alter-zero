#!/usr/bin/env bash
# Phase 43 — a background shell killed MID-TURN surfaces IMMEDIATELY

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# a background shell killed MID-TURN surfaces IMMEDIATELY
# (docs/background.md): with a dummy turn in flight, x-stopping the shell in
# the ↓ manager commits the red notice at the turn's next tool boundary —
# visible while the status line still spins — instead of only after the whole
# turn ends (the notice then sits above the turn's Done summary, not below).
S43="${S}_bgkill"
# A long pre-stream pause (the dummy's startup delay) is the window the kill
# lands in; the turn's own tool batch then settles the pending notice.
# A short `~/work` cwd, so the footer's `· 1 shell` tail fits (see work_dir).
tmux new-session -d -s "$S43" -x 80 -y 24 -c "$(work_dir)" \
	"env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=2500 $BIN_ABS"
sleep 0.4
submit "$S43" '!sleep 300'
# Wait for the Ctrl+B hint (it appears a few seconds into the run — the delay);
# its presence confirms the command is running before we background it.
wait_for 8 "$S43" -F "(ctrl+b to run in background)"
tmux send-keys -t "$S43" C-b
wait_for 3 "$S43" -F "· 1 shell"
# Start a dummy turn, then kill the shell during its pre-stream pause: ↓ lights
# the footer indicator and Enter opens the manager over the in-flight turn, and
# x stops the shell — which, it being the last one, closes the band on its own
# (docs/background.md). No Esc afterwards: with the band already gone that key
# would reach the composer and interrupt the very turn this phase measures.
submit "$S43" 'tell me about it'
sleep 0.4
tmux send-keys -t "$S43" Down
sleep 0.2
tmux send-keys -t "$S43" Enter
sleep 0.2
tmux send-keys -t "$S43" x
sleep 0.2
# The notice must commit while the turn is STILL RUNNING — the esc-to-interrupt
# status detail on the same screen — at the turn's first tool boundary.
bgkill_live_pane=""
for _ in $(seq 1 100); do
	bgkill_live_pane="$(tmux capture-pane -t "$S43" -p)"
	if printf '%s' "$bgkill_live_pane" | grep -qF "was stopped by the user" \
		&& printf '%s' "$bgkill_live_pane" | grep -qF "esc to interrupt"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 43: mid-turn stop notice while the status line still runs ===="
printf '%s\n' "$bgkill_live_pane"
# And once the turn ends, the notice sits ABOVE its Done summary in scrollback
# (the old behaviour settled it after, below the summary).
bgkill_done_pane="$(wait_pane 30 "$S43" -S -60 -- -E "$SUMMARY_RE")"
echo "==== Phase 43: pane after the turn ended ===="
printf '%s\n' "$bgkill_done_pane"
tmux kill-session -t "$S43" 2>/dev/null

# Phase 43: a mid-turn x-kill commits its notice immediately — on screen while
# the turn still streams (status line up) — and the notice precedes the turn's
# Done summary in scrollback (the old settle landed it after the summary).
if ! printf '%s' "$bgkill_live_pane" | grep -qF "was stopped by the user" \
	|| ! printf '%s' "$bgkill_live_pane" | grep -qF "esc to interrupt"; then
	fail "the mid-turn kill's notice never showed while the turn was still streaming (turn-end-only settle?)"
fi
bgkill_notice_row="$(printf '%s\n' "$bgkill_done_pane" | grep -nF "was stopped by the user" | head -1 | cut -d: -f1)"
bgkill_done_row="$(printf '%s\n' "$bgkill_done_pane" | grep -nE "$SUMMARY_RE" | head -1 | cut -d: -f1)"
if [ -z "$bgkill_notice_row" ] || [ -z "$bgkill_done_row" ] \
	|| [ "$bgkill_notice_row" -ge "$bgkill_done_row" ]; then
	fail "the kill notice (row ${bgkill_notice_row:-none}) does not precede the Done summary (row ${bgkill_done_row:-none})"
fi
