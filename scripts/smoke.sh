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
	tmux kill-session -t "${S}_overlayquit" 2>/dev/null
	tmux kill-session -t "${S}_interrupt" 2>/dev/null
	tmux kill-session -t "${S}_quit" 2>/dev/null
	tmux kill-session -t "${S}_recall" 2>/dev/null
	tmux kill-session -t "${S}_shortcuts" 2>/dev/null
	tmux kill-session -t "${S}_queue" 2>/dev/null
	tmux kill-session -t "${S}_queueint" 2>/dev/null
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

# --- Phase 7: finishing a turn *while the Ctrl+O overlay is open*, then quitting
# with Ctrl+C, must leave the RESTORED screen showing the committed "Done for Ns"
# summary — not the stale live "( ● ) {verb}… (… tokens)" status strip. The
# overlay defers scrollback commits (invariant 4); a normal Ctrl+O return repaints
# the inline view from history, but the quit path used to skip that and exit_overlay
# straight onto the stale strip. Run the binary *inside a shell* so the pane
# survives the app exiting and we can capture the restored terminal afterward. ---
S4="${S}_overlayquit"
BIN_ABS="$(realpath "$BIN" 2>/dev/null || echo "$BIN")"
tmux new-session -d -s "$S4" -x 80 -y 24
sleep 0.3
tmux send-keys -t "$S4" -l "$BIN_ABS"
tmux send-keys -t "$S4" Enter
sleep 0.6
tmux send-keys -t "$S4" -l "hello there"
sleep 0.2
tmux send-keys -t "$S4" Enter
# Open the overlay *mid-stream*: wait until the reply is visibly streaming (the
# turn is active) before pressing Ctrl+O.
for _ in $(seq 1 40); do # up to ~4s
	if tmux capture-pane -t "$S4" -p | grep -qF "Happy"; then
		break
	fi
	sleep 0.1
done
tmux send-keys -t "$S4" C-o
# Wait until the turn FINISHES while the overlay is up — the transcript gains the
# "Done for Ns" summary. This is the precondition for the bug (the turn ended with
# scrollback commits deferred).
overlay_done=""
for _ in $(seq 1 80); do # up to ~12s
	overlay_done="$(tmux capture-pane -t "$S4" -p)"
	if printf '%s' "$overlay_done" | grep -qF "Done for"; then
		break
	fi
	sleep 0.15
done
echo "==== captured pane (turn finished inside the Ctrl+O overlay) ===="
printf '%s\n' "$overlay_done"
# Now quit with Ctrl+C from inside the overlay and capture the restored terminal.
tmux send-keys -t "$S4" C-c
sleep 0.5
post_quit="$(tmux capture-pane -t "$S4" -p -S -40)"
echo "==== captured pane (restored terminal after quitting from the overlay) ===="
printf '%s\n' "$post_quit"
tmux kill-session -t "$S4" 2>/dev/null

# --- Phase 8: Esc INTERRUPTS a streaming turn (codex-style) instead of quitting.
# Mid-stream Esc must stop the generation promptly: the partial reply stays on
# screen, the red "Conversation interrupted" notice commits, the live status
# strip clears (no "tokens" line), and NO "Done for Ns" summary appears. The app
# keeps running — a follow-up message must stream and finish normally
# ("Finished for", turn 2's done verb). Esc when *idle* still quits — Phase 4's
# Escape (sent long after the turn ended) relies on exactly that. ---
S5="${S}_interrupt"
tmux new-session -d -s "$S5" -x 80 -y 24 "$BIN"
sleep 0.4
tmux send-keys -t "$S5" -l "hello there"
sleep 0.2
tmux send-keys -t "$S5" Enter
for _ in $(seq 1 40); do # up to ~4s: wait until the reply is visibly streaming
	if tmux capture-pane -t "$S5" -p | grep -qF "Happy"; then
		break
	fi
	sleep 0.1
