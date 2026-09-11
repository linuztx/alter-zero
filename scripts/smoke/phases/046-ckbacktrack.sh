#!/usr/bin/env bash
# Phase 46 — Esc-Esc BACKTRACK resets the CODE, not just the transcript

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Esc-Esc BACKTRACK resets the CODE, not just the transcript
# (docs/checkpoint.md). Each turn snapshots the working directory into an
# isolated git store (never the user's real .git); rewinding to an earlier user
# message restores the files to that point. Deterministic with the dummy
# backend: a pristine file, a text turn (no file change), then a `!` shell turn
# that MUTATES the file — backtracking to the first user message must revert the
# file to pristine.
S46="${S}_ckbacktrack"
CK_DIR="$(mktemp -d "$SMOKE_TMP/ck.XXXXXX")"
CK_SESS="$(mktemp -d "$SMOKE_TMP/cksess.XXXXXX")"
WORK46="$(mktemp -d "$SMOKE_TMP/work46.XXXXXX")"
printf 'pristine\n' >"$WORK46/file.txt"
# These phases run the app in a temp cwd (-c), so the binary must be an
# ABSOLUTE path — a relative $BIN would resolve against the temp dir and fail.
# Re-enable checkpoints here (overriding CFG_ENV's =0) — safe: this runs in a
# throwaway temp cwd ($WORK46), so a restore's git clean can't touch the repo.
CKAPP="env $CFG_ENV ALTER_ZERO_CHECKPOINTS=1 ALTER_ZERO_CHECKPOINTS_DIR=$CK_DIR ALTER_ZERO_SESSIONS_DIR=$CK_SESS ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"
launch -c "$WORK46" "$S46" 80 24 "$CKAPP"
submit "$S46" "$USER_MSG"
wait_for 20.1 "$S46" -S -40 -- -F "Done for" # text turn 1 → "Done for" (checkpoint {pristine})
submit "$S46" "!echo mutated > file.txt" # a `!` shell turn mutates the file
ckb_mutated="?"
for _ in $(seq 1 80); do # wait for the shell TURN to END (file written AND Running gone)
	ckb_mutated="$(cat "$WORK46/file.txt" 2>/dev/null)"
	if [ "$ckb_mutated" = "mutated" ] &&
		! tmux capture-pane -t "$S46" -p -S -20 | grep -qF "Running"; then
		break
	fi
	sleep 0.15
done
sleep 0.5 # let StreamDone → dispatch_after_turn snapshot + flush the {mutated} checkpoint
tmux send-keys -t "$S46" Escape # idle Esc → prime the backtrack
sleep 0.3
tmux send-keys -t "$S46" Escape # → transcript preview on the sole user message
sleep 0.4
tmux send-keys -t "$S46" Enter # rewind to before it → restore checkpoint {pristine}
ckb_restored="?"
for _ in $(seq 1 60); do
	ckb_restored="$(cat "$WORK46/file.txt" 2>/dev/null)"
	if [ "$ckb_restored" = "pristine" ]; then break; fi
	sleep 0.15
done
echo "==== Phase 46: working file after mutate='$ckb_mutated', after backtrack='$ckb_restored' ===="
tmux kill-session -t "$S46" 2>/dev/null
if [ "$ckb_mutated" != "mutated" ]; then
	fail "the ! shell turn did not mutate the working file (precondition; file is '$ckb_mutated')"
fi
if [ "$ckb_restored" != "pristine" ]; then
	fail "Esc-Esc backtrack did not reset the code to the checkpoint (file is '$ckb_restored', expected 'pristine')"
fi
