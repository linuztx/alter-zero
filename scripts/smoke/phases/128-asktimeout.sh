#!/usr/bin/env bash
# Phase 128 — an unanswered question times out

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the question timeout (docs/ask.md "When the user is away"). An
# AskUserQuestion call waits for an idle user only so long — six seconds here
# (ALTER_ZERO_ASK_TIMEOUT_SECS; the default is ten minutes) — and then the
# agent carries on without them: the modal closes by itself, the composer
# draft stashed under it comes back, a red "User did not answer within 6s"
# cell names the questions that went unanswered, and the model reads the
# short continue-without-them instruction (Ctrl+D shows exactly that). The
# clock measures ABSENCE, so every key restarts it: a user stepping through
# the options for longer than the timeout is never cut off. In its final
# minute the modal says so on the row above its closing rule — with six
# seconds in all, from the moment it opens.
S128="${S}_asktimeout"
APP_TIMEOUT="env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=300 ALTER_ZERO_ASK_TIMEOUT_SECS=6 $BIN"
launch "$S128" 100 44 "$APP_TIMEOUT"
submit "$S128" "ask me some questions"
sleep 0.3
# Typed while the request is on its way — the timeout must hand it back.
type_text "$S128" "a draft typed while it asks"
asked="$(wait_pane 15 "$S128" -F "What's your favorite way to drink coffee?")"
dump "the question, counting down" "$asked"
expect_has "$asked" -E "Continuing without your answer in 0:0[1-6]" \
	"the modal never warned that the agent will carry on without an answer"

# Present: a key every two seconds for eight — longer than the timeout.
for _ in 1 2 3 4; do
	sleep 2
	keys "$S128" Down
done
sleep 0.5
alive="$(pane "$S128")"
dump "eight seconds of keys later" "$alive"
expect_has "$alive" -F "What's your favorite way to drink coffee?" \
	"the question timed out while the user was pressing keys (a key must restart the clock)"
expect_lacks "$alive" -F "User did not answer" "a cell committed under an active user"

# Away: no more keys — within the timeout (and some slack) it carries on.
gone="$(wait_pane 12 "$S128" -F "User did not answer within 6s")"
dump "the user stopped pressing keys" "$gone"
expect_has "$gone" -F "User did not answer within 6s" "the question never timed out"
expect_has "$gone" -F "· What's your favorite way to drink coffee? (Black / Latte / Cold brew)" \
	"the timed-out cell does not name the unanswered question and its options"
expect_lacks "$gone" -F "Enter to select" "the modal is still on screen after the timeout"
settled="$(wait_pane 15 "$S128" -E "^$SUMMARY_RE")"
dump "the turn carried on and settled" "$settled"
expect_has "$settled" -F "carry on without you" "the turn did not carry on after the timeout"
expect_has "$settled" -F "$SETTLED_REPLY" "the demo reply never finished"
expect_has "$settled" -F "❯ a draft typed while it asks" \
	"the composer draft stashed under the modal did not come back"

# What the model read: the carry-on instruction, never a stop-and-wait.
keys "$S128" C-d
context="$(wait_pane 5 "$S128" -F "is not available")"
dump "Ctrl+D — the tool result the model read" "$context"
flat="$(printf '%s' "$context" | tr '\n' ' ' | tr -s ' ')"
expect_has "$flat" -F "The user did not answer within 6s and is not available. Do not wait or ask again" \
	"the model did not read the carry-on instruction"
expect_lacks "$flat" -F "STOP what you are doing" "the model read the decline's stop-and-wait instead"
keys "$S128" C-d
tmux kill-session -t "$S128" 2>/dev/null
