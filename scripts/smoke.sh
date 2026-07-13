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
	tmux kill-session -t "${S}_altup" 2>/dev/null
	tmux kill-session -t "${S}_tabqueue" 2>/dev/null
	tmux kill-session -t "${S}_sync" 2>/dev/null
	tmux kill-session -t "${S}_clearkill" 2>/dev/null
	tmux kill-session -t "${S}_resize" 2>/dev/null
	tmux kill-session -t "${S}_search" 2>/dev/null
	tmux kill-session -t "${S}_shell" 2>/dev/null
	tmux kill-session -t "${S}_delay" 2>/dev/null
	tmux kill-session -t "${S}_bigoutput" 2>/dev/null
	tmux kill-session -t "${S}_ctrlj" 2>/dev/null
	tmux kill-session -t "${S}_shellqueue" 2>/dev/null
	tmux kill-session -t "${S}_atmention" 2>/dev/null
	tmux kill-session -t "${S}_paste" 2>/dev/null
	tmux kill-session -t "${S}_imagepaste" 2>/dev/null
	tmux kill-session -t "${S}_copy" 2>/dev/null
	tmux kill-session -t "${S}_backtrack" 2>/dev/null
	tmux kill-session -t "${S}_resume" 2>/dev/null
	tmux kill-session -t "${S}_stall" 2>/dev/null
	tmux kill-session -t "${S}_curhide" 2>/dev/null
	tmux kill-session -t "${S}_midstream" 2>/dev/null
	rm -f /tmp/inline-tui-shell-*.txt 2>/dev/null
	rm -f /tmp/inline-tui-clipboard-*.png 2>/dev/null
	[ -n "${RESUME_DIR:-}" ] && rm -rf "$RESUME_DIR" 2>/dev/null
	[ -n "${SMOKE_CFG:-}" ] && rm -rf "$SMOKE_CFG" 2>/dev/null
}
trap cleanup EXIT

if [ ! -x "$BIN" ]; then
	echo "FAIL: binary not found at $BIN (run: cargo build)" >&2
	exit 1
fi

# The dummy AI now pauses before streaming (so the status indicator shows
# first) — 3s by default. Run every phase with a SHORT delay so the turns
# stream promptly, threaded through INLINE_TUI_STARTUP_DELAY_MS; Phase 20
# overrides it back to a visible pause to verify that behaviour. The `env`
# wrapper is robust even when a tmux server is already running (an exported
# var would not reach its panes).
SMOKE_STARTUP_MS="${SMOKE_STARTUP_MS:-200}"
# Isolate the config home (~/.inline-tui by default) to a throwaway dir so the
# dummy stays the backend regardless of any real key/model a developer has saved
# there (a saved config.json + .env key would otherwise activate a real backend
# and break the dummy-based assertions). Cleaned up on exit.
SMOKE_CFG="$(mktemp -d)"
CFG_ENV="INLINE_TUI_CONFIG_DIR=$SMOKE_CFG"
# Persistence (docs/history-persistence.md) seeds the input history from a file
# on startup. Point the base app at /dev/null so every phase starts with an
# EMPTY input history — the in-session ↑/↓ (Phase 10) and Ctrl+R (Phase 18)
# assertions then behave exactly as before, unaffected by earlier phases'
# submissions. Phase 37 overrides this with a real temp file to test that
# persistence spans sessions.
CFG_ENV_NOHIST="$CFG_ENV INLINE_TUI_HISTORY_FILE=/dev/null"
APP="env $CFG_ENV_NOHIST INLINE_TUI_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"

tmux new-session -d -s "$S" -x 80 -y 24 "$APP"
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

# The composer keeps the hardware cursor while the reply streams (codex keeps
# the box focused mid-turn — it used to be hidden until the turn finished).
# Probe tmux's live cursor state NOW, while chunks are still flowing. The screen
# scrolls between two tmux calls as lines commit, so retry the (row, text) pair
# until it lands on a settled frame.
cursor_mid_flag="$(tmux display-message -p -t "$S" '#{cursor_flag}')"
cursor_mid_row=""
for _ in $(seq 1 10); do
	cy="$(tmux display-message -p -t "$S" '#{cursor_y}')"
	cursor_mid_row="$(tmux capture-pane -t "$S" -p | sed -n "$((cy + 1))p")"
	case "$cursor_mid_row" in "❯"*) break ;; esac
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
# The pager opens pinned to the bottom (tail-following the live stream), so the
# user turn near the top has already scrolled off — jump Home, like Phase 29
# does, to check it's actually in the transcript. Poll rather than a fixed
# sleep (this file's own rule): the redraw races the still-streaming reply.
tmux send-keys -t "$S" Home
overlay=""
for _ in $(seq 1 20); do # up to ~3s
	overlay="$(tmux capture-pane -t "$S" -p)"
	if printf '%s' "$overlay" | grep -qF "$USER_MSG"; then
		break
	fi
	sleep 0.15
done
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

# Quit with Ctrl+C (empty composer): user messages exist by now, so idle Esc
# would arm the Esc-Esc backtrack instead of quitting (docs/backtrack.md).
tmux send-keys -t "$S" C-c
sleep 0.2

# --- Phase 5: after a reply finishes, the input box stays flush at the BOTTOM —
# no blank rows creep in below it when the streaming strip (preview + gap, drawn
# *above* the box) clears. A short 40x12 terminal makes one exchange overflow the
# screen so the box is pushed to the bottom while streaming; the bug let the box
# rise by the strip's height once the reply finished, leaving blank rows beneath. ---
S2="${S}_bottom"
TMP5="$(mktemp)"
tmux new-session -d -s "$S2" -x 40 -y 12 "$APP"
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
tmux new-session -d -s "$S3" -x 80 -y 24 "$APP"
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
tmux send-keys -t "$S4" -l "$CFG_ENV INLINE_TUI_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"
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
# ("Finished for", turn 2's done verb). Esc when idle now arms the Esc-Esc
# backtrack once user messages exist (Phase 30; quitting is Ctrl+C — Phase 4). ---
S5="${S}_interrupt"
tmux new-session -d -s "$S5" -x 80 -y 24 "$APP"
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
tmux new-session -d -s "$S6" -x 80 -y 24 "$APP"
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
tmux new-session -d -s "$S7" -x 80 -y 24 "$APP"
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
tmux new-session -d -s "$S8" -x 80 -y 24 "$APP"
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

# --- Phase 12: messages submitted WHILE a turn streams are QUEUED (codex's
# queued_user_messages, docs/queue.md): each shown like a user message ("  ❯ …",
# two-space inset) *above* the box while turn 1 streams, then the WHOLE backlog
# is sent as ONE batched turn when the first finishes (Claude-Code style). Both
# queued messages must commit ("❯ world", "❯ again") and exactly one extra turn
# runs: turn 2's "Finished for" summary appears, turn 3's "Completed for" must
# NOT (two separate turns would produce it). ---
S9="${S}_queue"
tmux new-session -d -s "$S9" -x 80 -y 24 "$APP"
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
tmux send-keys -t "$S9" -l "again"
sleep 0.2
tmux send-keys -t "$S9" Enter # second queued message
queued_band=""
for _ in $(seq 1 20); do # up to ~3s: both queued messages show above the box
	queued_band="$(tmux capture-pane -t "$S9" -p)"
	# While turn 1 still streams, the two-space inset ("  ❯ …" — committed user
	# lines sit at column 0) can only be the queued display; the status line
	# confirms the turn is active.
	if printf '%s' "$queued_band" | grep -qF "  ❯ world" &&
		printf '%s' "$queued_band" | grep -qF "  ❯ again" &&
		printf '%s' "$queued_band" | grep -qF "tokens"; then
		break
	fi
	sleep 0.15
done
echo "==== captured pane (world + again queued above the box) ===="
printf '%s\n' "$queued_band"
queue_done=""
for _ in $(seq 1 100); do # up to ~15s: turn 1 then the batched turn 2 finish
	queue_done="$(tmux capture-pane -t "$S9" -p -S -80)"
	if printf '%s' "$queue_done" | grep -qF "Finished for"; then
		break
	fi
	sleep 0.15
done
echo "==== captured pane (queued backlog batch-sent as one turn) ===="
printf '%s\n' "$queue_done"
tmux kill-session -t "$S9" 2>/dev/null

# --- Phase 13: Esc with a queued message interrupts the current turn AND sends
# the queued one right away (the user's spec; codex's steer-after-interrupt). Submit
# "hello there", queue "world" mid-stream, then Esc: the red "Conversation
# interrupted" notice commits for turn 1, and "world" is sent immediately as turn 2
# ("❯ world" + "Finished for"). ---
S10="${S}_queueint"
tmux new-session -d -s "$S10" -x 80 -y 24 "$APP"
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

# --- Phase 14: Alt+Up pulls only the LAST queued batch back into the composer
# (docs/queue.md): queue "world" with Enter (batch 1) and "again" with Tab
# (batch 2 — a separate turn) mid-stream, press Alt+Up — only "again" returns to
# the box as the draft ("❯ again"), while the earlier "world" batch stays queued
# (its "  ❯ world" inset row remains) and the pulled "  ❯ again" inset row is gone. ---
S11="${S}_altup"
tmux new-session -d -s "$S11" -x 80 -y 24 "$APP"
sleep 0.4
tmux send-keys -t "$S11" -l "hello there"
sleep 0.2
tmux send-keys -t "$S11" Enter
for _ in $(seq 1 40); do # up to ~4s: wait until turn 1 is visibly streaming
	if tmux capture-pane -t "$S11" -p | grep -qF "Happy"; then
		break
	fi
	sleep 0.1
done
tmux send-keys -t "$S11" -l "world"
sleep 0.2
tmux send-keys -t "$S11" Enter # batch 1 = [world]
tmux send-keys -t "$S11" -l "again"
sleep 0.2
tmux send-keys -t "$S11" Tab # batch 2 = [again] — a separate follow-up turn
for _ in $(seq 1 20); do # both queued rows visible before the restore
	if tmux capture-pane -t "$S11" -p | grep -qF "  ❯ again"; then
		break
	fi
	sleep 0.15
done
tmux send-keys -t "$S11" M-Up
sleep 0.4
altup="$(tmux capture-pane -t "$S11" -p)"
echo "==== captured pane (Alt+Up restored only the last batch into the composer) ===="
printf '%s\n' "$altup"
tmux kill-session -t "$S11" 2>/dev/null

# --- Phase 15: scrollback commits are FLICKER-FREE — every live-region clear
# rides inside a synchronized-update frame (docs/flicker.md). Record the raw
# byte stream of a whole streaming turn via pipe-pane: each ESC[J region clear
# (a commit blanking the live region before its repaint) must sit between
# ESC[?2026h and ESC[?2026l, so a terminal honouring mode 2026 can never
# present the boxless intermediate state. Before the fix insert_before cleared
# the region OUTSIDE any sync block and the box repaint came on a later flush —
# the streaming blink (the pre-fix stream shows every clear outside). The pipe
# closes before the quit: restore()'s teardown clear is legitimately bare. ---
S12="${S}_sync"
RAW15="$(mktemp)"
tmux new-session -d -s "$S12" -x 80 -y 24 "$APP"
sleep 0.4
tmux pipe-pane -t "$S12" -o "cat > $RAW15"
tmux send-keys -t "$S12" -l "hello there"
sleep 0.2
tmux send-keys -t "$S12" Enter
for _ in $(seq 1 80); do # up to ~12s: the whole turn (text + thinking + tools)
	if tmux capture-pane -t "$S12" -p -S -60 | grep -qF "Done for"; then
		break
	fi
	sleep 0.15
