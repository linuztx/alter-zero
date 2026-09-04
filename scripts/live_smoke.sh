#!/usr/bin/env bash
# Drive the TUI against a REAL provider inside tmux and assert the
# live-streaming display held together: the reply streams to scrollback,
# the turn settles with its summary, an Esc interrupt mid-stream keeps the
# session sane, a mid-stream resize reflows without a panic, and the
# `[PROMPT]` CLI shortcut starts (and `--resume {id}` continues) a real turn
# from the command line.
#
# The offline `smoke.sh` proves the terminal boundary against the dummy
# backend; this is its live sibling for the streaming pipeline — real SSE
# chunk boundaries, real markdown (code fences, tables, emoji/CJK), real
# first-token latency. Costs a few fractions of a cent per run.
#
#   OPENROUTER_API_KEY=sk-or-… scripts/live_smoke.sh [target/debug/alter-zero]
#
# `ALTER_ZERO_LIVE_PROVIDER` picks any provider from providers.toml (default
# openrouter); its key is read from that provider's default variable —
# `<ID_UPPERCASE>_API_KEY`, so `a0_venice` reads `A0_VENICE_API_KEY` — and
# `ALTER_ZERO_LIVE_MODEL` names the model (default openai/gpt-4o-mini, which
# only OpenRouter serves: another provider must name one).
#
#   ALTER_ZERO_LIVE_PROVIDER=a0_venice A0_VENICE_API_KEY=sk-a0-… \
#   ALTER_ZERO_LIVE_MODEL=llama-3.3-70b scripts/live_smoke.sh
set -uo pipefail

PROVIDER="${ALTER_ZERO_LIVE_PROVIDER:-openrouter}"
KEY_VAR="$(printf '%s' "$PROVIDER" | tr '[:lower:]-' '[:upper:]_')_API_KEY"
KEY="${!KEY_VAR:-}"
if [ -z "$KEY" ]; then
	echo "live_smoke: set $KEY_VAR to run (never committed)" >&2
	exit 2
fi
if [ "$PROVIDER" != "openrouter" ] && [ -z "${ALTER_ZERO_LIVE_MODEL:-}" ]; then
	echo "live_smoke: set ALTER_ZERO_LIVE_MODEL to a model $PROVIDER serves" >&2
	exit 2
fi

BIN="${1:-target/debug/alter-zero}"
if [ ! -x "$BIN" ]; then
	echo "live_smoke: $BIN not found — cargo build first" >&2
	exit 2
fi
MODEL="${ALTER_ZERO_LIVE_MODEL:-openai/gpt-4o-mini}"
S="alterzero_live_$$"

# Isolated config home so the run can't touch (or be steered by) the real
# ~/.alter-zero; checkpoints/history/project-config off for hermeticity, the
# sessions dir inside it (Phase L4 resumes one), the provider pinned with
# the env key. Tools are OFF: every phase here is about the streamed *text*,
# and a model that answers "print ZEBRAFISH" with a `bash` call parks the
# turn on a permission prompt instead (observed with gpt-4o-mini via Venice).
CFG="$(mktemp -d)"
cleanup() {
	tmux kill-session -t "$S" 2>/dev/null
	tmux kill-session -t "${S}_int" 2>/dev/null
	tmux kill-session -t "${S}_resize" 2>/dev/null
	tmux kill-session -t "${S}_prompt" 2>/dev/null
	rm -rf "$CFG" 2>/dev/null
}
trap cleanup EXIT

APP="env ALTER_ZERO_CONFIG_DIR=$CFG ALTER_ZERO_SESSIONS_DIR=$CFG/sessions ALTER_ZERO_CHECKPOINTS=0 \
ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_TOOLS=0 \
ALTER_ZERO_PROVIDER=$PROVIDER ALTER_ZERO_MODEL=$MODEL $KEY_VAR=$KEY $BIN"

fail() {
	echo "FAIL: $1" >&2
	shift
	for extra in "$@"; do
		printf '%s\n' "--- $extra" >&2
	done
	exit 1
}

