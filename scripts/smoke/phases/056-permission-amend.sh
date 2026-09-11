#!/usr/bin/env bash
# Phase 56 — Tab's amend feedback reaches the MODEL and keeps reaching it

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Tab's amend feedback reaches the MODEL and keeps reaching it
# (docs/permissions.md). Rejecting with typed instructions must (a) record them
# on the red cell — the transcript's only trace of what was asked for — and
# (b) put the model-facing denial, feedback included, into the derived LLM
# context, so Ctrl+D shows it and every later turn still carries it. The bug
# this guards: history kept only the one-line cell text, so the instructions
# reached the model for exactly one round and then vanished.
S56="${S}_permission_amend"
APP_AMEND="env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=1200 $BIN"
launch "$S56" 100 44 "$APP_AMEND"
submit "$S56" "permission demo please"
amend_prompt=""
for _ in $(seq 1 250); do
	cap="$(tmux capture-pane -t "$S56" -p)"
	if printf '%s' "$cap" | grep -qF "Do you want to create hello.py?"; then
		amend_prompt="$cap"
		break
	fi
	sleep 0.05
done
# Tab, type the instructions, Enter — reject WITH feedback.
tmux send-keys -t "$S56" Tab
sleep 0.3
tmux send-keys -t "$S56" -l "use pathlib instead"
sleep 0.3
tmux send-keys -t "$S56" Enter
amend_cell=""
for _ in $(seq 1 250); do
	cap="$(tmux capture-pane -t "$S56" -p -S -60)"
	if printf '%s' "$cap" | grep -qF "User rejected write to hello.py"; then
		amend_cell="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 56: captured pane (the rejected cell carries the instructions) ===="
printf '%s\n' "$amend_cell"
# Wait for the turn to settle, then read the derived context.
cap="$(wait_pane 12.5 "$S56" -S -60 -- -F "left the file alone")"
tmux send-keys -t "$S56" C-d
sleep 0.6
amend_ctx="$(tmux capture-pane -t "$S56" -p -S -60)"
echo "==== Phase 56: captured pane (Ctrl+D — what the model was told) ===="
printf '%s\n' "$amend_ctx"
tmux send-keys -t "$S56" q
sleep 0.3
tmux kill-session -t "$S56" 2>/dev/null
echo "==== Phase 56: Tab amend — instructions on the cell AND in the LLM context ===="
if [ -z "$amend_prompt" ]; then
	fail "the permission prompt never showed"
fi
if [ -z "$amend_cell" ]; then
	fail "the amended rejection never committed its red cell"
fi
expect_has "$amend_cell" -F "Instructions: use pathlib instead" "the typed instructions are not recorded on the rejected cell"
expect_lacks "$amend_cell" -F "STOP what you are doing" "the model-facing denial text leaked into the rendered cell"
expect_has "$amend_ctx" -F "STOP what you are doing" "the derived context lacks the model-facing denial (it replayed the cell text)"
expect_has "$amend_ctx" -F "use pathlib instead" "the amend feedback is missing from the LLM context (the logging bug)"