done
tmux send-keys -t "$S5" Escape
sleep 0.6
interrupted="$(tmux capture-pane -t "$S5" -p)"
echo "==== captured pane (turn interrupted with Esc) ===="
printf '%s\n' "$interrupted"
# The loop must survive the interrupt: a follow-up turn streams and finishes.
tmux send-keys -t "$S5" -l "again please"
sleep 0.2
tmux send-keys -t "$S5" Enter
after_interrupt=""
for _ in $(seq 1 60); do # up to ~9s: wait for the follow-up turn's summary
	after_interrupt="$(tmux capture-pane -t "$S5" -p -S -30)"
	if printf '%s' "$after_interrupt" | grep -qF "Finished for"; then
		break
	fi
	sleep 0.15
done
echo "==== captured pane (follow-up turn after the interrupt) ===="
printf '%s\n' "$after_interrupt"
tmux kill-session -t "$S5" 2>/dev/null

# --- Phase 9: Ctrl+C clears a typed draft (codex's composer-clear step) and
# the /quit command exits the app. A first Ctrl+C with text in the box must
# only empty it — the app keeps running — and typing "/quit" + Enter (the
# palette runs the highlighted command) must terminate the process, which ends
# the tmux session. ---
S6="${S}_quit"
tmux new-session -d -s "$S6" -x 80 -y 24 "$BIN"
sleep 0.4
tmux send-keys -t "$S6" -l "a draft the user wants gone"
sleep 0.3
tmux send-keys -t "$S6" C-c
sleep 0.4
after_clear="$(tmux capture-pane -t "$S6" -p)"
echo "==== captured pane (draft cleared by Ctrl+C) ===="
printf '%s\n' "$after_clear"
quit_alive=0
tmux has-session -t "$S6" 2>/dev/null && quit_alive=1
tmux send-keys -t "$S6" -l "/quit"
sleep 0.3
tmux send-keys -t "$S6" Enter
quit_exited=0
for _ in $(seq 1 20); do # up to ~2s for the process to exit
	if ! tmux has-session -t "$S6" 2>/dev/null; then
		quit_exited=1
		break
	fi
	sleep 0.1
done
echo "==== Phase 9: alive after Ctrl+C clear=$quit_alive, exited after /quit=$quit_exited ===="
tmux kill-session -t "$S6" 2>/dev/null

# --- Phase 10: ↑ recalls the last sent message into the input box, ↓ past the
# newest clears it, and ↑ + Enter RESUBMITS it (docs/input-history.md). The
# committed user line and the input prompt share the "❯ " glyph, so the
# assertions count occurrences: recall adds one (box + scrollback), the ↓ clear
# removes it, and the resubmit commits a second scrollback copy plus turn 2's
# "Finished for" summary. ---
S7="${S}_recall"
RECALL_MSG="history one"
tmux new-session -d -s "$S7" -x 80 -y 24 "$BIN"
sleep 0.4
tmux send-keys -t "$S7" -l "$RECALL_MSG"
sleep 0.2
tmux send-keys -t "$S7" Enter
for _ in $(seq 1 80); do # up to ~12s: wait for turn 1 to finish ("Done for")
	if tmux capture-pane -t "$S7" -p | grep -qF "Done for"; then
		break
	fi
	sleep 0.15
done
tmux send-keys -t "$S7" Up
sleep 0.4
recalled="$(tmux capture-pane -t "$S7" -p -S -40)"
echo "==== captured pane (last message recalled with Up) ===="
printf '%s\n' "$recalled"
recall_up_count=$(printf '%s\n' "$recalled" | grep -cF "❯ $RECALL_MSG")
tmux send-keys -t "$S7" Down
sleep 0.4
recall_down_count=$(tmux capture-pane -t "$S7" -p -S -40 | grep -cF "❯ $RECALL_MSG")
tmux send-keys -t "$S7" Up # recall again …
sleep 0.3
tmux send-keys -t "$S7" Enter # … and resubmit it
resubmitted=""
for _ in $(seq 1 80); do # up to ~12s: wait for turn 2's summary
	resubmitted="$(tmux capture-pane -t "$S7" -p -S -40)"
	if printf '%s' "$resubmitted" | grep -qF "Finished for"; then
		break
	fi
	sleep 0.15