# Poll the pane (with scrollback) until it contains $2, up to $3 seconds.
wait_for() {
	local sess="$1" needle="$2" secs="${3:-90}" pane=""
	for _ in $(seq 1 $((secs * 2))); do
		pane="$(tmux capture-pane -t "$sess" -p -S -400 2>/dev/null)"
		if printf '%s' "$pane" | grep -qF "$needle"; then
			printf '%s' "$pane"
			return 0
		fi
		sleep 0.5
	done
	printf '%s' "$pane"
	return 1
}

no_panic() {
	printf '%s' "$1" | grep -q "panicked at" && fail "$2: the binary panicked" "$1"
	return 0
}

echo "live_smoke: provider $PROVIDER, model $MODEL"

# --- Phase L1: a torture reply streams, renders, and settles -----------------
tmux new-session -d -s "$S" -x 100 -y 30 "$APP"
sleep 1.5
PROMPT_L1="Reply with exactly: a level-2 heading; one paragraph containing a **bold** phrase and the bare URL https://example.com/live ; a fenced python code block of 6 lines; a GFM pipe table (leading pipes) with 2 columns and 2 data rows, one cell holding 🎮 and one holding 世界; and a closing line. No other text."
tmux send-keys -t "$S" -l "$PROMPT_L1"
sleep 0.3
tmux send-keys -t "$S" Enter

pane="$(wait_for "$S" "Done for" 120)" || fail "L1: the turn never settled" "$pane"
no_panic "$pane" "L1"
printf '%s' "$pane" | grep -qF "❯ Reply with exactly" || fail "L1: the user echo committed" "$pane"
# The streamed markdown really rendered: the table became a grid and the
# fences are hidden (raw \`\`\` would mean the renderer fell apart).
printf '%s' "$pane" | grep -q "│" || fail "L1: the streamed table rendered as a grid" "$pane"
printf '%s' "$pane" | grep -qF '```' && fail "L1: a raw fence leaked into the render" "$pane"
printf '%s' "$pane" | grep -qF "世界" || fail "L1: the CJK cell survived" "$pane"
printf '%s' "$pane" | grep -qF "https://example.com/live" || fail "L1: the URL rendered" "$pane"
# The session footer shows the real model — this was the live backend.
visible="$(tmux capture-pane -t "$S" -p)"
printf '%s' "$visible" | grep -qF "$MODEL" || fail "L1: the footer names the live model" "$visible"
echo "PASS: L1 live torture reply streamed and settled"
tmux kill-session -t "$S" 2>/dev/null

# --- Phase L2: Esc interrupts a live stream cleanly --------------------------
S_INT="${S}_int"
tmux new-session -d -s "$S_INT" -x 100 -y 30 "$APP"
sleep 1.5
tmux send-keys -t "$S_INT" -l "Concatenate ZEBRA and FISH into one word and print it, then count from 1 to 300, one number per line. No other text."
sleep 0.3
tmux send-keys -t "$S_INT" Enter
# Wait until REPLY content is really streaming: the concatenation exists only
# in the model's output — the echoed prompt spells it in parts and the status
# line's token tally can't fake it (an Esc before the first chunk takes the
# interrupt-UNDO path by design: no notice, message back in the composer).
pane="$(wait_for "$S_INT" "ZEBRAFISH" 60)" || fail "L2: the stream never started" "$pane"
tmux send-keys -t "$S_INT" Escape
pane="$(wait_for "$S_INT" "Conversation interrupted" 20)" || fail "L2: no interrupt notice" "$pane"
no_panic "$pane" "L2"
# The app is still alive and idle: the composer prompt is on screen.
visible="$(tmux capture-pane -t "$S_INT" -p)"
printf '%s' "$visible" | grep -qF "$MODEL" || fail "L2: the session survived the interrupt" "$visible"
echo "PASS: L2 Esc interrupted the live stream cleanly"
tmux kill-session -t "$S_INT" 2>/dev/null