done
tmux pipe-pane -t "$S12" # close the recording before quitting
sleep 0.2
tmux send-keys -t "$S12" C-c # quit (idle Esc would arm the backtrack instead)
sleep 0.2
tmux kill-session -t "$S12" 2>/dev/null
# Tokenise the stream: each sync begin/end and each ESC[J / ESC[0J clear onto
# its own line, then walk them in order counting clears outside a block.
sync_clears=$(sed -e $'s/\x1b\[?2026h/\\\n@SYNC@\\\n/g' \
	-e $'s/\x1b\[?2026l/\\\n@ENDS@\\\n/g' \
	-e $'s/\x1b\[0\{0,1\}J/\\\n@CLRJ@\\\n/g' "$RAW15" | awk '
	/@SYNC@/ { depth = 1; seen = 1; next }
	/@ENDS@/ { depth = 0; next }
	/@CLRJ@/ { total++; if (seen && depth == 0) bad++ }
	END { printf "total=%d outside=%d", total + 0, bad + 0 }')
rm -f "$RAW15"
echo "==== Phase 15: live-region clears in the raw output stream — $sync_clears ===="

# --- Phase 16: /clear MID-TURN kills the generation (codex instead *disables*
# /new//clear during a task — the kill is our spec). Run /clear while the reply
# streams: the screen must blank with no trace of the turn (no echo, no reply
# text, no live status, no interrupt notice), and the backend must be cancelled
# + reaped + its channel drained — so nothing recommits while the rest of the
# turn's schedule would still have been streaming (the pre-fix bug: the screen
# cleared but chunks kept flowing in). The loop must survive the kill: a fresh
# message streams and finishes normally. ---
S13="${S}_clearkill"
tmux new-session -d -s "$S13" -x 80 -y 24 "$APP"
sleep 0.4
tmux send-keys -t "$S13" -l "hello there"
sleep 0.2
tmux send-keys -t "$S13" Enter
for _ in $(seq 1 40); do # up to ~4s: wait until the reply is visibly streaming
	if tmux capture-pane -t "$S13" -p | grep -qF "Happy"; then
		break
	fi
	sleep 0.1
done
tmux send-keys -t "$S13" -l "/clear"
sleep 0.2
tmux send-keys -t "$S13" Enter
sleep 0.5
# /clear now clears the visible screen AND purges the terminal's own scrollback
# (codex's clear_scrollback_and_visible_screen_ansi: ED2 to clear the screen +
# the ED3 scrollback purge, emitted as one ANSI write). So both must be blank —
# a bare clear_region(All)/ED2 used to leave the whole conversation sitting one
# scroll up. Capture the VISIBLE screen first, then the scrollback (-S).
cleared_now="$(tmux capture-pane -t "$S13" -p)"
echo "==== captured visible screen (right after /clear mid-stream) ===="
printf '%s\n' "$cleared_now"
cleared_scrollback="$(tmux capture-pane -t "$S13" -p -S -200)"
echo "==== captured scrollback (-S -200) right after /clear — must not hold the old turn ===="
printf '%s\n' "$cleared_scrollback"
# The dummy turn would keep streaming (text, thinking, tools) for several more
# seconds; if the backend survived the /clear its output would recommit into
# the blank screen. Let that window pass, then look again.
sleep 2.5
cleared_later="$(tmux capture-pane -t "$S13" -p)"
echo "==== captured visible screen (2.5s after /clear — must still be blank) ===="
printf '%s\n' "$cleared_later"
# The loop survives the kill: a fresh turn streams and finishes ("Finished
# for" — turn 2's done verb, as in the Esc-interrupt phase).
tmux send-keys -t "$S13" -l "again please"
sleep 0.2
tmux send-keys -t "$S13" Enter
after_clear=""
for _ in $(seq 1 80); do # up to ~12s: wait for the fresh turn's summary
	after_clear="$(tmux capture-pane -t "$S13" -p -S -30)"
	if printf '%s' "$after_clear" | grep -qF "Finished for"; then
		break
	fi
	sleep 0.15
done
echo "==== captured pane (fresh turn after the /clear kill) ===="
printf '%s\n' "$after_clear"
tmux kill-session -t "$S13" 2>/dev/null

# --- Phase 17: a terminal RESIZE re-presents the conversation at the new size —
# HEIGHT-ONLY changes included. codex redraws everything from source on every
# resize (and re-clamps its viewport into the new screen); the pre-fix bug here
# reflowed only on a *width* change, so a height-only resize repainted the box at
# a stale viewport row while the emulator had already moved the screen contents —
# leaving phantom input boxes on screen and pushing the conversation out of view.
# Finish a turn at 80x24, shrink to 80x12 (height only), grow back to 80x24, then
# shrink the height again MID-STREAM: after each step the visible screen must
# hold exactly ONE input box (one bare `❯` prompt row, two rules, one footer)
# with the conversation tail above it. ---
S14="${S}_resize"
tmux new-session -d -s "$S14" -x 80 -y 24 "$APP"
sleep 0.4
tmux send-keys -t "$S14" -l "hello there"
sleep 0.2
tmux send-keys -t "$S14" Enter
for _ in $(seq 1 80); do # up to ~8s: wait for the turn's committed summary
	if tmux capture-pane -t "$S14" -p | grep -qE "^Done for [0-9]+s"; then
		break
	fi
	sleep 0.1
done
tmux resize-window -t "$S14" -x 80 -y 12
sleep 0.6
resize_shrunk="$(tmux capture-pane -t "$S14" -p)"
echo "==== captured visible screen (after height-only shrink to 80x12) ===="
printf '%s\n' "$resize_shrunk"
tmux resize-window -t "$S14" -x 80 -y 24
sleep 0.6
resize_regrown="$(tmux capture-pane -t "$S14" -p)"
echo "==== captured visible screen (after height grow back to 80x24) ===="
printf '%s\n' "$resize_regrown"
# A WIDTH change is the classic duplication trigger: the emulator re-wraps the
# on-screen lines, and the old in-place overwrite left that reflowed copy behind
# (the TUI-text duplication this fix targets). The purge-mode resize clears the
# screen + scrollback and rebuilds the whole conversation from history, so the
# user message stays SINGLE. Shrink the width, capture the FULL pane (-S), then
# restore to 80x24 so the mid-stream section below starts where it expects.
tmux resize-window -t "$S14" -x 50 -y 24
sleep 0.6
resize_narrow_full="$(tmux capture-pane -t "$S14" -p -S -200)"
echo "==== captured full pane (-S) after width shrink to 50x24 — no duplication ===="
printf '%s\n' "$resize_narrow_full"
tmux resize-window -t "$S14" -x 80 -y 24
sleep 0.6
# A height shrink MID-STREAM must recover the same way: the repaint resets the
# committed count, so the in-flight reply re-commits itself at the new size as
# the remaining chunks flow ("Finished for" is turn 2's done verb).
tmux send-keys -t "$S14" -l "again please"
sleep 0.2
tmux send-keys -t "$S14" Enter
sleep 0.7 # mid-stream: the first text segment is flowing
tmux resize-window -t "$S14" -x 80 -y 14
resize_mid=""
for _ in $(seq 1 80); do # up to ~12s: wait for the resized turn's summary
	resize_mid="$(tmux capture-pane -t "$S14" -p)"
	if printf '%s' "$resize_mid" | grep -qE "^Finished for [0-9]+s"; then
		break
	fi
	sleep 0.15
done
echo "==== captured visible screen (height shrunk mid-stream, turn finished at 80x14) ===="
printf '%s\n' "$resize_mid"
tmux kill-session -t "$S14" 2>/dev/null

# --- Phase 18: Ctrl+R reverse history search (docs/history-search.md). Build
# two history entries — one submitted turn plus one Ctrl+C-cleared draft (the
# clear records it, no second slow turn needed) — then: Ctrl+R opens the
# reverse-i-search line in the footer slot; typing a query previews the newest
# matching entry in the composer; Ctrl+R again steps to the older match; Enter
# accepts it (the search line closes, the session footer returns, the text
# stays as an editable draft); a hopeless query shows "no match" with the
# draft restored; and Esc closes the search WITHOUT quitting the app. ---
S15="${S}_search"
tmux new-session -d -s "$S15" -x 80 -y 24 "$APP"
sleep 0.4
tmux send-keys -t "$S15" -l "alpha bravo"
sleep 0.2
tmux send-keys -t "$S15" Enter
for _ in $(seq 1 80); do # up to ~8s: the turn must finish first
	if tmux capture-pane -t "$S15" -p | grep -qE "^Done for [0-9]+s"; then
		break
	fi
	sleep 0.1
done
tmux send-keys -t "$S15" -l "charlie alpha"
sleep 0.2
tmux send-keys -t "$S15" C-c
sleep 0.2
tmux send-keys -t "$S15" C-r
sleep 0.3
search_open="$(tmux capture-pane -t "$S15" -p)"
echo "==== captured visible screen (Ctrl+R pressed — search open, idle) ===="
printf '%s\n' "$search_open"
tmux send-keys -t "$S15" -l "alpha"
sleep 0.3
search_match="$(tmux capture-pane -t "$S15" -p)"
echo "==== captured visible screen (query 'alpha' typed — newest match previews) ===="
printf '%s\n' "$search_match"
tmux send-keys -t "$S15" C-r
sleep 0.3
search_older="$(tmux capture-pane -t "$S15" -p)"
echo "==== captured visible screen (Ctrl+R again — older match) ===="
printf '%s\n' "$search_older"
tmux send-keys -t "$S15" Enter
sleep 0.3
search_accept="$(tmux capture-pane -t "$S15" -p)"
echo "==== captured visible screen (Enter — match accepted as a draft) ===="
printf '%s\n' "$search_accept"
tmux send-keys -t "$S15" C-r
sleep 0.2
tmux send-keys -t "$S15" -l "zzz"
sleep 0.3
search_nomatch="$(tmux capture-pane -t "$S15" -p)"
echo "==== captured visible screen (query 'zzz' — no match) ===="
printf '%s\n' "$search_nomatch"
tmux send-keys -t "$S15" Escape
sleep 0.3
search_cancel="$(tmux capture-pane -t "$S15" -p)"
echo "==== captured visible screen (Esc — search cancelled, app still alive) ===="
printf '%s\n' "$search_cancel"
tmux kill-session -t "$S15" 2>/dev/null

# --- Phase 19: `!` shell commands (docs/shell-command.md). Typing `!cmd` enters
# shell mode: the bang is absorbed into the prompt (the composer reads `! cmd`,
# not `❯ !cmd`) and the footer flips to "Shell mode". Enter from an idle
# composer runs the command locally as a codex-style exec cell — the `! cmd`
# header on the dark user-style line with its `⎿` output flush below (and
# `⎿ Running…` while it runs); a long command is interruptible with Esc. ---
S16="${S}_shell"
tmux new-session -d -s "$S16" -x 80 -y 24 "$APP"
sleep 0.4
# Shell mode: the bang becomes the prompt and the footer hint shows.
tmux send-keys -t "$S16" -l "!echo smoke_shell_ok"
sleep 0.3
shell_mode="$(tmux capture-pane -t "$S16" -p)"
echo "==== captured visible screen (typing !echo … — Shell mode hint) ===="
printf '%s\n' "$shell_mode"
# Run it: Enter dispatches the command; poll for the cell's ⎿ output line.
tmux send-keys -t "$S16" Enter
shell_ran=""
for _ in $(seq 1 60); do # up to ~6s
	shell_ran="$(tmux capture-pane -t "$S16" -p -S -40)"
	if printf '%s' "$shell_ran" | grep -qF "⎿ smoke_shell_ok"; then
		break
	fi
	sleep 0.1
done
echo "==== captured pane (after !echo ran) ===="
printf '%s\n' "$shell_ran"
# A failing command resolves the cell red (non-zero exit) and keeps running.
tmux send-keys -t "$S16" -l "!exit 3"
sleep 0.2
tmux send-keys -t "$S16" Enter
shell_fail=""
for _ in $(seq 1 60); do
	shell_fail="$(tmux capture-pane -t "$S16" -p -S -40)"
	if printf '%s' "$shell_fail" | grep -qF "exit status: 3"; then
		break
	fi
	sleep 0.1
done
echo "==== captured pane (after !exit 3 — failure) ===="
printf '%s\n' "$shell_fail"
# A long command runs with the `⎿ Running… (Ns)` preview and NO spinner status
# line (req 3 — the elapsed rides the preview instead of the hidden status).
tmux send-keys -t "$S16" -l "!sleep 9"
sleep 0.2
tmux send-keys -t "$S16" Enter
sleep 1.2 # let it run long enough to show a non-zero elapsed
shell_running="$(tmux capture-pane -t "$S16" -p -S -40)"
echo "==== captured pane (!sleep 9 running — ⎿ Running… (Ns) preview, no status) ===="
printf '%s\n' "$shell_running"
# Esc resolves the cell `⎿ Interrupted by user` — NOT the `Conversation
# interrupted` notice (req 2: the shell cell is its own record).
tmux send-keys -t "$S16" Escape
shell_interrupt=""
for _ in $(seq 1 40); do # up to ~4s — far less than the 9s sleep
	shell_interrupt="$(tmux capture-pane -t "$S16" -p -S -40)"
	if printf '%s' "$shell_interrupt" | grep -qF "Interrupted by user"; then
		break
	fi
	sleep 0.1
done
echo "==== captured pane (after Esc interrupts !sleep 9) ===="
printf '%s\n' "$shell_interrupt"
tmux kill-session -t "$S16" 2>/dev/null

# --- Phase 20: the dummy AI PAUSES before streaming so the status indicator is
# visible first (docs/status-indicator.md), and the just-sent user message is
# counted into the tally with the ↑ arrow. Launch with a longer startup delay
# (overriding the smoke-wide short one), submit, then capture MID-PAUSE: the
# status line must show with `↑ N tokens` and NO reply text yet — then the
# reply must still stream once the pause elapses. ---
S17="${S}_delay"
DELAY_MSG="count my input tokens"
# 21 chars → responses[0] ("Sure! This is a streaming demo …").
DELAY_REPLY="Sure! This is a streaming demo"
tmux new-session -d -s "$S17" -x 80 -y 24 "env $CFG_ENV INLINE_TUI_STARTUP_DELAY_MS=2000 $BIN"
sleep 0.5
tmux send-keys -t "$S17" -l "$DELAY_MSG"
sleep 0.2
tmux send-keys -t "$S17" Enter
sleep 0.9 # mid-pause: the 2s startup delay is still running
delay_pause="$(tmux capture-pane -t "$S17" -p)"
echo "==== captured visible screen (mid pre-stream pause — status shows, no reply yet) ===="
printf '%s\n' "$delay_pause"
# The reply must still arrive once the pause elapses (the pause is not a hang).
delay_reply=""
for _ in $(seq 1 60); do # up to ~6s
	delay_reply="$(tmux capture-pane -t "$S17" -p -S -40)"
	if printf '%s' "$delay_reply" | grep -qF "$DELAY_REPLY"; then
		break
	fi
	sleep 0.1
done
echo "==== captured pane (after the pause — reply streaming, arrow flipped down) ===="
printf '%s\n' "$delay_reply"
tmux kill-session -t "$S17" 2>/dev/null

# --- Phase 21: TAB queues a message as a SEPARATE follow-up turn (docs/queue.md),
# unlike Enter which batches into the next turn. Submit "hello there", then queue
# "world" with Enter and "later" with TAB while turn 1 streams: both show inset
# above the box ("  ❯ world", "  ❯ later"), divided by a blank batch boundary.
# "world" then runs as turn 2 ("Finished for") and "later" runs as a SEPARATE
# turn 3 ("Completed for") — the third turn Phase 12's all-Enter batching never
# produces. ---
S18="${S}_tabqueue"
tmux new-session -d -s "$S18" -x 80 -y 24 "$APP"
sleep 0.4
tmux send-keys -t "$S18" -l "hello there"
sleep 0.2
tmux send-keys -t "$S18" Enter
for _ in $(seq 1 40); do # up to ~4s: wait until turn 1 is visibly streaming
	if tmux capture-pane -t "$S18" -p | grep -qF "Happy"; then
		break
	fi
	sleep 0.1
done
tmux send-keys -t "$S18" -l "world"
sleep 0.2
tmux send-keys -t "$S18" Enter # streaming → batch 1 (the first queue)
tmux send-keys -t "$S18" -l "later"
sleep 0.2
tmux send-keys -t "$S18" Tab # streaming → a NEW batch (a separate follow-up turn)
tabqueue_band=""
for _ in $(seq 1 20); do # up to ~3s: both queued messages show above the box
	tabqueue_band="$(tmux capture-pane -t "$S18" -p)"
	if printf '%s' "$tabqueue_band" | grep -qF "  ❯ world" &&
		printf '%s' "$tabqueue_band" | grep -qF "  ❯ later" &&
		printf '%s' "$tabqueue_band" | grep -qF "tokens"; then
		break
	fi
	sleep 0.15
done
echo "==== captured pane (world via Enter + later via Tab queued above the box) ===="
printf '%s\n' "$tabqueue_band"
tabqueue=""
for _ in $(seq 1 160); do # up to ~24s: turns 1, 2, then the SEPARATE turn 3 finish
	tabqueue="$(tmux capture-pane -t "$S18" -p -S -100)"
	if printf '%s' "$tabqueue" | grep -qF "Completed for"; then
		break
	fi
	sleep 0.15
done
echo "==== captured pane (the Tab follow-up ran as a separate third turn) ===="
printf '%s\n' "$tabqueue"
tmux kill-session -t "$S18" 2>/dev/null

# --- Phase 22: a `!` command with HUGE output is CAPPED IN MEMORY, not buffered
# whole (docs/shell-command.md). Run a command producing >100KB (over the cap):
# the cell renders the retained head with the usual `+N lines (ctrl+o to expand)`
# peek hint, NO temp file is written (the output is never held in full — this is
# the memory fix), and the Ctrl+O view appends a `…` truncation marker at its
# end. ---
S19="${S}_bigoutput"
# Start from a clean slate so the "no temp file" check sees only what this run
# would create (the feature is gone, so it should stay zero).
rm -f /tmp/inline-tui-shell-*.txt 2>/dev/null
tmux new-session -d -s "$S19" -x 80 -y 24 "$APP"
sleep 0.4
# `seq 1 50000` is ~280KB across many lines (well over the 100KB cap) and
# contains no `…` of its own, so any `…` in the Ctrl+O view is the marker.
tmux send-keys -t "$S19" -l "!seq 1 50000"
sleep 0.2
tmux send-keys -t "$S19" Enter
bigoutput=""
for _ in $(seq 1 60); do # up to ~6s
	bigoutput="$(tmux capture-pane -t "$S19" -p -S -40)"
	if printf '%s' "$bigoutput" | grep -qF "ctrl+o to expand"; then
		break
	fi
	sleep 0.1
done
echo "==== captured pane (huge !output capped in memory) ===="
printf '%s\n' "$bigoutput"
# No temp file may be created — the full output is never written anywhere now.
bigoutput_tmpfiles="$(ls /tmp/inline-tui-shell-*.txt 2>/dev/null | wc -l | tr -d ' ')"
echo "bigoutput_tmpfiles=[$bigoutput_tmpfiles]"
# Open the Ctrl+O view (it opens pinned to the bottom) — its end carries the `…`.
tmux send-keys -t "$S19" C-o
bigoutput_overlay=""
for _ in $(seq 1 30); do
	bigoutput_overlay="$(tmux capture-pane -t "$S19" -p)"
	if printf '%s' "$bigoutput_overlay" | grep -qF "T R A N S C R I P T"; then
		break
	fi
	sleep 0.1
done
echo "==== captured Ctrl+O overlay (bottom — truncation marker) ===="
printf '%s\n' "$bigoutput_overlay"
tmux kill-session -t "$S19" 2>/dev/null

# --- Phase 23: Ctrl+J is the UNIVERSAL newline key (docs/shift-enter.md). Unlike
# Shift+Enter (which needs keyboard enhancement to even be reported), Ctrl+J grows
# the input box on every terminal — in raw mode the byte 0x0A parses to
# Char('j')+CONTROL. Type two lines separated by Ctrl+J: the box must grow to show
# the prompt on the first line and an indented continuation on the second (same
# shape as Phase 2's Alt+Enter, via a different key). A plain Enter then submits
# the whole multi-line draft. ---
S20="${S}_ctrlj"
tmux new-session -d -s "$S20" -x 80 -y 24 "$APP"
sleep 0.4
tmux send-keys -t "$S20" -l "CCC"
tmux send-keys -t "$S20" C-j
tmux send-keys -t "$S20" -l "DDD"
sleep 0.3
ctrlj_grown="$(tmux capture-pane -t "$S20" -p)"
echo "==== captured pane (Ctrl+J grew the input box) ===="
printf '%s\n' "$ctrlj_grown"
tmux send-keys -t "$S20" Enter # a plain Enter submits the multi-line draft
ctrlj_sent=""
for _ in $(seq 1 40); do # up to ~6s: the two-line message commits to scrollback
	ctrlj_sent="$(tmux capture-pane -t "$S20" -p -S -40)"
	if printf '%s' "$ctrlj_sent" | grep -qF "❯ CCC" &&
		printf '%s' "$ctrlj_sent" | grep -qF "DDD"; then
		break
	fi
	sleep 0.15
done
echo "==== captured pane (multi-line draft submitted with Enter) ===="
printf '%s\n' "$ctrlj_sent"
tmux kill-session -t "$S20" 2>/dev/null

# --- Phase 24: a `!` command typed WHILE A TURN STREAMS queues as its own
# STANDALONE shell entry and runs LOCALLY as a separate turn after it
# (docs/queue.md, docs/shell-command.md) — codex's action-tagged queued shell
# command, NOT the old v1 behaviour of sending "!echo …" to the backend as
# literal text. Submit "hello there", then mid-stream queue "world" (Enter, a
# text turn) and "!echo smoke_queue_ok" (Enter in shell mode, a standalone shell
# turn). Both show inset above the box ("  ❯ world", "  ! echo smoke_queue_ok");
# then "world" runs as turn 2 and the command runs LOCALLY as turn 3, committing
# an exec cell ("! echo …" header + "⎿ smoke_queue_ok" output) — never a
# "❯ !echo …" user message. ---
S21="${S}_shellqueue"
tmux new-session -d -s "$S21" -x 80 -y 24 "$APP"
sleep 0.4
tmux send-keys -t "$S21" -l "hello there"
sleep 0.2
tmux send-keys -t "$S21" Enter
for _ in $(seq 1 40); do # up to ~4s: wait until turn 1 is visibly streaming
	if tmux capture-pane -t "$S21" -p | grep -qF "Happy"; then
		break
	fi
	sleep 0.1
done
tmux send-keys -t "$S21" -l "world"
sleep 0.2
tmux send-keys -t "$S21" Enter # streaming → a text batch (runs as turn 2)
tmux send-keys -t "$S21" -l "!echo smoke_queue_ok"
sleep 0.2
tmux send-keys -t "$S21" Enter # streaming → a STANDALONE shell entry (turn 3, local)
shellqueue_band=""
for _ in $(seq 1 20); do # up to ~3s: both queued entries show inset above the box
	shellqueue_band="$(tmux capture-pane -t "$S21" -p)"
	if printf '%s' "$shellqueue_band" | grep -qF "  ❯ world" &&
		printf '%s' "$shellqueue_band" | grep -qF "  ! echo smoke_queue_ok"; then
		break
	fi
	sleep 0.15
done
echo "==== captured pane (text + shell command queued above the box) ===="
printf '%s\n' "$shellqueue_band"
shellqueue=""
for _ in $(seq 1 160); do # up to ~24s: turn 1, turn 2 (world), then the LOCAL shell turn
	shellqueue="$(tmux capture-pane -t "$S21" -p -S -100)"
	if printf '%s' "$shellqueue" | grep -qF "⎿ smoke_queue_ok"; then
		break
	fi
	sleep 0.15
done
echo "==== captured pane (the queued !command ran locally as its own turn) ===="
printf '%s\n' "$shellqueue"
tmux kill-session -t "$S21" 2>/dev/null

# --- Phase 25: `@` file-path mentions (docs/file-search.md). Typing `@query`
# opens a file picker BELOW the box listing workspace files that fuzzy-match the
# query (fetched asynchronously by a background walk+rank worker); Enter inserts
# the highlighted path into the composer, replacing the `@token`. Launch in a
# temp dir with known files so the match set is deterministic. ---
S22="${S}_atmention"
ATDIR="$(mktemp -d)"
: >"$ATDIR/alpha_smoke.txt"
: >"$ATDIR/readme_notes.md"
mkdir -p "$ATDIR/subdir"
: >"$ATDIR/subdir/beta_smoke.txt"
# The session starts in $ATDIR, so the binary needs an ABSOLUTE path ($APP's is
# relative to the project dir); the app then walks $ATDIR for the @ picker.
APP_ABS="env $CFG_ENV INLINE_TUI_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $(realpath "$BIN")"
tmux new-session -d -s "$S22" -x 80 -y 24 -c "$ATDIR" "$APP_ABS"
sleep 0.4
tmux send-keys -t "$S22" -l "see @alpha"
at_open=""
for _ in $(seq 1 30); do # up to ~3s: the worker walks + ranks, the picker shows
	at_open="$(tmux capture-pane -t "$S22" -p)"
	if printf '%s' "$at_open" | grep -qF "alpha_smoke.txt"; then
		break
	fi
	sleep 0.1
done
echo "==== captured pane (@ file picker open) ===="
printf '%s\n' "$at_open"
# Enter accepts the highlighted file: the `@alpha` token becomes the path + a space.
tmux send-keys -t "$S22" Enter
sleep 0.3
at_inserted="$(tmux capture-pane -t "$S22" -p)"
echo "==== captured pane (file path inserted into the composer) ===="
printf '%s\n' "$at_inserted"
tmux kill-session -t "$S22" 2>/dev/null
rm -rf "$ATDIR"

# --- Phase 26: a large BRACKETED PASTE collapses to a compact
# "[Pasted Content N chars]" placeholder in the composer instead of dumping the
# raw text (docs/paste.md). term::init enables bracketed paste, so tmux's
# `paste-buffer -p` (which wraps the buffer in the ESC[200~ … ESC[201~ control
# codes) is delivered as one Event::Paste — unlike Phase 6's `send-keys -l`
# burst, which is real keystrokes typed verbatim and stays unaffected. ---
S23="${S}_paste"
tmux new-session -d -s "$S23" -x 80 -y 24 "$APP"
sleep 0.4
PASTE_CONTENT="$(printf 'P%.0s' $(seq 1 1500))" # 1500 chars, over the 1000 threshold
tmux set-buffer -- "$PASTE_CONTENT"
tmux paste-buffer -p -t "$S23"
paste_pane=""
for _ in $(seq 1 20); do # up to ~2s for the placeholder to render
	paste_pane="$(tmux capture-pane -t "$S23" -p)"
	if printf '%s' "$paste_pane" | grep -qF "[Pasted Content 1500 chars]"; then
		break
	fi
	sleep 0.1
done
echo "==== captured pane (large bracketed paste → placeholder) ===="
printf '%s\n' "$paste_pane"
# A single Backspace removes the WHOLE placeholder atomically (docs/paste.md) —
# not one of its ~27 characters. After one keystroke the composer is empty again.
tmux send-keys -t "$S23" BSpace
paste_backspaced=""
for _ in $(seq 1 20); do # up to ~2s for the redraw
	paste_backspaced="$(tmux capture-pane -t "$S23" -p)"
	if ! printf '%s' "$paste_backspaced" | grep -qF "[Pasted Content"; then
		break
	fi
	sleep 0.1
done
echo "==== captured pane (after one Backspace — placeholder gone) ===="
printf '%s\n' "$paste_backspaced"
tmux kill-session -t "$S23" 2>/dev/null

# --- Phase 27: Ctrl+V image paste (docs/image-paste.md). The happy path needs a
# real clipboard server (covered by the attach_image unit tests, which call it
# with a path directly — codex tests it the same way). Here — headless, with no
# clipboard — Ctrl+V must fail GRACEFULLY: a red "Failed to paste image" notice
# commits to scrollback, and the composer stays responsive afterwards (no hang,
# no crash). This exercises the key binding + the boundary error path. ---
S24="${S}_imagepaste"
tmux new-session -d -s "$S24" -x 80 -y 24 "$APP"
sleep 0.4
tmux send-keys -t "$S24" C-v
imgpaste=""
for _ in $(seq 1 40); do # up to ~4s (the first clipboard probe can stall briefly)
	imgpaste="$(tmux capture-pane -t "$S24" -p -S -20)"
	if printf '%s' "$imgpaste" | grep -qF "Failed to paste image"; then
		break
	fi
	sleep 0.1
done
echo "==== captured pane (Ctrl+V with no clipboard → graceful notice) ===="
printf '%s\n' "$imgpaste"
# Still responsive after the failed paste: typing into the composer still works.
tmux send-keys -t "$S24" -l "still alive"
sleep 0.3
imgalive="$(tmux capture-pane -t "$S24" -p)"
echo "==== captured pane (composer responsive after the failed paste) ===="
printf '%s\n' "$imgalive"
tmux kill-session -t "$S24" 2>/dev/null

# --- Phase 28: /copy copies the last assistant response to the clipboard
# (docs/copy.md). Headless here, so arboard has no clipboard server and the
# OSC 52 fallback fires — which tmux (set-clipboard on) captures into its paste
# buffer, so `show-buffer` reads it back. First an empty conversation: /copy
# reports "No agent response to copy". Then after a reply finishes, /copy writes
# the reply's tail to the clipboard and confirms "Copied last message to
# clipboard". ---
S25="${S}_copy"
tmux new-session -d -s "$S25" -x 80 -y 24 "$APP"
# tmux must capture OSC 52 from the app into its own buffer (default is
# `external`: forward-only, not stored) — `on` stores it so show-buffer sees it.
tmux set-option -g set-clipboard on
sleep 0.4
# Empty conversation: nothing to copy → the red "No agent response to copy".
tmux send-keys -t "$S25" -l "/copy"
sleep 0.3
tmux send-keys -t "$S25" Enter
copy_empty=""
for _ in $(seq 1 30); do # up to ~3s
	copy_empty="$(tmux capture-pane -t "$S25" -p -S -20)"
	if printf '%s' "$copy_empty" | grep -qF "No agent response to copy"; then
		break
	fi
	sleep 0.1
done
echo "==== captured pane (/copy with nothing to copy) ===="
printf '%s\n' "$copy_empty"
# Now send a message and let the dummy reply finish (its tail "changes size"
# committed AND the screen settled, so the final segment is in history).
tmux send-keys -t "$S25" -l "hello there"
sleep 0.2
tmux send-keys -t "$S25" Enter
copy_prev=""
for _ in $(seq 1 60); do # up to ~12s
	copy_cur="$(tmux capture-pane -t "$S25" -p)"
	if printf '%s' "$copy_cur" | grep -qF "changes size" && [ "$copy_cur" = "$copy_prev" ]; then
		break
	fi
	copy_prev="$copy_cur"
	sleep 0.2
done
# Clear any pre-existing tmux paste buffers so show-buffer reflects *our* copy.
while tmux delete-buffer 2>/dev/null; do :; done
tmux send-keys -t "$S25" -l "/copy"
sleep 0.3
tmux send-keys -t "$S25" Enter
copy_ok=""
for _ in $(seq 1 30); do # up to ~3s
	copy_ok="$(tmux capture-pane -t "$S25" -p -S -20)"
	if printf '%s' "$copy_ok" | grep -qF "Copied last message to clipboard"; then
		break
	fi
	sleep 0.1
done
copy_clip="$(tmux show-buffer 2>/dev/null)"
echo "==== captured pane (after /copy) ===="
printf '%s\n' "$copy_ok"
echo "==== tmux clipboard buffer (the OSC 52 fallback landed here) ===="
printf '%s\n' "$copy_clip"
tmux kill-session -t "$S25" 2>/dev/null

# --- Phase 29: the QUEUE follows into the Ctrl+O overlay (docs/queue.md). While
# turn 1 streams, a message queued mid-turn shows in the transcript view as the
# inline strip's inset "  ❯ world" row; when turn 1 ends UNDER the overlay the
# queue auto-dispatches (codex's turn-end drain runs regardless of its Ctrl+T
# view): the overlay gains the real column-0 "❯ world" user entry and turn 2
# runs to its "Finished for" summary — all without leaving the overlay. The
# Ctrl+O return then repaints the inline conversation with both turns. ---
S29="${S}_queueoverlay"
tmux new-session -d -s "$S29" -x 80 -y 24 "$APP"
sleep 0.4
tmux send-keys -t "$S29" -l "hello there"
sleep 0.2
tmux send-keys -t "$S29" Enter
for _ in $(seq 1 40); do # up to ~4s: wait until turn 1 is visibly streaming
	if tmux capture-pane -t "$S29" -p | grep -qF "Happy"; then
		break
	fi
	sleep 0.1
done
tmux send-keys -t "$S29" -l "world"
sleep 0.2
tmux send-keys -t "$S29" Enter # streaming → queued, not submitted
tmux send-keys -t "$S29" C-o   # open the transcript view mid-stream
queued_overlay=""
for _ in $(seq 1 20); do # up to ~3s: the queued row shows inside the overlay
	queued_overlay="$(tmux capture-pane -t "$S29" -p)"
	if printf '%s' "$queued_overlay" | grep -qF "T R A N S C R I P T" &&
		printf '%s' "$queued_overlay" | grep -qF "  ❯ world"; then
		break
	fi
	sleep 0.15
done
echo "==== captured overlay (queued message shown while turn 1 streams) ===="
printf '%s\n' "$queued_overlay"
# Turn 1 ends under the overlay → the queue dispatches right there: "world"
# becomes a real transcript user entry and turn 2 streams to its summary.
overlay_advanced=""
for _ in $(seq 1 150); do # up to ~22s: turn 1 finishes, then turn 2 completes
	overlay_advanced="$(tmux capture-pane -t "$S29" -p)"
	if printf '%s' "$overlay_advanced" | grep -qF "Finished for"; then
		break
	fi
	sleep 0.15
done
echo "==== captured overlay (queued turn dispatched under the overlay) ===="
printf '%s\n' "$overlay_advanced"
# The view is pinned to the bottom, so the dispatched "❯ world" user entry has
# scrolled off the visible pane; jump Home and page down (the pager's jump/page
# keys) until it scrolls into view.
tmux send-keys -t "$S29" Home
sleep 0.2
overlay_world=""
for _ in $(seq 1 12); do
	overlay_world="$(tmux capture-pane -t "$S29" -p)"
	if printf '%s' "$overlay_world" | grep -qE '^❯ world'; then
		break
	fi
	tmux send-keys -t "$S29" PageDown
	sleep 0.2
done
echo "==== captured overlay (scrolled to the dispatched user entry) ===="
printf '%s\n' "$overlay_world"
tmux send-keys -t "$S29" C-o # return: the inline view repaints from history
sleep 0.6
queue_overlay_returned="$(tmux capture-pane -t "$S29" -p -S -80)"
echo "==== captured pane (inline view after returning from the overlay) ===="
printf '%s\n' "$queue_overlay_returned"
tmux kill-session -t "$S29" 2>/dev/null

# --- Phase 30: Esc-Esc BACKTRACK (docs/backtrack.md). After two finished
# exchanges, the first idle Esc ARMS the gesture (the footer slot shows the
# "esc again to edit previous message" hint instead of quitting), the second
# opens the transcript overlay as a preview (backtrack key hints replace the
# quit hint), a further Esc steps the highlight to the OLDER user message, and
# Enter REWINDS: back inline, the conversation truncated from that message on
# (here: everything — it was the first), its text back in the composer to
# edit. Resubmitting it must stream a fresh turn to its summary ("Completed
# for" — turn 3's done verb), proving the loop survived the rewind. ---
S30="${S}_backtrack"
tmux new-session -d -s "$S30" -x 80 -y 24 "$APP"
sleep 0.4
tmux send-keys -t "$S30" -l "alpha question"
sleep 0.2
tmux send-keys -t "$S30" Enter
for _ in $(seq 1 80); do # up to ~12s: turn 1 runs to its "Done for" summary
	if tmux capture-pane -t "$S30" -p -S -40 | grep -qF "Done for"; then
		break
	fi
	sleep 0.15
done
tmux send-keys -t "$S30" -l "beta question"
sleep 0.2
tmux send-keys -t "$S30" Enter
for _ in $(seq 1 80); do # turn 2 → "Finished for"
	if tmux capture-pane -t "$S30" -p -S -40 | grep -qF "Finished for"; then
		break
	fi
	sleep 0.15
done
tmux send-keys -t "$S30" Escape # arm the gesture
sleep 0.3
backtrack_armed="$(tmux capture-pane -t "$S30" -p)"
echo "==== captured pane (first Esc — backtrack armed, hint in the footer slot) ===="
printf '%s\n' "$backtrack_armed"
tmux send-keys -t "$S30" Escape # open the transcript preview
sleep 0.4
backtrack_preview="$(tmux capture-pane -t "$S30" -p)"
echo "==== captured pane (second Esc — transcript preview with backtrack hints) ===="
printf '%s\n' "$backtrack_preview"
tmux send-keys -t "$S30" Escape # step older: "beta question" → "alpha question"
sleep 0.3
tmux send-keys -t "$S30" Enter # confirm the rewind
sleep 0.6
backtrack_rewound="$(tmux capture-pane -t "$S30" -p)"
echo "==== captured pane (Enter — rewound, the first message back in the composer) ===="
printf '%s\n' "$backtrack_rewound"
tmux send-keys -t "$S30" Enter # resubmit the recalled draft
backtrack_resent=""
for _ in $(seq 1 80); do # turn 3 → "Completed for"
	backtrack_resent="$(tmux capture-pane -t "$S30" -p -S -40)"
	if printf '%s' "$backtrack_resent" | grep -qF "Completed for"; then
		break
	fi
	sleep 0.15
done
echo "==== captured pane (rewound message resubmitted — a fresh turn streamed) ===="
printf '%s\n' "$backtrack_resent"
tmux kill-session -t "$S30" 2>/dev/null

# --- Phase 31: /resume SESSION RECORDING + PICKER (docs/resume.md). Every
# conversation records to a rollout JSONL file under INLINE_TUI_SESSIONS_DIR
# (created lazily on the first user message — the session_meta line first). A
# second launch's /resume opens the full-screen session picker (the
# slash-tiled R E S U M E title, a "Type to search" line, the saved session's
# `❯ {age} {preview}` row); Enter loads the conversation back inline — the old
# exchange repainted from the file — and a follow-up turn APPENDS to the SAME
# file (still exactly one rollout). /clear then starts a FRESH file: the next
# message must land in a second rollout, the resumed one untouched. ---
S31="${S}_resume"
RESUME_DIR="$(mktemp -d /tmp/inline-tui-smoke-sessions-XXXXXX)"
RAPP="env $CFG_ENV INLINE_TUI_SESSIONS_DIR=$RESUME_DIR INLINE_TUI_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux new-session -d -s "$S31" -x 80 -y 24 "$RAPP"
sleep 0.4
tmux send-keys -t "$S31" -l "$USER_MSG"
sleep 0.2
tmux send-keys -t "$S31" Enter
for _ in $(seq 1 80); do # instance 1, turn 1 → "Done for"
	if tmux capture-pane -t "$S31" -p -S -40 | grep -qF "Done for"; then
		break
	fi
	sleep 0.15
done
tmux send-keys -t "$S31" C-c # quit instance 1 (empty composer)
sleep 0.4
tmux kill-session -t "$S31" 2>/dev/null
resume_files_after_one="$(find "$RESUME_DIR" -type f -name 'rollout-*.jsonl' | wc -l | tr -d ' ')"
resume_first_file="$(find "$RESUME_DIR" -type f -name 'rollout-*.jsonl' | head -1)"
resume_file_head="$(head -c 300 "$resume_first_file" 2>/dev/null)"
echo "==== recorded rollout head (instance 1's session file) ===="
printf '%s\n' "$resume_file_head"
tmux new-session -d -s "$S31" -x 80 -y 24 "$RAPP"
sleep 0.4
tmux send-keys -t "$S31" -l "/resume"
sleep 0.3
tmux send-keys -t "$S31" Enter # run the palette's highlighted /resume
sleep 0.6
resume_picker="$(tmux capture-pane -t "$S31" -p)"
echo "==== captured pane (/resume — the session picker on the alt screen) ===="
printf '%s\n' "$resume_picker"
tmux send-keys -t "$S31" Enter # resume the highlighted session
sleep 0.8
resume_loaded="$(tmux capture-pane -t "$S31" -p)"
echo "==== captured pane (Enter — the saved conversation repainted inline) ===="
printf '%s\n' "$resume_loaded"
tmux send-keys -t "$S31" -l "again please"
sleep 0.2
tmux send-keys -t "$S31" Enter
resume_appended=""
for _ in $(seq 1 80); do # the follow-up turn (this process's turn 1 → "Done for" #2)
	resume_appended="$(tmux capture-pane -t "$S31" -p -S -40)"
	if [ "$(printf '%s' "$resume_appended" | grep -cF "Done for")" -ge 2 ]; then
		break
	fi
	sleep 0.15
done
echo "==== captured pane (follow-up turn on the resumed session) ===="
printf '%s\n' "$resume_appended"
resume_files_after_append="$(find "$RESUME_DIR" -type f -name 'rollout-*.jsonl' | wc -l | tr -d ' ')"
resume_appended_tail="$(tail -c 2000 "$resume_first_file" 2>/dev/null)"
tmux send-keys -t "$S31" -l "/clear"
sleep 0.3
tmux send-keys -t "$S31" Enter
sleep 0.4
tmux send-keys -t "$S31" -l "fresh session"
sleep 0.2
tmux send-keys -t "$S31" Enter
for _ in $(seq 1 80); do # post-/clear turn (this process's turn 2 → "Finished for")
	if tmux capture-pane -t "$S31" -p -S -40 | grep -qF "Finished for"; then
		break
	fi
	sleep 0.15
done
sleep 0.3
resume_files_after_clear="$(find "$RESUME_DIR" -type f -name 'rollout-*.jsonl' | wc -l | tr -d ' ')"
tmux kill-session -t "$S31" 2>/dev/null

# --- Phase 32: an Esc interrupt stays PROMPT even when the backend is slow to
# observe the cancel (docs/interrupt.md — the interrupt-lag fix). INLINE_TUI_STALL_MS
# selects a test backend that ignores the cancel for N ms, modelling a real
# network backend wedged in a blocking read during the pre-first-token pause. The
# old loop did `cancel + join`, which blocks the single-threaded loop until the
# thread unwinds — freezing the whole UI for ~N ms; the fix detaches the thread
# and swaps the reply channel, so Esc settles within a frame. The stall backend
# streams NOTHING before the stall, so Esc UNDOES the turn (req 1: no output yet)
# — no `Conversation interrupted` notice — and the signal that the loop stayed
# responsive is the status line (its `esc to interrupt` hint) clearing promptly:
# a join()ing loop keeps it up ~N ms, a detaching loop clears it in tens of ms. ---
STALL_S="${S}_stall"
STALL_MS=3000
tmux new-session -d -s "$STALL_S" -x 80 -y 24 "env $CFG_ENV INLINE_TUI_STALL_MS=$STALL_MS $BIN"
sleep 0.4
tmux send-keys -t "$STALL_S" -l "hello there"
sleep 0.2
tmux send-keys -t "$STALL_S" Enter
# Wait for the pre-first-token status window (the stall backend sends nothing yet).
stall_streaming=0
for _ in $(seq 1 40); do # up to ~2s
	if tmux capture-pane -t "$STALL_S" -p | grep -qF "esc to interrupt"; then
		stall_streaming=1
		break
	fi
	sleep 0.05
done
# Esc, then measure how long the live status line takes to clear (the undo's
# prompt settle — no notice commits for a no-output interrupt).
stall_t0="$(date +%s.%N)"
tmux send-keys -t "$STALL_S" Escape
stall_gap=""
for _ in $(seq 1 250); do # up to ~5s (well past the 3s stall)
	if ! tmux capture-pane -t "$STALL_S" -p | grep -qF "esc to interrupt"; then
		stall_gap="$(awk "BEGIN{printf \"%.3f\", $(date +%s.%N) - $stall_t0}")"
		break
	fi
	sleep 0.02
done
echo "==== Phase 32: interrupt latency under a ${STALL_MS}ms stalled backend ===="
echo "stall_streaming=$stall_streaming stall_gap=${stall_gap:-none}"
stall_after="$(tmux capture-pane -t "$STALL_S" -p -S -30)"
printf '%s\n' "$stall_after"
tmux kill-session -t "$STALL_S" 2>/dev/null

# --- Phase 33: transient TOASTS above the box (docs/toast.md). A one-line status
# message that self-clears after a few seconds instead of committing a scrollback
# bullet. /copy's confirmation is a toast — it APPEARS then VANISHES (a committed
# message would persist). /help and /resume run mid-turn surface a toast rejection
# rather than a bulletpoint; /model now OPENS its inline picker mid-turn (it only
# swaps the composer, never the running turn). A 2s startup delay widens the
# pre-stream pause so the turn is reliably active when the mid-turn keys land. ---
S33="${S}_toast"
tmux new-session -d -s "$S33" -x 80 -y 24 "env $CFG_ENV INLINE_TUI_STARTUP_DELAY_MS=2000 $BIN"
sleep 0.4
# Get a finished reply into history so /copy has something to confirm.
tmux send-keys -t "$S33" -l "hello there"
sleep 0.2
tmux send-keys -t "$S33" Enter
toast_prev=""
for _ in $(seq 1 90); do # up to ~18s: reply committed AND the screen settled
	toast_cur="$(tmux capture-pane -t "$S33" -p)"
	if printf '%s' "$toast_cur" | grep -qF "changes size" && [ "$toast_cur" = "$toast_prev" ]; then
		break
	fi
	toast_prev="$toast_cur"
	sleep 0.2
done
# /copy → the confirmation toast appears above the box.
tmux send-keys -t "$S33" -l "/copy"
sleep 0.3
tmux send-keys -t "$S33" Enter
toast_shown=""
for _ in $(seq 1 20); do # up to ~2s
	toast_shown="$(tmux capture-pane -t "$S33" -p)"
	if printf '%s' "$toast_shown" | grep -qF "Copied last message to clipboard"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 33: pane with the /copy toast shown ===="
printf '%s\n' "$toast_shown"
# Wait past the ~4s TTL: the toast self-clears (a committed bullet would remain).
sleep 5
toast_cleared="$(tmux capture-pane -t "$S33" -p)"
echo "==== Phase 33: pane ~5s later (the toast has cleared) ===="
printf '%s\n' "$toast_cleared"
# Mid-turn: start a turn, then during its 2s pre-stream pause run /help — its
# command list would interleave with the reply, so it's rejected with a toast.
tmux send-keys -t "$S33" -l "second question"
sleep 0.2
tmux send-keys -t "$S33" Enter
sleep 0.3 # inside the 2s pause: the turn is active
tmux send-keys -t "$S33" -l "/help"
sleep 0.3
tmux send-keys -t "$S33" Enter
toast_help=""
for _ in $(seq 1 15); do # up to ~1.5s (still inside the pause)
	toast_help="$(tmux capture-pane -t "$S33" -p)"
	if printf '%s' "$toast_help" | grep -qF "/help is disabled while a task is in progress"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 33: pane with the mid-turn /help toast ===="
printf '%s\n' "$toast_help"
# Still mid-turn (same active turn): /model OPENS its inline picker rather than
# being blocked. No provider key is configured in the smoke env, so the picker
# opens on its needs-login hint — proof it opened rather than being rejected.
tmux send-keys -t "$S33" -l "/model"
sleep 0.3
tmux send-keys -t "$S33" Enter
toast_model=""
for _ in $(seq 1 15); do # up to ~1.5s
	toast_model="$(tmux capture-pane -t "$S33" -p)"
	if printf '%s' "$toast_model" | grep -qF "run /login to add one"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 33: pane with /model opened mid-turn ===="
printf '%s\n' "$toast_model"
tmux kill-session -t "$S33" 2>/dev/null

# --- Phase 34: the hardware cursor is HIDDEN for the whole of a redraw so a
# terminal cursor-trail animation (kitty) can't streak across the screen when a
# reflow drags the cursor around — the fix for "the cursor animation starts on
# top" on a resize (term.rs hides the cursor right after opening the frame's
# synchronized update and reshows it only at the prompt seat). RECORD THE RAW
# OUTPUT STREAM (pipe-pane, like Phase 15) across a resize: the Purge reflow
# homes the cursor to the top (ESC[H) and clears the screen (ESC[2J/ESC[3J), so
# the frame must emit a Hide (ESC[?25l) BEFORE that home/clear and a Show
# (ESC[?25h) after. Pre-fix the reflow only ever Showed the cursor at the end, so
# no ESC[?25l ever rode the resize frame → the visible cursor got dragged to the
# top → the trail. ---
S34="${S}_curhide"
RAW34="$(mktemp)"
tmux new-session -d -s "$S34" -x 80 -y 24 "$APP"
sleep 0.5
tmux send-keys -t "$S34" -l "hello there"
sleep 0.2
tmux send-keys -t "$S34" Enter
curhide_done=0
for _ in $(seq 1 80); do # up to ~8s for the committed summary (a seated box)
	if tmux capture-pane -t "$S34" -p | grep -qE "^Done for [0-9]+s"; then
		curhide_done=1
		break
	fi
	sleep 0.1
done
# Record raw app output, then resize twice (width AND height → the Purge reflow
# rebuilds from history, homing the cursor to the top first).
tmux pipe-pane -t "$S34" -o "cat > $RAW34"
sleep 0.2
tmux resize-window -t "$S34" -x 100 -y 30
sleep 0.6
tmux resize-window -t "$S34" -x 72 -y 20
sleep 0.6
tmux pipe-pane -t "$S34" # stop recording
# Tokenise each cursor Hide/Show and each screen home/clear onto its own line
# (NR = stream order), then reason about the ordering: at least one Hide, the
# first Hide before the first home/clear, and a Show after the Hide.
cursor_hide_tokens=$(sed \
	-e $'s/\x1b\[?25l/\\\n@HIDE@\\\n/g' \
	-e $'s/\x1b\[?25h/\\\n@SHOW@\\\n/g' \
	-e $'s/\x1b\[2J/\\\n@CLR@\\\n/g' \
	-e $'s/\x1b\[3J/\\\n@CLR@\\\n/g' \
	-e $'s/\x1b\[H/\\\n@HOME@\\\n/g' \
	"$RAW34" | awk '
	/@HIDE@/ { hide++; if (first_hide == 0) first_hide = NR; next }
	/@SHOW@/ { show++; last_show = NR; next }
	/@CLR@/  { clr++;  if (first_clr  == 0) first_clr  = NR; next }
	/@HOME@/ { home++; if (first_home == 0) first_home = NR; next }
	END {
		earliest = first_clr
		if (first_home > 0 && (earliest == 0 || first_home < earliest)) earliest = first_home
		before = (hide > 0 && earliest > 0 && first_hide < earliest) ? 1 : 0
		shown  = (show > 0 && (hide == 0 || first_hide < last_show)) ? 1 : 0
		printf "ch_hide=%d ch_show=%d ch_clrhome=%d ch_before=%d ch_shown=%d",
			hide + 0, show + 0, (clr + home) + 0, before, shown
	}')
rm -f "$RAW34"
tmux kill-session -t "$S34" 2>/dev/null
echo "==== Phase 34: resize reflow raw-stream cursor tokens — $cursor_hide_tokens ===="
eval "$cursor_hide_tokens"

# --- Phase 35: a Ctrl+O round trip MID-STREAM keeps the already-streamed partial
# reply on the restored screen. The dummy pauses ~1.2s in its thinking phase right
# after streaming the first half of the reply — a deterministic window where the
# streaming buffer is non-empty and NO chunk will arrive to repair the screen.
# Pre-fix, the return repainted from history alone, so the partial's committed
# rows vanished until the next chunk re-inserted the whole partial from scratch —
# the disappear-then-flicker bug (and, on long replies, duplicated rows already
# scrolled into the terminal's kept scrollback). ---
S35="${S}_midstream"
tmux new-session -d -s "$S35" -x 80 -y 24 "$APP"
sleep 0.5
tmux send-keys -t "$S35" -l "hello there"
sleep 0.2
tmux send-keys -t "$S35" Enter
# Wait for the thinking pause (the status gains "Thinking for") — the first half
# of the reply has streamed, and its completed rows are committed, by then.
midstream_thinking=0
for _ in $(seq 1 60); do # up to ~6s
	if tmux capture-pane -t "$S35" -p | grep -qF "Thinking for"; then
		midstream_thinking=1
		break
	fi
	sleep 0.1
done
tmux send-keys -t "$S35" C-o
sleep 0.15
tmux send-keys -t "$S35" C-o # straight back, well inside the thinking pause
sleep 0.15
midstream_returned="$(tmux capture-pane -t "$S35" -p)"
echo "==== Phase 35: returned from Ctrl+O mid-stream (thinking pause still open) ===="
printf '%s\n' "$midstream_returned"
# Let the turn finish, then check the reply committed exactly ONCE — the return's
# catch-up must not re-insert rows the screen already holds.
midstream_done=""
for _ in $(seq 1 80); do # up to ~8s
	midstream_done="$(tmux capture-pane -t "$S35" -p -S -80)"
	if printf '%s' "$midstream_done" | grep -qE "^Done for [0-9]+s"; then
		break
	fi
	sleep 0.1
done
midstream_dupes=$(printf '%s\n' "$midstream_done" | grep -cF "Happy to help")
tmux kill-session -t "$S35" 2>/dev/null

# --- Phase 36: Ctrl+D opens the CONTEXT-DEBUG view (docs/context.md) — the raw
# LLM context window on the alternate screen: role-tagged entries with the
# conversation verbatim and tool calls in the provider-native form (an assistant
# `→ name(args)` request + a `tool:` result entry, the shape a real backend
# sends), q returns to the conversation. ---
S36="${S}_ctxdebug"
tmux new-session -d -s "$S36" -x 80 -y 24 "$APP"
sleep 0.5
tmux send-keys -t "$S36" -l "hello there"
sleep 0.2
tmux send-keys -t "$S36" Enter
for _ in $(seq 1 80); do # let the turn finish so the tools are in history
	if tmux capture-pane -t "$S36" -p | grep -qE "^Done for [0-9]+s"; then
		break
	fi
	sleep 0.1
done
tmux send-keys -t "$S36" C-d
sleep 0.4
tmux send-keys -t "$S36" Home # the view opens at the bottom; jump to the top
sleep 0.3
ctxdebug_pane="$(tmux capture-pane -t "$S36" -p)"
echo "==== Phase 36: Ctrl+D context-debug view ===="
printf '%s\n' "$ctxdebug_pane"
tmux send-keys -t "$S36" q # closes the view
sleep 0.4
ctxdebug_returned="$(tmux capture-pane -t "$S36" -p)"
tmux kill-session -t "$S36" 2>/dev/null

# --- Phase 37: the input history PERSISTS across sessions (docs/history-persistence.md).
# Submit a distinctive message in one process — written to an isolated
# INLINE_TUI_HISTORY_FILE — then launch a SECOND process against the same file:
# ↑ recalls the previous session's message and Ctrl+R finds it. Proves both the
# ↑/↓ recall and the Ctrl+R search span sessions (the seed loads the file into
# InputHistory::entries, which both read). ---
HISTFILE="$(mktemp -u /tmp/inline-tui-smoke-hist-XXXXXX).jsonl"
HAPP="env $CFG_ENV INLINE_TUI_HISTORY_FILE=$HISTFILE INLINE_TUI_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
PMSG="persist_across_sessions_42"
S37="${S}_persist"
tmux new-session -d -s "$S37" -x 80 -y 24 "$HAPP"
sleep 0.5
tmux send-keys -t "$S37" -l "$PMSG"
sleep 0.2
tmux send-keys -t "$S37" Enter
# The submit is flushed to the history file at the next loop tick — poll the
# file (this file's own rule: poll, don't fixed-sleep) rather than guessing.
persist_written=""
for _ in $(seq 1 40); do # up to ~4s
	if [ -f "$HISTFILE" ] && grep -qF "$PMSG" "$HISTFILE"; then
		persist_written="yes"
		break
	fi
	sleep 0.1
done
persist_file_contents="$(cat "$HISTFILE" 2>/dev/null)"
echo "==== Phase 37: history file after session A ===="
printf '%s\n' "$persist_file_contents"
tmux send-keys -t "$S37" C-c # empty composer → quit session A
sleep 0.3
tmux kill-session -t "$S37" 2>/dev/null

# Corrupt the file's tail with an invalid-UTF-8 line (a torn/interleaved append
# leaves such bytes): the lossy load must skip ONLY this line, not discard the
# whole history — so session B's recall below still works. With the old
# read_to_string load this single bad byte wiped all persisted history.
printf '\377\376 torn-not-valid-utf8\n' >>"$HISTFILE"

# Session B: a fresh process against the SAME history file.
S37B="${S}_persist_b"
tmux new-session -d -s "$S37B" -x 80 -y 24 "$HAPP"
sleep 0.5
tmux send-keys -t "$S37B" Up # recall from the persisted (cross-session) history
sleep 0.3
persist_recall="$(tmux capture-pane -t "$S37B" -p)"
echo "==== Phase 37: session B — Up recalls the persisted message ===="
printf '%s\n' "$persist_recall"
tmux send-keys -t "$S37B" C-c # clears the recalled draft (non-empty composer)
sleep 0.2
tmux send-keys -t "$S37B" C-r # open reverse-i-search
sleep 0.2
tmux send-keys -t "$S37B" -l "persist_across"
sleep 0.3
persist_search="$(tmux capture-pane -t "$S37B" -p)"
echo "==== Phase 37: session B — Ctrl+R finds the persisted message ===="
printf '%s\n' "$persist_search"
tmux send-keys -t "$S37B" Escape
sleep 0.2
tmux kill-session -t "$S37B" 2>/dev/null
rm -f "$HISTFILE"

# Exactly one input box on a captured screen: one bare prompt row (the composer's
# `❯` — trailing blanks are trimmed by capture-pane; echoed messages are `❯ text`),
# two horizontal rules (the box's frame), one session footer. Phantom stale boxes
# add extras of each.
count_bare_prompts() { printf '%s\n' "$1" | grep -cE '^❯[[:space:]]*$'; }
# (`(─)+` groups the multibyte rule char so the repeat applies to the whole
# UTF-8 sequence even under a byte-wise C locale.)
count_rules() { printf '%s\n' "$1" | grep -cE '^(─)+$'; }
count_footers() { printf '%s\n' "$1" | grep -cF 'dummy_model_name ·'; }

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
# The session-context footer ("{model} · {cwd}", docs/footer.md) sits under the
# box from startup — including mid-stream, when this pane was captured. The cwd
# half varies by environment, so assert on the model name + separator.
if ! printf '%s' "$pane" | grep -qF "dummy_model_name ·"; then
	echo "FAIL: the session footer ('dummy_model_name · …') is not shown under the input box" >&2
	status=1
fi
# The cursor stays visible ON THE PROMPT ROW while the reply streams (probed
# live during Phase 1): the box keeps focus mid-turn so typing/queueing has a
# blinking cursor, codex-style. Before the fix the cursor was hidden
# (cursor_flag=0) for the whole turn.
if [ "$cursor_mid_flag" != "1" ]; then
	echo "FAIL: the hardware cursor is hidden while the reply streams (cursor_flag=$cursor_mid_flag)" >&2
	status=1
fi
case "$cursor_mid_row" in
"❯"*) ;;
*)
	echo "FAIL: mid-stream the cursor is not on the input's prompt row: '$cursor_mid_row'" >&2
	status=1
	;;
