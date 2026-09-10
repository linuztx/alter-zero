#!/usr/bin/env bash
# Phase 69 — the task tools' live checklist

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the task tools' live checklist (docs/task-tools.md) — the
# "todo" demo drives a real TaskStore: the ⎿ ◻ rows render under the status
# line while the turn runs (the blocked suffix included), the spinner wears
# the active task's activeForm, NO task call ever commits a tool cell to the
# conversation, and the Ctrl+O transcript keeps the full per-call record.
tmux new-session -d -s "${S}_tasks" -x 100 -y 30 "$APP"
sleep 0.4
tmux send-keys -t "${S}_tasks" -l "demo the todo tool i want to see how it works"
sleep 0.2
tmux send-keys -t "${S}_tasks" Enter
# Poll the LIVE screen for the checklist and the spinner override while the
# turn streams (the demo paces one task call per TOOL_DELAY, so both states
# hold for whole seconds).
tasks_checklist=""
tasks_verb=""
for _ in $(seq 1 200); do # up to ~20s
	pane="$(tmux capture-pane -t "${S}_tasks" -p)"
	if [ -z "$tasks_checklist" ] && printf '%s' "$pane" | grep -qF "› blocked by #1"; then
		tasks_checklist="$pane"
	fi
	if [ -z "$tasks_verb" ] && printf '%s' "$pane" | grep -qE "(Setting up the project structure|Writing the core logic)…"; then
		tasks_verb="$pane"
	fi
	if [ -n "$tasks_checklist" ] && [ -n "$tasks_verb" ]; then
		break
	fi
	sleep 0.1
done
# Let the turn actually SETTLE before reading the resting screen. The
# hand-off text alone is not that signal — it streams while the turn is
# still running, so polling for it lands on a mid-turn frame whose strip
# still shows the in-turn checklist. Wait for the status line to be gone
# from the VISIBLE screen (the turn is over) with the hand-off already in
# scrollback.
tasks_done=""
tasks_rest=""
for _ in $(seq 1 300); do # up to ~30s
	pane="$(tmux capture-pane -t "${S}_tasks" -p -S -120)"
	screen="$(tmux capture-pane -t "${S}_tasks" -p)"
	if printf '%s' "$pane" | grep -qF "$SETTLED_REPLY" &&
		! printf '%s' "$screen" | grep -qF "esc to interrupt"; then
		# The committed conversation (with scrollback) for the
		# no-cells assertions; the visible screen alone for the resting
		# block — scrollback still holds the mid-turn frames' rows,
		# which are exactly what the resting assertions must not see.
		tasks_done="$pane"
		tasks_rest="$screen"
		break
	fi
	sleep 0.1
done
# The Ctrl+O transcript keeps the record the conversation hides.
tmux send-keys -t "${S}_tasks" C-o
sleep 0.5
tasks_overlay="$(tmux capture-pane -t "${S}_tasks" -p -S -120)"
tmux send-keys -t "${S}_tasks" C-o
sleep 0.3
echo "==== Phase 69: captured pane (mid-turn checklist) ===="
printf '%s\n' "$tasks_checklist" | grep -v "^$" | tail -12
echo "==== Phase 69: captured pane (at rest — the standalone block) ===="
printf '%s\n' "$tasks_rest" | grep -v "^$" | tail -8
tmux kill-session -t "${S}_tasks" 2>/dev/null
echo "==== Phase 69: the task tools' live checklist ===="
if [ -z "$tasks_checklist" ]; then
	fail "the checklist (with its '› blocked by #1' suffix) never rendered under the status line"
else
	if ! printf '%s' "$tasks_checklist" | grep -qF "⎿  ◻"; then
		expect_has "$tasks_checklist" -E "⎿  [◻◼✔]" "the checklist rows are missing the ⎿ gutter + status glyph"
	fi
fi
if [ -z "$tasks_verb" ]; then
	fail "the spinner never wore an in-progress task's activeForm"
fi
if [ -z "$tasks_done" ]; then
	fail "the tasks demo never settled on the hand-off"
else
	expect_lacks "$tasks_done" -E "● Task(Create|Update|List|Get)" "a task call committed a tool cell to the conversation (they must render nothing inline)"
	expect_has "$tasks_done" -F "Three tasks created, all pending" "the narration bullets did not commit around the hidden calls"
fi
# The demo ends with work left (#1 done, #2 in progress, #3 pending), so the
# STANDALONE block takes over at rest: the dim count line over the remaining
# rows, above the composer, gutter-less — there is no spinner left to hang
# from (docs/task-tools.md). Asserted against the VISIBLE screen: scrollback
# still holds the mid-turn frames, whose rows do wear the gutter.
if [ -z "$tasks_rest" ]; then
	fail "the turn never settled, so the resting screen was never read"
else
	expect_has "$tasks_rest" -F "3 tasks (1 done, 1 in progress, 1 open)" "the resting screen is missing the standalone task count line"
	expect_has "$tasks_rest" -F "◼ Write the core logic" "the resting block is missing the remaining task rows"
	expect_has "$tasks_rest" -F "◻ Add tests › blocked by #2" "the resting block dropped the blocked-by suffix"
	expect_lacks "$tasks_rest" -F "⎿  ✔ Set up the project structure" "the resting block still wears the in-turn ⎿ gutter"
fi
expect_has "$tasks_overlay" -F "TaskUpdate(#1 → completed)" "the Ctrl+O transcript is missing the task-call record"
expect_has "$tasks_overlay" -F "Updated task #1 status" "the Ctrl+O record is missing the executor's result text"
