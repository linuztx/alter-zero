#!/usr/bin/env bash
# Phase 95 — the AGENT SESSION VIEW streams like the main one

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the AGENT SESSION VIEW streams like the main one
# (docs/agent-view-streaming.md). The view has its own streaming strip, and it
# used to render the last row of a BATCH render of the agent's buffer — while
# its commits went through a `StreamRender`, which withholds a forming table
# WHOLE. So a subagent streaming a table showed one row (the block's closing
# border) and every row above it was on screen nowhere: the reported
# "streaming disappears inside the subagent TUI". The `agent-stream` demo
# launches one background subagent and plays its own round on the subagent
# channel — a thinking phase, then that table — so the whole thing is drivable
# offline. Walk the roster into its session and assert (a) the live
# `● Thinking…` block shows there, and (b) the forming GRID is in the strip
# mid-stream, not just its border.
S95="${S}_agentstream"
tmux new-session -d -s "$S95" -x 100 -y 34 "$APP"
sleep 0.7
tmux send-keys -t "$S95" -l "launch a subagent that streams a table"
sleep 0.3
tmux send-keys -t "$S95" Enter
agent_row=""
for _ in $(seq 1 200); do # the background launch puts its row on the roster
	if tmux capture-pane -t "$S95" -p | grep -qF "Stream a comparison table"; then
		agent_row=1
		break
	fi
	sleep 0.1
done
if [ -z "$agent_row" ]; then
	fail "the subagent demo never put its row on the footer roster"
fi
# ↓ opens the roster on `● main`, a second ↓ steps onto the agent, Enter opens
# its session view (the demo's pre-roll leaves time for exactly this).
tmux send-keys -t "$S95" Down
sleep 0.2
tmux send-keys -t "$S95" Down
sleep 0.2
tmux send-keys -t "$S95" Enter
sleep 0.3
agent_view="$(tmux capture-pane -t "$S95" -p)"
echo "==== Phase 95: the agent session view ===="
printf '%s\n' "$agent_view" | sed -n '1,20p'
expect_has "$agent_view" -F "Compare four languages" "the agent session view did not open on the agent's own transcript"
# (a) the live thinking block, and (b) the forming grid — both while the
# agent's own status line still says it is working, i.e. before the block
# commits. A broken strip shows `└────┴…┘` alone at (b).
agent_thinking=""
agent_grid=""
for _ in $(seq 1 250); do
	frame="$(tmux capture-pane -t "$S95" -p)"
	if [ -z "$agent_thinking" ] && printf '%s' "$frame" | grep -qF "● Thinking…"; then
		agent_thinking="$frame"
	fi
	if printf '%s' "$frame" | grep -qF "Working…" &&
		printf '%s' "$frame" | grep -qF "│ Python" &&
		printf '%s' "$frame" | grep -qF "│ Rust"; then
		agent_grid="$frame"
		break
	fi
	sleep 0.1
done
if [ -z "$agent_thinking" ]; then
	fail "the subagent's live '● Thinking…' block never showed in its session view"
else
	echo "==== Phase 95: the subagent's live thinking block ===="
	printf '%s\n' "$agent_thinking" | grep -A 4 "● Thinking…" | sed -n '1,5p'
fi
if [ -z "$agent_grid" ]; then
	fail "the forming table never showed in the agent view's strip: the streaming rows are on screen nowhere (the reported bug)"
	echo "---- the last frame seen ----" >&2
	tmux capture-pane -t "$S95" -p >&2
else
	echo "==== Phase 95: the forming grid in the agent view's strip ===="
	printf '%s\n' "$agent_grid" | grep -E "│|┌|└" | sed -n '1,12p'
fi
# …and its **parallel `write` batch** commits its cells at once, with no
# resize. The file tools resolve through the two-text split
# (`StreamEvent::ToolAnswered`), which the view's commit arm did not list, so
# the cells reached the agent's transcript and stopped there — a resize
# rebuilt the view from history and they all appeared at once (the reported
# bug). Assert them in the pane *before* anything resizes it.
wait_for 25 "$S95" -S -200 -- -F "Wrote 3 lines to notes/sources.md"
agent_writes="$(tmux capture-pane -t "$S95" -p -S -200)"
for want in "Write(notes/languages.md)" "Wrote 3 lines to notes/languages.md" \
	"Write(notes/sources.md)" "Wrote 3 lines to notes/sources.md"; do
	expect_has "$agent_writes" -F "$want" "the subagent's parallel write batch never committed '$want' to scrollback (it needs a resize to appear)"
done
echo "==== Phase 95: the subagent's committed write cells ===="
printf '%s\n' "$agent_writes" | grep -A 3 -F "Write(notes/" | sed -n '1,10p'
# …and the table commits exactly once when it closes.
wait_for 20 "$S95" -F "Four rows, one grid."
sleep 0.6
agent_settled="$(tmux capture-pane -t "$S95" -p -S -200)"
grid_rows="$(printf '%s' "$agent_settled" | grep -cF "│ Python")"
if [ "${grid_rows:-0}" -ne 1 ]; then
	fail "the committed table appears $grid_rows times in scrollback (expected exactly 1)"
fi
expect_has "$agent_settled" -F "Thought for" "the subagent's settled 'Thought for …' cell is missing from its transcript"
tmux kill-session -t "$S95" 2>/dev/null
echo "==== Phase 95: the agent session view streams its frontier like the main one ===="