esac
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
# "T R A N S C R I P T" is unique to the overlay's pager title row (it never
# appears in the conversation), so it's a clean marker for "open / closed".
if ! printf '%s' "$overlay" | grep -qF "T R A N S C R I P T"; then
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
if printf '%s' "$returned" | grep -qF "T R A N S C R I P T"; then
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
# The open palette DISPLACES the session footer (codex's popups take its row).
if printf '%s' "$palette_open" | grep -qF "dummy_model_name"; then
	echo "FAIL: the session footer is still shown while the palette is open (the band must displace it)" >&2
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
if ! printf '%s' "$band_open" | grep -qF "alt+↑ to edit queue"; then
	echo "FAIL: the shortcuts band is missing the alt+↑ queue-edit entry" >&2
	status=1
fi
if printf '%s' "$band_closed" | grep -qF "for commands"; then
	echo "FAIL: a second '?' did not close the shortcuts band" >&2
	status=1
fi
# The shortcuts band displaces the session footer too; dismissing it brings the
# footer back (same slot, codex's shortcut-overlay behaviour).
if printf '%s' "$band_open" | grep -qF "dummy_model_name"; then
	echo "FAIL: the session footer is still shown while the shortcuts band is open" >&2
	status=1
fi
if ! printf '%s' "$band_closed" | grep -qF "dummy_model_name ·"; then
	echo "FAIL: the session footer did not return after the shortcuts band closed" >&2
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
# Phase 12: a message submitted mid-stream is queued (shown like a user message,
# inset two columns — "  ❯ world" — above the box, while turn 1 still streams)
# and auto-sent as its own turn when the first finishes (docs/queue.md).
if ! printf '%s' "$queued_band" | grep -qF "  ❯ world"; then
	echo "FAIL: a message submitted while streaming was not shown queued (two-space inset '  ❯ world') above the box while turn 1 streamed" >&2
	status=1