done
echo "==== captured pane (recalled message resubmitted) ===="
printf '%s\n' "$resubmitted"
recall_resubmit_count=$(printf '%s\n' "$resubmitted" | grep -cF "❯ $RECALL_MSG")
echo "==== Phase 10: '❯ $RECALL_MSG' lines — after Up=$recall_up_count, after Down=$recall_down_count, after resubmit=$recall_resubmit_count ===="
tmux kill-session -t "$S7" 2>/dev/null

# --- Phase 11: `?` from an empty composer toggles the shortcuts band below the
# box (codex's footer shortcut overlay — docs/shortcuts.md); a second `?` hides
# it; and with a draft in the box `?` is just a character (no band). ---
S8="${S}_shortcuts"
tmux new-session -d -s "$S8" -x 80 -y 24 "$BIN"
sleep 0.4
tmux send-keys -t "$S8" -l "?"
sleep 0.3
band_open="$(tmux capture-pane -t "$S8" -p)"
echo "==== captured pane (shortcuts band open) ===="
printf '%s\n' "$band_open"
tmux send-keys -t "$S8" -l "?"
sleep 0.3
band_closed="$(tmux capture-pane -t "$S8" -p)"
tmux send-keys -t "$S8" -l "really?"
sleep 0.3
band_typed="$(tmux capture-pane -t "$S8" -p)"
echo "==== captured pane (after typing a draft containing '?') ===="
printf '%s\n' "$band_typed"
tmux kill-session -t "$S8" 2>/dev/null

# --- Phase 12: a message submitted WHILE a turn streams is QUEUED (codex's
# queued_user_messages, docs/queue.md): shown like a user message "❯ world"
# *above* the box while turn 1 streams, then auto-sent as its OWN turn when the
# first finishes. Both "❯ hello there" and
# "❯ world" must land, and turn 2's "Finished for" summary confirms the queued
# message was sent on its own. ---
S9="${S}_queue"
tmux new-session -d -s "$S9" -x 80 -y 24 "$BIN"
sleep 0.4
tmux send-keys -t "$S9" -l "hello there"
sleep 0.2
tmux send-keys -t "$S9" Enter
for _ in $(seq 1 40); do # up to ~4s: wait until turn 1 is visibly streaming
	if tmux capture-pane -t "$S9" -p | grep -qF "Happy"; then
		break
	fi
	sleep 0.1
done
tmux send-keys -t "$S9" -l "world"
sleep 0.2
tmux send-keys -t "$S9" Enter # streaming → queued, not submitted
queued_band=""
for _ in $(seq 1 20); do # up to ~3s: the queued message shows above the box
	queued_band="$(tmux capture-pane -t "$S9" -p)"
	# While turn 1 still streams, "❯ world" can only be the queued display
	# (it has not been sent yet); the status line confirms the turn is active.
	if printf '%s' "$queued_band" | grep -qF "❯ world" &&
		printf '%s' "$queued_band" | grep -qF "tokens"; then
		break
	fi
	sleep 0.15
done
echo "==== captured pane (world queued above the box) ===="
printf '%s\n' "$queued_band"
queue_done=""
for _ in $(seq 1 100); do # up to ~15s: both turns finish
	queue_done="$(tmux capture-pane -t "$S9" -p -S -60)"
	if printf '%s' "$queue_done" | grep -qF "Finished for"; then
		break
	fi
	sleep 0.15
done
echo "==== captured pane (queued message auto-sent as its own turn) ===="
printf '%s\n' "$queue_done"
tmux kill-session -t "$S9" 2>/dev/null

