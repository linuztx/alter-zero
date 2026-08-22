#!/usr/bin/env bash
# Drive the TUI against a REAL provider (OpenRouter) inside tmux and assert
# the live-streaming display held together: the reply streams to scrollback,
# the turn settles with its summary, an Esc interrupt mid-stream keeps the
# session sane, and a mid-stream resize reflows without a panic.
#
# The offline `smoke.sh` proves the terminal boundary against the dummy
# backend; this is its live sibling for the streaming pipeline — real SSE
# chunk boundaries, real markdown (code fences, tables, emoji/CJK), real
# first-token latency. Costs a few fractions of a cent per run.
#
#   OPENROUTER_API_KEY=sk-or-… scripts/live_smoke.sh [target/debug/alter-zero]
#
# `ALTER_ZERO_LIVE_MODEL` overrides the model (default openai/gpt-4o-mini).
set -uo pipefail

if [ -z "${OPENROUTER_API_KEY:-}" ]; then
	echo "live_smoke: set OPENROUTER_API_KEY to run (never committed)" >&2
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
# provider pinned to OpenRouter with the env key.
CFG="$(mktemp -d)"
cleanup() {
	tmux kill-session -t "$S" 2>/dev/null
	tmux kill-session -t "${S}_int" 2>/dev/null
	tmux kill-session -t "${S}_resize" 2>/dev/null
	rm -rf "$CFG" 2>/dev/null
}
trap cleanup EXIT

APP="env ALTER_ZERO_CONFIG_DIR=$CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_PROJECT_CONFIG=0 \
ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_PROVIDER=openrouter ALTER_ZERO_MODEL=$MODEL \
OPENROUTER_API_KEY=$OPENROUTER_API_KEY $BIN"

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

echo "live_smoke: model $MODEL"

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

echo "live_smoke: all phases passed"