fi
if ! printf '%s' "$queued_band" | grep -qF "  ❯ again"; then
	echo "FAIL: the second queued message was not shown ('  ❯ again' missing) — is the queued display capped?" >&2
	status=1
fi
if ! printf '%s' "$queue_done" | grep -qF "❯ world" ||
	! printf '%s' "$queue_done" | grep -qF "❯ again"; then
	echo "FAIL: the queued backlog was never sent — '❯ world' and '❯ again' did not both reach scrollback" >&2
	status=1
fi
if ! printf '%s' "$queue_done" | grep -qF "Finished for"; then
	echo "FAIL: the queued backlog did not run as the second turn — no 'Finished for' summary" >&2
	status=1
fi
if printf '%s' "$queue_done" | grep -qF "Completed for"; then
	echo "FAIL: the two queued messages ran as separate turns ('Completed for' = a third turn) — the backlog must batch into ONE next turn" >&2
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
# Phase 14: Alt+Up restores only the LAST batch into the composer. The box shows
# "❯ again" (the Tab batch pulled back), the earlier "world" batch stays queued
# (its "  ❯ world" inset row remains), and the pulled "again" inset row is gone.
if ! printf '%s' "$altup" | grep -qF "❯ again"; then
	echo "FAIL: Alt+Up did not restore the last batch into the composer ('❯ again' draft line missing)" >&2
	status=1
