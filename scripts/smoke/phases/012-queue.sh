#!/usr/bin/env bash
# Phase 12 — messages submitted WHILE a turn streams go INTO THAT TURN (codex's steering, d

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# messages submitted WHILE a turn streams go INTO THAT TURN
# (codex's steering, docs/queue.md): each waits like a user message ("  ❯ …",
# two-space inset) *above* the box, and the turn takes them at its next round
# boundary — the dummy's tool boundary — where each becomes a real user bubble
# at column 0 ("❯ world", "❯ again") **while the turn is still running** (the
# status line is still up). So exactly ONE turn runs: turn 1's "Done for"
# summary appears and turn 2's "Finished for" must NOT — a backlog that waited
# for the turn to end would produce it, which is the behaviour this replaced.
S9="${S}_queue"
launch "$S9" 80 24
submit "$S9" "hello there"
wait_for 4 "$S9" -F "Happy" # up to ~4s: wait until turn 1 is visibly streaming
submit "$S9" "world" # streaming → queued, not submitted
submit "$S9" "again" # second queued message
queued_band=""
for _ in $(seq 1 20); do # up to ~3s: both queued messages show above the box
	queued_band="$(tmux capture-pane -t "$S9" -p)"
	# While turn 1 still streams, the two-space inset ("  ❯ …" — committed user
	# lines sit at column 0) can only be the queued display; the status line
	# confirms the turn is active.
	if printf '%s' "$queued_band" | grep -qF "  ❯ world" &&
		printf '%s' "$queued_band" | grep -qF "  ❯ again" &&
		printf '%s' "$queued_band" | grep -qF "tokens"; then
		break
	fi
	sleep 0.15
done
echo "==== captured pane (world + again queued above the box) ===="
printf '%s\n' "$queued_band"
# The delivery itself: both messages committed at column 0 while the status
# line still says the turn is running. A message that only landed after the
# summary is the old wait-for-the-turn behaviour.
queue_midturn=""
for _ in $(seq 1 400); do # up to ~60s: the turn reaches its first tool boundary
	frame="$(tmux capture-pane -t "$S9" -p -S -120)"
	if printf '%s' "$frame" | grep -qE '^❯ world$' &&
		printf '%s' "$frame" | grep -qE '^❯ again$' &&
		printf '%s' "$frame" | grep -qF "esc to interrupt"; then
		queue_midturn="$frame"
		break
	fi
	sleep 0.15
done
echo "==== captured pane (both queued messages taken INTO the running turn) ===="
printf '%s\n' "$queue_midturn"
queue_done="$(wait_pane 45 "$S9" -S -80 -- -F "Done for")" # up to ~45s: the one turn finishes
# …and give a would-be second turn time to start before asserting there is none.
sleep 2
queue_done="$(tmux capture-pane -t "$S9" -p -S -80)"
echo "==== captured pane (the turn that read them, finished) ===="
printf '%s\n' "$queue_done"
tmux kill-session -t "$S9" 2>/dev/null

# Phase 12: a message submitted mid-stream is queued (shown like a user message,
# inset two columns — "  ❯ world" — above the box, while turn 1 still streams)
# and auto-sent as its own turn when the first finishes (docs/queue.md).
expect_has "$queued_band" -F "  ❯ world" "a message submitted while streaming was not shown queued (two-space inset '  ❯ world') above the box while turn 1 streamed"
expect_has "$queued_band" -F "  ❯ again" "the second queued message was not shown ('  ❯ again' missing) — is the queued display capped?"
if [ -z "$queue_midturn" ]; then
	fail "the queued messages never reached the RUNNING turn — '❯ world'/'❯ again' did not commit at column 0 while the status line was still up (docs/queue.md)"
	echo "---- the last frame seen ----" >&2
	tmux capture-pane -t "$S9" -p -S -120 >&2
fi
if ! printf '%s' "$queue_done" | grep -qF "❯ world" ||
	! printf '%s' "$queue_done" | grep -qF "❯ again"; then
	fail "the queued messages were never sent — '❯ world' and '❯ again' did not both reach scrollback"
fi
expect_has "$queue_done" -F "Done for" "the turn that read the queued messages never finished — no 'Done for' summary"
expect_lacks "$queue_done" -F "Finished for" "a SECOND turn ran ('Finished for') — the queued messages must be read by the turn already running, not dispatched after it"
