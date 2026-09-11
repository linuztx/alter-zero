#!/usr/bin/env bash
# Phase 19 — `!` shell commands

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# `!` shell commands (docs/shell-command.md). Typing `!cmd` enters
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
