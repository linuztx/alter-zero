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

cleanup() {
	tmux kill-session -t "$S" 2>/dev/null
	tmux kill-session -t "${S}_bottom" 2>/dev/null
	tmux kill-session -t "${S}_burst" 2>/dev/null
}
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

# --- Phase 3: Ctrl+O opens the full-screen tool-output view, Ctrl+O returns. ---
# The dummy interleaves tool calls in its reply; wait until the second (Bash) has
# run, then open the view: it must show each tool's FULL output (the Read tool's
# second line is hidden in the collapsed inline view but shown here).
for _ in $(seq 1 60); do
	full="$(tmux capture-pane -t "$S" -p -S -80)"
	if printf '%s' "$full" | grep -qF "Bash(grep"; then
		break
	fi
	sleep 0.15
done
tmux send-keys -t "$S" C-o
sleep 0.4
overlay="$(tmux capture-pane -t "$S" -p)"
echo "==== captured pane (Ctrl+O tool-output view) ===="
printf '%s\n' "$overlay"
tmux send-keys -t "$S" C-o # back to the conversation
sleep 0.4
returned="$(tmux capture-pane -t "$S" -p)"
echo "==== captured pane (returned to conversation) ===="
printf '%s\n' "$returned"

# --- Phase 4: the slash-command palette. Typing "/" opens the command list below
# the box (/help and /clear); running /help posts a system notice listing them. ---
# Clear the leftover "AAA\nBBB" draft from Phase 2 first — the palette only opens
# when the input *starts* with "/".
for _ in $(seq 1 12); do
	tmux send-keys -t "$S" BSpace
done
sleep 0.2
tmux send-keys -t "$S" -l "/"
sleep 0.3
palette_open="$(tmux capture-pane -t "$S" -p)"
echo "==== captured pane (slash palette open) ===="
printf '%s\n' "$palette_open"

# Run /help (highlighted first) → posts a system notice listing the commands.
tmux send-keys -t "$S" Enter
sleep 0.3
help_ran="$(tmux capture-pane -t "$S" -p -S -20)"
echo "==== captured pane (after running /help) ===="
printf '%s\n' "$help_ran"

tmux send-keys -t "$S" Escape # quit
sleep 0.2

# --- Phase 5: after a reply finishes, the input box stays flush at the BOTTOM —
# no blank rows creep in below it when the streaming strip (preview + gap, drawn
# *above* the box) clears. A short 40x12 terminal makes one exchange overflow the
# screen so the box is pushed to the bottom while streaming; the bug let the box
# rise by the strip's height once the reply finished, leaving blank rows beneath. ---
S2="${S}_bottom"
TMP5="$(mktemp)"
tmux new-session -d -s "$S2" -x 40 -y 12 "$BIN"
sleep 0.4
tmux send-keys -t "$S2" -l "hello there"
sleep 0.2
tmux send-keys -t "$S2" Enter
# Wait until the reply has fully finished (its last text "changes size" is
# committed) AND the screen has stopped changing — so we measure the *settled*
# layout, not a mid-stream frame (where the box legitimately sits at the bottom).
settled_prev=""
for _ in $(seq 1 60); do # up to ~12s
	tmux capture-pane -t "$S2" -p >"$TMP5"
	settled_cur="$(cat "$TMP5")"
	if printf '%s' "$settled_cur" | grep -qF "changes size" &&
		[ "$settled_cur" = "$settled_prev" ]; then
		break
	fi
	settled_prev="$settled_cur"
	sleep 0.2
done
echo "==== captured pane (box settled at the bottom after the reply) ===="
cat "$TMP5"
# Count blank rows below the box: walk up from the last pane row while it is blank
# (strip only ASCII space/tab so the multibyte box rule still counts as content).
trailing_blanks=$(awk '{a[NR]=$0} END{c=0; for(i=NR;i>=1;i--){t=a[i]; gsub(/[ \t]/,"",t); if(t==""){c++}else break} print c}' "$TMP5")
tmux kill-session -t "$S2" 2>/dev/null
rm -f "$TMP5"

