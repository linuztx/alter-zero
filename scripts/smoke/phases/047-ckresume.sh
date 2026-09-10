#!/usr/bin/env bash
# Phase 47 — /resume resets the CODE to the saved session's checkpoint

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

CK_DIR="$(mktemp -d "$SMOKE_TMP/ck.XXXXXX")"
CK_SESS="$(mktemp -d "$SMOKE_TMP/cksess.XXXXXX")"

# /resume resets the CODE to the saved session's checkpoint
# (docs/checkpoint.md). Launch 1 records a session whose `!` shell turn leaves
# the file at v1; the process quits. The file is then DIVERGED on disk (as if
# later work changed it). Launch 2's /resume of that session must restore the
# file to v1 — the transcript and the code agree again. Same cwd across
# launches, so the isolated store (keyed by cwd) still holds the v1 commit.
S47="${S}_ckresume"
WORK47="$(mktemp -d "$SMOKE_TMP/work47.XXXXXX")"
printf 'pristine\n' >"$WORK47/file.txt"
CKAPP2="env $CFG_ENV ALTER_ZERO_CHECKPOINTS=1 ALTER_ZERO_CHECKPOINTS_DIR=$CK_DIR ALTER_ZERO_SESSIONS_DIR=$CK_SESS ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"
launch -c "$WORK47" "$S47" 80 24 "$CKAPP2"
submit "$S47" "$USER_MSG" # a user message so the session lists in the picker
wait_for 20.1 "$S47" -S -40 -- -F "Done for"
submit "$S47" "!echo v1 > file.txt" # the session's final code state
for _ in $(seq 1 80); do # wait for the shell TURN to END so its {v1} checkpoint records
	if [ "$(cat "$WORK47/file.txt" 2>/dev/null)" = "v1" ] &&
		! tmux capture-pane -t "$S47" -p -S -20 | grep -qF "Running"; then
		break
	fi
	sleep 0.15
done
sleep 0.5 # let the shell turn's {v1} checkpoint record + flush to the session file
tmux send-keys -t "$S47" C-c # quit launch 1 (empty composer)
sleep 0.4
tmux kill-session -t "$S47" 2>/dev/null
printf 'divergent\n' >"$WORK47/file.txt" # later work diverges the code on disk
ckr_diverged="$(cat "$WORK47/file.txt" 2>/dev/null)"
launch -c "$WORK47" "$S47" 80 24 "$CKAPP2"
tmux send-keys -t "$S47" -l "/resume"
sleep 0.3
tmux send-keys -t "$S47" Enter # open the picker
sleep 0.6
tmux send-keys -t "$S47" Enter # resume the highlighted session → restore its final checkpoint
ckr_restored="?"
for _ in $(seq 1 60); do
	ckr_restored="$(cat "$WORK47/file.txt" 2>/dev/null)"
	if [ "$ckr_restored" = "v1" ]; then break; fi
	sleep 0.15
done
echo "==== Phase 47: working file diverged='$ckr_diverged', after resume='$ckr_restored' ===="
tmux kill-session -t "$S47" 2>/dev/null
if [ "$ckr_diverged" != "divergent" ]; then
	fail "the working file was not diverged before resume (precondition; file is '$ckr_diverged')"
fi
if [ "$ckr_restored" != "v1" ]; then
	fail "/resume did not reset the code to the session's checkpoint (file is '$ckr_restored', expected 'v1')"
fi
