#!/usr/bin/env bash
# Phase 130 — the agent companions control a launched agent by id

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the agent companions (docs/agent-tools.md). The `agent-control` demo
# launches the table-streaming subagent in the background and then plays the
# four companions as scripted cells whose results are the registry's REAL
# reports: `AgentOutput` while the agent's write batch runs (the call headers,
# the second still `← running`), `AgentList`, `AgentWait` once it finishes
# (the `Done (agent …)` frame over the response), and `AgentSend` with a
# follow-up that continues the finished agent — whose row, retired or not,
# comes back onto the roster for the continuation. Every cell names the agent
# by its task, the way a bashsend cell names its session by its command.
S130="${S}_agenttools"
launch "$S130" 110 50
submit "$S130" "launch a subagent and control it"
output_cell="$(wait_pane 30 "$S130" -S -300 -- -F "Running (agent a")"
echo "==== Phase 130: the AgentOutput cell ===="
printf '%s\n' "$output_cell" | grep -B 2 -A 4 -F "AgentOutput(" | sed -n '1,12p'
expect_has "$output_cell" -F "● AgentOutput(Stream a comparison table)" "the AgentOutput cell does not name the agent by its task"
expect_has "$output_cell" -E "Running \(agent a[0-9a-z]{8}\) — general-purpose \"Stream a comparison table\"" "the AgentOutput report lacks the Running frame"
expect_has "$output_cell" -F "Write(notes/languages.md)" "the AgentOutput report lacks the agent's first call header"
list_cell="$(wait_pane 30 "$S130" -S -300 -- -F "1 agent:")"
expect_has "$list_cell" -F "● AgentList" "the AgentList cell never showed"
expect_has "$list_cell" -E "^.*- a[0-9a-z]{8}: general-purpose \"Stream a comparison table\" — (running|done after)" "the AgentList row lacks the agent's id, type and task"
wait_cell="$(wait_pane 40 "$S130" -S -300 -- -F "Done (agent a")"
echo "==== Phase 130: the AgentWait cell ===="
printf '%s\n' "$wait_cell" | grep -A 4 -F "AgentWait(" | sed -n '1,8p'
expect_has "$wait_cell" -F "● AgentWait(Stream a comparison table)" "the AgentWait cell does not name the agent by its task"
send_cell="$(wait_pane 30 "$S130" -S -300 -- -F "is working on your message")"
echo "==== Phase 130: the AgentSend cell ===="
printf '%s\n' "$send_cell" | grep -A 3 -F "AgentSend(" | sed -n '1,6p'
expect_has "$send_cell" -F "● AgentSend(Stream a comparison table ← Add a row for Zig to the table.)" "the AgentSend cell lacks the task and the message"
# The demo turn settles, and the continued agent's row is back on the roster
# (it is running its follow-up turn) — then its completion notice commits.
wait_for 30 "$S130" -E "$SUMMARY_RE"
settled="$(wait_pane 40 "$S130" -S -300 -- -F "finished ·")"
echo "==== Phase 130: the continuation's completion notice ===="
printf '%s\n' "$settled" | grep -F "Agent \"Stream a comparison table\"" | sed -n '1,3p'
expect_has "$settled" -F "● Agent \"Stream a comparison table\" finished" "the continued agent's completion never posted its notice cell"
# Ctrl+O: the companions' full reports — the response the wait carried.
tmux send-keys -t "$S130" C-o
sleep 0.6
overlay="$(tmux capture-pane -t "$S130" -p -S -400)"
tmux send-keys -t "$S130" q
expect_has "$overlay" -F "Response:" "the Ctrl+O transcript lacks the AgentWait report's Response: heading"
expect_has "$overlay" -F "Four rows, one grid." "the Ctrl+O transcript lacks the agent's response inside the AgentWait report"
tmux kill-session -t "$S130" 2>/dev/null
echo "==== Phase 130: the agent companions — AgentOutput, AgentList, AgentWait, AgentSend cells over the registry's real reports ===="
