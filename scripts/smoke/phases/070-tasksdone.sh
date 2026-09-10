#!/usr/bin/env bash
# Phase 70 — a FINISHED checklist bows out with its turn and is gone for good

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# a FINISHED checklist bows out with its turn and is gone for
# good (docs/task-tools.md) — the reported stale-list bug: an all-✔ list kept
# riding the next turn's spinner. "finish the todo demo" plays the twin
# scenario, which walks two tasks to completed.
S70="${S}_tasksdone"
launch "$S70" 100 30
submit "$S70" "finish the todo demo"
# Mid-turn: the all-✔ closure shows — the payoff belongs to the turn that
# earned it, so the last task ticks while the status line is still up.
tasks_fin_live=""
for _ in $(seq 1 400); do # up to ~20s
	cap="$(tmux capture-pane -t "$S70" -p)"
	if printf '%s' "$cap" | grep -qF "✔ Run the demo script" &&
		printf '%s' "$cap" | grep -qF "esc to interrupt"; then
		tasks_fin_live="$cap"
		break
	fi
	sleep 0.05
done
# At rest: nothing. Wait for the status line to go (the turn is over), the
# same settle signal Phase 69 uses.
tasks_fin_rest=""
for _ in $(seq 1 300); do # up to ~30s
	cap="$(tmux capture-pane -t "$S70" -p)"
	if printf '%s' "$cap" | grep -qE "Done for [0-9]" &&
		! printf '%s' "$cap" | grep -qF "esc to interrupt"; then
		tasks_fin_rest="$cap"
		break
	fi
	sleep 0.1
done
# The next turn: the retired list must not come back under its spinner.
submit "$S70" "Thanks"
tasks_fin_next=""
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S70" -p)"
	if printf '%s' "$cap" | grep -qF "esc to interrupt"; then
		tasks_fin_next="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 70: captured pane (the all-✔ closure, inside its turn) ===="
printf '%s\n' "$tasks_fin_live" | grep -v "^$" | tail -10
echo "==== Phase 70: captured pane (at rest — the list is gone) ===="
printf '%s\n' "$tasks_fin_rest" | grep -v "^$" | tail -8
echo "==== Phase 70: captured pane (the next turn starts clean) ===="
printf '%s\n' "$tasks_fin_next" | grep -v "^$" | tail -8
tmux kill-session -t "$S70" 2>/dev/null
echo "==== Phase 70: a finished checklist retires ===="
if [ -z "$tasks_fin_live" ]; then
	fail "the finished demo never showed its all-✔ checklist inside the turn"
fi
if [ -z "$tasks_fin_rest" ]; then
	fail "the finished demo's turn never settled"
else
	expect_lacks "$tasks_fin_rest" -E "✔ (Create the demo workspace|Run the demo script)" "a finished checklist still showed at rest (it belongs to the turn that finished it)"
	expect_lacks "$tasks_fin_rest" -E "[0-9] tasks \(" "the standalone panel showed for a list with no work left"
fi
if [ -z "$tasks_fin_next" ]; then
	fail "the follow-up turn never started"
elif printf '%s' "$tasks_fin_next" | grep -qE "✔ (Create the demo workspace|Run the demo script)"; then
	fail "the retired checklist came back under the next turn's spinner (the reported stale-list bug)"
fi
