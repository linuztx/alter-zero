#!/usr/bin/env bash
# Phase 109 — PER-DIRECTORY state

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# PER-DIRECTORY state (docs/per-directory-state.md). /model and
# /settings remember the directory they were used in — one config home, three
# working directories. (a) A pre-seeded LAST model selection (the shape a
# pre-directory config.json has, so this doubles as its compat check) is what
# a directory launched in for the first time runs, PINNED as its own entry
# right then. (b) A knob cycled in that directory is the directory's alone —
# settings.json keys the entry by the directory's path under `projects`, its
# top level (the seed) untouched — and a second directory starts at the
# defaults, hooks and checkpoints OFF among them, while adopting the same
# last model. (c) A later change to the last selection moves a third, new
# directory and never the pinned first one, whose knob also survived.
S109="${S}_perdir"
S109B="${S}_perdir2"
S109C="${S}_perdir3"
PD_CFG="$(mktemp -d)"
PD_A="$(mktemp -d "$SMOKE_TMP/perdir-a.XXXXXX")"
PD_B="$(mktemp -d "$SMOKE_TMP/perdir-b.XXXXXX")"
PD_C="$(mktemp -d "$SMOKE_TMP/perdir-c.XXXXXX")"
# The paths as the app keys them — the kernel's cwd, symlinks resolved.
PD_A_KEY="$(cd "$PD_A" && pwd -P)"
PD_B_KEY="$(cd "$PD_B" && pwd -P)"
cat >"$PD_CFG/providers.toml" <<'PROVIDERS'
[providers.deadend]
name = "Dead End"
api_model_base = "http://127.0.0.1:9/v1"

