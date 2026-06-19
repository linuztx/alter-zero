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
	tmux kill-session -t "${S}_tabqueue" 2>/dev/null
	tmux kill-session -t "${S}_bigoutput" 2>/dev/null
	tmux kill-session -t "${S}_ctrlj" 2>/dev/null
	tmux kill-session -t "${S}_shellqueue" 2>/dev/null
	tmux kill-session -t "${S}_atmention" 2>/dev/null
	tmux kill-session -t "${S}_paste" 2>/dev/null
	tmux kill-session -t "${S}_imagepaste" 2>/dev/null
	rm -f /tmp/inline-tui-shell-*.txt 2>/dev/null
	rm -f /tmp/inline-tui-clipboard-*.png 2>/dev/null
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
APP="env INLINE_TUI_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"

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
tmux send-keys -t "$S4" -l "INLINE_TUI_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"
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
tmux send-keys -t "$S12" Escape
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
# Capture the VISIBLE screen only (no -S): the empty-tail reflow clears with
# clear_region(All), which tmux answers by spilling the old frame into its
# scrollback (same as an idle /clear, or a shell `clear`) — the contract here
# is that the screen the user sees is blank.
cleared_now="$(tmux capture-pane -t "$S13" -p)"
echo "==== captured visible screen (right after /clear mid-stream) ===="
printf '%s\n' "$cleared_now"
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
# A long command is interruptible: Esc commits the interrupt notice promptly.
tmux send-keys -t "$S16" -l "!sleep 9"
sleep 0.2
tmux send-keys -t "$S16" Enter
sleep 0.6 # let it start running
tmux send-keys -t "$S16" Escape
shell_interrupt=""
for _ in $(seq 1 40); do # up to ~4s — far less than the 9s sleep
	shell_interrupt="$(tmux capture-pane -t "$S16" -p -S -40)"
	if printf '%s' "$shell_interrupt" | grep -qF "Conversation interrupted"; then
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
tmux new-session -d -s "$S17" -x 80 -y 24 "env INLINE_TUI_STARTUP_DELAY_MS=2000 $BIN"
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
	if printf '%s' "$bigoutput_overlay" | grep -qF "PgUp/PgDn"; then
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
APP_ABS="env INLINE_TUI_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $(realpath "$BIN")"
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
if ! printf '%s' "$shell_interrupt" | grep -qF "Conversation interrupted"; then
	echo "FAIL: Esc did not interrupt a long-running !command" >&2
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

if [ "$status" -eq 0 ]; then
	echo "PASS: reply + tools streamed to scrollback, the cursor stays visible on the prompt row mid-stream, the input box grows and stays flush at the bottom after a reply, typing bursts render in one repaint, Ctrl+O opens the tool-output view, the slash-command palette opens and runs commands, Esc interrupts a streaming turn, Ctrl+C clears a draft before /quit exits, Up recalls the last sent message for resubmission, ? toggles the shortcuts band, messages submitted mid-turn queue (all shown) and batch-send as the next turn (Esc sends the backlog right away, Alt+Up pulls the last batch back to edit, and Tab queues a message as a separate follow-up turn that runs after the first queue, and a !command queued mid-turn runs locally as its own standalone shell turn after — never sent to the backend as text), the session footer ({model} · {cwd}) sits under the box except while a band is open, every scrollback commit clears+repaints the live region inside one synchronized frame (no flicker), /clear mid-turn kills the generation and blanks the screen (nothing streams in afterwards), a resize — height-only included, mid-stream included — re-presents the conversation at the new size with a single input box, Ctrl+R reverse-searches the input history (typed queries preview matches in the composer, Enter accepts, Esc cancels without quitting), and !commands run locally (the bang is absorbed into a '! cmd' prompt with a Shell mode hint, the run commits as a codex-style exec cell — the dark '! cmd' header with its ⎿ output flush below, ⎿ Running… while it runs, no summary — a non-zero exit reports its status, Esc interrupts a long one, multi-line output shows a 4-line ⎿ preview with a '+N lines (ctrl+o to expand)' hint, and a huge output is capped in memory — no temp file, peak RSS bounded — with a '…' truncation marker at the end of the Ctrl+O view), and the dummy AI pauses before streaming so the status indicator shows first — the just-sent user message counted as ↑ tokens during the pause, flipping to ↓ once the reply streams, and Ctrl+J inserts a newline (the universal Shift+Enter fallback) so the box grows and a plain Enter then submits the multi-line draft, and typing @query opens a file picker below the box (async walk+rank) whose Enter inserts the highlighted path into the composer, and a large bracketed paste collapses to a '[Pasted Content N chars]' placeholder in the composer instead of dumping the raw text (and one Backspace removes the whole placeholder atomically), and Ctrl+V pastes a clipboard image as an '[Image #N]' placeholder (here, headless with no clipboard, it fails gracefully with a red 'Failed to paste image' notice and the composer stays responsive)"
fi
exit "$status"
