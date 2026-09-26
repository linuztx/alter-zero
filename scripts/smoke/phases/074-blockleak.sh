#!/usr/bin/env bash
# Phase 74 — a BLOCKED prompt is erased from the ROLLOUT too

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# a BLOCKED prompt is erased from the ROLLOUT too (docs/hooks.md).
# The recorder used to key its truncation rewrite on history length alone, and
# block_prompt removes the submission AND records the notice — length holds
# still, so the file kept the censored prompt, lost the notice, and a
# --continue fed the secret straight back into the model's context (the
# resume-leak an independent review caught). The recorder now keys on
# App::history_generation. Round trip: block → quit → --continue → the prompt
# text must be gone from the resumed transcript AND the rollout file, while
# the red notice survives both.
S74="${S}_blockleak"
BL_DIR="$(mktemp -d "$SMOKE_TMP/blockleak.XXXXXX")"
BLAPP="env $CFG_ENV_NOHIST ALTER_ZERO_SESSIONS_DIR=$BL_DIR ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux new-session -d -s "$S74" -x 100 -y 30 "$BLAPP; echo CLI_APP_EXITED; sleep 60"
sleep 0.5
# A real turn first: --continue resumes conversations, and a session whose
# every prompt was blocked holds none — the leak only ever mattered on a
# session with something in it.
submit "$S74" "$USER_MSG"
wait_for 20.1 "$S74" -S -40 -- -F "$SUMMARY_TURN1"
submit "$S74" "hook demo: block my prompt"
wait_for 10 "$S74" -S -200 -- -F "blocked the prompt"
# The block restored the draft to the composer: Ctrl+C once clears it, again quits.
tmux send-keys -t "$S74" C-c
sleep 0.3
tmux send-keys -t "$S74" C-c
wait_for 4 "$S74" -F "CLI_APP_EXITED"
bl_rollout="$(find "$BL_DIR" -type f -name 'rollout-*.jsonl' | head -1)"
echo "==== Phase 74: rollout file after the blocked prompt ===="
if [ -n "$bl_rollout" ]; then cat "$bl_rollout"; else echo "(no rollout file)"; fi
if [ -z "$bl_rollout" ]; then
	fail "no rollout was recorded (the notice should have created one)"
else
	if grep -qF "hook demo: block my prompt" "$bl_rollout"; then
		fail "the censored prompt is still in the rollout on disk"
	fi
	if ! grep -qF "blocked the prompt" "$bl_rollout"; then
		fail "the block notice never reached the rollout"
	fi
fi
tmux kill-session -t "$S74" 2>/dev/null || true
# --continue: the resumed transcript must carry the notice, never the prompt.
tmux new-session -d -s "$S74" -x 100 -y 30 "$BLAPP --continue; echo CLI_APP_EXITED; sleep 60"
bl_resumed="$(wait_pane 4 "$S74" -S -120 -- -F "blocked the prompt")"
echo "==== Phase 74: --continue after a blocked prompt (no leak) ===="
printf '%s\n' "$bl_resumed"
expect_lacks "$bl_resumed" -F "hook demo: block my prompt" "the censored prompt came back in the resumed transcript"
expect_has "$bl_resumed" -F "blocked the prompt" "the resumed transcript lost the block notice"
tmux send-keys -t "$S74" C-c
sleep 0.3
tmux kill-session -t "$S74" 2>/dev/null || true
rm -rf "$BL_DIR"
