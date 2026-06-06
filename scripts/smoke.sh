#!/usr/bin/env bash
# Drive the TUI inside a real terminal (tmux): type a message, let the dummy AI
# stream, then ASSERT the rendered conversation contains the expected lines.
#
# This is the only automated coverage of main.rs (the terminal I/O boundary), so
# it asserts rather than just eyeballing: it polls for the streamed reply (no
# blind fixed sleep racing the 45ms-per-chunk stream) and exits non-zero on
# mismatch, so it can gate in CI or a pre-commit hook.
set -uo pipefail

BIN="${1:-target/debug/inline-tui}"
S="inlinetui_smoke_$$"
USER_MSG="hello there"
# The dummy reply is deterministic per prompt (dummy_response: char-count % 3).
# "hello there" is 11 chars → responses[2], which opens with this phrase.
EXPECT_REPLY="Happy to help"

cleanup() { tmux kill-session -t "$S" 2>/dev/null; }
trap cleanup EXIT

if [ ! -x "$BIN" ]; then
	echo "FAIL: binary not found at $BIN (run: cargo build)" >&2
	exit 1
fi

tmux new-session -d -s "$S" -x 80 -y 24 "$BIN"
sleep 0.4
tmux send-keys -t "$S" -l "$USER_MSG"
sleep 0.2
tmux send-keys -t "$S" Enter

# Poll for the streamed reply rather than sleeping a fixed amount.
pane=""
for _ in $(seq 1 50); do # up to ~5s
	pane="$(tmux capture-pane -t "$S" -p -S -60)"
	if printf '%s' "$pane" | grep -qF "$EXPECT_REPLY"; then
		break
	fi
	sleep 0.1
done

echo "==== captured pane (with scrollback) ===="
printf '%s\n' "$pane"

# --- Phase 2: the input box GROWS for a multi-line draft. ---
# Type two lines separated by Alt+Enter; the box must grow to show both, with the
# prompt on the first line and an indented continuation on the second.
tmux send-keys -t "$S" -l "AAA"
tmux send-keys -t "$S" M-Enter
tmux send-keys -t "$S" -l "BBB"
sleep 0.3
grown="$(tmux capture-pane -t "$S" -p)"
echo "==== captured pane (grown input box) ===="
printf '%s\n' "$grown"

tmux send-keys -t "$S" Escape # quit
sleep 0.2

status=0
if ! printf '%s' "$pane" | grep -qF "❯ $USER_MSG"; then
	echo "FAIL: user message line '❯ $USER_MSG' not echoed to scrollback" >&2
	status=1
fi
if ! printf '%s' "$pane" | grep -qF "$EXPECT_REPLY"; then
	echo "FAIL: streamed reply '$EXPECT_REPLY' not found" >&2
	status=1
fi
if ! printf '%s' "$grown" | grep -qF "❯ AAA"; then
	echo "FAIL: first draft line '❯ AAA' not shown in the input box" >&2
	status=1
fi
if ! printf '%s' "$grown" | grep -qF "  BBB"; then
	echo "FAIL: input box did not grow — indented continuation '  BBB' missing" >&2
	status=1
fi
if [ "$status" -eq 0 ]; then
	echo "PASS: message streamed to scrollback and the input box grows for multi-line input"
fi
exit "$status"
