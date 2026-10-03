#!/usr/bin/env bash
# Phase 130 — the agent tools: agentlist · agentsend · agentoutput · agentwait · agentkill

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The model's side of a launched agent (docs/agent-tools.md), drivable
# offline: the `agent-tools` demo launches the table-streaming subagent in
# the background and works it with every companion through the REAL
# executor. Assert each cell — named by the agent's DESCRIPTION, as a session
# cell is named by its command — and the real reports inside them: the list
# naming the agent by id, the delivery acknowledgement, the progress report
# listing the agent's `Write(…)` steps and the delivered message, the wait's
# `finished in …` over the final response, and a kill that finds nothing left
# to stop. The id shows on the user's side too: the launch cell's
# `Running in the background as a…` row and the roster row's suffix. Then
# the agent's own session view: the message the model sent is a bubble on
# its transcript, taken at its round boundary.
S130="${S}_agenttools"
launch "$S130" 110 50
submit "$S130" "control an agent with the agent tools"
wait_for 60 "$S130" -S -500 -- -F "Nothing to stop."
sleep 1
main="$(tmux capture-pane -t "$S130" -p -S -500)"
echo "==== Phase 130: the companions' cells (main view) ===="
printf '%s\n' "$main" | grep -E "Agent(List|Send|Output|Wait|Kill)\(|agent:|Message delivered|Running in the background as|comparison table +a[0-9a-z]{8} ·" | sed -n '1,30p'
expect_has "$main" -F "● AgentList" "the agentlist cell never committed"
expect_has "$main" -F "1 agent:" "agentlist's report is missing its count"
expect_has "$main" -E "a[0-9a-z]{8}: general-purpose \"Stream a comparison table\" — running" "agentlist's row does not name the agent by id and state"
expect_has "$main" -F "● AgentSend(Stream a comparison table ← Add a fifth row for Elixir)" "the agentsend cell is not named by the agent's description and message"
expect_has "$main" -F "Message delivered to agent" "agentsend's delivery acknowledgement is missing"
expect_has "$main" -F "● AgentOutput(Stream a comparison table)" "the agentoutput cell never committed"
expect_has "$main" -F "● AgentWait(Stream a comparison table)" "the agentwait cell never committed"
expect_has "$main" -F "● AgentKill(Stream a comparison table)" "the agentkill cell never committed"
expect_has "$main" -F "is not running" "agentkill on the finished agent did not report it as finished"
expect_has "$main" -E "Running in the background as a[0-9a-z]{8} \(↓ to manage" "the launch cell does not name the agent's id"
expect_has "$main" -E "Stream a comparison table +a[0-9a-z]{8} · " "the roster row does not carry the agent's id before its elapsed"
# The cells fold past their first rows inline; the Ctrl+O transcript holds
# every cell whole — page up through it for the reports' bodies.
tmux send-keys -t "$S130" C-o
sleep 0.6
transcript="$(tmux capture-pane -t "$S130" -p)"
for _ in 1 2 3 4 5 6; do
	tmux send-keys -t "$S130" PageUp
	sleep 0.2
	transcript="$transcript
$(tmux capture-pane -t "$S130" -p)"
done
tmux send-keys -t "$S130" q
sleep 0.4
echo "==== Phase 130: the reports (Ctrl+O transcript) ===="
printf '%s\n' "$transcript" | grep -E "Steps|Write\(notes|Message: |Reply so far|Final response|finished in|running [0-9]+s" | sed -n '1,30p'
expect_has "$transcript" -F "Steps so far:" "agentoutput's report does not list the steps so far"
expect_has "$transcript" -F "Write(notes/languages.md)" "agentoutput's report does not list the agent's Write step"
expect_has "$transcript" -F "Message: Add a fifth row for Elixir" "the delivered message is not among the agent's steps"
expect_has "$transcript" -F "finished in" "agentwait did not report the agent finished"
expect_has "$transcript" -F "Final response:" "agentwait's report has no final response"
# The agent's own session: ↓ onto main, ↓ onto the agent, Enter — the
# message the model sent is a user bubble on its transcript.
tmux send-keys -t "$S130" Down
sleep 0.3
tmux send-keys -t "$S130" Down
sleep 0.3
tmux send-keys -t "$S130" Enter
sleep 0.8
view="$(tmux capture-pane -t "$S130" -p -S -200)"
echo "==== Phase 130: the agent's session view ===="
printf '%s\n' "$view" | grep -E "❯ |Compare four languages|Stream a comparison table" | sed -n '1,10p'
expect_has "$view" -F "Compare four languages" "the agent session view did not open on the agent's transcript"
expect_has "$view" -F "❯ Add a fifth row for Elixir" "the message the model sent is not a bubble on the agent's transcript"
tmux kill-session -t "$S130" 2>/dev/null
echo "==== Phase 130: the agent tools — every companion's cell carries the real executor's report ===="
