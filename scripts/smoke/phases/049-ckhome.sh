#!/usr/bin/env bash
# Phase 49 — checkpoints REFUSE a home-directory cwd

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

CK_SESS="$(mktemp -d "$SMOKE_TMP/cksess.XXXXXX")"

# checkpoints REFUSE a home-directory cwd (docs/checkpoint.md).
# The session-start snapshot `git add -A`s the whole cwd before the first
# frame ever paints; run with cwd = `~` itself that hashed the user's entire
# home directory into the store — minutes of blocked, blank, raw-mode
# terminal and hundreds of MB per snapshot (the "alter-zero hangs in ~" bug).
# The
# guard (`checkpoint::cwd_scope`) disables the store when the
# cwd IS the home dir (or an ancestor of it, or a filesystem root) — even
# with an explicit ALTER_ZERO_CHECKPOINTS=1 — while a project dir UNDER home
# checkpoints exactly as before. Assert on the store dir itself: it must stay
# empty after a home-cwd launch and populate after a project-cwd launch.
S49="${S}_ckhome"
WORK49="$(mktemp -d "$SMOKE_TMP/home49.XXXXXX")"
CK49="$(mktemp -d "$SMOKE_TMP/ck49.XXXXXX")"
mkdir -p "$WORK49/proj"
CKAPP49="env $CFG_ENV ALTER_ZERO_CHECKPOINTS=1 ALTER_ZERO_CHECKPOINTS_DIR=$CK49 ALTER_ZERO_SESSIONS_DIR=$CK_SESS HOME=$WORK49 ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"
tmux new-session -d -s "$S49" -x 80 -y 24 -c "$WORK49" "$CKAPP49"
ckhome_ready=0
for _ in $(seq 1 40); do # the footer proves startup completed (init ran or was skipped)
	if tmux capture-pane -t "$S49" -p | grep -qF "dummy_model_name"; then
		ckhome_ready=1
		break
	fi
	sleep 0.1
done
ckhome_store="$(ls -A "$CK49" 2>/dev/null | wc -l)"
tmux send-keys -t "$S49" C-c # quit (empty composer)
sleep 0.3
tmux kill-session -t "$S49" 2>/dev/null
# The contrast run: a project dir UNDER the same home must still checkpoint —
# the guard is scoped, not a blanket disable.
tmux new-session -d -s "$S49" -x 80 -y 24 -c "$WORK49/proj" "$CKAPP49"
ckproj_store=0
for _ in $(seq 1 40); do # the session-start snapshot inits the store before the loop
	if [ "$(ls -A "$CK49" 2>/dev/null | wc -l)" -gt 0 ]; then
		ckproj_store=1
		break
	fi
	sleep 0.1
done
tmux kill-session -t "$S49" 2>/dev/null
echo "==== Phase 49: home-cwd store entries=$ckhome_store (want 0), project-cwd store created=$ckproj_store (want 1) ===="
if [ "$ckhome_ready" != 1 ]; then
	fail "precondition — the app did not start in the home-cwd launch"
fi
if [ "$ckhome_store" != 0 ]; then
	fail "a home-directory cwd initialized the checkpoint store (it must never snapshot ~)"
fi
if [ "$ckproj_store" != 1 ]; then
	fail "a project dir under home did not checkpoint (the guard must be scoped to ~ itself, not everything under it)"
fi
