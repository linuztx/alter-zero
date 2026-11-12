#!/usr/bin/env bash
# Phase 19 — `!` shell commands

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# `!` shell commands (docs/input/shell-command.md). Typing `!cmd` enters
# shell mode: the bang is absorbed into the prompt (the composer reads `! cmd`,
# not `❯ !cmd`) and the footer flips to "Shell mode". Enter from an idle
# composer runs the command locally as a codex-style exec cell — the `! cmd`
# header on the dark user-style line with its `⎿` output flush below (and
# `⎿ Running…` while it runs); a long command is interruptible with Esc.
S16="${S}_shell"
launch "$S16" 80 24
# Shell mode: the bang becomes the prompt and the footer hint shows.
tmux send-keys -t "$S16" -l "!echo smoke_shell_ok"
sleep 0.3
shell_mode="$(tmux capture-pane -t "$S16" -p)"
echo "==== captured visible screen (typing !echo … — Shell mode hint) ===="
printf '%s\n' "$shell_mode"
# Run it: Enter dispatches the command; poll for the cell's ⎿ output line.
tmux send-keys -t "$S16" Enter
shell_ran="$(wait_pane 6 "$S16" -S -40 -- -F "⎿  smoke_shell_ok")" # up to ~6s
echo "==== captured pane (after !echo ran) ===="
printf '%s\n' "$shell_ran"
# A failing command resolves the cell red (non-zero exit) and keeps running.
submit "$S16" "!exit 3"
shell_fail="$(wait_pane 6 "$S16" -S -40 -- -F "exit status: 3")"
echo "==== captured pane (after !exit 3 — failure) ===="
printf '%s\n' "$shell_fail"
# A long command runs with the `⎿ Running… (Ns)` preview and NO spinner status
# line (req 3 — the elapsed rides the preview instead of the hidden status).
submit "$S16" "!sleep 9"
sleep 1.2 # let it run long enough to show a non-zero elapsed
shell_running="$(tmux capture-pane -t "$S16" -p -S -40)"
echo "==== captured pane (!sleep 9 running — ⎿ Running… (Ns) preview, no status) ===="
printf '%s\n' "$shell_running"
# Esc resolves the cell `⎿ Interrupted by user` — NOT the `Conversation
# interrupted` notice (req 2: the shell cell is its own record).
tmux send-keys -t "$S16" Escape
shell_interrupt="$(wait_pane 4 "$S16" -S -40 -- -F "Interrupted by user")" # up to ~4s — far less than the 9s sleep
echo "==== captured pane (after Esc interrupts !sleep 9) ===="
printf '%s\n' "$shell_interrupt"
tmux kill-session -t "$S16" 2>/dev/null
# How each command ran rides its record in the rollout
# (docs/state/rollout-records.md): its exit status, its time, why it failed.
# The recorder writes at the loop bottom, so poll for the third call's line.
shell_rollout=""
for _ in $(seq 1 30); do
	shell_rollout="$(cat "$SMOKE_SESSIONS"/*/*/*/rollout-*.jsonl 2>/dev/null | grep -F '"type":"tool"')"
	[ "$(printf '%s\n' "$shell_rollout" | grep -c '"shell":true')" -ge 3 ] && break
	sleep 0.1
done
echo "==== the rollout's tool lines ===="
printf '%s\n' "$shell_rollout"
shell_ok_line="$(printf '%s\n' "$shell_rollout" | grep -F '"name":"echo smoke_shell_ok"')"
shell_fail_line="$(printf '%s\n' "$shell_rollout" | grep -F '"name":"exit 3"')"
shell_sleep_line="$(printf '%s\n' "$shell_rollout" | grep -F '"name":"sleep 9"')"

# Phase 19: `!` shell commands.
expect_has "$shell_mode" -F "Shell mode" "typing a !command did not show the 'Shell mode' footer hint"
expect_has "$shell_mode" -E "^! echo smoke_shell_ok" "the composer does not absorb the bang into a '! cmd' prompt"
expect_lacks "$shell_mode" -F "❯ !echo" "the composer still shows the bang as text ('❯ !echo …')"
expect_has "$shell_ran" -E "^! echo smoke_shell_ok" "the committed cell is missing its '! echo …' header line"
expect_has "$shell_ran" -F "⎿  smoke_shell_ok" "the shell command's ⎿ output line is not in view"
expect_lacks "$shell_ran" -E "● echo|^Ran for" "the old '● cmd' tool header / 'Ran for' summary resurfaced"
expect_has "$shell_fail" -F "exit status: 3" "a failing !command did not report its non-zero exit status"
# Req 3: a running !command shows the `⎿ Running…` preview with its elapsed and
# NO spinner status line (its elapsed rides the preview instead).
expect_has "$shell_running" -E "⎿  Running… \([0-9]+s\)" "a running !command did not show the '⎿ Running… (Ns)' preview"
expect_lacks "$shell_running" -F "esc to interrupt" "a running !command showed the spinner status line — req 3: shell hides it (the elapsed rides the ⎿ Running… preview)"
# Req 2: Esc resolves the shell cell `⎿ Interrupted by user` — and does NOT
# commit the redundant `Conversation interrupted` notice (the cell is the record).
expect_has "$shell_interrupt" -F "Interrupted by user" "Esc did not resolve a long-running !command as '⎿ Interrupted by user'"
expect_lacks "$shell_interrupt" -F "Conversation interrupted" "a shell interrupt committed the redundant 'Conversation interrupted' notice — req 2: the ⎿ cell is the record"
# The rollout records how each command ran (docs/state/rollout-records.md): a
# success's exit and time, a failure's exit and kind, and an Esc'd command's
# kind with the second-plus it ran before the Esc — but no exit status.
expect_has "$shell_ok_line" -F '"exit_code":0' "a !command's record lacks its exit status"
expect_has "$shell_ok_line" -E '"duration_ms":[0-9]+' "a !command's record lacks its time"
expect_lacks "$shell_ok_line" -F '"error_kind"' "a successful !command's record names an error kind"
expect_has "$shell_fail_line" -F '"exit_code":3' "a failing !command's record lacks its exit status"
expect_has "$shell_fail_line" -F '"error_kind":"tool_error"' "a failing !command's record is not a tool_error"
expect_has "$shell_sleep_line" -F '"error_kind":"interrupted"' "an Esc'd !command's record is not interrupted"
expect_has "$shell_sleep_line" -E '"duration_ms":[0-9]{4,}' "an Esc'd !command's record lost the time it ran"
expect_lacks "$shell_sleep_line" -F '"exit_code"' "an Esc'd !command's record claims an exit status"