fi
if ! printf '%s' "$altup" | grep -qF "  ❯ world"; then
	echo "FAIL: Alt+Up pulled the earlier 'world' batch too — it should have stayed queued ('  ❯ world' inset row missing)" >&2
	status=1
fi
if printf '%s' "$altup" | grep -qF "  ❯ again"; then
	echo "FAIL: the pulled 'again' batch is still shown queued after Alt+Up restored it into the composer" >&2
	status=1
fi
# Phase 21: TAB queues a SEPARATE follow-up turn (docs/queue.md). While turn 1
# streams, "world" (Enter) and "later" (Tab) both show inset above the box; then
# "world" runs as turn 2 and "later" as a SEPARATE turn 3, so "Completed for"
# (turn 3's done verb) MUST appear — unlike Phase 12's batched backlog, which
# asserts the opposite.
if ! printf '%s' "$tabqueue_band" | grep -qF "  ❯ world"; then
	echo "FAIL: the Enter-queued 'world' was not shown inset above the box while turn 1 streamed" >&2
	status=1
fi
if ! printf '%s' "$tabqueue_band" | grep -qF "  ❯ later"; then
	echo "FAIL: the Tab-queued 'later' was not shown inset above the box — did Tab fail to queue?" >&2
	status=1
fi
if ! printf '%s' "$tabqueue" | grep -qF "❯ world" ||
	! printf '%s' "$tabqueue" | grep -qF "❯ later"; then
	echo "FAIL: the queued messages never reached scrollback — '❯ world' and '❯ later' did not both commit" >&2
	status=1
