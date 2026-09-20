#!/usr/bin/env bash
# Phase 22 — a `!` command with HUGE output is CAPPED IN MEMORY, not buffered whole

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# a `!` command with HUGE output is CAPPED IN MEMORY, not buffered
# whole (docs/shell-command.md). Run a command producing >100KB (over the cap):
# the cell renders the whole retained head INLINE — the `!` shell cell never
# folds, so there is no `+N lines (ctrl+o to expand)` hint — closed by the
# `…` truncation marker where the cap cut it, NO temp file is written (the
# output is never held in full — this is the memory fix), and the Ctrl+O view
# ends with the same `…`.
S19="${S}_bigoutput"
# Start from a clean slate so the "no temp file" check sees only what this run
# would create (the feature is gone, so it should stay zero).
rm -f /tmp/alter-zero-shell-*.txt 2>/dev/null
launch "$S19" 80 24
# `seq 1 50000` is ~280KB across many lines (well over the 100KB cap) and
# contains no `…` of its own, so any `…` on screen is the marker.
submit "$S19" "!seq 1 50000"
# The retained head is thousands of rows committed to scrollback; the visible
# pane settles on the cell's tail — the last retained numbers, the `…` marker
# as a row of its own, then the composer.
bigoutput="$(wait_settled 30 "$S19" -S -40 -- -E '^[[:space:]]+…[[:space:]]*$')" # up to ~30s
echo "==== captured pane (huge !output capped in memory, shown whole) ===="
printf '%s\n' "$bigoutput"
# No temp file may be created — the full output is never written anywhere now.
bigoutput_tmpfiles="$(ls /tmp/alter-zero-shell-*.txt 2>/dev/null | wc -l | tr -d ' ')"
echo "bigoutput_tmpfiles=[$bigoutput_tmpfiles]"
# Open the Ctrl+O view (it opens pinned to the bottom) — its end carries the `…`.
tmux send-keys -t "$S19" C-o
bigoutput_overlay="$(wait_pane 3 "$S19" -F "T R A N S C R I P T")"
echo "==== captured Ctrl+O overlay (bottom — truncation marker) ===="
printf '%s\n' "$bigoutput_overlay"
tmux kill-session -t "$S19" 2>/dev/null

# Phase 22: a huge !output is capped in memory (no temp file), shown whole
# inline, `…` marks the cut.
expect_has "$bigoutput" -E '^[[:space:]]+[0-9]+[[:space:]]*$' "the retained head's rows are not in view — the shell cell should show its output inline"
expect_lacks "$bigoutput" -F "ctrl+o to expand" "a huge !output showed a '+N lines (ctrl+o to expand)' fold hint — the shell cell shows its whole output inline"
expect_has "$bigoutput" -E '^[[:space:]]+…[[:space:]]*$' "the inline cell did not end with the '…' truncation marker for the capped output"
if [ "${bigoutput_tmpfiles:-x}" != "0" ]; then
	fail "a huge !output wrote a temp file ($bigoutput_tmpfiles found) — output must be capped in memory, not saved"
fi
expect_has "$bigoutput_overlay" -F "…" "the Ctrl+O view did not append a '…' truncation marker for the capped output"
