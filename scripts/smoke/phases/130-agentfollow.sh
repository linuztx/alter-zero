#!/usr/bin/env bash
# Phase 130 — the lead WORKS ITS AGENT: wait, follow up, wait, list

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The agent tools (docs/agent-tools.md). The `agent-follow-up` demo launches
# a background agent and works it the way a live model did: `agentoutput`
# waits on it, `agentsend` resumes its FINISHED conversation with a
# follow-up, a second wait reports that answer, and `agentlist` lists it —
# every companion call through the real executor against the real registry.
# Assert each cell shows what the executor reported (the frame, the call
# summary, the mark where the follow-up arrived), that NEITHER answer comes
# back a second time as a completion notice — the waits reported them, the
# observed settle — and that the agent's own session view shows the lead's
# message as a bubble between its two answers. Then the user messages the
# agent in that view, and the lead's notice of the answer must name the
# user's message as the user's (Ctrl+D shows what the lead read).
S130="${S}_agentfollow"
launch "$S130" 120 50
submit "$S130" "send an agent a follow-up"
settled="$(wait_pane 40 "$S130" -F "$SETTLED_REPLY")" ||
	fail "the follow-up demo never finished its turn"
done_pane="$(wait_settled 10 "$S130" -E "^$SUMMARY_RE")" ||
	fail "the follow-up demo's turn never settled"
full="$(pane "$S130" -S -200)"
dump "the lead's transcript" "$full"
# The launch's result is model-facing (its cell shows the fixed background
# row), so its id is checked where the model reads it: Ctrl+D.
keys "$S130" C-d
sleep 0.5
keys "$S130" Home
context="$(wait_pane 5 "$S130" -F "launched as a")"
dump "Ctrl+D — the launch result the model read" "$context"
keys "$S130" C-d
sleep 0.3
# ↓ onto the roster, ↓ onto the agent, Enter: its own session.
keys "$S130" Down
sleep 0.2
keys "$S130" Down
sleep 0.2
keys "$S130" Enter
view="$(wait_pane 5 "$S130" -F "Read CHANGELOG.md")" ||
	fail "the agent's session view did not open"
view="$(pane "$S130" -S -200)"
dump "the agent's own session" "$view"
# Now the USER talks to the agent, in its session view. The lead never wrote
# this message, so the answer's notice must say who did — otherwise the lead
# reads a reply to a question it never asked as its own conversation.
submit "$S130" "thanks, keep it short"
wait_pane 15 "$S130" -F "this reply is its next turn" >/dev/null ||
	fail "the agent never answered the user's message"
keys "$S130" Escape
wait_pane 15 "$S130" -F 'Agent "Read the release notes" finished' >/dev/null ||
	fail "the user's conversation with the agent was never noticed to the lead"
wait_settled 30 "$S130" -E "^$SUMMARY_RE" >/dev/null ||
	fail "the follow-up turn the notice started never settled"
keys "$S130" C-d
sleep 0.5
keys "$S130" End
told="$(wait_pane 5 "$S130" -F "The user messaged this agent directly")"
dump "Ctrl+D — what the lead was told about the user's message" "$told"
keys "$S130" C-d
tmux kill-session -t "$S130" 2>/dev/null

note "the agent tools — launch, wait, follow-up, wait, list; observed settles; the session view"
expect_has "$context" -E "launched as a[0-9a-z]{8}" "the launch result the model read never named the agent's id"
expect_has "$full" -F "● AgentOutput(Read the release notes)" "the agentoutput cell is missing or not named by the agent's task"
expect_has "$full" -E "Done \(agent a[0-9a-z]{8} · Read the release notes · [0-9]+s · 1 tool use\)" "the first wait did not report the settled agent's frame"
expect_has "$full" -F "Bash(head -12 CHANGELOG.md)" "the report did not summarize the agent's call"
expect_has "$full" -F "● AgentSend(Read the release notes ← And what did it add for questions nobody answers?)" "the agentsend cell is missing or not named by the agent's task"
expect_has "$full" -F "resumed with your message" "agentsend did not resume the finished agent"
expect_has "$full" -F "— message received —" "the second report did not mark where the follow-up arrived"
expect_has "$full" -F "● AgentList" "the agentlist cell is missing"
expect_has "$full" -F "1 agent:" "agentlist did not list the agent"
expect_lacks "$full" -F 'Agent "Read the release notes" finished' "an answer a wait already reported came back again as a notice"
expect_has "$view" -F "❯ And what did it add for questions nobody answers?" "the agent's session view does not show the lead's follow-up as a bubble"
expect_has "$view" -F "keeps working on its best judgment" "the agent's session view does not show its answer to the follow-up"
expect_has "$told" -F "The user messaged this agent directly in its session view" "the lead's notice does not say the user wrote to the agent"
expect_has "$told" -F -- "- thanks, keep it short" "the lead's notice does not quote the user's message"
smoke_finish
