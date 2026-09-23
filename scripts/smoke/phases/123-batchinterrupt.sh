#!/usr/bin/env bash
# Phase 123 — Esc mid-batch keeps a red cell for EVERY call (running and waiting): on screen, through a rebuild, and in the context

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Esc during a PARALLEL batch (docs/interrupt.md; docs/parallel-tools.md
# "Interrupting a batch"). The dummy's "parallel" prompt announces three
# `Bash(ping …)` calls up front; its output lines are slowed so the first call
# is still running — its two siblings `⎿ Waiting…` — when Esc lands. Every one
# of the three must then commit its red `⎿ Interrupted by user` cell ABOVE the
# `Conversation interrupted` notice, in the batch's order. Two bugs lived here:
# the interrupt dropped the waiting siblings outright (no record, so no cell and
# no place in the model's next context), and it committed no cell at all — the
# notice was already recorded behind the resolved call, and the commit path
# only ever looked at the last history item — so the pane showed the notice
# alone until something rebuilt it from history. Ctrl+D then shows the context
# the next request derives: all three calls, each answered. And a resize's
# purge rebuild renders the same history: each cell exactly once.
S_INT="${S}_batchint"
launch "$S_INT" 90 40 "env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS ALTER_ZERO_CHUNK_DELAY_MS=400 $BIN"
submit "$S_INT" "run three pings in parallel"
if ! wait_for 25 "$S_INT" -F "Waiting…"; then
	fail "the parallel batch never showed a ⎿ Waiting… sibling"
fi
keys "$S_INT" Escape
interrupted="$(wait_settled 5 "$S_INT" -S -60 -- -F "Conversation interrupted")"
note "the pane after Esc mid-batch — no rebuild has run"
printf '%s\n' "$interrupted"

keys "$S_INT" C-d
context="$(wait_settled 5 "$S_INT" -- -F "C O N T E X T")"
note "Ctrl+D: the context the next request derives"
printf '%s\n' "$context"
keys "$S_INT" C-d # close it (idle Esc could arm a backtrack instead)

# A width change purge-rebuilds scrollback from history (invariant 3).
tmux resize-window -t "$S_INT" -x 80 -y 40
rebuilt="$(wait_settled 5 "$S_INT" -S -200 -- -F "Conversation interrupted")"
note "after a resize rebuild (scrollback included)"
printf '%s\n' "$rebuilt"
tmux kill-session -t "$S_INT" 2>/dev/null

# Every call of the batch keeps its cell — the running one and both siblings.
expect_has "$interrupted" -F "Conversation interrupted" "Esc mid-batch did not commit the 'Conversation interrupted' notice"
expect_lacks "$interrupted" -F "Waiting…" "a ⎿ Waiting… cell survived the interrupt (a sibling still live)"
expect_lacks "$interrupted" -F "Running…" "a ⎿ Running… cell survived the interrupt"
expect_lacks "$interrupted" -F "esc to interrupt" "the live status line is still showing after the interrupt"
for cmd in "ping -c 20 google.com" "ping -c 20 facebook.com" "ping -c 20 x.invalid"; do
	expect_has "$interrupted" -F "Bash($cmd)" "the interrupted batch lost its 'Bash($cmd)' cell"
done
cells=$(printf '%s\n' "$interrupted" | grep -cF "Interrupted by user")
expect_eq "${cells:-0}" 3 "each of the three calls resolves to its own '⎿ Interrupted by user' row"
# …in the batch's order, all above the notice.
line_of() { printf '%s\n' "$interrupted" | grep -nF "$1" | head -1 | cut -d: -f1; }
google="$(line_of "Bash(ping -c 20 google.com)")"
facebook="$(line_of "Bash(ping -c 20 facebook.com)")"
invalid="$(line_of "Bash(ping -c 20 x.invalid)")"
notice="$(line_of "Conversation interrupted")"
if ! [ "${google:-0}" -gt 0 ] || ! [ "${google:-0}" -lt "${facebook:-0}" ] ||
	! [ "${facebook:-0}" -lt "${invalid:-0}" ] || ! [ "${invalid:-0}" -lt "${notice:-0}" ]; then
	fail "the interrupted cells are not in batch order above the notice (google=$google facebook=$facebook x.invalid=$invalid notice=$notice)"
fi

# The context replays the whole round: three calls, three answers.
calls=$(printf '%s\n' "$context" | grep -cF '→ bash(')
expect_eq "${calls:-0}" 3 "Ctrl+D does not carry every call of the interrupted batch"
answers=$(printf '%s\n' "$context" | grep -cF "Interrupted by user")
expect_eq "${answers:-0}" 3 "Ctrl+D does not answer every interrupted call"
expect_has "$context" -F "[error] Conversation interrupted" "Ctrl+D lost the interrupt notice"

# The rebuild agrees with the live commit: every cell, each exactly once — no
# record duplicated, nothing the purge left behind.
rebuilt_cells=$(printf '%s\n' "$rebuilt" | grep -cF "Interrupted by user")
expect_eq "${rebuilt_cells:-0}" 3 "the rebuild does not show the three interrupted calls exactly once each"
for cmd in "ping -c 20 google.com" "ping -c 20 facebook.com" "ping -c 20 x.invalid"; do
	count=$(printf '%s\n' "$rebuilt" | grep -cF "Bash($cmd)")
	expect_eq "${count:-0}" 1 "the rebuild does not show the 'Bash($cmd)' cell exactly once"
done
rebuilt_notices=$(printf '%s\n' "$rebuilt" | grep -cF "Conversation interrupted")
expect_eq "${rebuilt_notices:-0}" 1 "the rebuild does not show the notice exactly once"

# ---- the reported case: Esc on the batch's PERMISSION prompt ----
# The approve seam runs before a call's ToolStart, so while the first call
# asks, BOTH cells still read `⎿ Waiting…`. Esc there cancels the prompt and
# interrupts the turn — and both calls must resolve, the asked-about one and
# its sibling, instead of the first alone (the prompt's close purge-rebuilds
# the screen from history, which is why that one used to show). The model
# must read both next turn, too.
S_PERM="${S}_batchperm"
launch "$S_PERM" 90 40
submit "$S_PERM" "parallel permission demo"
asking="$(wait_pane 20 "$S_PERM" -S -60 -- -F "Do you want to proceed?")"
note "the batch's first permission prompt"
printf '%s\n' "$asking"
keys "$S_PERM" Escape
cancelled="$(wait_settled 5 "$S_PERM" -S -60 -- -F "Conversation interrupted")"
note "the pane after Esc on the prompt"
printf '%s\n' "$cancelled"
keys "$S_PERM" C-d
perm_context="$(wait_settled 5 "$S_PERM" -- -F "C O N T E X T")"
note "Ctrl+D after Esc on the prompt"
printf '%s\n' "$perm_context"
keys "$S_PERM" C-d
tmux kill-session -t "$S_PERM" 2>/dev/null

waiting=$(printf '%s\n' "$asking" | grep -cF "Waiting…")
expect_eq "${waiting:-0}" 2 "while the first call asks, both cells of the batch read ⎿ Waiting…"
expect_has "$cancelled" -F "Conversation interrupted" "Esc on the prompt did not interrupt the turn"
expect_lacks "$cancelled" -F "Waiting…" "a ⎿ Waiting… cell survived Esc on the prompt"
expect_lacks "$cancelled" -F "Do you want to proceed?" "the permission prompt is still open"
for cmd in "sudo whoami" "ping -c 4 google.com"; do
	expect_has "$cancelled" -F "Bash($cmd)" "Esc on the prompt lost the 'Bash($cmd)' cell"
done
perm_cells=$(printf '%s\n' "$cancelled" | grep -cF "Interrupted by user")
expect_eq "${perm_cells:-0}" 2 "the asked-about call and its sibling each resolve to '⎿ Interrupted by user'"
expect_has "$perm_context" -F 'bash({"command":"sudo whoami"})' "Ctrl+D lost the call the prompt asked about"
expect_has "$perm_context" -F 'bash({"command":"ping -c 4 google.com"})' "Ctrl+D lost the waiting sibling — the model would never know it asked for it"
perm_answers=$(printf '%s\n' "$perm_context" | grep -cF "Interrupted by user")
expect_eq "${perm_answers:-0}" 2 "Ctrl+D does not answer both calls of the cancelled batch"
