#!/usr/bin/env bash
# Phase 22 — a `!` command with HUGE output is CAPPED IN MEMORY, not buffered whole

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# a `!` command with HUGE output is CAPPED IN MEMORY, not buffered
# whole (docs/shell-command.md). Run a command producing >100KB (over the cap):
# the cell renders the retained head with the usual `+N lines (ctrl+o to expand)`
# peek hint, NO temp file is written (the output is never held in full — this is
# the memory fix), and the Ctrl+O view appends a `…` truncation marker at its
# end.
S19="${S}_bigoutput"
# Start from a clean slate so the "no temp file" check sees only what this run
# would create (the feature is gone, so it should stay zero).
rm -f /tmp/alter-zero-shell-*.txt 2>/dev/null
launch "$S19" 80 24
# `seq 1 50000` is ~280KB across many lines (well over the 100KB cap) and
# contains no `…` of its own, so any `…` in the Ctrl+O view is the marker.
submit "$S19" "!seq 1 50000"
bigoutput="$(wait_pane 6 "$S19" -S -40 -- -F "ctrl+o to expand")" # up to ~6s
echo "==== captured pane (huge !output capped in memory) ===="
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

# Phase 22: a huge !output is capped in memory (no temp file), `…` marks the cut.
expect_has "$bigoutput" -F "⎿  1" "a huge !output did not render its retained head (the '⎿ 1' first line)"
expect_has "$bigoutput" -F "ctrl+o to expand" "a huge !output did not show the '+N lines (ctrl+o to expand)' peek hint"
if [ "${bigoutput_tmpfiles:-x}" != "0" ]; then
	fail "a huge !output wrote a temp file ($bigoutput_tmpfiles found) — output must be capped in memory, not saved"
fi
expect_has "$bigoutput_overlay" -F "…" "the Ctrl+O view did not append a '…' truncation marker for the capped output"