fi
if ! printf '%s' "$tabqueue" | grep -qF "Completed for"; then
	echo "FAIL: the Tab-queued 'later' did not run as a SEPARATE third turn ('Completed for' = turn 3 missing) — Tab must open a new follow-up batch, not merge like Enter" >&2
	status=1
fi
# Phase 15: flicker-free commits (docs/flicker.md). The recorded turn must have
# committed lines (so the clear-the-region path actually ran), and every one of
# those clears must sit inside a synchronized-update block — a clear outside
# means a terminal could present the boxless state (the streaming blink).
case "$sync_clears" in
total=0*)
	echo "FAIL: the recorded turn shows no live-region clears — the commit path did not run (recording broken?)" >&2
	status=1
	;;
esac
case "$sync_clears" in
*outside=0) ;;
*)
	echo "FAIL: live-region clears OUTSIDE a synchronized-update frame ($sync_clears) — scrollback commits can flicker the box" >&2
	status=1
	;;
esac
# Phase 16: /clear mid-turn killed the generation. Right after the clear the
# screen holds no trace of the turn …
if printf '%s' "$cleared_now" | grep -qF "❯ hello there"; then
	echo "FAIL: the old conversation ('❯ hello there') survived a mid-turn /clear" >&2
	status=1
fi
# … and the SCROLLBACK is purged too (codex's ED3), not just the visible screen:
# scrolling up after /clear must show nothing of the old turn.
if printf '%s' "$cleared_scrollback" | grep -qF "❯ hello there"; then
	echo "FAIL: /clear did not purge scrollback — the old conversation ('❯ hello there') is still one scroll up (the ED3 purge is missing)" >&2
	status=1
fi
if printf '%s' "$cleared_now" | grep -qF "esc to interrupt"; then
	echo "FAIL: the live status line is still up after a mid-turn /clear" >&2
	status=1
fi
if printf '%s' "$cleared_now" | grep -qF "Conversation interrupted"; then
	echo "FAIL: /clear recorded the interrupt notice — it must wipe, not interrupt" >&2
	status=1
fi
if ! printf '%s' "$cleared_now" | grep -qF "dummy_model_name ·"; then
	echo "FAIL: the idle input box + footer did not reseat after a mid-turn /clear" >&2
	status=1
fi
# … and the backend is dead: nothing recommitted while the remainder of the
# turn's schedule played out (reply text, tool peeks, a summary).
for leak in "Happy" "⎿" "Done for" "esc to interrupt"; do
	if printf '%s' "$cleared_later" | grep -qF "$leak"; then
		echo "FAIL: the backend kept streaming after a mid-turn /clear ('$leak' appeared on the cleared screen)" >&2
		status=1
	fi
done
# … and the loop survived the kill: the fresh turn streamed to completion.
if ! printf '%s' "$after_clear" | grep -qF "❯ again please"; then
	echo "FAIL: the message sent after a mid-turn /clear was not echoed" >&2
	status=1
fi
if ! printf '%s' "$after_clear" | grep -qF "Finished for"; then
	echo "FAIL: the turn after a mid-turn /clear did not finish (no 'Finished for' summary)" >&2
	status=1
fi
# Phase 17: every resize re-presents the conversation at the new size. Each
# captured screen must hold exactly one input box — phantom boxes (extra bare
# prompts / rules / footers) are the stale-viewport-row bug — with the
# conversation tail (the turn's committed summary) still in view, and no stale
# streaming strip ("esc to interrupt" rides the live status line only).
for step in shrunk regrown mid; do
	case "$step" in
	shrunk)
		cap="$resize_shrunk"
		label="height-only shrink to 80x12"
		tail_marker="Done for"
		;;
	regrown)
		cap="$resize_regrown"
		label="height grow back to 80x24"
		tail_marker="Done for"
		;;
	mid)
		cap="$resize_mid"
		label="mid-stream height shrink to 80x14"
		tail_marker="Finished for"
		;;
	esac
	prompts="$(count_bare_prompts "$cap")"
	rules="$(count_rules "$cap")"
	footers="$(count_footers "$cap")"
	if [ "$prompts" != "1" ] || [ "$rules" != "2" ] || [ "$footers" != "1" ]; then
		echo "FAIL: after the $label the screen does not hold exactly one input box (bare prompts=$prompts, rules=$rules, footers=$footers)" >&2
		status=1
	fi
	if ! printf '%s' "$cap" | grep -qF "$tail_marker"; then
		echo "FAIL: after the $label the conversation tail ('$tail_marker') is not in view" >&2
		status=1
	fi
	if printf '%s' "$cap" | grep -qF "esc to interrupt"; then
		echo "FAIL: after the $label a stale streaming status line is still on screen" >&2
		status=1
	fi
done
# The full conversation fits again once the screen regrows: the repaint must
# rebuild the *whole* tail from history, not just the rows the shrunken screen
# showed.
if ! printf '%s' "$resize_regrown" | grep -qF "❯ $USER_MSG"; then
	echo "FAIL: after growing back to 80x24 the user message did not return to view" >&2
	status=1
fi
if ! printf '%s' "$resize_regrown" | grep -qF "$EXPECT_REPLY"; then
	echo "FAIL: after growing back to 80x24 the reply did not return to view" >&2
	status=1
fi
# No duplication: after a WIDTH resize the whole pane (visible + scrollback) must
# hold the user message EXACTLY once. The old in-place overwrite left the
# emulator's own reflowed copy behind on a width change — the TUI-text
# duplication this fix targets; the purge-mode rebuild keeps it single.
resize_dup="$(printf '%s\n' "$resize_narrow_full" | grep -cF "❯ $USER_MSG")"
if [ "$resize_dup" != "1" ]; then
	echo "FAIL: after a width resize the conversation is duplicated ('❯ $USER_MSG' ×$resize_dup, expected 1) — the reflowed copy was not cleared" >&2
	status=1
fi

# Phase 18: the Ctrl+R reverse history search. "❯ alpha bravo" appears once on
# screen from the committed turn, so the composer holding it shows up as a
# SECOND occurrence (the never-submitted "charlie alpha" is unambiguous).
count_msg_lines() { printf '%s\n' "$1" | grep -cF "❯ $2"; }
if ! printf '%s' "$search_open" | grep -qF "reverse-i-search:"; then
	echo "FAIL: Ctrl+R did not open the reverse-i-search line" >&2
	status=1
fi
if ! printf '%s' "$search_match" | grep -qF "reverse-i-search: alpha"; then
	echo "FAIL: the search line does not show the typed query" >&2
	status=1
fi
if [ "$(count_msg_lines "$search_match" "charlie alpha")" != "1" ]; then
	echo "FAIL: the newest match (the Ctrl+C-cleared draft) did not preview in the composer" >&2
	status=1
fi
if [ "$(count_msg_lines "$search_older" "alpha bravo")" != "2" ]; then
	echo "FAIL: Ctrl+R again did not step the preview to the older match" >&2
	status=1
fi
if printf '%s' "$search_accept" | grep -qF "reverse-i-search:"; then
	echo "FAIL: accepting with Enter did not close the search line" >&2
	status=1
fi
if [ "$(count_msg_lines "$search_accept" "alpha bravo")" != "2" ]; then
	echo "FAIL: the accepted match did not stay in the composer as a draft" >&2
	status=1
fi
if ! printf '%s' "$search_accept" | grep -qF "dummy_model_name ·"; then
	echo "FAIL: the session footer did not return once the search closed" >&2
	status=1
fi
if ! printf '%s' "$search_nomatch" | grep -qF "no match"; then
	echo "FAIL: a hopeless query does not show 'no match'" >&2
	status=1
fi
if [ "$(count_msg_lines "$search_nomatch" "alpha bravo")" != "2" ]; then
	echo "FAIL: the draft was not restored while the query has no match" >&2
	status=1
fi
if printf '%s' "$search_cancel" | grep -qF "reverse-i-search:"; then
	echo "FAIL: Esc did not close the search" >&2
	status=1
fi
if ! printf '%s' "$search_cancel" | grep -qF "dummy_model_name ·"; then
	echo "FAIL: the app quit on Esc instead of only closing the search" >&2
	status=1
fi
if [ "$(count_msg_lines "$search_cancel" "alpha bravo")" != "2" ]; then
	echo "FAIL: Esc-cancel did not keep the restored draft" >&2
	status=1
fi

# Phase 19: `!` shell commands.
if ! printf '%s' "$shell_mode" | grep -qF "Shell mode"; then
	echo "FAIL: typing a !command did not show the 'Shell mode' footer hint" >&2
	status=1
fi
if ! printf '%s' "$shell_mode" | grep -qE "^! echo smoke_shell_ok"; then
	echo "FAIL: the composer does not absorb the bang into a '! cmd' prompt" >&2
	status=1
fi
if printf '%s' "$shell_mode" | grep -qF "❯ !echo"; then
	echo "FAIL: the composer still shows the bang as text ('❯ !echo …')" >&2
	status=1
fi
if ! printf '%s' "$shell_ran" | grep -qE "^! echo smoke_shell_ok"; then
	echo "FAIL: the committed cell is missing its '! echo …' header line" >&2
	status=1
