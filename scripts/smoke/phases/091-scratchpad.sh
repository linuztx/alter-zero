#!/usr/bin/env bash
# Phase 91 — the SESSION TEMP TREE

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the SESSION TEMP TREE (docs/scratchpad.md). One per-user,
# per-session root holds two named leaves: `scratchpad/` — created before the
# first frame, so the system prompt can name a directory that exists — and
# `tasks/`, where a background shell tees its interim output. TMPDIR points the
# whole tree at a throwaway dir so this phase can find it unambiguously (and so
# a concurrent session of the developer's own is never mistaken for it).
S91="${S}_scratchpad"
SP_TMP="$(mktemp -d "$SMOKE_TMP/pad.XXXXXX")"
SPAPP="env $CFG_ENV_NOHIST TMPDIR=$SP_TMP ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"
tmux new-session -d -s "$S91" -x 80 -y 24 "$SPAPP"
sp_dir=""
for _ in $(seq 1 60); do
	sp_dir="$(find "$SP_TMP" -maxdepth 3 -type d -name scratchpad 2>/dev/null | head -1)"
	if [ -n "$sp_dir" ]; then break; fi
	sleep 0.15
done
# A backgrounded `!` command tees into the tasks leaf beside it.
submit "$S91" "!sleep 30"
sleep 0.6
tmux send-keys -t "$S91" C-b # move the running command to the background
sp_out=""
for _ in $(seq 1 60); do
	sp_out="$(find "$SP_TMP" -maxdepth 4 -path '*/tasks/*.output' 2>/dev/null | head -1)"
	if [ -n "$sp_out" ]; then break; fi
	sleep 0.15
done
sp_pane="$(tmux capture-pane -t "$S91" -p)"
tmux kill-session -t "$S91" 2>/dev/null
echo "==== Phase 91: scratchpad='${sp_dir#"$SP_TMP"}', task output='${sp_out#"$SP_TMP"}' ===="
if [ -z "$sp_dir" ]; then
	fail "no scratchpad directory was created under the session root"
elif ! printf '%s' "$sp_dir" | grep -qE "^$SP_TMP/alter-zero-[0-9]+/[^/]+/scratchpad$"; then
	fail "the scratchpad is not at {tmp}/alter-zero-{uid}/{session}/scratchpad (got '$sp_dir')"
fi
if [ -z "$sp_out" ]; then
	fail "a backgrounded command left no interim output file"
elif [ "$(dirname "$(dirname "$sp_out")")" != "$(dirname "$sp_dir")" ]; then
	fail "the tasks dir is not the scratchpad's sibling (output '$sp_out', scratchpad '$sp_dir')"
fi
expect_has "$sp_pane" -F "Running in the background" "Ctrl+B did not move the command to the background"
rm -rf "$SP_TMP" 2>/dev/null