# --- Phase 13: Esc with a queued message interrupts the current turn AND sends
# the queued one right away (the user's spec; codex's steer-after-interrupt). Submit
# "hello there", queue "world" mid-stream, then Esc: the red "Conversation
# interrupted" notice commits for turn 1, and "world" is sent immediately as turn 2
# ("❯ world" + "Finished for"). ---
S10="${S}_queueint"
tmux new-session -d -s "$S10" -x 80 -y 24 "$BIN"
sleep 0.4
tmux send-keys -t "$S10" -l "hello there"
sleep 0.2
tmux send-keys -t "$S10" Enter
for _ in $(seq 1 40); do # up to ~4s: wait until turn 1 is visibly streaming
	if tmux capture-pane -t "$S10" -p | grep -qF "Happy"; then
		break
	fi
	sleep 0.1
done
tmux send-keys -t "$S10" -l "world"
sleep 0.2
tmux send-keys -t "$S10" Enter # queued while streaming
sleep 0.3
tmux send-keys -t "$S10" Escape # interrupt turn 1 → send "world" right away
queueint=""
for _ in $(seq 1 80); do # up to ~12s: the flushed "world" turn finishes
	queueint="$(tmux capture-pane -t "$S10" -p -S -60)"
	if printf '%s' "$queueint" | grep -qF "Finished for"; then
		break
	fi
	sleep 0.15
done
echo "==== captured pane (Esc interrupted turn 1 and sent the queued 'world') ===="
printf '%s\n' "$queueint"
tmux kill-session -t "$S10" 2>/dev/null

status=0
if ! printf '%s' "$pane" | grep -qF "❯ $USER_MSG"; then
	echo "FAIL: user message line '❯ $USER_MSG' not echoed to scrollback" >&2
	status=1
fi
if ! printf '%s' "$pane" | grep -qF "$EXPECT_REPLY"; then
	echo "FAIL: streamed reply '$EXPECT_REPLY' not found" >&2
	status=1
fi
# While the reply streams, the live status line shows a running token count (the
# pane was captured mid-stream above, so text — and so tokens — is flowing).
if ! printf '%s' "$pane" | grep -qF "tokens"; then
	echo "FAIL: the live status line (token count) was not shown while streaming" >&2
	status=1