fi
if ! printf '%s' "$shell_ran" | grep -qF "⎿ smoke_shell_ok"; then
	echo "FAIL: the shell command's ⎿ output line is not in view" >&2
	status=1
fi
if printf '%s' "$shell_ran" | grep -qE "● echo|^Ran for"; then
	echo "FAIL: the old '● cmd' tool header / 'Ran for' summary resurfaced" >&2
	status=1
fi
if ! printf '%s' "$shell_fail" | grep -qF "exit status: 3"; then
	echo "FAIL: a failing !command did not report its non-zero exit status" >&2
	status=1
fi
# Req 3: a running !command shows the `⎿ Running…` preview with its elapsed and
# NO spinner status line (its elapsed rides the preview instead).
if ! printf '%s' "$shell_running" | grep -qE "⎿ Running… \([0-9]+s\)"; then
	echo "FAIL: a running !command did not show the '⎿ Running… (Ns)' preview" >&2
	status=1
fi
if printf '%s' "$shell_running" | grep -qF "esc to interrupt"; then
	echo "FAIL: a running !command showed the spinner status line — req 3: shell hides it (the elapsed rides the ⎿ Running… preview)" >&2
	status=1
fi
# Req 2: Esc resolves the shell cell `⎿ Interrupted by user` — and does NOT
# commit the redundant `Conversation interrupted` notice (the cell is the record).
if ! printf '%s' "$shell_interrupt" | grep -qF "Interrupted by user"; then
	echo "FAIL: Esc did not resolve a long-running !command as '⎿ Interrupted by user'" >&2
	status=1
fi
if printf '%s' "$shell_interrupt" | grep -qF "Conversation interrupted"; then
	echo "FAIL: a shell interrupt committed the redundant 'Conversation interrupted' notice — req 2: the ⎿ cell is the record" >&2
	status=1
fi

# Phase 20: the pre-stream pause shows the status indicator with the input
# counted as ↑ tokens, before any reply text.
if ! printf '%s' "$delay_pause" | grep -qF "esc to interrupt"; then
	echo "FAIL: the status indicator is not visible during the pre-stream pause" >&2
	status=1
fi
if ! printf '%s' "$delay_pause" | grep -qE "↑ [0-9]+ tokens"; then
	echo "FAIL: the just-sent user message is not counted as '↑ N tokens' during the pause" >&2
	status=1
fi
if printf '%s' "$delay_pause" | grep -qF "$DELAY_REPLY"; then
	echo "FAIL: the reply streamed during the pause (the startup delay did not hold)" >&2
	status=1
fi
if ! printf '%s' "$delay_reply" | grep -qF "$DELAY_REPLY"; then
	echo "FAIL: the reply never streamed after the pause (a hang, not a delay)" >&2
	status=1
fi
if ! printf '%s' "$delay_reply" | grep -qE "↓ [0-9]+ tokens"; then
	echo "FAIL: the arrow did not flip to ↓ once the reply started streaming" >&2
	status=1
fi

# Phase 22: a huge !output is capped in memory (no temp file), `…` marks the cut.
if ! printf '%s' "$bigoutput" | grep -qF "⎿ 1"; then
	echo "FAIL: a huge !output did not render its retained head (the '⎿ 1' first line)" >&2
	status=1
fi
if ! printf '%s' "$bigoutput" | grep -qF "ctrl+o to expand"; then
	echo "FAIL: a huge !output did not show the '+N lines (ctrl+o to expand)' peek hint" >&2
	status=1
fi
if [ "${bigoutput_tmpfiles:-x}" != "0" ]; then
	echo "FAIL: a huge !output wrote a temp file ($bigoutput_tmpfiles found) — output must be capped in memory, not saved" >&2
	status=1
fi
if ! printf '%s' "$bigoutput_overlay" | grep -qF "…"; then
	echo "FAIL: the Ctrl+O view did not append a '…' truncation marker for the capped output" >&2
	status=1
fi

# Phase 23: Ctrl+J grows the box (the universal newline fallback, docs/shift-enter.md).
if ! printf '%s' "$ctrlj_grown" | grep -qF "❯ CCC"; then
	echo "FAIL: first draft line '❯ CCC' not shown in the input box after Ctrl+J" >&2
	status=1
fi
if ! printf '%s' "$ctrlj_grown" | grep -qF "  DDD"; then
	echo "FAIL: Ctrl+J did not insert a newline — indented continuation '  DDD' missing (the box did not grow)" >&2
	status=1
fi
# A plain Enter then submits the whole multi-line draft to scrollback.
if ! printf '%s' "$ctrlj_sent" | grep -qF "❯ CCC"; then
	echo "FAIL: a plain Enter did not submit the Ctrl+J multi-line draft ('❯ CCC' not committed)" >&2
	status=1
fi

# Phase 24: a !command queued mid-turn runs LOCALLY as its own turn (docs/queue.md).
# While turn 1 streams, the text "world" (❯, a model turn) and the command
# "! echo …" (the red shell prompt) both show inset above the box; then the
# command commits an exec cell (⎿ output), proving it ran locally — NOT a
# "❯ !echo …" user message sent to the backend (the old v1 limitation).
if ! printf '%s' "$shellqueue_band" | grep -qF "  ❯ world"; then
	echo "FAIL: the Enter-queued 'world' was not shown inset above the box while turn 1 streamed" >&2
	status=1
fi
if ! printf '%s' "$shellqueue_band" | grep -qF "  ! echo smoke_queue_ok"; then
	echo "FAIL: the mid-turn !command did not queue as an inset '! echo …' shell entry (the red bang prompt)" >&2
	status=1
fi
if ! printf '%s' "$shellqueue" | grep -qE "^! echo smoke_queue_ok"; then
	echo "FAIL: the queued !command did not commit its '! echo …' exec-cell header — did it run locally?" >&2
	status=1
fi
if ! printf '%s' "$shellqueue" | grep -qF "⎿ smoke_queue_ok"; then
	echo "FAIL: the queued !command produced no '⎿' output cell — it was not run locally as its own turn" >&2
	status=1
fi
if printf '%s' "$shellqueue" | grep -qF "❯ !echo smoke_queue_ok"; then
	echo "FAIL: the queued !command was sent to the backend as literal text ('❯ !echo …') — the old v1 limitation, not run locally" >&2
	status=1
fi

# Phase 25: the `@` file picker (docs/file-search.md). Typing "@alpha" lists the
# matching workspace file below the box; Enter inserts its path into the composer.
if ! printf '%s' "$at_open" | grep -qF "alpha_smoke.txt"; then
	echo "FAIL: typing '@alpha' did not list the matching file in the picker below the box" >&2
	status=1
fi
# The picker displaces the session footer (codex's popups take its row), like the palette.
if printf '%s' "$at_open" | grep -qF "dummy_model_name"; then
	echo "FAIL: the session footer is still shown while the @ file picker is open (the band must displace it)" >&2
	status=1
fi
if ! printf '%s' "$at_inserted" | grep -qF "❯ see alpha_smoke.txt"; then
	echo "FAIL: Enter did not insert the highlighted file path into the composer (expected '❯ see alpha_smoke.txt')" >&2
	status=1
fi
# The picker closed on accept: the session footer returns to its row.
if ! printf '%s' "$at_inserted" | grep -qF "dummy_model_name ·"; then
	echo "FAIL: the session footer did not return after the @ file picker closed on accept" >&2
	status=1
fi
# Phase 26: a large bracketed paste collapses to the "[Pasted Content N chars]"
# placeholder in the composer rather than dumping the raw text (docs/paste.md).
if ! printf '%s' "$paste_pane" | grep -qF "[Pasted Content 1500 chars]"; then
	echo "FAIL: a large bracketed paste did not collapse to the '[Pasted Content 1500 chars]' placeholder in the composer" >&2
	status=1
fi
if printf '%s' "$paste_pane" | grep -qE 'P{20,}'; then
	echo "FAIL: the raw pasted text was dumped into the composer instead of the placeholder" >&2
	status=1
fi
# … and a single Backspace removes the whole placeholder atomically (one
# keystroke, not one of its characters — docs/paste.md).
if printf '%s' "$paste_backspaced" | grep -qF "[Pasted Content"; then
	echo "FAIL: one Backspace did not remove the whole '[Pasted Content …]' placeholder (atomic delete regressed)" >&2
	status=1
fi
# Phase 27: Ctrl+V with no clipboard fails gracefully — a red "Failed to paste
# image" notice — and the composer stays responsive afterwards (docs/image-paste.md).
if ! printf '%s' "$imgpaste" | grep -qF "Failed to paste image"; then
	echo "FAIL: Ctrl+V with no clipboard did not show a 'Failed to paste image' notice" >&2
	status=1
fi
if ! printf '%s' "$imgalive" | grep -qF "still alive"; then
	echo "FAIL: the composer was unresponsive after a failed image paste (Ctrl+V hung or crashed the app)" >&2
	status=1
fi
# Phase 28: /copy reports the empty case, confirms the copy, and the OSC 52
# fallback actually reached the clipboard — tmux's buffer holds the reply tail
# (docs/copy.md).
if ! printf '%s' "$copy_empty" | grep -qF "No agent response to copy"; then
	echo "FAIL: /copy with no assistant message did not show 'No agent response to copy'" >&2
	status=1
fi
if ! printf '%s' "$copy_ok" | grep -qF "Copied last message to clipboard"; then
	echo "FAIL: /copy did not confirm with 'Copied last message to clipboard'" >&2
	status=1
fi
if ! printf '%s' "$copy_clip" | grep -qF "changes size"; then
	echo "FAIL: /copy's OSC 52 fallback did not put the reply on the clipboard (tmux buffer missing the reply tail)" >&2
	status=1
fi

# Phase 29: the queued message follows into the Ctrl+O overlay and dispatches
# there when turn 1 ends — the overlay never hides (or freezes) the queue.
if ! printf '%s' "$queued_overlay" | grep -qF "  ❯ world"; then
	echo "FAIL: the queued message row ('  ❯ world') was missing from the Ctrl+O transcript view" >&2
	status=1
fi
if ! printf '%s' "$overlay_advanced" | grep -qF "T R A N S C R I P T"; then
	echo "FAIL: the overlay was not still open when the queued turn dispatched" >&2
	status=1
fi
if ! printf '%s' "$overlay_world" | grep -qE '^❯ world'; then
	echo "FAIL: the queued message was not dispatched under the overlay (no column-0 '❯ world' user entry found via Home/PageDown)" >&2
	status=1
fi
if ! printf '%s' "$overlay_advanced" | grep -qF "Finished for"; then
	echo "FAIL: the queued turn never ran to its 'Finished for' summary under the overlay" >&2
	status=1
fi
if ! printf '%s' "$queue_overlay_returned" | grep -qE '^❯ world'; then
	echo "FAIL: after returning from the overlay the dispatched 'world' turn is missing from the inline view" >&2
	status=1
fi
if ! printf '%s' "$queue_overlay_returned" | grep -qF "Finished for"; then
	echo "FAIL: after returning from the overlay turn 2's summary is missing from the inline view" >&2
	status=1
fi

# Phase 30: Esc-Esc backtrack — arm, preview, step, rewind, resubmit.
if ! printf '%s' "$backtrack_armed" | grep -qF "esc again to edit previous message"; then
	echo "FAIL: the first idle Esc did not show the backtrack hint in the footer slot (did the app quit?)" >&2
	status=1
fi
if ! printf '%s' "$backtrack_preview" | grep -qF "T R A N S C R I P T"; then
	echo "FAIL: the second Esc did not open the transcript overlay as the backtrack preview" >&2
	status=1
fi
if ! printf '%s' "$backtrack_preview" | grep -qF "enter to edit message"; then
	echo "FAIL: the preview's key-hint row does not show the backtrack hints" >&2
	status=1
fi
if ! printf '%s' "$backtrack_rewound" | grep -qF "❯ alpha question"; then
	echo "FAIL: after Enter the composer does not hold the rewound first message" >&2
	status=1
fi
if printf '%s' "$backtrack_rewound" | grep -qF "beta question"; then
	echo "FAIL: the second exchange survived the rewind on the repainted screen" >&2
	status=1
fi
if ! printf '%s' "$backtrack_resent" | grep -qF "Completed for"; then
	echo "FAIL: resubmitting the rewound message never streamed to turn 3's 'Completed for' summary" >&2
	status=1
fi

# Phase 31: /resume — record a session, pick it, load it, append to it.
if [ "$resume_files_after_one" != "1" ]; then
	echo "FAIL: instance 1 should have recorded exactly one rollout file, found $resume_files_after_one" >&2
	status=1
fi
if ! printf '%s' "$resume_file_head" | grep -qF '"type":"session_meta"'; then
	echo "FAIL: the rollout file does not open with a session_meta line" >&2
	status=1
fi
if ! printf '%s' "$resume_picker" | grep -qF "R E S U M E"; then
	echo "FAIL: /resume did not open the session picker (no slash-tiled R E S U M E title)" >&2
	status=1
fi
if ! printf '%s' "$resume_picker" | grep -qF "Type to search"; then
	echo "FAIL: the picker's search line placeholder is missing" >&2
	status=1
fi
if ! printf '%s' "$resume_picker" | grep -qF "Filter: [Cwd] All"; then
	echo "FAIL: the picker's Filter toolbar tab pair is missing (or not defaulting to Cwd)" >&2
	status=1
fi
if ! printf '%s' "$resume_picker" | grep -qF "Sort: [Updated] Created"; then
	echo "FAIL: the picker's Sort toolbar tab pair is missing (or not defaulting to Updated)" >&2
	status=1
fi
if ! printf '%s' "$resume_picker" | grep -qF "ago"; then
	echo "FAIL: the picker shows no humanized session age" >&2
	status=1
fi
if ! printf '%s' "$resume_picker" | grep -qF "❯" || ! printf '%s' "$resume_picker" | grep -qF "$USER_MSG"; then
	echo "FAIL: the saved session's preview row ('❯ {age} $USER_MSG') is not listed" >&2
	status=1
fi
if ! printf '%s' "$resume_loaded" | grep -qF "❯ $USER_MSG"; then
	echo "FAIL: resuming did not repaint the saved user message inline" >&2
	status=1
fi
if ! printf '%s' "$resume_loaded" | grep -qF "$EXPECT_REPLY"; then
	echo "FAIL: resuming did not repaint the saved assistant reply inline" >&2
	status=1
