#!/usr/bin/env bash
# Phase 59 — the conversation stays VISIBLE through a batch's back-to-back prompts

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the conversation stays VISIBLE through a batch's back-to-back
# prompts (docs/permissions.md). The prompt sits at the bottom like any other
# region, so the just-sent user message, the previous turn, and both batch
# cells are real rows above it — and the first call's resolved cell commits
# above the still-open second prompt the moment it lands (no held queue). The
# close purge-rebuilds, so the final screen is whole: box flush at the
# bottom, each message committed exactly once. The dummy's "parallel
# permission" turn scripts two gated Bash calls with NO pause between the
# first cell's resolution and the second request — the hardest timing.
S59="${S}_parperm"
launch "$S59" 80 44
# Fill the screen so the composer sits flush at the bottom (the covering
# precondition — a short conversation's prompt fits below and covers nothing).
for parperm_msg in "hello there" "tell me more about it" "and a little more"; do
	submit "$S59" "$parperm_msg"
	# Wait for the turn to START before waiting for it to end: the settle loop
	# below breaks on the ABSENCE of the status line, so on a loaded machine —
	# eight workers on four cores — it can sample before the first frame paints,
	# break at once and race the turn it was meant to wait out. Phase 62 failed
	# exactly that way in a full parallel run while passing alone.
	wait_for 20 "$S59" -F "esc to interrupt"
	sleep 0.6
	for _ in $(seq 1 300); do
		cap="$(tmux capture-pane -t "$S59" -p)"
		if ! printf '%s' "$cap" | grep -qF "esc to interrupt"; then
			break
		fi
		sleep 0.1
	done
	sleep 0.3
done
submit "$S59" "parallel permission demo"
parperm_prompt1=""
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S59" -p)"
	if printf '%s' "$cap" | grep -qF "Do you want to proceed?"; then
		parperm_prompt1="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 59: pane with the first prompt (sudo whoami) open ===="
printf '%s\n' "$parperm_prompt1"
# Approve the first command: its cell resolves and the second request lands in
# the same frame gap (the dummy scripts no pause between them).
tmux send-keys -t "$S59" -l "1"
parperm_prompt2=""
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S59" -p)"
	if printf '%s' "$cap" | grep -qF "Ping google.com 4 times"; then
		parperm_prompt2="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 59: pane with the second prompt (ping) open ===="
printf '%s\n' "$parperm_prompt2"
tmux send-keys -t "$S59" -l "1"
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S59" -p)"
	if printf '%s' "$cap" | grep -qF "Both commands are done" &&
		! printf '%s' "$cap" | grep -qF "esc to interrupt"; then
		break
	fi
	sleep 0.05
done
sleep 0.5
parperm_final="$(tmux capture-pane -t "$S59" -p)"
echo "==== Phase 59: final pane (the conversation must be whole) ===="
printf '%s\n' "$parperm_final"
parperm_footer=$(printf '%s\n' "$parperm_final" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)
parperm_hist="$(tmux capture-pane -t "$S59" -p -S -200)"
parperm_dupes=$(printf '%s\n' "$parperm_hist" | grep -cF '❯ parallel permission demo')
tmux kill-session -t "$S59" 2>/dev/null
echo "==== Phase 59: covering prompts replay the conversation and keep it whole ===="
if [ -z "$parperm_prompt1" ]; then
	fail "the first permission prompt never showed"
fi
# While the FIRST prompt is up: the just-sent message, the previous turn, and
# both batch cells (each ⎿ Waiting…) are all still on screen above it. The
# previous turn is checked by its REPLY (its closing hand-off paragraph, right
# above the just-sent message), not by its user message: a demo turn is a
# dozen rows taller now that its Read cell renders the real numbered file body
# (docs/dummy-backend.md), so three of them plus a screen-tall prompt no longer
# fit on a 30-row pane — the older rows scroll into real scrollback, which is
# what Phase 58 checks with `-S`. What matters here is unchanged: the rows
# above the prompt are real, visible conversation, not a covered void.
expect_has "$parperm_prompt1" -F "❯ parallel permission demo" "the just-sent user message is hidden while the first prompt is up"
expect_has "$parperm_prompt1" -F "$SETTLED_REPLY" "the previous turn is hidden while the first prompt is up"
if [ "$(printf '%s\n' "$parperm_prompt1" | grep -cF "⎿  Waiting…")" -lt 2 ]; then
	fail "the batch's two pending calls don't both show ⎿ Waiting…"
fi
if [ -z "$parperm_prompt2" ]; then
	fail "the second permission prompt never showed"
fi
# While the SECOND prompt is up: the first call's finished cell (committed in
# the gap between the prompts) and the user message are still on screen.
expect_has "$parperm_prompt2" -F "❯ parallel permission demo" "the user message is hidden while the second prompt is up"
expect_has "$parperm_prompt2" -F "a terminal is required" "the first call's finished cell is hidden while the second prompt is up"
if [ "${parperm_footer:-0}" != "44" ]; then
	fail "after the prompts the footer sits on row ${parperm_footer:-none} of 44 (the box is not back flush at the bottom)"
fi
expect_has "$parperm_final" -F "a terminal is required" "the first call's cell is missing from the final conversation"
if [ "${parperm_dupes:-0}" != "1" ]; then
	fail "'❯ parallel permission demo' appears $parperm_dupes times in scrollback+screen (the close rebuild lost or duplicated a row)"
fi
