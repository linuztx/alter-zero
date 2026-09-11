#!/usr/bin/env bash
# Phase 71 — checkpoints refuse the directories that are not projects, and say so

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

CK_SESS="$(mktemp -d "$SMOKE_TMP/cksess.XXXXXX")"

# checkpoints refuse the directories that are not projects, and
# say so (docs/checkpoint.md). Phase 49 covers the home dir; these are the
# three that made alter-zero "take seconds to boot": a SHARED scratch parent
# (`/tmp` — every program's junk, measured 9.7 s to first frame on a 235 MB
# tree), alter-zero's OWN state dir (where the store lives *inside* the work
# tree, so each snapshot hashed the previous ones back in — the reported 2 GB
# `.alter-zero`), and a tree simply past the cost budget (forced here with a
# tiny ALTER_ZERO_CHECKPOINT_MAX_BYTES rather than by writing 128 MiB). Each
# must disable the store AND raise the reason as a toast — a feature that goes
# quiet without saying why is what made this hard to place.
S71="${S}_ckscope"
CK71="$(mktemp -d "$SMOKE_TMP/ck71.XXXXXX")"
WORK71="$(mktemp -d "$SMOKE_TMP/work71.XXXXXX")"
printf 'print("hi")\n' >"$WORK71/app.py"
CKAPP71="env HOME=$SMOKE_REAL_HOME $CFG_ENV ALTER_ZERO_CHECKPOINTS=1 ALTER_ZERO_CHECKPOINTS_DIR=$CK71 ALTER_ZERO_SESSIONS_DIR=$CK_SESS ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"

# Launch in $2, wait for the footer, and report the toast row + store entries.
ck71_launch() {
	tmux new-session -d -s "$S71" -x 100 -y 24 -c "$2" "$3"
	ck71_ready=0
	for _ in $(seq 1 60); do
		if tmux capture-pane -t "$S71" -p | grep -qF "dummy_model_name"; then
			ck71_ready=1
			break
		fi
		sleep 0.1
	done
	ck71_pane="$(tmux capture-pane -t "$S71" -p)"
	ck71_store="$(ls -A "$CK71" 2>/dev/null | wc -l)"
	tmux kill-session -t "$S71" 2>/dev/null
	echo "==== Phase 71 ($1): ready=$ck71_ready store_entries=$ck71_store ===="
	printf '%s\n' "$ck71_pane" | grep -F "Checkpoints off" || echo "     (no refusal toast)"
	if [ "$ck71_ready" != 1 ]; then
		fail "($1) the app did not start"
	fi
}

# (a) `/tmp` itself — refused before the store is ever created.
ck71_launch "shared scratch parent" /tmp "$CKAPP71"
if [ "$ck71_store" != 0 ]; then
	fail "launching in /tmp initialized the checkpoint store (a shared scratch dir must never be snapshot)"
fi
expect_has "$ck71_pane" -F "shared scratch directory" "no 'Checkpoints off — a shared scratch directory…' toast in /tmp"

# (b) alter-zero's own state dir (here $SMOKE_CFG, via ALTER_ZERO_CONFIG_DIR)
# — the self-inclusion case, refused in both directions.
ck71_launch "own state directory" "$SMOKE_CFG" "$CKAPP71"
if [ "$ck71_store" != 0 ]; then
	fail "launching in the state dir initialized the store (the store would snapshot itself)"
fi
expect_has "$ck71_pane" -F "own state directory" "no 'Checkpoints off — this is alter-zero's own state directory' toast"

# (c) past the cost budget — the general guard, for the huge project no
# denylist can name. The store is init'd (the probe needs it) but must hold
# no commit.
ck71_launch "over the cost budget" "$WORK71" "ALTER_ZERO_CHECKPOINT_MAX_BYTES=1 $CKAPP71"
expect_has "$ck71_pane" -F "too big to snapshot" "no 'Checkpoints off — … too big to snapshot per turn' toast over the budget"
ck71_dir="$CK71/$(printf '%s' "$WORK71" | sed 's/[^a-zA-Z0-9]/-/g')"
if git --git-dir="$ck71_dir" rev-parse HEAD >/dev/null 2>&1; then
	fail "an over-budget tree was snapshot anyway (the probe must refuse before 'git add -A')"
fi
expect_lacks "$ck71_pane" -F "Snapshotting" "a refused tree announced a snapshot it will not take"

# (d) the control: the same directory under the default budget checkpoints —
# and, cold, its session-start snapshot ANNOUNCES itself above the banner
# (`Snapshotting 1 file (12 B) for checkpoints…`, docs/checkpoint.md
# "Saying so").
ck71_launch "in-budget project (control)" "$WORK71" "$CKAPP71"
if ! git --git-dir="$ck71_dir" rev-parse HEAD >/dev/null 2>&1; then
	fail "an ordinary project did not snapshot (the guards must be scoped, not a blanket disable)"
fi
expect_lacks "$ck71_pane" -F "Checkpoints off" "an ordinary project raised a refusal toast"
expect_has "$ck71_pane" -F "Snapshotting 1 file (" "the cold session-start snapshot never announced itself (docs/checkpoint.md 'Saying so')"

# (e) a checkpoints root pointed INSIDE the project. This is the mechanism
# behind the 2 GB `.alter-zero`: without the anchored self-exclude every
# `git add -A` re-stages the previous snapshots' objects and the tracked set
# compounds turn after turn. It must NOT cost the project its checkpoints —
# the store simply keeps itself out of its own snapshots. Two launches, so a
# second snapshot would have the first one's objects to re-stage.
WORK71B="$(mktemp -d "$SMOKE_TMP/work71b.XXXXXX")"
printf 'print("hi")\n' >"$WORK71B/app.py"
CK71B="$WORK71B/.ck"
CKAPP71B="env HOME=$SMOKE_REAL_HOME $CFG_ENV ALTER_ZERO_CHECKPOINTS=1 ALTER_ZERO_CHECKPOINTS_DIR=$CK71B ALTER_ZERO_SESSIONS_DIR=$CK_SESS ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"
CK71="$CK71B" ck71_launch "store inside the project" "$WORK71B" "$CKAPP71B"
expect_has "$ck71_pane" -F "Snapshotting 1 file (" "the cold first launch never announced its snapshot"
CK71="$CK71B" ck71_launch "store inside the project (2)" "$WORK71B" "$CKAPP71B"
# The relaunch finds a warm store with nothing new: the probe reports zero,
# so there is nothing to announce (the files>0 gate keeps relaunches quiet).
expect_lacks "$ck71_pane" -F "Snapshotting" "a warm store announced a snapshot with nothing new"
ck71b_dir="$CK71B/$(printf '%s' "$WORK71B" | sed 's/[^a-zA-Z0-9]/-/g')"
ck71b_tracked="$(git --git-dir="$ck71b_dir" --work-tree="$WORK71B" ls-files 2>/dev/null)"
echo "==== Phase 71 (store inside the project): tracked=$(printf '%s' "$ck71b_tracked" | tr '\n' ' ') ===="
expect_has "$ck71b_tracked" -x "app.py" "a project holding its own checkpoints root lost its snapshots (the self-exclude must cost it nothing)"
expect_lacks "$ck71b_tracked" "^\.ck/" "the store staged its own objects (the compounding that made .alter-zero 2 GB)"