fi
if ! printf '%s' "$resume_appended" | grep -qF "❯ again please"; then
	echo "FAIL: the follow-up turn on the resumed session never ran" >&2
	status=1
fi
if [ "$resume_files_after_append" != "1" ]; then
	echo "FAIL: the follow-up turn should append to the SAME rollout file, found $resume_files_after_append files" >&2
	status=1
fi
if ! printf '%s' "$resume_appended_tail" | grep -qF "again please"; then
	echo "FAIL: the resumed rollout file did not gain the follow-up message" >&2
	status=1
fi
if [ "$resume_files_after_clear" != "2" ]; then
	echo "FAIL: /clear should start a fresh rollout file (expected 2 files, found $resume_files_after_clear)" >&2
	status=1
fi

# Phase 32: an Esc interrupt is prompt even when the backend is slow to observe
# the cancel — the loop detaches the thread instead of join()ing it, so the UI
# never freezes (docs/interrupt.md, the interrupt-lag fix). The stall backend
# streams nothing, so Esc UNDOES the turn (req 1) — the promptness signal is the
# status line clearing, and the undo restores the message with no notice.
if [ "$stall_streaming" -ne 1 ]; then
	echo "FAIL: Phase 32 never reached the streaming status line under the stalled backend" >&2
	status=1
fi
if [ -z "$stall_gap" ]; then
	echo "FAIL: Phase 32 status line never cleared after Esc under the stalled backend" >&2
	status=1
elif awk "BEGIN{exit !($stall_gap > 1.5)}"; then
	# 1.5s is a generous ceiling — half the 3s stall. A join()ing loop lands near
	# 3s; the detaching fix lands in tens of ms. Anything over 1.5s means the loop
	# is blocking on the backend thread again (the interrupt-lag regression).
	echo "FAIL: Phase 32 Esc took ${stall_gap}s to settle (>1.5s) — the loop is blocking on the backend (join(), not detach)" >&2
	status=1
fi
# The no-output interrupt undoes the submission: the message returns to the
# composer and NO `Conversation interrupted` notice is committed (req 1).
if ! printf '%s' "$stall_after" | grep -qF "hello there"; then
	echo "FAIL: Phase 32 undo did not restore 'hello there' to the composer" >&2
	status=1
fi
if printf '%s' "$stall_after" | grep -qF "Conversation interrupted"; then
	echo "FAIL: Phase 32 committed a 'Conversation interrupted' notice — a no-output interrupt must undo, not notify (req 1)" >&2
	status=1
fi

# Phase 33: the /copy confirmation shows as a transient toast that self-clears.
if ! printf '%s' "$toast_shown" | grep -qF "Copied last message to clipboard"; then
	echo "FAIL: Phase 33 /copy did not show the 'Copied last message to clipboard' toast" >&2
	status=1
fi
if printf '%s' "$toast_cleared" | grep -qF "Copied last message to clipboard"; then
	echo "FAIL: Phase 33 the toast did NOT self-clear after its TTL (a transient toast must vanish, not persist like a scrollback bullet)" >&2
	status=1
fi
# Phase 33: /help run mid-turn is rejected with a toast, not committed.
if ! printf '%s' "$toast_help" | grep -qF "/help is disabled while a task is in progress"; then
	echo "FAIL: Phase 33 mid-turn /help did not show the disabled toast" >&2
	status=1
fi
# Phase 33: /model OPENS its inline picker mid-turn (no rejection).
if ! printf '%s' "$toast_model" | grep -qF "run /login to add one"; then
	echo "FAIL: Phase 33 mid-turn /model did not open the inline picker (it should no longer be blocked while a task runs)" >&2
	status=1
fi

# Phase 34: a resize reflow hides the hardware cursor before homing/clearing the
# screen (so kitty's cursor-trail can't streak from the top) and reshows it at
# the prompt seat. Read from the RAW output stream recorded across the resize.
if [ "${curhide_done:-0}" -ne 1 ]; then
	echo "FAIL: Phase 34 precondition — the turn never finished, the resize reflow was never probed" >&2
	status=1
fi
if [ "${ch_clrhome:-0}" -eq 0 ]; then
	echo "FAIL: Phase 34 precondition — the resize produced no screen home/clear in the raw stream (did the reflow run?)" >&2
	status=1
fi
if [ "${ch_hide:-0}" -eq 0 ]; then
	echo "FAIL: Phase 34 the resize reflow never HID the cursor (no ESC[?25l) — kitty's cursor-trail streaks from the top" >&2
	status=1
fi
if [ "${ch_before:-0}" -ne 1 ]; then
	echo "FAIL: Phase 34 the cursor was hidden only AFTER the screen was homed/cleared — the cursor-trail already fired" >&2
	status=1
fi
if [ "${ch_shown:-0}" -ne 1 ]; then
	echo "FAIL: Phase 34 the cursor was not reshown (ESC[?25h) at its prompt seat after the reflow — it would stay hidden" >&2
	status=1
fi

# Phase 35: a mid-stream Ctrl+O round trip keeps the streamed partial visible.
if [ "${midstream_thinking:-0}" -ne 1 ]; then
	echo "FAIL: Phase 35 precondition — the thinking pause was never observed, so the mid-stream return was not probed (retune the timing)" >&2
	status=1
fi
if ! printf '%s' "$midstream_returned" | grep -qF "esc to interrupt"; then
	echo "FAIL: Phase 35 precondition — the turn was no longer in flight when the overlay returned (too slow to probe the bug)" >&2
	status=1
fi
if ! printf '%s' "$midstream_returned" | grep -qF "Happy to help"; then
	echo "FAIL: Phase 35 the streamed partial reply vanished from the restored screen after a mid-stream Ctrl+O round trip (the disappear-then-flicker bug)" >&2
	status=1
fi
if [ "${midstream_dupes:-0}" != "1" ]; then
	echo "FAIL: Phase 35 the reply text appears ${midstream_dupes} times after the turn settled (expected exactly 1 — the overlay catch-up re-inserted rows the screen already had)" >&2
	status=1
fi

# Phase 36: the Ctrl+D context-debug view shows the raw context window.
if ! printf '%s' "$ctxdebug_pane" | grep -qF "C O N T E X T"; then
	echo "FAIL: Phase 36 Ctrl+D did not open the context-debug view (its slash-tiled title is missing)" >&2
	status=1
fi
if ! printf '%s' "$ctxdebug_pane" | grep -qE "^user:"; then
	echo "FAIL: Phase 36 the context view is missing the role-tagged user entry" >&2
	status=1
fi
if ! printf '%s' "$ctxdebug_pane" | grep -qF "hello there"; then
	echo "FAIL: Phase 36 the context view is missing the user message's raw text" >&2
	status=1
fi
if ! printf '%s' "$ctxdebug_pane" | grep -qF 'read({"path":"src/main.rs"})'; then
	echo "FAIL: Phase 36 the context view is missing the native tool call (→ read(...))" >&2
	status=1
fi
if ! printf '%s' "$ctxdebug_pane" | grep -qE "^tool:"; then
	echo "FAIL: Phase 36 the context view is missing the tool-result role entry" >&2
	status=1
fi
if printf '%s' "$ctxdebug_pane" | grep -qF "[tool "; then
	echo "FAIL: Phase 36 the context view still shows the old bracketed tool record" >&2
	status=1
fi
if ! printf '%s' "$ctxdebug_pane" | grep -qF "q/esc/ctrl+d to quit"; then
	echo "FAIL: Phase 36 the context view's key-hint row is missing" >&2
	status=1
fi
if printf '%s' "$ctxdebug_returned" | grep -qF "C O N T E X T"; then
	echo "FAIL: Phase 36 q did not close the context-debug view" >&2
	status=1
fi
if ! printf '%s' "$ctxdebug_returned" | grep -qF "Done for"; then
	echo "FAIL: Phase 36 the conversation did not repaint after closing the context-debug view" >&2
	status=1
fi

# Phase 37: the input history persists across sessions (docs/history-persistence.md).
if [ "$persist_written" != "yes" ]; then
	echo "FAIL: Phase 37 — the submitted message was never written to the history file (append broken)" >&2
	status=1
fi
if ! printf '%s' "$persist_file_contents" | grep -qF "\"text\":\"$PMSG\""; then
	echo "FAIL: Phase 37 — the history file lacks the JSONL record for the submitted message" >&2
	status=1
fi
if ! printf '%s' "$persist_recall" | grep -qF "$PMSG"; then
	echo "FAIL: Phase 37 — Up in a FRESH session did not recall the previous session's message (seed/persistence broken)" >&2
	status=1
fi
if ! printf '%s' "$persist_search" | grep -qF "reverse-i-search:"; then
	echo "FAIL: Phase 37 — Ctrl+R did not open the reverse-i-search line in the fresh session" >&2
	status=1
fi
if ! printf '%s' "$persist_search" | grep -qF "$PMSG"; then
	echo "FAIL: Phase 37 — Ctrl+R search did not find the persisted message in the fresh session" >&2
	status=1
fi

if [ "$status" -eq 0 ]; then
	echo "PASS: reply + tools streamed to scrollback, the cursor stays visible on the prompt row mid-stream, the input box grows and stays flush at the bottom after a reply, typing bursts render in one repaint, Ctrl+O opens the tool-output view, the slash-command palette opens and runs commands, Esc interrupts a streaming turn, Ctrl+C clears a draft before /quit exits, Up recalls the last sent message for resubmission, ? toggles the shortcuts band, messages submitted mid-turn queue (all shown) and batch-send as the next turn (Esc sends the backlog right away, Alt+Up pulls the last batch back to edit, and Tab queues a message as a separate follow-up turn that runs after the first queue, and a !command queued mid-turn runs locally as its own standalone shell turn after — never sent to the backend as text), the session footer ({model} · {cwd}) sits under the box except while a band is open, every scrollback commit clears+repaints the live region inside one synchronized frame (no flicker), /clear mid-turn kills the generation and blanks the screen (nothing streams in afterwards), a resize — height-only included, mid-stream included — re-presents the conversation at the new size with a single input box, Ctrl+R reverse-searches the input history (typed queries preview matches in the composer, Enter accepts, Esc cancels without quitting), and !commands run locally (the bang is absorbed into a '! cmd' prompt with a Shell mode hint, the run commits as a codex-style exec cell — the dark '! cmd' header with its ⎿ output flush below, ⎿ Running… (Ns) while it runs (no spinner status line — the elapsed rides the preview), no summary — a non-zero exit reports its status, Esc interrupts a long one (resolving ⎿ Interrupted by user with no 'Conversation interrupted' notice), multi-line output shows a 4-line ⎿ preview with a '+N lines (ctrl+o to expand)' hint, and a huge output is capped in memory — no temp file, peak RSS bounded — with a '…' truncation marker at the end of the Ctrl+O view), and the dummy AI pauses before streaming so the status indicator shows first — the just-sent user message counted as ↑ tokens during the pause, flipping to ↓ once the reply streams, and Ctrl+J inserts a newline (the universal Shift+Enter fallback) so the box grows and a plain Enter then submits the multi-line draft, and typing @query opens a file picker below the box (async walk+rank) whose Enter inserts the highlighted path into the composer, and a large bracketed paste collapses to a '[Pasted Content N chars]' placeholder in the composer instead of dumping the raw text (and one Backspace removes the whole placeholder atomically), and Ctrl+V pastes a clipboard image as an '[Image #N]' placeholder (here, headless with no clipboard, it fails gracefully with a red 'Failed to paste image' notice and the composer stays responsive), and a message queued mid-turn shows inside the Ctrl+O transcript view and auto-dispatches there when the turn ends (the overlay follows the new turn live), and /copy copies the last assistant response to the clipboard (an empty conversation reports 'No agent response to copy'; after a reply it confirms 'Copied last message to clipboard' and — arboard having no clipboard here — its OSC 52 fallback lands the reply text in tmux's paste buffer), and Esc Esc backtracks to a previous user message (the first idle Esc arms with an 'esc again to edit previous message' footer hint, the second opens the transcript preview whose hint row shows the backtrack keys, a further Esc steps to the older message, and Enter rewinds the conversation to that point with the message back in the composer — resubmitting it streams a fresh turn to its summary), and /resume picks up a saved session (every conversation records to a rollout JSONL file — session_meta line first, created lazily on the first user message — a later launch's /resume lists it in a full-screen picker with a humanized age and the first-user-message preview, Enter repaints the whole saved conversation inline and appends the turns that follow to the same file, /clear starts a fresh rollout so the next message lands in a new one, and the picker carries codex's Filter/Sort toolbar — 'Filter: [Cwd] All   Sort: [Updated] Created' on the search row, Tab + arrows toggling — with the selected row lit on a full-width background tint), and an Esc interrupt stays prompt even when the backend is slow to observe the cancel — under a stalled backend (INLINE_TUI_STALL_MS, ignoring the cancel for 3s) that streamed nothing, Esc undoes the no-output turn and settles within a frame (the status line clears and 'hello there' returns to the composer, no 'Conversation interrupted' notice) because the loop detaches the thread and swaps the reply channel instead of join()ing it (the interrupt-lag fix — no UI freeze), and slash-command confirmations and soft rejections surface as transient toasts above the box that self-clear after a few seconds instead of committing scrollback bullets (/copy confirms with a toast that then vanishes; /help and /resume run mid-turn are rejected with a toast; /model and /login now open their inline pickers mid-turn since they only swap the composer, never the running turn), and a resize reflow hides the hardware cursor before it homes/clears the screen and reshows it only at the prompt seat — so a terminal cursor-trail animation (kitty) can't streak from the top when the redraw drags the cursor around, and a mid-stream Ctrl+O round trip keeps the already-streamed partial reply on the restored screen (the repaint carries the stream's committed rows and catches up on what streamed under the overlay exactly once — no vanish, no flicker, no duplicate), and Ctrl+D opens the full-screen context-debug view showing the raw LLM context window (role-tagged entries, the conversation verbatim, tool calls in the provider-native wire format — an assistant '→ name(args)' request plus a 'tool:' result entry) with q returning to the repainted conversation, and the input history PERSISTS across sessions (a message submitted in one process is written to an append-only history.jsonl and, in a fresh process against the same file, Up recalls it and Ctrl+R finds it — both ↑/↓ recall and reverse-search span sessions like codex)"
fi
exit "$status"
