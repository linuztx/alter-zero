#!/usr/bin/env bash
# Phase 44 — shell children are DETACHED from the controlling terminal (crate::subprocess, 

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# shell children are DETACHED from the controlling terminal
# (crate::subprocess, docs/shell-command.md). A command that opens /dev/tty — which
# is exactly what sudo's password prompt does — must error at once instead of
# printing over the TUI and blocking on the keyboard the event loop owns: the
# runner re-execs the binary's detached-exec helper mode, which setsid()s into
# a fresh session (no controlling terminal → the open fails ENXIO). The probe
# writes a marker to /dev/tty (deterministic where sudo isn't installed or the
# runner is root): attached, the marker prints OVER the TUI and the command
# exits 0; detached, the redirect fails fast. The marker is assembled by
# printf so the echoed command text can never contain it verbatim.
S44="${S}_notty"
launch "$S44" 80 24
submit "$S44" "!printf 'LEAK%s\\n' _MARK > /dev/tty"
notty_pane="$(wait_pane 6 "$S44" -S -40 -- -F "exit status:")" # up to ~6s — the whole point is that it fails fast
echo "==== Phase 44: pane after '! printf … > /dev/tty' (detached — fails, never prints) ===="
printf '%s\n' "$notty_pane"
tmux kill-session -t "$S44" 2>/dev/null

# Exactly one input box on a captured screen: one bare prompt row (the composer's

# Phase 44: shell children have no controlling terminal — the /dev/tty probe
# (sudo's password-prompt mechanism) fails fast inside the cell instead of
# printing over the TUI or blocking on the keyboard.
expect_has "$notty_pane" -F "No such device or address" "the /dev/tty open did not fail (the shell child still has a controlling terminal, so a sudo prompt would hijack the TUI)"
expect_has "$notty_pane" -F "exit status:" "the '! … > /dev/tty' cell never resolved (the command blocked — the sudo-hang bug)"
expect_lacks "$notty_pane" -F "LEAK_MARK" "the marker printed over the TUI (the child wrote straight to /dev/tty)"