fi
# …and a blank gap row separates the status line from the box's top rule ("… ("
# is unique to the status line: verb + ellipsis + the opening metrics paren).
status_gap=$(printf '%s\n' "$pane" | awk '
	/… \(/ {
		ok = "bad"
		if ((getline gap) > 0 && (getline rule) > 0) {
			gsub(/[ \t]/, "", gap)
			if (gap == "" && rule ~ /─/) ok = "ok"
		}
		print ok
		exit
	}')
if [ "${status_gap:-missing}" != "ok" ]; then
	echo "FAIL: no blank gap row between the live status line and the input box (${status_gap:-status line missing})" >&2
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
# Stamps in the tool view: only the USER message shows one — alone on its own
# right-aligned line, 12-hour hh:mm AM/PM, no seconds, no date.
if ! printf '%s' "$overlay" | grep -qE '^ +(0[1-9]|1[0-2]):[0-5][0-9] (AM|PM)$'; then
	echo "FAIL: the user message's right-aligned hh:mm AM/PM stamp line is missing from the tool view" >&2
	status=1
fi
if printf '%s' "$overlay" | grep -qE '[0-9]{4}-[0-9]{2}-[0-9]{2}|[0-9]{2}:[0-9]{2}:[0-9]{2}'; then
	echo "FAIL: a dated/seconds timestamp is still shown in the tool view (stamps are hh:mm AM/PM, user messages only)" >&2
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
if ! printf '%s' "$palette_open" | grep -qF "Exit inline-tui"; then
	echo "FAIL: the command palette did not list /quit" >&2
	status=1
fi
if ! printf '%s' "$help_ran" | grep -qF "Available commands:"; then
	echo "FAIL: running /help did not post its system notice" >&2
	status=1
fi
if ! printf '%s' "$settled_cur" | grep -qF "changes size"; then
	echo "FAIL: the reply never finished on the short terminal (Phase 5 could not settle)" >&2
	status=1
else
	# When the turn ends the live status line is replaced by a committed
	# "{done verb} for Ns" summary (a fresh session → turn 0 → the verb "Done").
	if ! printf '%s' "$settled_cur" | grep -qF "Done for"; then
		echo "FAIL: the committed 'Done for Ns' turn summary was not shown after the reply finished" >&2
		status=1
	fi
	if [ "${trailing_blanks:-99}" -ne 0 ]; then
		echo "FAIL: $trailing_blanks blank row(s) left below the input box after the reply settled — the box should stay flush at the bottom" >&2
		status=1
	fi
fi
if [ "$burst_ok" -ne 1 ]; then
	echo "FAIL: a 1000-char input burst was not fully rendered within 600ms (took ${burst_ms}ms) — input is not coalesced into one repaint, typing lag regressed" >&2
	status=1
fi
# Phase 7: a turn that finished while the Ctrl+O overlay was open must, on quit,
# leave the restored screen showing the committed summary — not the stale live
# status line whose commits were deferred while the overlay was up.
if ! printf '%s' "$overlay_done" | grep -qF "Done for"; then
	echo "FAIL: the turn never finished inside the Ctrl+O overlay (Phase 7 precondition not met — retune the timing)" >&2
	status=1
else
	if ! printf '%s' "$post_quit" | grep -qF "Done for"; then
		echo "FAIL: after quitting (Ctrl+C) from the overlay, the committed 'Done for Ns' summary was not restored to the screen — the quit path left the stale live status strip behind" >&2
		status=1
	fi
	if printf '%s' "$post_quit" | grep -qF "tokens"; then
		echo "FAIL: after quitting (Ctrl+C) from the overlay, the stale live status line ('… tokens') was still on screen instead of the 'Done for Ns' summary" >&2
		status=1
	fi
fi
# Phase 8: Esc mid-stream interrupts the turn, codex-style (docs/interrupt.md).
# The hint rides the live status line — Phase 1's pane was captured mid-stream.
if ! printf '%s' "$pane" | grep -qF "esc to interrupt"; then
	echo "FAIL: the live status line does not show the 'esc to interrupt' hint while streaming" >&2
	status=1
fi
if ! printf '%s' "$interrupted" | grep -qF "Conversation interrupted"; then
	echo "FAIL: Esc mid-stream did not commit the 'Conversation interrupted' notice (did the app quit instead?)" >&2
	status=1
fi
if ! printf '%s' "$interrupted" | grep -qF "Happy"; then
	echo "FAIL: the partial reply was not kept on screen after the interrupt" >&2
	status=1
fi
if printf '%s' "$interrupted" | grep -qF "tokens"; then
	echo "FAIL: the live status line ('… tokens') is still showing after the interrupt" >&2
	status=1
fi
if printf '%s' "$interrupted" | grep -qF "Done for"; then
	echo "FAIL: an interrupted turn must not commit a 'Done for Ns' summary (the notice is its terminal state)" >&2
	status=1
fi
if ! printf '%s' "$after_interrupt" | grep -qF "Finished for"; then
	echo "FAIL: the app did not complete a follow-up turn after the interrupt — the loop or backend channel is wedged" >&2
	status=1
fi
# Phase 9: Ctrl+C clears a non-empty draft (the app keeps running); /quit exits.
if printf '%s' "$after_clear" | grep -qF "a draft the user"; then
	echo "FAIL: Ctrl+C did not clear the typed draft from the input box" >&2
	status=1
fi
if [ "$quit_alive" -ne 1 ]; then
	echo "FAIL: the app quit on the first Ctrl+C instead of clearing the non-empty input" >&2
	status=1
fi
if [ "$quit_exited" -ne 1 ]; then
	echo "FAIL: running /quit did not exit the app" >&2
	status=1
fi
# Phase 10: ↑/↓ input-history recall (docs/input-history.md). After turn 1 the
# committed user line is the only "❯ $RECALL_MSG"; ↑ adds the recalled copy in
# the input box, ↓ clears it again, and ↑ + Enter commits a second copy.
if [ "${recall_up_count:-0}" -lt 2 ]; then
	echo "FAIL: Up did not recall the sent message into the input box (saw $recall_up_count '❯ $RECALL_MSG' lines, expected the committed one plus the recalled draft)" >&2
	status=1
fi
if [ "${recall_down_count:-99}" -ge "${recall_up_count:-0}" ]; then
	echo "FAIL: Down past the newest entry did not clear the recalled draft (still $recall_down_count '❯ $RECALL_MSG' lines)" >&2
	status=1
fi
if ! printf '%s' "$resubmitted" | grep -qF "Finished for"; then
	echo "FAIL: resubmitting the recalled message (Up + Enter) never finished a second turn" >&2
	status=1
fi
if [ "${recall_resubmit_count:-0}" -lt 2 ]; then
	echo "FAIL: the recalled message was not resubmitted — expected a second committed '❯ $RECALL_MSG' line (saw $recall_resubmit_count)" >&2
	status=1
fi
# Phase 11: the `?` shortcuts band (docs/shortcuts.md). "for commands" only
# ever appears in the band, so it's a clean open/closed marker.
if ! printf '%s' "$band_open" | grep -qF "for commands"; then
	echo "FAIL: '?' with an empty composer did not open the shortcuts band" >&2
	status=1
fi
if ! printf '%s' "$band_open" | grep -qF "ctrl+c to quit"; then
	echo "FAIL: the shortcuts band is missing its quit entry" >&2
	status=1
fi
if printf '%s' "$band_closed" | grep -qF "for commands"; then
	echo "FAIL: a second '?' did not close the shortcuts band" >&2
	status=1
fi
if ! printf '%s' "$band_typed" | grep -qF "❯ really?"; then
	echo "FAIL: '?' inside a draft was not typed as a literal character" >&2
	status=1
fi
if printf '%s' "$band_typed" | grep -qF "for commands"; then
	echo "FAIL: typing a draft ending in '?' re-opened the shortcuts band" >&2
	status=1
fi
# Phase 12: a message submitted mid-stream is queued (shown like a user message
# "❯ world" above the box, while turn 1 still streams) and auto-sent as its own
# turn when the first finishes (docs/queue.md).
if ! printf '%s' "$queued_band" | grep -qF "❯ world"; then
	echo "FAIL: a message submitted while streaming was not shown queued above the box ('❯ world' missing while turn 1 streamed)" >&2
	status=1
fi
if ! printf '%s' "$queue_done" | grep -qF "❯ world"; then
	echo "FAIL: the queued message was never sent — '❯ world' did not reach scrollback" >&2
	status=1
fi
if ! printf '%s' "$queue_done" | grep -qF "Finished for"; then
	echo "FAIL: the queued message did not run as its own (second) turn — no 'Finished for' summary" >&2
	status=1
fi
# Phase 13: Esc with a queued message interrupts the current turn and sends the
# queued one right away (the user's spec; codex's steer-after-interrupt).
if ! printf '%s' "$queueint" | grep -qF "Conversation interrupted"; then
	echo "FAIL: Esc with a queued message did not interrupt the current turn" >&2
	status=1
fi
if ! printf '%s' "$queueint" | grep -qF "❯ world"; then
	echo "FAIL: Esc did not send the queued 'world' right away ('❯ world' missing)" >&2
	status=1
fi
if ! printf '%s' "$queueint" | grep -qF "Finished for"; then
	echo "FAIL: the queued message sent on interrupt never finished its turn" >&2
	status=1
fi
if [ "$status" -eq 0 ]; then
	echo "PASS: reply + tools streamed to scrollback, the input box grows and stays flush at the bottom after a reply, typing bursts render in one repaint, Ctrl+O opens the tool-output view, the slash-command palette opens and runs commands, Esc interrupts a streaming turn, Ctrl+C clears a draft before /quit exits, Up recalls the last sent message for resubmission, ? toggles the shortcuts band, and messages submitted mid-turn queue and auto-send (Esc sends the queued one right away)"
fi
exit "$status"
