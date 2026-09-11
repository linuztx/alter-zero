#!/usr/bin/env bash
# Phase 98 — a subagent's PERMISSION PROMPT is about the conversation ON SCREEN

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# a subagent's PERMISSION PROMPT is about the conversation ON
# SCREEN (docs/permissions.md, docs/agent-view-streaming.md). In manual mode,
# standing inside a lone FOREGROUND subagent's session view, its `bash` request
# opened over the LEAD's live `● Agent(…)` / `⎿ Working…` cell — the main
# strip's lone-agent tree, which that screen does not show — hiding the agent's
# own `● Bash(ls -la)` / `⎿ Waiting…` and every waiting sibling of its parallel
# batch. The `agent-permission` demo launches one foreground subagent that
# announces two `bash` calls and asks at the shared gate before each: walk into
# its session and assert the prompt's context is ITS cells, not the lead's —
# then leave and assert the MAIN view still shows the lead's cell, which is
# what is on screen there.
S98="${S}_agentperm"
tmux new-session -d -s "$S98" -x 100 -y 34 "$APP"
sleep 0.7
tmux send-keys -t "$S98" -l "subagent permission demo"
sleep 0.3
tmux send-keys -t "$S98" Enter
agentperm_row=""
for _ in $(seq 1 200); do # the foreground launch puts its row on the roster
	if tmux capture-pane -t "$S98" -p | grep -qF "Run ls -la via subagent"; then
		agentperm_row=1
		break
	fi
	sleep 0.1
done
if [ -z "$agentperm_row" ]; then
	fail "the gated subagent demo never put its row on the footer roster"
fi
# The lead's own cell IS right in the main view — a lone foreground launch
# wears the tool-cell look there (docs/agent-tool.md).
agentperm_main="$(tmux capture-pane -t "$S98" -p)"
expect_has "$agentperm_main" -F "● Agent(Run ls -la via subagent)" "the main view is missing the lead's live agent cell"
# ↓ opens the roster on `● main`, a second ↓ steps onto the agent, Enter opens
# its session — all inside the demo's pre-roll, before its first request.
tmux send-keys -t "$S98" Down
sleep 0.2
tmux send-keys -t "$S98" Down
sleep 0.2
tmux send-keys -t "$S98" Enter
agentperm_entered=""
for _ in $(seq 1 60); do # the view is up once its rule carries the description
	if tmux capture-pane -t "$S98" -p | grep -qF "─ Run ls -la via subagent ─"; then
		agentperm_entered=1
		break
	fi
	sleep 0.1
done
if [ -z "$agentperm_entered" ]; then
	fail "never reached the subagent's session view (the demo's pre-roll must outlast the ↓ ↓ Enter walk)"
fi
agentperm_view="$(wait_pane 20 "$S98" -F "Do you want to proceed?")" # …and its first `bash` call raises the prompt
echo "==== Phase 98: the prompt inside the subagent's session view ===="
printf '%s\n' "$agentperm_view"
expect_has "$agentperm_view" -F "● Bash(ls -la)" "the prompt does not show the AGENT's own call above it"
expect_has "$agentperm_view" -F "● Bash(pwd)" "the batch's waiting sibling is missing from the prompt's context"
if [ "$(printf '%s' "$agentperm_view" | grep -cF "⎿  Waiting…")" -ne 2 ]; then
	fail "both queued cells must read '⎿ Waiting…' above the prompt"
fi
expect_lacks "$agentperm_view" -F "● Agent(Run ls -la via subagent)" "the LEAD's agent cell covered the agent's own cells (the reported bug)"
expect_has "$agentperm_view" -F "Bash command · from the general-purpose agent" "the prompt does not say which subagent asked"
# Answer it (option 1), let the batch drain, then Esc back to the main view:
# the lead's cell is on screen there, and the agent's own cells are not.
tmux send-keys -t "$S98" Enter
sleep 0.6
wait_for 20 "$S98" -F "don't ask again for: pwd" # the second call raises its own prompt
agentperm_second="$(tmux capture-pane -t "$S98" -p)"
expect_has "$agentperm_second" -F "● Bash(pwd)" "the second prompt lost the call it is about"
tmux send-keys -t "$S98" Enter
sleep 0.8
tmux send-keys -t "$S98" Escape
sleep 0.8
agentperm_back="$(tmux capture-pane -t "$S98" -p)"
echo "==== Phase 98: back in the main view ===="
printf '%s\n' "$agentperm_back"
expect_has "$agentperm_back" -F "subagent permission demo" "Esc did not return to the main conversation"
tmux kill-session -t "$S98" 2>/dev/null
echo "==== Phase 98: a subagent's prompt asks about the screen it opens on ===="
