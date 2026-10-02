#!/usr/bin/env bash
# Phase 128 — an unanswered question times out and the agent carries on

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The AskUserQuestion idle timeout (docs/ask.md). With
# ALTER_ZERO_ASK_TIMEOUT_SECS=8 the dummy's question demo waits eight seconds
# of IDLENESS: the modal's closing rule counts down, a key press starts the
# wait over, and once nothing is touched the question resolves unanswered —
# the red "User did not finish answering within 8s" cell keeping the one
# answer given — while the turn carries on to its closing reply and the
# composer draft typed before the modal comes back.
S128="${S}_asktimeout"
APP_AT="env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=800 ALTER_ZERO_ASK_TIMEOUT_SECS=8 $BIN"
launch "$S128" 100 44 "$APP_AT"
submit "$S128" "ask me some questions"
sleep 0.3
# Typed while the request is on its way — this draft must survive the modal.
type_text "$S128" "a draft typed while it asks"
modal="$(wait_pane 10 "$S128" -F "continues without you in")" ||
	fail "the modal never showed its countdown"
dump "the question counts down in its closing rule" "$modal"
# 2 picks Latte: the first question is answered, and the key starts the wait
# over — the clock now runs out 8s from here unless another key comes.
type_text "$S128" "2"
sleep 4
before="$(pane "$S128")"
# Halfway there, a plain ↓ says the user is still here: the count jumps back.
keys "$S128" Down
restarted="$(wait_pane 3 "$S128" -F "continues without you in 0:08")" ||
	fail "a key did not start the wait over (no 0:08 after ↓)"
dump "↓ started the wait over" "$restarted"
# Past the deadline the pick had set — still open, because of the ↓.
sleep 6
alive="$(pane "$S128")"
dump "past the first deadline, still waiting" "$alive"
# Then, untouched, it runs out on its own and the turn goes on.
carried="$(wait_pane 15 "$S128" -F "$SETTLED_REPLY")" ||
	fail "the turn never carried on to its closing reply"
settled="$(wait_settled 10 "$S128" -F "a draft typed while it asks")" ||
	fail "the draft never came back to the composer"
dump "timed out; the agent carried on and the draft is back" "$settled"
tmux kill-session -t "$S128" 2>/dev/null

note "the AskUserQuestion idle timeout — countdown, key restarts, expiry, the cell, the carry-on, the draft"
expect_has "$modal" -F "What's your favorite way to drink coffee?" "the modal showed the wrong page"
expect_has "$before" -E "continues without you in 0:0[3-5]" "the countdown did not run down while idle"
expect_has "$alive" -F "continues without you in" "the question expired on the deadline the key had replaced"
expect_has "$carried" -F "User did not finish answering within 8s" "the timeout cell is missing its headline"
expect_has "$carried" -F "→ Latte" "the timeout cell dropped the answer given before the user left"
expect_has "$carried" -F "carrying on without you" "the demo did not carry on after the timeout"
expect_has "$settled" -F "❯ a draft typed while it asks" "the composer draft did not come back"
expect_lacks "$settled" -F "continues without you in" "the countdown outlived its question"