# --- Phase L3: a mid-stream resize reflows without a panic -------------------
S_RS="${S}_resize"
tmux new-session -d -s "$S_RS" -x 100 -y 30 "$APP"
sleep 1.5
tmux send-keys -t "$S_RS" -l "Write a 150-word paragraph about rivers, then a fenced rust code block of 10 lines. No other text."
sleep 0.3
tmux send-keys -t "$S_RS" Enter
pane="$(wait_for "$S_RS" "river" 60)" || fail "L3: the stream never started" "$pane"
# Two live resizes mid-stream: narrower, then wider (the purge-rebuild path).
tmux resize-window -t "$S_RS" -x 60 -y 24 2>/dev/null || tmux set-option -t "$S_RS" -g default-size 60x24
sleep 1
tmux resize-window -t "$S_RS" -x 100 -y 30 2>/dev/null
pane="$(wait_for "$S_RS" "Done for" 120)" || fail "L3: the turn never settled after resizes" "$pane"
no_panic "$pane" "L3"
echo "PASS: L3 mid-stream resizes reflowed and the turn settled"
tmux kill-session -t "$S_RS" 2>/dev/null

# --- Phase L4: the [PROMPT] shortcut starts a real turn from the command
# line, and --resume {id} "…" continues it (docs/cli.md) -------------------
S_P="${S}_prompt"
# Its own sessions dir (L1–L3 recorded theirs in the shared one), so the
# one-rollout-file check below counts only this phase's conversation; a
# later `env` assignment overrides the earlier one in APP.
L4_SESSIONS="$CFG/sessions-l4"
APP_L4="$APP"
APP_L4="${APP_L4/ $BIN/ ALTER_ZERO_SESSIONS_DIR=$L4_SESSIONS $BIN}"
tmux new-session -d -s "$S_P" -x 100 -y 30 "$APP_L4 \"Reply with exactly the single word PROMPTOK and nothing else.\"; echo LIVE_APP_EXITED; sleep 60"
pane="$(wait_for "$S_P" "Done for" 120)" || fail "L4: the command-line prompt's turn never settled" "$pane"
no_panic "$pane" "L4"
printf '%s' "$pane" | grep -qF "❯ Reply with exactly the single word PROMPTOK" || fail "L4: the prompt was not committed as the user bubble" "$pane"
printf '%s' "$pane" | grep -q "PROMPTOK" || fail "L4: the reply to the command-line prompt never streamed" "$pane"
tmux send-keys -t "$S_P" C-c
pane="$(wait_for "$S_P" "LIVE_APP_EXITED" 20)" || fail "L4: the app did not exit on Ctrl+C" "$pane"
printf '%s' "$pane" | grep -qF "Resume this session with:" || fail "L4: no resume hint after the shortcut's session" "$pane"
hint_id="$(printf '%s\n' "$pane" | sed -n 's/.*--resume \([a-f0-9-]*\).*/\1/p' | tail -1)"
[ -n "$hint_id" ] || fail "L4: no --resume {id} line under the hint" "$pane"
tmux kill-session -t "$S_P" 2>/dev/null
tmux new-session -d -s "$S_P" -x 100 -y 30 "$APP_L4 --resume $hint_id \"Now reply with exactly the single word RESUMEOK and nothing else.\"; echo LIVE_APP_EXITED; sleep 60"
pane="$(wait_for "$S_P" "RESUMEOK" 120)" || fail "L4: the resumed session's prompt turn never streamed" "$pane"
no_panic "$pane" "L4"
printf '%s' "$pane" | grep -qF "❯ Reply with exactly the single word PROMPTOK" || fail "L4: --resume {id} did not reload the first turn above the new one" "$pane"
# The reloaded transcript already carries the first turn's summary, so the
# settle of the NEW turn is the second "Done for" — count, don't match.
for _ in $(seq 1 240); do
	pane="$(tmux capture-pane -t "$S_P" -p -S -400 2>/dev/null)"
	if [ "$(printf '%s' "$pane" | grep -cF "Done for")" -ge 2 ]; then
		break
	fi
	sleep 0.5
done
[ "$(printf '%s' "$pane" | grep -cF "Done for")" -ge 2 ] || fail "L4: the resumed prompt turn never settled (the reloaded summary and the new one should both show)" "$pane"
[ "$(find "$L4_SESSIONS" -type f -name 'rollout-*.jsonl' | wc -l | tr -d ' ')" = "1" ] || fail "L4: the prompt turns should append to ONE rollout file"
echo "PASS: L4 the command-line prompt ran live and --resume {id} continued it"
tmux kill-session -t "$S_P" 2>/dev/null

echo "live_smoke: all phases passed"
