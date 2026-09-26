#!/usr/bin/env bash
# Phase 38 — a parallel tool-call batch shows ⎿ Waiting… siblings and tails the running call's output

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# ---- drive: Phase 38 ----
# PARALLEL tool-call batch — the not-yet-run calls show `⎿ Waiting…`
# (docs/parallel-tools.md). A prompt mentioning "parallel" makes the dummy announce
# a three-call `Bash(ping …)` batch up front (the user's example): while the first
# runs, the other two show dim `⎿ Waiting…` cells in the live region above the box,
# the whole batch visible at once. Drive a fresh, tall session and poll for that
# transient state (the batch runs after the pre-stream pause + first text +
# thinking). The default turn (other phases) keeps its compact 2-call batch.
S_BATCH="${S}_batch"
launch "$S_BATCH" 90 40
# Record the pane's RAW byte stream for Phase 39 below, which reads it instead
# of sampling the screen — see there for why. Start it before the turn so no
# frame is missed.
RAW39="$(mktemp)"
tmux pipe-pane -t "$S_BATCH" -o "cat >> $RAW39"
submit "$S_BATCH" "run three pings in parallel"
batch_waiting=""
for _ in $(seq 1 130); do # up to ~20s (pause + first-half text + thinking, then tools)
	cap="$(tmux capture-pane -t "$S_BATCH" -p -S -50)"
	if printf '%s' "$cap" | grep -qF "Waiting…"; then
		batch_waiting="$cap"
		break
	fi
	sleep 0.15
done
echo "==== Phase 38: captured pane (parallel batch — a running Bash(ping) cell + ⎿ Waiting… siblings) ===="
printf '%s\n' "$batch_waiting"

# Phase 38: a parallel tool-call batch shows its not-yet-run calls as ⎿ Waiting…
# (docs/parallel-tools.md).
expect_has "$batch_waiting" -F "Waiting…" "a parallel batch did not show '⎿ Waiting…' for its queued calls"
# The running call + at least one waiting sibling are visible at once, so two or
# more `● Bash(ping …)` cells show together (the whole batch is visible before the
# calls finish one at a time).
batch_cells=$(printf '%s\n' "$batch_waiting" | grep -cF "Bash(ping")
if [ "${batch_cells:-0}" -lt 2 ]; then
	fail "only ${batch_cells:-0} 'Bash(ping …)' cells visible at once (expected >= 2: the running call + a waiting sibling)"
fi
# ---- drive: Phase 39 ----
# the running Bash(ping) cell TAILS its live output — the last
# lines under the `⎿` gutter and a `+N lines (Ns)` footer (docs/tool-streaming.md),
# Claude-Code's running-command look. The output streams in *after* the Waiting…
# capture above.
#
# Read the RECORDING, not the screen. The footer only exists while the output
# has overflowed the peek window and the call has not yet resolved, and the
# dummy's scripted pings make that window tiny: the output lines pace at
# CHUNK_DELAY (45ms) and the batch's longest is six lines, so the footer is on
# screen for ~95ms on the first call and ~48ms on the second — measured. A
# `capture-pane` poll samples at its own cadence (~115ms with the subprocess),
# so it misses the window outright perhaps one run in ten: the reported
# Phase 39 failure, which reproduces on any machine and is a fixture race, not
# a regression. `pipe-pane` has no cadence — it records every byte tmux wrote
# — so the footer's arrival is caught whatever it costs the app to paint it,
# and the assertion below still reads the real `+N lines (Ns · wait 2m)`
# row — the elapsed beside the timeout the call runs under, the tool's 2m
# default here since the demo's scripted call names none
# (docs/tool-streaming.md).
batch_tail=""
for _ in $(seq 1 200); do # up to ~20s — the tail streams as each ping runs
	if grep -qaE '\+[0-9]+ lines \([0-9]+s · wait 2m\)' "$RAW39"; then
		batch_tail="$(grep -aoE '\+[0-9]+ lines \([0-9]+s · wait 2m\)' "$RAW39" | head -4)"
		break
	fi
	sleep 0.1
done
tmux pipe-pane -t "$S_BATCH" # stop recording
echo "==== Phase 39: the running cell's tail footer, off the pane's raw byte stream ===="
printf '%s\n' "$batch_tail"
echo "==== Phase 39: the pane at the end of the batch ===="
tmux capture-pane -t "$S_BATCH" -p -S -60
rm -f "$RAW39"
tmux kill-session -t "$S_BATCH" 2>/dev/null

# Phase 39: a running Bash(ping) cell TAILS its live output — the last lines plus
# a `+N lines (Ns · wait 2m)` footer, Claude-Code's running-command look
# with the call's timeout beside the elapsed (docs/tool-streaming.md).
expect_has "$batch_tail" -E '\+[0-9]+ lines \([0-9]+s · wait 2m\)' "a running Bash(ping) cell did not tail its streamed output (no '+N lines (Ns · wait 2m)' footer)"