# --- Phase 6: typing stays responsive. The event loop drains every buffered key
# before redrawing, so a burst of input renders in ONE repaint, not one per key.
# A 1000-char burst (ending in a unique marker) must therefore show up almost at
# once: before the coalescing fix it took ~0.9s (O(n²) — a full redraw per key),
# after it is a single redraw (~10ms). Assert the marker lands well inside that
# gap. (Kept under tmux's 1024-char single-burst cap so it's delivered at once.) ---
S3="${S}_burst"
tmux new-session -d -s "$S3" -x 80 -y 24 "$BIN"
sleep 0.4
BURST="$(printf 'x%.0s' $(seq 1 997))END"
burst_ms=0
burst_ok=0
burst_start=$(date +%s%3N)
tmux send-keys -t "$S3" -l "$BURST"
while [ "$burst_ms" -lt 600 ]; do
	if tmux capture-pane -t "$S3" -p | grep -qF "END"; then
		burst_ok=1
		break
	fi
	burst_ms=$(($(date +%s%3N) - burst_start))
done
burst_ms=$(($(date +%s%3N) - burst_start))
echo "==== Phase 6: 1000-char burst fully rendered in ${burst_ms}ms (ok=$burst_ok) ===="
tmux kill-session -t "$S3" 2>/dev/null

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
# "PgUp/PgDn" is unique to the overlay's title bar (it never appears in the
# conversation), so it's a clean marker for "the view is open / closed".
if ! printf '%s' "$overlay" | grep -qF "PgUp/PgDn"; then
	echo "FAIL: Ctrl+O did not open the tool-output view" >&2
	status=1
fi
if ! printf '%s' "$overlay" | grep -qF "$USER_MSG"; then
	echo "FAIL: tool-output view did not include the user/AI conversation" >&2
	status=1
fi
if ! printf '%s' "$overlay" | grep -qF "InlineViewport::init"; then
	echo "FAIL: tool-output view did not show the full (expanded) Read output" >&2
	status=1
fi
if printf '%s' "$returned" | grep -qF "PgUp/PgDn"; then
	echo "FAIL: Ctrl+O did not return to the conversation" >&2
	status=1
fi
# Typing "/" lists both commands below the box (their descriptions are unique to
# the open palette).
if ! printf '%s' "$palette_open" | grep -qF "List the available commands"; then
	echo "FAIL: typing '/' did not open the command palette (/help missing)" >&2
	status=1
fi
if ! printf '%s' "$palette_open" | grep -qF "Clear the conversation"; then
	echo "FAIL: the command palette did not list /clear" >&2
	status=1
fi
if ! printf '%s' "$help_ran" | grep -qF "Available commands:"; then
	echo "FAIL: running /help did not post its system notice" >&2
	status=1
fi
if ! printf '%s' "$settled_cur" | grep -qF "changes size"; then
	echo "FAIL: the reply never finished on the short terminal (Phase 5 could not settle)" >&2
	status=1
elif [ "${trailing_blanks:-99}" -ne 0 ]; then
	echo "FAIL: $trailing_blanks blank row(s) left below the input box after the reply settled — the box should stay flush at the bottom" >&2
	status=1
fi
if [ "$burst_ok" -ne 1 ]; then
	echo "FAIL: a 1000-char input burst was not fully rendered within 600ms (took ${burst_ms}ms) — input is not coalesced into one repaint, typing lag regressed" >&2
	status=1
fi
if [ "$status" -eq 0 ]; then
	echo "PASS: reply + tools streamed to scrollback, the input box grows and stays flush at the bottom after a reply, typing bursts render in one repaint, Ctrl+O opens the tool-output view, and the slash-command palette opens and runs commands"
fi
exit "$status"