[providers.deadend.kwargs]
api_base = "http://127.0.0.1:9/v1"
PROVIDERS
# A key resolves for the provider (Phase 65's trick), so a saved selection
# activates a real backend whose model name the footer shows. No request is
# ever made: no turn runs, and the capability probe fails fast on a dead port.
cat >"$PD_CFG/config.json" <<'CONFIG'
{ "provider": "deadend", "model": "pinned-model-a" }
CONFIG
# No ALTER_ZERO_CHECKPOINTS here, deliberately: the default is under test, and
# the cwds are throwaway temp dirs a snapshot could not hurt.
APP_PD="env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$PD_CFG ALTER_ZERO_SESSIONS_DIR=$PD_CFG/sessions ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_SKILLS_DIR=$SMOKE_SKILLS ALTER_ZERO_AGENTS_DIR=$SMOKE_AGENTS ALTER_ZERO_PROVIDERS_FILE=$PD_CFG/providers.toml ALTER_ZERO_API_KEY=not-a-real-key ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"
# Poll a pane (visible screen) for a needle, up to ~5s; prints the last capture.
pd_wait() {
	local pane=""
	pane="$(wait_pane 5 "$1" -F "$2")"
	printf '%s' "$pane"
}
# The value column of one /settings row, reached by type-to-search from the
# open menu; the query is cleared again afterwards.
pd_setting() {
	local sess="$1" query="$2" pane
	tmux send-keys -t "$sess" -l "$query"
	sleep 0.4
	pane="$(tmux capture-pane -t "$sess" -p)"
	for _ in $(seq 1 ${#query}); do tmux send-keys -t "$sess" BSpace; done
	sleep 0.3
	printf '%s' "$pane"
}

# (a) The first directory: the last selection, pinned.
tmux new-session -d -s "$S109" -x 100 -y 30 -c "$PD_A" "$APP_PD"
pd_a="$(pd_wait "$S109" "pinned-model-a")"
echo "==== Phase 109: a directory launched in for the first time ===="
printf '%s\n' "$pd_a"
expect_has "$pd_a" -F "pinned-model-a" "the first launch in a directory did not take the last model selection"
if ! grep -qF "\"$PD_A_KEY\"" "$PD_CFG/config.json"; then
	echo "==== Phase 109: config.json ===="
	cat "$PD_CFG/config.json"
	fail "the first launch did not pin the selection as the directory's own entry"
fi
# (b) A knob cycled here is this directory's alone.
tmux send-keys -t "$S109" -l "/settings"
sleep 0.3
tmux send-keys -t "$S109" Enter
sleep 0.6
tmux send-keys -t "$S109" -l "retry"
sleep 0.4
tmux send-keys -t "$S109" Enter
sleep 0.5
pd_cycled="$(tmux capture-pane -t "$S109" -p)"
echo "==== Phase 109: Error retry cycled in the first directory ===="
printf '%s\n' "$pd_cycled"
expect_has "$pd_cycled" -E "Error retry +5" "Enter did not cycle Error retry from 3 to 5"
tmux send-keys -t "$S109" Escape
sleep 0.3
tmux send-keys -t "$S109" Escape
sleep 0.4
tmux send-keys -t "$S109" -l "/quit"
tmux send-keys -t "$S109" Enter
sleep 0.6
tmux kill-session -t "$S109" 2>/dev/null
if [ ! -f "$PD_CFG/settings.json" ]; then
	fail "no settings.json was written to the config home"
else
	echo "==== Phase 109: settings.json ===="
	cat "$PD_CFG/settings.json"
	if ! grep -qF "\"$PD_A_KEY\"" "$PD_CFG/settings.json"; then
		fail "settings.json is not keyed by the directory the knob was cycled in"
	fi
	if ! grep -q '"error_retry": *5' "$PD_CFG/settings.json"; then
		fail "settings.json does not record the changed value"
	fi
	# The seed — every key ABOVE the `projects` map — must stay untouched.
	if ! awk '/"projects"/ { inside = 1 } /"error_retry"/ { if (!inside) top = 1 } END { exit top }' "$PD_CFG/settings.json"; then
		fail "the cycled value was written to the file's top level instead of the directory's entry"
	fi
fi

# (c) A second directory: the same last model, adopted — and the defaults.
tmux new-session -d -s "$S109B" -x 100 -y 30 -c "$PD_B" "$APP_PD"
pd_b="$(pd_wait "$S109B" "pinned-model-a")"
echo "==== Phase 109: a second directory, launched in for the first time ===="
printf '%s\n' "$pd_b"
expect_has "$pd_b" -F "pinned-model-a" "the second directory did not take the last model selection"
tmux send-keys -t "$S109B" -l "/settings"
sleep 0.3
tmux send-keys -t "$S109B" Enter
sleep 0.6
pd_b_retry="$(pd_setting "$S109B" "retry")"
pd_b_ck="$(pd_setting "$S109B" "checkpoints")"
pd_b_hooks="$(pd_setting "$S109B" "hooks")"
echo "==== Phase 109: the second directory's Error retry / Checkpoints / Hooks rows ===="
printf '%s\n' "$pd_b_retry" | grep -E "Error retry" || true
printf '%s\n' "$pd_b_ck" | grep -E "Checkpoints" || true
printf '%s\n' "$pd_b_hooks" | grep -E "Hooks" || true
expect_has "$pd_b_retry" -E "Error retry +3" "the first directory's Error retry leaked into the second (expected the default 3)"
expect_has "$pd_b_ck" -E "Checkpoints +false" "Checkpoints should default to false"
expect_has "$pd_b_hooks" -E "Hooks +false" "Hooks should default to false"
tmux send-keys -t "$S109B" Escape
sleep 0.4
tmux send-keys -t "$S109B" -l "/quit"
tmux send-keys -t "$S109B" Enter
sleep 0.6
tmux kill-session -t "$S109B" 2>/dev/null
if ! grep -qF "\"$PD_B_KEY\"" "$PD_CFG/config.json"; then
	echo "==== Phase 109: config.json ===="
	cat "$PD_CFG/config.json"
	fail "the second directory's launch did not pin its own entry"
fi

# (d) The last selection moves on (as a /model switch elsewhere would move it):
# a third, new directory takes the new one; the pinned first keeps its own,
# and its cycled knob.
cat >"$PD_CFG/config.json" <<CONFIG
{
  "provider": "deadend",
  "model": "pinned-model-b",
  "projects": {
    "$PD_A_KEY": { "provider": "deadend", "model": "pinned-model-a" },
    "$PD_B_KEY": { "provider": "deadend", "model": "pinned-model-a" }
  }
}
CONFIG
tmux new-session -d -s "$S109" -x 100 -y 30 -c "$PD_A" "$APP_PD"
pd_a2="$(pd_wait "$S109" "pinned-model-a")"
tmux send-keys -t "$S109" -l "/settings"
sleep 0.3
tmux send-keys -t "$S109" Enter
sleep 0.6
pd_a2_retry="$(pd_setting "$S109" "retry")"
echo "==== Phase 109: the first directory relaunched after the last selection moved ===="
printf '%s\n' "$pd_a2"
printf '%s\n' "$pd_a2_retry" | grep -E "Error retry" || true
expect_has "$pd_a2" -F "pinned-model-a" "a pinned directory followed the last selection instead of keeping its own model"
expect_has "$pd_a2_retry" -E "Error retry +5" "the directory's own Error retry did not survive the restart"
tmux send-keys -t "$S109" Escape
sleep 0.4
tmux send-keys -t "$S109" -l "/quit"
tmux send-keys -t "$S109" Enter
sleep 0.6
tmux kill-session -t "$S109" 2>/dev/null
tmux new-session -d -s "$S109C" -x 100 -y 30 -c "$PD_C" "$APP_PD"
pd_c="$(pd_wait "$S109C" "pinned-model-b")"
echo "==== Phase 109: a third directory takes the moved last selection ===="
printf '%s\n' "$pd_c"
expect_has "$pd_c" -F "pinned-model-b" "a new directory did not take the newest last selection"
tmux send-keys -t "$S109C" -l "/quit"
tmux send-keys -t "$S109C" Enter
sleep 0.6
tmux kill-session -t "$S109C" 2>/dev/null
rm -rf "$PD_CFG" "$PD_A" "$PD_B" "$PD_C"
