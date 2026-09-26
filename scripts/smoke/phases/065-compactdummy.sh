#!/usr/bin/env bash
# Phase 65 — /compact NEVER reaches the network when the session is running the DUMMY

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# /compact NEVER reaches the network when the session is running
# the DUMMY (docs/compact.md). The trap: `is_usable()` only checks that a key
# resolved, so a configured provider + NO model selection used to build a REAL
# one-off summarization backend for the model name the dummy answers as —
# firing `POST /chat/completions` for "dummy_model_name" and resolving the turn
# red instead of committing the marker. Reproduced offline with a provider whose
# base is the discard port (nothing is ever sent, and if it were the connection
# is refused at once): the footer must show the dummy, and /compact must still
# land the cyan '● Context compacted' cell.
S65="${S}_compactdummy"
CD_DIR="$(mktemp -d "$SMOKE_TMP/compactdummy.XXXXXX")"
cat >"$CD_DIR/providers.toml" <<'PROVIDERS'
[providers.deadend]
name = "Dead End"
api_model_base = "http://127.0.0.1:9/v1"

[providers.deadend.kwargs]
api_base = "http://127.0.0.1:9/v1"
PROVIDERS
# A key resolves for the provider, but ALTER_ZERO_MODEL is unset — so the
# session falls back to the dummy while `active_model` is "dummy_model_name".
APP_CD="env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$CD_DIR/cfg ALTER_ZERO_SESSIONS_DIR=$CD_DIR/sessions ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_PROVIDERS_FILE=$CD_DIR/providers.toml ALTER_ZERO_API_KEY=not-a-real-key ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
launch "$S65" 80 24 "$APP_CD"
compact_dummy_footer="$(tmux capture-pane -t "$S65" -p)"
# One real turn, settled (every dummy reply ends on the hand-off paragraph).
submit "$S65" "hello there"
# Waits for the committed SUMMARY, not for the reply text: the
# hand-off paragraph starts a second before the stream ends, and /compact is
# rejected with a toast while a turn is still in flight.
wait_for 20 "$S65" -S -40 -- -E "^$SUMMARY_TURN1 [0-9]+s" # up to ~20s
tmux send-keys -t "$S65" -l "/compact"
sleep 0.3
tmux send-keys -t "$S65" Enter
compact_dummy=""
for _ in $(seq 1 80); do # up to ~8s
	compact_dummy="$(tmux capture-pane -t "$S65" -p -S -40)"
	if printf '%s' "$compact_dummy" | grep -qF "Context compacted"; then
		break
	fi
	if printf '%s' "$compact_dummy" | grep -qF "request failed"; then
		break # the bug: a real request went out
	fi
	sleep 0.1
done
echo "==== Phase 65: captured pane (/compact while the session runs the dummy) ===="
printf '%s\n' "$compact_dummy"
expect_has "$compact_dummy_footer" -F "dummy_model_name" "the session did not fall back to the dummy (the phase tested nothing)"
expect_lacks "$compact_dummy" -F "request failed" "/compact sent a real request while the session runs the dummy"
expect_has "$compact_dummy" -F "Context compacted" "the '● Context compacted' cell never committed on the dummy"
tmux kill-session -t "$S65" 2>/dev/null
rm -rf "$CD_DIR"
