#!/usr/bin/env bash
# Drive the TUI inside a real terminal (tmux): type a message, let the dummy AI
# stream, then ASSERT the rendered conversation contains the expected lines.
#
# This is the only automated coverage of main.rs (the terminal I/O boundary), so
# it asserts rather than just eyeballing: it polls for the streamed reply (no
# blind fixed sleep racing the 45ms-per-chunk stream) and exits non-zero on
# mismatch, so it can gate in CI or a pre-commit hook.
set -uo pipefail

BIN="${1:-target/debug/alter-zero}"
S="alterzero_smoke_$$"
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
	tmux kill-session -t "${S}_ckbacktrack" 2>/dev/null
	tmux kill-session -t "${S}_ckresume" 2>/dev/null
	tmux kill-session -t "${S}_ckhome" 2>/dev/null
	[ -n "${CK_DIR:-}" ] && rm -rf "$CK_DIR" 2>/dev/null
	[ -n "${CK_SESS:-}" ] && rm -rf "$CK_SESS" 2>/dev/null
	[ -n "${WORK46:-}" ] && rm -rf "$WORK46" 2>/dev/null
	[ -n "${WORK47:-}" ] && rm -rf "$WORK47" 2>/dev/null
	[ -n "${WORK49:-}" ] && rm -rf "$WORK49" 2>/dev/null
	[ -n "${CK49:-}" ] && rm -rf "$CK49" 2>/dev/null
	tmux kill-session -t "${S}_stall" 2>/dev/null
	tmux kill-session -t "${S}_curhide" 2>/dev/null
	tmux kill-session -t "${S}_midstream" 2>/dev/null
	tmux kill-session -t "${S}_batch" 2>/dev/null
	tmux kill-session -t "${S}_tableflush" 2>/dev/null
	tmux kill-session -t "${S}_background" 2>/dev/null
	tmux kill-session -t "${S}_bgkill" 2>/dev/null
	tmux kill-session -t "${S}_notty" 2>/dev/null
	tmux kill-session -t "${S}_header" 2>/dev/null
	tmux kill-session -t "${S}_ctrlofast" 2>/dev/null
	tmux kill-session -t "${S}_compact" 2>/dev/null
	tmux kill-session -t "${S}_autocompact" 2>/dev/null
	tmux kill-session -t "${S}_init" 2>/dev/null
	tmux kill-session -t "${S}_agents" 2>/dev/null
	tmux kill-session -t "${S}_bgagents" 2>/dev/null
	rm -rf /tmp/alter-zero-smoke-init-* 2>/dev/null
	[ -n "${CTRLO_DIR:-}" ] && rm -rf "$CTRLO_DIR" 2>/dev/null
	rm -f /tmp/alter-zero-shell-*.txt 2>/dev/null
	rm -f /tmp/alter-zero-clipboard-*.png 2>/dev/null
	[ -n "${RESUME_DIR:-}" ] && rm -rf "$RESUME_DIR" 2>/dev/null
	[ -n "${SMOKE_CFG:-}" ] && rm -rf "$SMOKE_CFG" 2>/dev/null
	tmux kill-session -t "${S}_permission" 2>/dev/null
	tmux kill-session -t "${S}_permission_amend" 2>/dev/null
	tmux kill-session -t "${S}_permission_flush" 2>/dev/null
	tmux kill-session -t "${S}_parperm" 2>/dev/null
	tmux kill-session -t "${S}_permresize" 2>/dev/null
	tmux kill-session -t "${S}_permoverlay" 2>/dev/null
	tmux kill-session -t "${S}_pulse" 2>/dev/null
	tmux kill-session -t "${S}_cliresume" 2>/dev/null
	[ -n "${CLIR_DIR:-}" ] && rm -rf "$CLIR_DIR" 2>/dev/null
}
trap cleanup EXIT

if [ ! -x "$BIN" ]; then
	echo "FAIL: binary not found at $BIN (run: cargo build)" >&2
	exit 1
fi

# The dummy AI now pauses before streaming (so the status indicator shows
# first) — 3s by default. Run every phase with a SHORT delay so the turns
# stream promptly, threaded through ALTER_ZERO_STARTUP_DELAY_MS; Phase 20
# overrides it back to a visible pause to verify that behaviour. The `env`
# wrapper is robust even when a tmux server is already running (an exported
# var would not reach its panes).
SMOKE_STARTUP_MS="${SMOKE_STARTUP_MS:-200}"
# Isolate the config home (~/.alter-zero by default) to a throwaway dir so the
# dummy stays the backend regardless of any real key/model a developer has saved
# there (a saved config.json + .env key would otherwise activate a real backend
# and break the dummy-based assertions). Cleaned up on exit.
SMOKE_CFG="$(mktemp -d)"
# Disable filesystem checkpoints for EVERY phase that runs in this repo's cwd
# (docs/checkpoint.md): a checkpoint restore does `git reset --hard` + `git
# clean` on the working directory, which — run here — would delete repo files a
# snapshot didn't capture. Only Phases 46-47, which run in a throwaway temp cwd,
# re-enable checkpoints (with ALTER_ZERO_CHECKPOINTS=1 in their own env).
CFG_ENV="ALTER_ZERO_CONFIG_DIR=$SMOKE_CFG ALTER_ZERO_CHECKPOINTS=0"
# Persistence (docs/history-persistence.md) seeds the input history from a file
# on startup. Point the base app at /dev/null so every phase starts with an
# EMPTY input history — the in-session ↑/↓ (Phase 10) and Ctrl+R (Phase 18)
# assertions then behave exactly as before, unaffected by earlier phases'
# submissions. Phase 37 overrides this with a real temp file to test that
# persistence spans sessions.
CFG_ENV_NOHIST="$CFG_ENV ALTER_ZERO_HISTORY_FILE=/dev/null"
APP="env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"

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
# The dummy interleaves tool calls in its reply (a parallel Bash batch, then a
# lone Read); wait until the Read has FINISHED — its committed inline peek shows
# `fn main(`, and only then is its full output in history — before opening the
# view, which must show each tool's FULL output (the Read tool's later lines are
# hidden in the collapsed inline view but shown here). See docs/parallel-tools.md.
for _ in $(seq 1 80); do
	full="$(tmux capture-pane -t "$S" -p -S -80)"
	if printf '%s' "$full" | grep -qF "fn main("; then
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
# The Read tool's later output lines sit deeper in the transcript than the
# first window shows (the header banner + conversation above push them down) —
# page towards them until the expanded output's marker scrolls into view
# (TOOL_VIEW_PAGE < the body height, so consecutive windows overlap and the
# walk can't skip rows).
overlay_deep="$overlay"
for _ in $(seq 1 20); do
	if printf '%s' "$overlay_deep" | grep -qF "InlineViewport::init"; then
		break
	fi
	tmux send-keys -t "$S" NPage
	sleep 0.2
	overlay_deep="$(tmux capture-pane -t "$S" -p)"
done
echo "==== captured pane (Ctrl+O tool-output view, paged to the Read output) ===="
printf '%s\n' "$overlay_deep"
tmux send-keys -t "$S" C-o # back to the conversation
sleep 0.4
returned="$(tmux capture-pane -t "$S" -p)"
echo "==== captured pane (returned to conversation) ===="
printf '%s\n' "$returned"
# The InPlace return must NOT paint a second banner copy on screen when the
# banner already sits in the terminal's kept scrollback (ui::banner_tail
# re-caps it to the window; docs/header.md). Duplication only — presence is
# pinned by Phase 45 at settled moments: this return lands MID-STREAM, and a
# reply that grew past the window under the overlay legitimately moves the
# rebuilt window beyond banner rows that never reached scrollback (the InPlace
# overwrite's lossy edge, docs/header.md's residual note), so 0 is possible.
returned_full="$(tmux capture-pane -t "$S" -p -S -80)"
returned_banner_count=$(printf '%s\n' "$returned_full" | grep -cF "autonomous ai agent")

# --- Phase 38: PARALLEL tool-call batch — the not-yet-run calls show `⎿ Waiting…`
# (docs/parallel-tools.md). A prompt mentioning "parallel" makes the dummy announce
# a three-call `Bash(ping …)` batch up front (the user's example): while the first
# runs, the other two show dim `⎿ Waiting…` cells in the live region above the box,
# the whole batch visible at once. Drive a fresh, tall session and poll for that
# transient state (the batch runs after the pre-stream pause + first text +
# thinking). The default turn (other phases) keeps its compact 2-call batch. ---
S_BATCH="${S}_batch"
tmux new-session -d -s "$S_BATCH" -x 90 -y 40 "$APP"
sleep 0.4
tmux send-keys -t "$S_BATCH" -l "run three pings in parallel"
sleep 0.2
tmux send-keys -t "$S_BATCH" Enter
batch_waiting=""
for _ in $(seq 1 130); do # up to ~20s (pause + first-half text + thinking, then tools)
	cap="$(tmux capture-pane -t "$S_BATCH" -p -S -50)"
	if printf '%s' "$cap" | grep -qF "Waiting…"; then
		batch_waiting="$cap"
		break
	fi
	sleep 0.15
done
echo "==== Phase 38: captured pane (parallel batch — a running Bash(ping) cell + ⎿ Waiting… siblings) ===="
printf '%s\n' "$batch_waiting"

# --- Phase 39: the running Bash(ping) cell TAILS its live output — the last
# lines under the `⎿` gutter and a `+N lines (Ns)` footer (docs/tool-streaming.md),
# Claude-Code's running-command look. The output streams in *after* the Waiting…
# capture above, so keep polling the same batch session until the tail footer
# shows (one of the pings has > TOOL_PEEK_LINES lines, so a tail always appears). ---
batch_tail=""
for _ in $(seq 1 200); do # up to ~20s — the tail streams as each ping runs
	cap="$(tmux capture-pane -t "$S_BATCH" -p -S -60)"
	if printf '%s' "$cap" | grep -qE '\+[0-9]+ lines \([0-9]+s\)'; then
		batch_tail="$cap"
		break
	fi
	sleep 0.1
done
echo "==== Phase 39: captured pane (a running Bash(ping) cell tailing its live output) ===="
printf '%s\n' "$batch_tail"
tmux kill-session -t "$S_BATCH" 2>/dev/null

# --- Phase 40: the Ctrl+O tool-output overlay shows a running bash tool's output
# LIVE (docs/tool-streaming.md) — unlike Claude Code, whose transcript only shows
# tool output once the tool finishes. Open the overlay during a fresh parallel
# turn and poll for the fix-demonstrating state: a Bash(ping) cell RUNNING with
# streamed output (an `icmp_seq` line) while BOTH batch siblings still show
# `⎿ Waiting…`. When two calls are still Waiting, the first is the only active one
# and nothing has committed, so an `icmp_seq` line can ONLY come from the running
# call's live stream — impossible if the overlay were static (it would show a
# frozen `⎿ Running…`). The overlay tail-follows, so the frontier stays in view. ---
S_OVL="${S}_ovl"
tmux new-session -d -s "$S_OVL" -x 100 -y 44 "$APP"
sleep 0.4
tmux send-keys -t "$S_OVL" -l "run three pings in parallel"
sleep 0.2
tmux send-keys -t "$S_OVL" Enter
sleep 0.7 # let the reply text start, then open the overlay before the tools stream
tmux send-keys -t "$S_OVL" C-o
overlay_stream=""
for _ in $(seq 1 250); do # ~15s cap — catches the running call's live-output window
	cap="$(tmux capture-pane -t "$S_OVL" -p)"
	waiting=$(printf '%s\n' "$cap" | grep -c 'Waiting…')
	if [ "$waiting" -ge 2 ] && printf '%s' "$cap" | grep -qE 'icmp_seq'; then
		overlay_stream="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 40: captured pane (Ctrl+O overlay — a running Bash(ping) cell streaming live over ⎿ Waiting… siblings) ===="
printf '%s\n' "$overlay_stream"
tmux kill-session -t "$S_OVL" 2>/dev/null

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
tmux send-keys -t "$S4" -l "$CFG_ENV ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"
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
	if printf '%s' "$shell_ran" | grep -qF "⎿  smoke_shell_ok"; then
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
tmux new-session -d -s "$S17" -x 80 -y 24 "env $CFG_ENV ALTER_ZERO_STARTUP_DELAY_MS=2000 $BIN"
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
rm -f /tmp/alter-zero-shell-*.txt 2>/dev/null
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
bigoutput_tmpfiles="$(ls /tmp/alter-zero-shell-*.txt 2>/dev/null | wc -l | tr -d ' ')"
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
	if printf '%s' "$shellqueue" | grep -qF "⎿  smoke_queue_ok"; then
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
APP_ABS="env $CFG_ENV ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $(realpath "$BIN")"
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
# The rewind PURGES scrollback (docs/backtrack.md — like /resume/resize): the
# dropped exchange must not linger even in the terminal's scrollback (the
# duplication bug where it only cleared on the next resize). Capture WITH
# scrollback and assert "beta question" is gone entirely — the composer holds
# "alpha question", so "beta question" must appear zero times anywhere.
backtrack_rewound_scroll="$(tmux capture-pane -t "$S30" -p -S -120)"
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
# conversation records to a rollout JSONL file under ALTER_ZERO_SESSIONS_DIR
# (created lazily on the first user message — the session_meta line first). A
# second launch's /resume opens the full-screen session picker (the
# slash-tiled R E S U M E title, a "Type to search" line, the saved session's
# `❯ {age} {preview}` row); Enter loads the conversation back inline — the old
# exchange repainted from the file — and a follow-up turn APPENDS to the SAME
# file (still exactly one rollout). /clear then starts a FRESH file: the next
# message must land in a second rollout, the resumed one untouched. ---
S31="${S}_resume"
RESUME_DIR="$(mktemp -d /tmp/alter-zero-smoke-sessions-XXXXXX)"
RAPP="env $CFG_ENV ALTER_ZERO_SESSIONS_DIR=$RESUME_DIR ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
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
# observe the cancel (docs/interrupt.md — the interrupt-lag fix). ALTER_ZERO_STALL_MS
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
tmux new-session -d -s "$STALL_S" -x 80 -y 24 "env $CFG_ENV ALTER_ZERO_STALL_MS=$STALL_MS $BIN"
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
tmux new-session -d -s "$S33" -x 80 -y 24 "env $CFG_ENV ALTER_ZERO_STARTUP_DELAY_MS=2000 $BIN"
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
# ALTER_ZERO_HISTORY_FILE — then launch a SECOND process against the same file:
# ↑ recalls the previous session's message and Ctrl+R finds it. Proves both the
# ↑/↓ recall and the Ctrl+R search span sessions (the seed loads the file into
# InputHistory::entries, which both read). ---
HISTFILE="$(mktemp -u /tmp/alter-zero-smoke-hist-XXXXXX).jsonl"
HAPP="env $CFG_ENV ALTER_ZERO_HISTORY_FILE=$HISTFILE ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
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

# --- Phase 41: a streamed TABLE keeps the box flush at the bottom. The dummy's
# "table" reply streams a 10-row GFM grid with prose after it, so the whole
# block commits in ONE flush at its close while the strip collapses from the
# tall forming-table preview to a single row (docs/table-streaming.md). That
# flush must sync to the collapsed height first — pre-fix it reserved the stale
# taller strip below the grid, so the box rose off the bottom and a blank band
# was left beneath it (the reported bug). ---
S41="${S}_tableflush"
tmux new-session -d -s "$S41" -x 80 -y 24 "$APP"
sleep 0.5
tmux send-keys -t "$S41" -l "table demo"
sleep 0.2
tmux send-keys -t "$S41" Enter
tableflush_pane=""
for _ in $(seq 1 150); do # the ~140-word table reply streams in ~7s
	tableflush_pane="$(tmux capture-pane -t "$S41" -p)"
	if printf '%s' "$tableflush_pane" | grep -qF "properly in Markdown." \
		&& printf '%s' "$tableflush_pane" | grep -qE "^Done for [0-9]+s"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 41: pane after the streamed-table turn ended ===="
printf '%s\n' "$tableflush_pane"
# capture-pane trims trailing blank rows, so judge the footer's ABSOLUTE row
# against the 24-row pane: flush-at-bottom puts it on the last row; the bug
# left it floating ~a strip-height higher with blank rows beneath.
tableflush_footer_row=$(printf '%s\n' "$tableflush_pane" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)
# The EMOJI half of the table bug (docs/table-streaming.md): the demo table's
# Description cells open with a two-column ✅/❌, and every grid row on screen
# must still be exactly as wide as the border rows. Pre-fix the draw paths also
# printed the blank filler cell ratatui reserves *after* a wide grapheme, so an
# emoji row came out one column wider per emoji — the right border stepped out
# of line and, on a table that fills the width, wrapped onto the next row.
# Measured in awk's byte mode (the suite runs in a POSIX locale): map every
# 3-byte box-drawing glyph to ONE ascii byte and each 3-byte emoji to TWO, then
# a byte count is the display width.
grid_row_widths() { # → the distinct display widths of a pane's grid rows
	printf '%s\n' "$1" |
		grep -E '^[[:space:]]*(│|┌|├|└)' |
		sed 's/^[[:space:]]*//' |
		awk '{
			line = $0
			gsub(/│|┌|┐|└|┘|├|┤|┬|┴|┼|─/, "#", line)
			gsub(/✅|❌/, "##", line)
			print length(line)
		}' | sort -u | tr '\n' ' '
}
tableflush_row_widths="$(grid_row_widths "$tableflush_pane")"
tableflush_emoji_rows=$(printf '%s\n' "$tableflush_pane" | grep -cE '^[[:space:]]*│.*(✅|❌)')
# …and the SAME must hold in the Ctrl+O transcript overlay, which paints the
# alternate screen through its own full-cell emitter (`draw_overlay`). Fixing
# only the inline emitters left the overlay — the one view you open to read a
# table in full — still drifting a column per emoji.
tmux send-keys -t "$S41" C-o
sleep 1.2
tmux send-keys -t "$S41" End
sleep 0.8
tableflush_overlay="$(tmux capture-pane -t "$S41" -p)"
echo "==== Phase 41: Ctrl+O transcript overlay over the emoji table ===="
printf '%s\n' "$tableflush_overlay"
tableflush_overlay_widths="$(grid_row_widths "$tableflush_overlay")"
tableflush_overlay_emoji=$(printf '%s\n' "$tableflush_overlay" | grep -cE '^[[:space:]]*│.*(✅|❌)')
tmux kill-session -t "$S41" 2>/dev/null

# --- Phase 42: BACKGROUND SHELLS (docs/background.md). A long `!` command is
# moved to the background with Ctrl+B: its cell resolves to the fixed
# `⎿ Running in the background (↓ to manage)` row, the footer counts
# `· 1 shell`, ↓ LIGHTS THAT COUNT on cyan (the rest of the footer intact, no
# band yet) and Enter opens the manager band (the list page, then Enter opens
# the details page whose output box tails the STILL-STREAMING live output),
# `x` stops the shell — committing the red `was stopped by the user` notice —
# and the manager falls back to the `No tasks currently running` empty state. ---
S42="${S}_background"
# A short command line (the notice headline must fit one row at 80 cols), with
# the slow streamer in a script file.
BG_SCRIPT="$SMOKE_CFG/bg.sh"
cat >"$BG_SCRIPT" <<'EOS'
i=0
while [ $i -lt 400 ]; do
	echo bgline$i
	i=$((i + 1))
	sleep 0.05
done
EOS
tmux new-session -d -s "$S42" -x 80 -y 24 "$APP"
sleep 0.4
tmux send-keys -t "$S42" -l "!sh $BG_SCRIPT"
sleep 0.2
tmux send-keys -t "$S42" Enter
# The hint is DELAYED (TOOL_BACKGROUND_HINT_DELAY, 3s): early in the run the
# command is clearly executing (its `⎿ Running…` row shows) but the Ctrl+B hint
# must NOT be there yet — a command that finishes right away never flashes it.
sleep 0.6
bg_early_pane="$(tmux capture-pane -t "$S42" -p)"
echo "==== Phase 42: early in the run — Running… but no Ctrl+B hint yet ===="
printf '%s\n' "$bg_early_pane"
# The running `!` cell shows the live-only Ctrl+B hint under its Running row —
# but only after the command has run a few seconds, Claude-Code-style, so a
# fast command never flashes it. Poll well past the delay.
bg_hint_pane=""
for _ in $(seq 1 80); do
	bg_hint_pane="$(tmux capture-pane -t "$S42" -p)"
	if printf '%s' "$bg_hint_pane" | grep -qF "(ctrl+b to run in background)"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 42: running ! command with the Ctrl+B hint ===="
printf '%s\n' "$bg_hint_pane"
# Ctrl+B: the runner hands the child to the registry; the cell resolves to the
# fixed backgrounded row and the footer gains the shell count.
tmux send-keys -t "$S42" C-b
bg_cell_pane=""
for _ in $(seq 1 30); do
	bg_cell_pane="$(tmux capture-pane -t "$S42" -p)"
	if printf '%s' "$bg_cell_pane" | grep -qF "Running in the background (↓ to manage)" \
		&& printf '%s' "$bg_cell_pane" | grep -qF "· 1 shell"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 42: cell resolved backgrounded + footer count ===="
printf '%s\n' "$bg_cell_pane"
# ↓ from the empty composer FOCUSES the footer's shell count first — it lights
# up on the palette cyan (SGR `48;2;86;182;194`, captured with -e) while the
# model/cwd segments stay put and no band opens. Enter is what opens it.
tmux send-keys -t "$S42" Down
sleep 0.3
bg_focus_pane="$(tmux capture-pane -t "$S42" -p)"
bg_focus_ansi="$(tmux capture-pane -t "$S42" -p -e)"
echo "==== Phase 42: ↓ lights the footer's shell indicator (no band yet) ===="
printf '%s\n' "$bg_focus_pane"
# Enter steps into the manager band on the list page.
tmux send-keys -t "$S42" Enter
sleep 0.3
bg_list_pane="$(tmux capture-pane -t "$S42" -p)"
echo "==== Phase 42: the ↓ manager list ===="
printf '%s\n' "$bg_list_pane"
# Enter opens the details page; its output box tails the live output.
tmux send-keys -t "$S42" Enter
bg_details_pane=""
for _ in $(seq 1 30); do
	bg_details_pane="$(tmux capture-pane -t "$S42" -p)"
	if printf '%s' "$bg_details_pane" | grep -qF "Shell details" \
		&& printf '%s' "$bg_details_pane" | grep -qF "bgline"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 42: the details page with live output ===="
printf '%s\n' "$bg_details_pane"
# The box follows the stream: the newest visible line number keeps advancing.
bg_seq_before="$(printf '%s\n' "$bg_details_pane" | grep -oE 'bgline[0-9]+' | tail -1)"
bg_seq_after="$bg_seq_before"
for _ in $(seq 1 30); do
	bg_details_later="$(tmux capture-pane -t "$S42" -p)"
	bg_seq_after="$(printf '%s\n' "$bg_details_later" | grep -oE 'bgline[0-9]+' | tail -1)"
	if [ -n "$bg_seq_after" ] && [ "$bg_seq_after" != "$bg_seq_before" ]; then
		break
	fi
	sleep 0.2
done
# `x` stops the shell: the red stopped notice commits and — every shell gone —
# the manager shows its empty state.
tmux send-keys -t "$S42" x
bg_stopped_pane=""
for _ in $(seq 1 40); do
	bg_stopped_pane="$(tmux capture-pane -t "$S42" -p -S -40)"
	if printf '%s' "$bg_stopped_pane" | grep -qF "was stopped by the user" \
		&& printf '%s' "$bg_stopped_pane" | grep -qF "No tasks currently running"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 42: stopped notice + empty manager ===="
printf '%s\n' "$bg_stopped_pane"
# Esc closes the band; the composer returns with no shell in the footer.
tmux send-keys -t "$S42" Escape
sleep 0.3
bg_closed_pane="$(tmux capture-pane -t "$S42" -p)"
tmux kill-session -t "$S42" 2>/dev/null

# --- Phase 43: a background shell killed MID-TURN surfaces IMMEDIATELY
# (docs/background.md): with a dummy turn in flight, x-stopping the shell in
# the ↓ manager commits the red notice at the turn's next tool boundary —
# visible while the status line still spins — instead of only after the whole
# turn ends (the notice then sits above the turn's Done summary, not below).
S43="${S}_bgkill"
# A long pre-stream pause (the dummy's startup delay) is the window the kill
# lands in; the turn's own tool batch then settles the pending notice.
tmux new-session -d -s "$S43" -x 80 -y 24 \
	"env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=2500 $BIN"
sleep 0.4
tmux send-keys -t "$S43" -l '!sleep 300'
sleep 0.2
tmux send-keys -t "$S43" Enter
# Wait for the Ctrl+B hint (it appears a few seconds into the run — the delay);
# its presence confirms the command is running before we background it.
for _ in $(seq 1 80); do
	if tmux capture-pane -t "$S43" -p | grep -qF "(ctrl+b to run in background)"; then
		break
	fi
	sleep 0.1
done
tmux send-keys -t "$S43" C-b
for _ in $(seq 1 30); do
	if tmux capture-pane -t "$S43" -p | grep -qF "· 1 shell"; then
		break
	fi
	sleep 0.1
done
# Start a dummy turn, then kill the shell during its pre-stream pause: ↓ lights
# the footer indicator and Enter opens the manager over the in-flight turn, x
# stops the shell, Esc closes the band.
tmux send-keys -t "$S43" -l 'tell me about it'
sleep 0.2
tmux send-keys -t "$S43" Enter
sleep 0.4
tmux send-keys -t "$S43" Down
sleep 0.2
tmux send-keys -t "$S43" Enter
sleep 0.2
tmux send-keys -t "$S43" x
sleep 0.2
tmux send-keys -t "$S43" Escape
# The notice must commit while the turn is STILL RUNNING — the esc-to-interrupt
# status detail on the same screen — at the turn's first tool boundary.
bgkill_live_pane=""
for _ in $(seq 1 100); do
	bgkill_live_pane="$(tmux capture-pane -t "$S43" -p)"
	if printf '%s' "$bgkill_live_pane" | grep -qF "was stopped by the user" \
		&& printf '%s' "$bgkill_live_pane" | grep -qF "esc to interrupt"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 43: mid-turn stop notice while the status line still runs ===="
printf '%s\n' "$bgkill_live_pane"
# And once the turn ends, the notice sits ABOVE its Done summary in scrollback
# (the old behaviour settled it after, below the summary).
bgkill_done_pane=""
for _ in $(seq 1 150); do
	bgkill_done_pane="$(tmux capture-pane -t "$S43" -p -S -60)"
	if printf '%s' "$bgkill_done_pane" | grep -qE "Done for [0-9]+s"; then
		break
	fi
	sleep 0.2
done
echo "==== Phase 43: pane after the turn ended ===="
printf '%s\n' "$bgkill_done_pane"
tmux kill-session -t "$S43" 2>/dev/null

# --- Phase 44: shell children are DETACHED from the controlling terminal
# (crate::subprocess, docs/shell-command.md). A command that opens /dev/tty — which
# is exactly what sudo's password prompt does — must error at once instead of
# printing over the TUI and blocking on the keyboard the event loop owns: the
# runner re-execs the binary's detached-exec helper mode, which setsid()s into
# a fresh session (no controlling terminal → the open fails ENXIO). The probe
# writes a marker to /dev/tty (deterministic where sudo isn't installed or the
# runner is root): attached, the marker prints OVER the TUI and the command
# exits 0; detached, the redirect fails fast. The marker is assembled by
# printf so the echoed command text can never contain it verbatim. ---
S44="${S}_notty"
tmux new-session -d -s "$S44" -x 80 -y 24 "$APP"
sleep 0.4
tmux send-keys -t "$S44" -l "!printf 'LEAK%s\\n' _MARK > /dev/tty"
sleep 0.2
tmux send-keys -t "$S44" Enter
notty_pane=""
for _ in $(seq 1 60); do # up to ~6s — the whole point is that it fails fast
	notty_pane="$(tmux capture-pane -t "$S44" -p -S -40)"
	if printf '%s' "$notty_pane" | grep -qF "exit status:"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 44: pane after '! printf … > /dev/tty' (detached — fails, never prints) ===="
printf '%s\n' "$notty_pane"
tmux kill-session -t "$S44" 2>/dev/null

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
if ! printf '%s' "$overlay_deep" | grep -qF "InlineViewport::init"; then
	echo "FAIL: tool-output view did not show the full (expanded) Read output" >&2
	status=1
fi
if [ "${returned_banner_count:-0}" -gt 1 ]; then
	echo "FAIL: the Ctrl+O return duplicated the header banner over a long conversation — expected at most 1 'autonomous ai agent' in screen+scrollback, got ${returned_banner_count:-0}" >&2
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
if ! printf '%s' "$palette_open" | grep -qF "Exit alter-zero"; then
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
if ! printf '%s' "$shell_ran" | grep -qF "⎿  smoke_shell_ok"; then
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
if ! printf '%s' "$shell_running" | grep -qE "⎿  Running… \([0-9]+s\)"; then
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
if ! printf '%s' "$bigoutput" | grep -qF "⎿  1"; then
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
if ! printf '%s' "$shellqueue" | grep -qF "⎿  smoke_queue_ok"; then
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
# The dropped exchange must be gone from SCROLLBACK too, not just the visible
# screen — an in-place overwrite left it lingering above the fold until the next
# resize (the duplication bug). A Purge-rebuild clears scrollback, so nothing
# should scroll back to "beta question" (the composer holds "alpha question").
if printf '%s' "$backtrack_rewound_scroll" | grep -qF "beta question"; then
	echo "FAIL: the rewound exchange lingered in scrollback (backtrack must Purge-rebuild, not overwrite in place)" >&2
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

# Phase 38: a parallel tool-call batch shows its not-yet-run calls as ⎿ Waiting…
# (docs/parallel-tools.md).
if ! printf '%s' "$batch_waiting" | grep -qF "Waiting…"; then
	echo "FAIL: Phase 38 — a parallel batch did not show '⎿ Waiting…' for its queued calls" >&2
	status=1
fi
# The running call + at least one waiting sibling are visible at once, so two or
# more `● Bash(ping …)` cells show together (the whole batch is visible before the
# calls finish one at a time).
batch_cells=$(printf '%s\n' "$batch_waiting" | grep -cF "Bash(ping")
if [ "${batch_cells:-0}" -lt 2 ]; then
	echo "FAIL: Phase 38 — only ${batch_cells:-0} 'Bash(ping …)' cells visible at once (expected >= 2: the running call + a waiting sibling)" >&2
	status=1
fi

# Phase 39: a running Bash(ping) cell TAILS its live output — the last lines plus
# a `+N lines (Ns)` footer, Claude-Code's running-command look (docs/tool-streaming.md).
if ! printf '%s' "$batch_tail" | grep -qE '\+[0-9]+ lines \([0-9]+s\)'; then
	echo "FAIL: Phase 39 — a running Bash(ping) cell did not tail its streamed output (no '+N lines (Ns)' footer)" >&2
	status=1
fi

# Phase 40: the Ctrl+O overlay shows a running bash tool's output LIVE — a running
# Bash(ping) cell streamed an `icmp_seq` line while both siblings were still
# `⎿ Waiting…` (only the live-updating overlay can show that; docs/tool-streaming.md).
overlay_waiting=$(printf '%s\n' "$overlay_stream" | grep -c 'Waiting…')
if [ "${overlay_waiting:-0}" -lt 2 ] || ! printf '%s' "$overlay_stream" | grep -qE 'icmp_seq'; then
	echo "FAIL: Phase 40 — the Ctrl+O overlay did not show a running bash tool's live output (no streamed 'icmp_seq' line while two siblings were still ⎿ Waiting…) — the overlay stayed static during the stream" >&2
	status=1
fi

# Phase 41: a streamed table's close-flush keeps the box flush at the bottom —
# the footer ends the turn on the pane's last row, not floating above a blank
# band (docs/table-streaming.md; the flush syncs to the collapsed strip height).
if ! printf '%s' "$tableflush_pane" | grep -qF "│ 10 │ Regex"; then
	echo "FAIL: Phase 41 — the streamed table's grid rows never reached the screen" >&2
	status=1
fi
if [ "${tableflush_footer_row:-0}" -lt 23 ]; then
	echo "FAIL: Phase 41 — after the table turn the footer sits on row ${tableflush_footer_row:-none} of the 24-row pane: the box rose off the bottom, leaving a blank band beneath it" >&2
	status=1
fi
# Every grid row the same width, emoji rows included — in the inline view AND in
# the Ctrl+O overlay, the two independent full-cell emitters (`term::visible_cells`).
if [ "${tableflush_emoji_rows:-0}" -lt 1 ]; then
	echo "FAIL: Phase 41 — no emoji grid row reached the screen, so the wide-glyph alignment check proved nothing" >&2
	status=1
elif [ "$(printf '%s' "$tableflush_row_widths" | wc -w)" -ne 1 ]; then
	echo "FAIL: Phase 41 — the streamed table's grid rows are not all the same width (${tableflush_row_widths}): a wide glyph (✅/❌) is costing an extra terminal column, so the right border steps out of line" >&2
	status=1
fi
if [ "${tableflush_overlay_emoji:-0}" -lt 1 ]; then
	echo "FAIL: Phase 41 — no emoji grid row reached the Ctrl+O overlay, so its wide-glyph alignment check proved nothing" >&2
	status=1
elif [ "$(printf '%s' "$tableflush_overlay_widths" | wc -w)" -ne 1 ]; then
	echo "FAIL: Phase 41 — the Ctrl+O transcript's grid rows are not all the same width (${tableflush_overlay_widths}): draw_overlay is emitting the cells shadowed by a wide glyph, so the table tears in the overlay even though the inline view is fine" >&2
	status=1
fi

if printf '%s' "$bg_early_pane" | grep -qF "(ctrl+b to run in background)"; then
	echo "FAIL: Phase 42 — the Ctrl+B hint showed immediately (it must wait a few seconds so a fast command never flashes it)" >&2
	status=1
fi
if ! printf '%s' "$bg_early_pane" | grep -qF "Running…"; then
	echo "FAIL: Phase 42 — the ! command was not visibly running early in its run (can't trust the no-hint check)" >&2
	status=1
fi
if ! printf '%s' "$bg_hint_pane" | grep -qF "(ctrl+b to run in background)"; then
	echo "FAIL: Phase 42 — the running ! command never showed the Ctrl+B hint (even after the delay)" >&2
	status=1
fi
if ! printf '%s' "$bg_cell_pane" | grep -qF "Running in the background (↓ to manage)"; then
	echo "FAIL: Phase 42 — Ctrl+B did not resolve the cell as backgrounded" >&2
	status=1
fi
if ! printf '%s' "$bg_cell_pane" | grep -qF "· 1 shell"; then
	echo "FAIL: Phase 42 — the footer never counted the running background shell" >&2
	status=1
fi
# ↓ must FOCUS the footer's count, not open the band: the whole footer row
# survives (model · cwd · 1 shell), the count is painted on the cyan
# background, and the manager's list is nowhere on screen yet.
if ! printf '%s\n' "$bg_focus_pane" | grep -qE 'dummy_model_name · .* · 1 shell'; then
	echo "FAIL: Phase 42 — ↓ did not keep the whole footer row (model · cwd · 1 shell) while highlighting the indicator" >&2
	status=1
fi
if printf '%s' "$bg_focus_pane" | grep -qF "1 active shell"; then
	echo "FAIL: Phase 42 — ↓ opened the manager band outright (it must light the footer indicator and wait for Enter)" >&2
	status=1
fi
if ! printf '%s' "$bg_focus_ansi" | grep -q '48;2;86;182;194'; then
	echo "FAIL: Phase 42 — the focused shell indicator is not painted on the cyan background" >&2
	status=1
fi
if ! printf '%s' "$bg_list_pane" | grep -qF "Background" \
	|| ! printf '%s' "$bg_list_pane" | grep -qF "1 active shell" \
	|| ! printf '%s' "$bg_list_pane" | grep -qF "(running)" \
	|| ! printf '%s' "$bg_list_pane" | grep -qF "↑/↓ to select · Enter to view · x to stop · Esc to close"; then
	echo "FAIL: Phase 42 — Enter on the lit indicator did not open the manager list (title/count/row/hints)" >&2
	status=1
fi
if ! printf '%s' "$bg_details_pane" | grep -qF "Shell details" \
	|| ! printf '%s' "$bg_details_pane" | grep -qF "Status:   running" \
	|| ! printf '%s' "$bg_details_pane" | grep -qF "Runtime:" \
	|| ! printf '%s' "$bg_details_pane" | grep -qF "Showing"; then
	echo "FAIL: Phase 42 — the details page is missing its fields/output box" >&2
	status=1
fi
if [ -z "$bg_seq_after" ] || [ "$bg_seq_after" = "$bg_seq_before" ]; then
	echo "FAIL: Phase 42 — the details output box did not stream (stuck at ${bg_seq_before:-nothing})" >&2
	status=1
fi
if ! printf '%s' "$bg_stopped_pane" | grep -qF "was stopped by the user"; then
	echo "FAIL: Phase 42 — stopping the shell committed no notice" >&2
	status=1
fi
if ! printf '%s' "$bg_stopped_pane" | grep -qF "No tasks currently running"; then
	echo "FAIL: Phase 42 — the manager did not fall back to its empty state" >&2
	status=1
fi
if printf '%s' "$bg_closed_pane" | grep -qF "· 1 shell"; then
	echo "FAIL: Phase 42 — the footer still counts a shell after the stop" >&2
	status=1
fi

# Phase 43: a mid-turn x-kill commits its notice immediately — on screen while
# the turn still streams (status line up) — and the notice precedes the turn's
# Done summary in scrollback (the old settle landed it after the summary).
if ! printf '%s' "$bgkill_live_pane" | grep -qF "was stopped by the user" \
	|| ! printf '%s' "$bgkill_live_pane" | grep -qF "esc to interrupt"; then
	echo "FAIL: Phase 43 — the mid-turn kill's notice never showed while the turn was still streaming (turn-end-only settle?)" >&2
	status=1
fi
bgkill_notice_row="$(printf '%s\n' "$bgkill_done_pane" | grep -nF "was stopped by the user" | head -1 | cut -d: -f1)"
bgkill_done_row="$(printf '%s\n' "$bgkill_done_pane" | grep -nE "Done for [0-9]+s" | head -1 | cut -d: -f1)"
if [ -z "$bgkill_notice_row" ] || [ -z "$bgkill_done_row" ] \
	|| [ "$bgkill_notice_row" -ge "$bgkill_done_row" ]; then
	echo "FAIL: Phase 43 — the kill notice (row ${bgkill_notice_row:-none}) does not precede the Done summary (row ${bgkill_done_row:-none})" >&2
	status=1
fi

# Phase 44: shell children have no controlling terminal — the /dev/tty probe
# (sudo's password-prompt mechanism) fails fast inside the cell instead of
# printing over the TUI or blocking on the keyboard.
if ! printf '%s' "$notty_pane" | grep -qF "No such device or address"; then
	echo "FAIL: Phase 44 — the /dev/tty open did not fail (the shell child still has a controlling terminal, so a sudo prompt would hijack the TUI)" >&2
	status=1
fi
if ! printf '%s' "$notty_pane" | grep -qF "exit status:"; then
	echo "FAIL: Phase 44 — the '! … > /dev/tty' cell never resolved (the command blocked — the sudo-hang bug)" >&2
	status=1
fi
if printf '%s' "$notty_pane" | grep -qF "LEAK_MARK"; then
	echo "FAIL: Phase 44 — the marker printed over the TUI (the child wrote straight to /dev/tty)" >&2
	status=1
fi

# Phase 45: the startup header banner (docs/header.md). A fresh session shows the
# ASCII wordmark + version + cwd + hint at the top of scrollback. It is chrome
# (never in `history`), re-emitted on every full repaint — so it survives a
# resize (a width change purges scrollback and rebuilds from history, which the
# header is NOT part of, so it must be re-emitted) and re-shows after /clear (a
# fresh-start banner). The tier-independent tagline is the marker; the borderless
# design adds no `─` rule / bare prompt / footer, so Phases 16/17 stay green.
HEADER_MARK="autonomous ai agent"
S_HEADER="${S}_header"
tmux new-session -d -s "$S_HEADER" -x 80 -y 24 "$APP"
sleep 0.5
header_start="$(tmux capture-pane -t "$S_HEADER" -p)"
echo "==== Phase 45: captured pane (startup header banner) ===="
printf '%s\n' "$header_start"
tmux resize-window -t "$S_HEADER" -x 50 -y 24
sleep 0.6
header_narrow="$(tmux capture-pane -t "$S_HEADER" -p)"
tmux resize-window -t "$S_HEADER" -x 80 -y 24
sleep 0.6
header_regrown="$(tmux capture-pane -t "$S_HEADER" -p)"
echo "==== Phase 45: captured pane (header after a 80→50→80 resize round-trip) ===="
printf '%s\n' "$header_regrown"
tmux send-keys -t "$S_HEADER" -l "/clear"
sleep 0.2
tmux send-keys -t "$S_HEADER" Enter
sleep 0.5
header_cleared="$(tmux capture-pane -t "$S_HEADER" -p)"
echo "==== Phase 45: captured pane (header re-shown after /clear) ===="
printf '%s\n' "$header_cleared"
# The Ctrl+O round trip (the disappearing-header bug): the overlay transcript
# itself opens with the banner at its top, and the InPlace return repaint must
# restore the banner on the inline screen — it lives outside `history`, and the
# pre-fix return rebuilt from history alone, wiping it until the next resize
# or /clear re-emitted it (docs/header.md).
tmux send-keys -t "$S_HEADER" C-o
sleep 0.5
header_overlay="$(tmux capture-pane -t "$S_HEADER" -p)"
echo "==== Phase 45: captured pane (Ctrl+O overlay — the banner tops the empty transcript) ===="
printf '%s\n' "$header_overlay"
tmux send-keys -t "$S_HEADER" C-o
sleep 0.5
header_returned="$(tmux capture-pane -t "$S_HEADER" -p)"
echo "==== Phase 45: captured pane (banner still on the inline screen after the Ctrl+O return) ===="
printf '%s\n' "$header_returned"
# The reported repro — a real conversation with tool output, then the round
# trip. Grow the pane first so the whole banner + turn fit the repaint window
# (the InPlace return re-caps the banner to the visible rows).
tmux resize-window -t "$S_HEADER" -x 80 -y 45
sleep 0.6
tmux send-keys -t "$S_HEADER" -l "hello there"
sleep 0.2
tmux send-keys -t "$S_HEADER" Enter
for _ in $(seq 1 90); do # up to ~13s — the full dummy turn, tools included
	if tmux capture-pane -t "$S_HEADER" -p | grep -qF "Done for"; then
		break
	fi
	sleep 0.15
done
tmux send-keys -t "$S_HEADER" C-o
sleep 0.5
tmux send-keys -t "$S_HEADER" Home # the pager opens at the bottom; the banner is at the top
sleep 0.3
header_overlay_conv="$(tmux capture-pane -t "$S_HEADER" -p)"
echo "==== Phase 45: captured pane (Ctrl+O overlay — the banner atop a real conversation) ===="
printf '%s\n' "$header_overlay_conv"
tmux send-keys -t "$S_HEADER" C-o
sleep 0.5
header_roundtrip="$(tmux capture-pane -t "$S_HEADER" -p)"
echo "==== Phase 45: captured pane (banner + conversation after the Ctrl+O round trip) ===="
printf '%s\n' "$header_roundtrip"
tmux kill-session -t "$S_HEADER" 2>/dev/null
if ! printf '%s' "$header_start" | grep -qF "$HEADER_MARK"; then
	echo "FAIL: Phase 45 — the startup header banner did not show ('$HEADER_MARK' missing)" >&2
	status=1
fi
if ! printf '%s' "$header_narrow" | grep -qF "$HEADER_MARK"; then
	echo "FAIL: Phase 45 — the header did not survive a width shrink to 50 (not re-emitted on the Purge rebuild)" >&2
	status=1
fi
if ! printf '%s' "$header_regrown" | grep -qF "$HEADER_MARK"; then
	echo "FAIL: Phase 45 — the header did not survive a resize round-trip" >&2
	status=1
fi
if ! printf '%s' "$header_cleared" | grep -qF "$HEADER_MARK"; then
	echo "FAIL: Phase 45 — the header did not re-show after /clear" >&2
	status=1
fi
if ! printf '%s' "$header_overlay" | grep -qF "$HEADER_MARK"; then
	echo "FAIL: Phase 45 — the Ctrl+O transcript does not open with the banner" >&2
	status=1
fi
if ! printf '%s' "$header_returned" | grep -qF "$HEADER_MARK"; then
	echo "FAIL: Phase 45 — the header vanished on the Ctrl+O return (the InPlace repaint dropped the banner)" >&2
	status=1
fi
if ! printf '%s' "$header_overlay_conv" | grep -qF "$HEADER_MARK"; then
	echo "FAIL: Phase 45 — the Ctrl+O transcript of a real conversation is missing the banner at its top" >&2
	status=1
fi
if ! printf '%s' "$header_roundtrip" | grep -qF "$HEADER_MARK"; then
	echo "FAIL: Phase 45 — the header vanished after a Ctrl+O round trip over a real conversation" >&2
	status=1
fi
if ! printf '%s' "$header_roundtrip" | grep -qF "Happy to help"; then
	echo "FAIL: Phase 45 — the conversation itself did not survive the Ctrl+O round trip beneath the banner" >&2
	status=1
fi

# --- Phase 46: Esc-Esc BACKTRACK resets the CODE, not just the transcript
# (docs/checkpoint.md). Each turn snapshots the working directory into an
# isolated git store (never the user's real .git); rewinding to an earlier user
# message restores the files to that point. Deterministic with the dummy
# backend: a pristine file, a text turn (no file change), then a `!` shell turn
# that MUTATES the file — backtracking to the first user message must revert the
# file to pristine. ---
S46="${S}_ckbacktrack"
CK_DIR="$(mktemp -d /tmp/alter-zero-smoke-ck-XXXXXX)"
CK_SESS="$(mktemp -d /tmp/alter-zero-smoke-cksess-XXXXXX)"
WORK46="$(mktemp -d /tmp/alter-zero-smoke-work46-XXXXXX)"
printf 'pristine\n' >"$WORK46/file.txt"
# These phases run the app in a temp cwd (-c), so the binary must be an
# ABSOLUTE path — a relative $BIN would resolve against the temp dir and fail.
BIN_ABS="$(readlink -f "$BIN")"
# Re-enable checkpoints here (overriding CFG_ENV's =0) — safe: this runs in a
# throwaway temp cwd ($WORK46), so a restore's git clean can't touch the repo.
CKAPP="env $CFG_ENV ALTER_ZERO_CHECKPOINTS=1 ALTER_ZERO_CHECKPOINTS_DIR=$CK_DIR ALTER_ZERO_SESSIONS_DIR=$CK_SESS ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"
tmux new-session -d -s "$S46" -x 80 -y 24 -c "$WORK46" "$CKAPP"
sleep 0.5
tmux send-keys -t "$S46" -l "$USER_MSG"
sleep 0.2
tmux send-keys -t "$S46" Enter
for _ in $(seq 1 80); do # text turn 1 → "Done for" (checkpoint {pristine})
	if tmux capture-pane -t "$S46" -p -S -40 | grep -qF "Done for"; then break; fi
	sleep 0.15
done
tmux send-keys -t "$S46" -l "!echo mutated > file.txt" # a `!` shell turn mutates the file
sleep 0.2
tmux send-keys -t "$S46" Enter
ckb_mutated="?"
for _ in $(seq 1 80); do # wait for the shell TURN to END (file written AND Running gone)
	ckb_mutated="$(cat "$WORK46/file.txt" 2>/dev/null)"
	if [ "$ckb_mutated" = "mutated" ] &&
		! tmux capture-pane -t "$S46" -p -S -20 | grep -qF "Running"; then
		break
	fi
	sleep 0.15
done
sleep 0.5 # let StreamDone → dispatch_after_turn snapshot + flush the {mutated} checkpoint
tmux send-keys -t "$S46" Escape # idle Esc → prime the backtrack
sleep 0.3
tmux send-keys -t "$S46" Escape # → transcript preview on the sole user message
sleep 0.4
tmux send-keys -t "$S46" Enter # rewind to before it → restore checkpoint {pristine}
ckb_restored="?"
for _ in $(seq 1 60); do
	ckb_restored="$(cat "$WORK46/file.txt" 2>/dev/null)"
	if [ "$ckb_restored" = "pristine" ]; then break; fi
	sleep 0.15
done
echo "==== Phase 46: working file after mutate='$ckb_mutated', after backtrack='$ckb_restored' ===="
tmux kill-session -t "$S46" 2>/dev/null
if [ "$ckb_mutated" != "mutated" ]; then
	echo "FAIL: Phase 46 — the ! shell turn did not mutate the working file (precondition; file is '$ckb_mutated')" >&2
	status=1
fi
if [ "$ckb_restored" != "pristine" ]; then
	echo "FAIL: Phase 46 — Esc-Esc backtrack did not reset the code to the checkpoint (file is '$ckb_restored', expected 'pristine')" >&2
	status=1
fi

# --- Phase 47: /resume resets the CODE to the saved session's checkpoint
# (docs/checkpoint.md). Launch 1 records a session whose `!` shell turn leaves
# the file at v1; the process quits. The file is then DIVERGED on disk (as if
# later work changed it). Launch 2's /resume of that session must restore the
# file to v1 — the transcript and the code agree again. Same cwd across
# launches, so the isolated store (keyed by cwd) still holds the v1 commit. ---
S47="${S}_ckresume"
WORK47="$(mktemp -d /tmp/alter-zero-smoke-work47-XXXXXX)"
printf 'pristine\n' >"$WORK47/file.txt"
CKAPP2="env $CFG_ENV ALTER_ZERO_CHECKPOINTS=1 ALTER_ZERO_CHECKPOINTS_DIR=$CK_DIR ALTER_ZERO_SESSIONS_DIR=$CK_SESS ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"
tmux new-session -d -s "$S47" -x 80 -y 24 -c "$WORK47" "$CKAPP2"
sleep 0.5
tmux send-keys -t "$S47" -l "$USER_MSG" # a user message so the session lists in the picker
sleep 0.2
tmux send-keys -t "$S47" Enter
for _ in $(seq 1 80); do
	if tmux capture-pane -t "$S47" -p -S -40 | grep -qF "Done for"; then break; fi
	sleep 0.15
done
tmux send-keys -t "$S47" -l "!echo v1 > file.txt" # the session's final code state
sleep 0.2
tmux send-keys -t "$S47" Enter
for _ in $(seq 1 80); do # wait for the shell TURN to END so its {v1} checkpoint records
	if [ "$(cat "$WORK47/file.txt" 2>/dev/null)" = "v1" ] &&
		! tmux capture-pane -t "$S47" -p -S -20 | grep -qF "Running"; then
		break
	fi
	sleep 0.15
done
sleep 0.5 # let the shell turn's {v1} checkpoint record + flush to the session file
tmux send-keys -t "$S47" C-c # quit launch 1 (empty composer)
sleep 0.4
tmux kill-session -t "$S47" 2>/dev/null
printf 'divergent\n' >"$WORK47/file.txt" # later work diverges the code on disk
ckr_diverged="$(cat "$WORK47/file.txt" 2>/dev/null)"
tmux new-session -d -s "$S47" -x 80 -y 24 -c "$WORK47" "$CKAPP2"
sleep 0.5
tmux send-keys -t "$S47" -l "/resume"
sleep 0.3
tmux send-keys -t "$S47" Enter # open the picker
sleep 0.6
tmux send-keys -t "$S47" Enter # resume the highlighted session → restore its final checkpoint
ckr_restored="?"
for _ in $(seq 1 60); do
	ckr_restored="$(cat "$WORK47/file.txt" 2>/dev/null)"
	if [ "$ckr_restored" = "v1" ]; then break; fi
	sleep 0.15
done
echo "==== Phase 47: working file diverged='$ckr_diverged', after resume='$ckr_restored' ===="
tmux kill-session -t "$S47" 2>/dev/null
if [ "$ckr_diverged" != "divergent" ]; then
	echo "FAIL: Phase 47 — the working file was not diverged before resume (precondition; file is '$ckr_diverged')" >&2
	status=1
fi
if [ "$ckr_restored" != "v1" ]; then
	echo "FAIL: Phase 47 — /resume did not reset the code to the session's checkpoint (file is '$ckr_restored', expected 'v1')" >&2
	status=1
fi

# --- Phase 48: Ctrl+O on a resumed CODE-HEAVY session opens WARM and ATOMIC
# (docs/tool-view-performance.md). A handcrafted rollout carries three Write
# tools of ~1000 numbered HTML lines each (~110 KB — the grammar-highlight
# heavy shape that made the old open re-render everything on a blank alt
# screen for hundreds of ms, kitty's cursor-trail streaking up it). After
# /resume, the loop-bottom warm pre-renders the transcript, and enter_overlay
# only QUEUES the switch so the first overlay frame lands in the same flush:
# Ctrl+O must show the transcript promptly, and NO capture taken while it
# opens may ever be a blank screen (the old flushed-blank window). The return
# must land back on the intact composer. ---
S48="${S}_ctrlofast"
CTRLO_DIR="$(mktemp -d /tmp/alter-zero-smoke-ctrlo-XXXXXX)"
ctrlo_day="$CTRLO_DIR/2026/07/23"
mkdir -p "$ctrlo_day"
ctrlo_file="$ctrlo_day/rollout-2026-07-23T10-00-00-48484848.jsonl"
{
	printf '{"timestamp":"2026-07-23T10:00:00.000Z","type":"session_meta","payload":{"id":"smoke-48","timestamp":"2026-07-23T10:00:00.000Z","cwd":"%s","model":"dummy_model_name","originator":"alter-zero","version":"0.1.0"}}\n' "$PWD"
	printf '{"timestamp":"2026-07-23T10:00:01.000Z","type":"message","payload":{"role":"user","text":"resumed ctrlo html app","timestamp":"10:00 AM"}}\n'
	for t in 1 2 3; do
		body="Created /tmp/buddies${t}.html (1000 lines)"
		n=1
		while [ "$n" -le 1000 ]; do
			# Right-aligned 4-wide gutter — the numbered-file-cell shape the
			# overlay syntax-highlights by the .html extension. No quotes or
			# backslashes in the content, so the line is JSON-safe verbatim.
			body="$body\\n$(printf '%4d' "$n") <div class=b${n}>buddy ${n} of file ${t}</div>"
			n=$((n + 1))
		done
		printf '{"timestamp":"2026-07-23T10:00:02.000Z","type":"tool","payload":{"name":"Write","args":"/tmp/buddies%s.html","ok":true,"output":"%s","timestamp":"10:00 AM","shell":false,"truncated":false}}\n' "$t" "$body"
	done
	printf '{"timestamp":"2026-07-23T10:00:03.000Z","type":"message","payload":{"role":"assistant","text":"All three buddy files saved and animated.","timestamp":"10:00 AM"}}\n'
} >"$ctrlo_file"
tmux new-session -d -s "$S48" -x 100 -y 30 "env $CFG_ENV_NOHIST ALTER_ZERO_SESSIONS_DIR=$CTRLO_DIR ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
sleep 0.5
tmux send-keys -t "$S48" -l "/resume"
sleep 0.3
tmux send-keys -t "$S48" Enter # open the picker
ctrlo_listed=0
for _ in $(seq 1 40); do # the seeded session's preview row (same cwd → Cwd filter keeps it)
	if tmux capture-pane -t "$S48" -p | grep -qF "resumed ctrlo html app"; then
		ctrlo_listed=1
		break
	fi
	sleep 0.1
done
tmux send-keys -t "$S48" Enter # load it
ctrlo_loaded=0
for _ in $(seq 1 60); do # the loaded conversation repaints inline (collapsed Write cell)
	if tmux capture-pane -t "$S48" -p -S -60 | grep -qF "buddies3.html"; then
		ctrlo_loaded=1
		break
	fi
	sleep 0.1
done
sleep 1.0 # let the loop-bottom transcript warm finish after the load
ctrlo_t0="$(date +%s.%N)"
tmux send-keys -t "$S48" C-o
ctrlo_blank=0
ctrlo_open_ms=""
for _ in $(seq 1 200); do # sample tightly: no capture may be a blank screen
	ctrlo_pane="$(tmux capture-pane -t "$S48" -p)"
	if [ "$(printf '%s' "$ctrlo_pane" | grep -cve '^[[:space:]]*$')" -eq 0 ]; then
		ctrlo_blank=1
	fi
	if printf '%s' "$ctrlo_pane" | grep -q "T R A N S C R I P T"; then
		ctrlo_open_ms="$(awk "BEGIN{printf \"%.0f\", ($(date +%s.%N) - $ctrlo_t0)*1000}")"
		break
	fi
	sleep 0.01
done
# The overlay opens tail-following: the resumed conversation's final reply is
# in view — proof the transcript content itself rendered, not just the chrome.
ctrlo_tail=0
if tmux capture-pane -t "$S48" -p | grep -qF "All three buddy files saved"; then
	ctrlo_tail=1
fi
tmux send-keys -t "$S48" C-o # return to the inline view
ctrlo_back=0
for _ in $(seq 1 40); do
	if tmux capture-pane -t "$S48" -p | grep -qF "dummy_model_name"; then
		ctrlo_back=1
		break
	fi
	sleep 0.1
done
echo "==== Phase 48: resumed code-heavy Ctrl+O — listed=$ctrlo_listed loaded=$ctrlo_loaded open_ms=${ctrlo_open_ms:-none} blank_capture=$ctrlo_blank tail=$ctrlo_tail back=$ctrlo_back ===="
tmux capture-pane -t "$S48" -p | grep -v '^$' | tail -4
tmux kill-session -t "$S48" 2>/dev/null
if [ "$ctrlo_listed" != 1 ] || [ "$ctrlo_loaded" != 1 ]; then
	echo "FAIL: Phase 48 precondition — the seeded code-heavy session did not list/load (listed=$ctrlo_listed loaded=$ctrlo_loaded)" >&2
	status=1
fi
if [ -z "$ctrlo_open_ms" ]; then
	echo "FAIL: Phase 48 — Ctrl+O never showed the transcript overlay" >&2
	status=1
elif [ "$ctrlo_open_ms" -gt 2000 ]; then
	echo "FAIL: Phase 48 — Ctrl+O took ${ctrlo_open_ms}ms on the resumed session (warm open must not rebuild the transcript)" >&2
	status=1
fi
if [ "$ctrlo_blank" != 0 ]; then
	echo "FAIL: Phase 48 — a capture during the Ctrl+O switch was a BLANK screen (the switch must land with the frame in one flush)" >&2
	status=1
fi
if [ "$ctrlo_tail" != 1 ]; then
	echo "FAIL: Phase 48 — the overlay did not show the resumed conversation's tail content" >&2
	status=1
fi
if [ "$ctrlo_back" != 1 ]; then
	echo "FAIL: Phase 48 — the return from Ctrl+O did not restore the inline view" >&2
	status=1
fi

# --- Phase 49: checkpoints REFUSE a home-directory cwd (docs/checkpoint.md).
# The session-start snapshot `git add -A`s the whole cwd before the first
# frame ever paints; run with cwd = `~` itself that hashed the user's entire
# home directory into the store — minutes of blocked, blank, raw-mode
# terminal and hundreds of MB per snapshot (the "alter0 hangs in ~" bug). The
# guard (`checkpoint::cwd_allows_checkpoints`) disables the store when the
# cwd IS the home dir (or an ancestor of it, or a filesystem root) — even
# with an explicit ALTER_ZERO_CHECKPOINTS=1 — while a project dir UNDER home
# checkpoints exactly as before. Assert on the store dir itself: it must stay
# empty after a home-cwd launch and populate after a project-cwd launch. ---
S49="${S}_ckhome"
WORK49="$(mktemp -d /tmp/alter-zero-smoke-home49-XXXXXX)"
CK49="$(mktemp -d /tmp/alter-zero-smoke-ck49-XXXXXX)"
mkdir -p "$WORK49/proj"
CKAPP49="env $CFG_ENV ALTER_ZERO_CHECKPOINTS=1 ALTER_ZERO_CHECKPOINTS_DIR=$CK49 ALTER_ZERO_SESSIONS_DIR=$CK_SESS HOME=$WORK49 ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"
tmux new-session -d -s "$S49" -x 80 -y 24 -c "$WORK49" "$CKAPP49"
ckhome_ready=0
for _ in $(seq 1 40); do # the footer proves startup completed (init ran or was skipped)
	if tmux capture-pane -t "$S49" -p | grep -qF "dummy_model_name"; then
		ckhome_ready=1
		break
	fi
	sleep 0.1
done
ckhome_store="$(ls -A "$CK49" 2>/dev/null | wc -l)"
tmux send-keys -t "$S49" C-c # quit (empty composer)
sleep 0.3
tmux kill-session -t "$S49" 2>/dev/null
# The contrast run: a project dir UNDER the same home must still checkpoint —
# the guard is scoped, not a blanket disable.
tmux new-session -d -s "$S49" -x 80 -y 24 -c "$WORK49/proj" "$CKAPP49"
ckproj_store=0
for _ in $(seq 1 40); do # the session-start snapshot inits the store before the loop
	if [ "$(ls -A "$CK49" 2>/dev/null | wc -l)" -gt 0 ]; then
		ckproj_store=1
		break
	fi
	sleep 0.1
done
tmux kill-session -t "$S49" 2>/dev/null
echo "==== Phase 49: home-cwd store entries=$ckhome_store (want 0), project-cwd store created=$ckproj_store (want 1) ===="
if [ "$ckhome_ready" != 1 ]; then
	echo "FAIL: Phase 49 precondition — the app did not start in the home-cwd launch" >&2
	status=1
fi
if [ "$ckhome_store" != 0 ]; then
	echo "FAIL: Phase 49 — a home-directory cwd initialized the checkpoint store (it must never snapshot ~)" >&2
	status=1
fi
if [ "$ckproj_store" != 1 ]; then
	echo "FAIL: Phase 49 — a project dir under home did not checkpoint (the guard must be scoped to ~ itself, not everything under it)" >&2
	status=1
fi

# --- Phase 50: /compact runs codex's summarization turn against the dummy and
# lands the marker (docs/compact.md). An empty conversation is rejected with the
# 'Nothing to compact' toast; after a real turn, /compact streams the dummy's
# canned summary into the compact buffer (NEVER rendered — the pane must not
# show the summary text), commits the '● Context compacted' cell with the
# transcript above it untouched, and the Ctrl+D context-debug view then derives
# the COMPACTED context: the SUMMARY_PREFIX bridge in place of the old reply
# (the old assistant text gone from the derivation, the user text retained). ---
S50="${S}_compact"
tmux new-session -d -s "$S50" -x 80 -y 24 "$APP"
sleep 0.4
# Empty conversation: nothing to compact → the transient toast.
tmux send-keys -t "$S50" -l "/compact"
sleep 0.3
tmux send-keys -t "$S50" Enter
compact_empty=""
for _ in $(seq 1 30); do # up to ~3s
	compact_empty="$(tmux capture-pane -t "$S50" -p -S -20)"
	if printf '%s' "$compact_empty" | grep -qF "Nothing to compact"; then
		break
	fi
	sleep 0.1
done
echo "==== captured pane (/compact with nothing to compact) ===="
printf '%s\n' "$compact_empty"
# A real turn first (the dummy reply for "hello there" ends "changes size"),
# settled like Phase 28: the tail committed AND the screen stable.
tmux send-keys -t "$S50" -l "hello there"
sleep 0.2
tmux send-keys -t "$S50" Enter
compact_prev=""
for _ in $(seq 1 60); do # up to ~12s
	compact_cur="$(tmux capture-pane -t "$S50" -p)"
	if printf '%s' "$compact_cur" | grep -qF "changes size" && [ "$compact_cur" = "$compact_prev" ]; then
		break
	fi
	compact_prev="$compact_cur"
	sleep 0.2
done
tmux send-keys -t "$S50" -l "/compact"
sleep 0.3
tmux send-keys -t "$S50" Enter
compact_pane=""
for _ in $(seq 1 60); do # up to ~6s (the dummy pause + the summary stream)
	compact_pane="$(tmux capture-pane -t "$S50" -p -S -80)"
	if printf '%s' "$compact_pane" | grep -qF "Context compacted"; then
		break
	fi
	sleep 0.1
done
echo "==== captured pane (after /compact — marker cell, summary never rendered) ===="
printf '%s\n' "$compact_pane"
# The derived context is now the compacted shape: Ctrl+D shows the bridge.
tmux send-keys -t "$S50" C-d
sleep 0.6
compact_ctx="$(tmux capture-pane -t "$S50" -p)"
echo "==== captured pane (Ctrl+D context-debug after /compact) ===="
printf '%s\n' "$compact_ctx"
tmux send-keys -t "$S50" q
sleep 0.4
tmux kill-session -t "$S50" 2>/dev/null
echo "==== Phase 50: /compact — empty-reject, marker cell, hidden summary, compacted Ctrl+D ===="
if ! printf '%s' "$compact_empty" | grep -qF "Nothing to compact"; then
	echo "FAIL: Phase 50 — /compact on an empty conversation did not toast 'Nothing to compact'" >&2
	status=1
fi
if ! printf '%s' "$compact_pane" | grep -qF "Context compacted"; then
	echo "FAIL: Phase 50 — the '● Context compacted' cell never committed" >&2
	status=1
fi
if ! printf '%s' "$compact_pane" | grep -qF "hello there"; then
	echo "FAIL: Phase 50 — the transcript above the marker was not preserved (append-only compaction)" >&2
	status=1
fi
if printf '%s' "$compact_pane" | grep -qF "canned handoff summary"; then
	echo "FAIL: Phase 50 — the streamed summary text rendered into the conversation (it must stay hidden)" >&2
	status=1
fi
if ! printf '%s' "$compact_ctx" | grep -qF "Another language model"; then
	echo "FAIL: Phase 50 — Ctrl+D does not show the SUMMARY_PREFIX bridge (the derivation did not compact)" >&2
	status=1
fi
if printf '%s' "$compact_ctx" | grep -qF "Happy to help"; then
	echo "FAIL: Phase 50 — the old assistant reply is still in the derived context (it must compact away)" >&2
	status=1
fi
if ! printf '%s' "$compact_ctx" | grep -qF "hello there"; then
	echo "FAIL: Phase 50 — the recent user message dropped from the compacted context (the budget walk must keep it)" >&2
	status=1
fi

# --- Phase 51: AUTO-compact + the footer context gauge (docs/compact.md).
# `ALTER_ZERO_CONTEXT_WINDOW=100` forces a tiny window onto the dummy: the
# footer shows the `{used}/{window} ({pct}%)` gauge, and one turn's estimate
# blows past codex's 90% threshold — the loop then starts the summarization
# turn ON ITS OWN (no /compact typed): the marker cell commits with the
# `· {before} → {after} tokens[ · {elapsed}] · auto` clause (the duration
# appears when the summarization turn took ≥1s) and the transcript is
# untouched. ---
S51="${S}_autocompact"
tmux new-session -d -s "$S51" -x 100 -y 24 "env ALTER_ZERO_CONTEXT_WINDOW=100 $APP"
sleep 0.4
gauge_idle="$(tmux capture-pane -t "$S51" -p)"
echo "==== captured pane (idle footer gauge under a forced 100-token window) ===="
printf '%s\n' "$gauge_idle"
tmux send-keys -t "$S51" -l "hello there"
sleep 0.2
tmux send-keys -t "$S51" Enter
# The turn ends, the estimate crosses the threshold, and the loop auto-runs
# the compact turn — poll straight for the auto-tagged marker cell.
auto_pane=""
for _ in $(seq 1 100); do # up to ~10s (turn + startup pause + summary stream)
	auto_pane="$(tmux capture-pane -t "$S51" -p -S -80)"
	if printf '%s' "$auto_pane" | grep -qE "tokens( · [0-9]+[hms][0-9ms ]*)? · auto"; then
		break
	fi
	sleep 0.1
done
echo "==== captured pane (auto-compact after one turn) ===="
printf '%s\n' "$auto_pane"
tmux kill-session -t "$S51" 2>/dev/null
echo "==== Phase 51: footer gauge + threshold-triggered auto-compact ===="
if ! printf '%s' "$gauge_idle" | grep -qE '/100 \([0-9]+\.[0-9]%\)'; then
	echo "FAIL: Phase 51 — the footer does not show the context gauge under ALTER_ZERO_CONTEXT_WINDOW" >&2
	status=1
fi
if ! printf '%s' "$auto_pane" | grep -qF "Context compacted"; then
	echo "FAIL: Phase 51 — no auto-compaction happened past the 90% threshold" >&2
	status=1
fi
if ! printf '%s' "$auto_pane" | grep -qE "tokens( · [0-9]+[hms][0-9ms ]*)? · auto"; then
	echo "FAIL: Phase 51 — the marker cell is missing the '· {before} → {after} tokens[ · {elapsed}] · auto' clause" >&2
	status=1
fi
if ! printf '%s' "$auto_pane" | grep -qF "hello there"; then
	echo "FAIL: Phase 51 — the transcript above the auto marker was not preserved" >&2
	status=1
fi

# --- Phase 52: /init + the AGENTS.md instructions in the context
# (docs/init.md, docs/project-doc.md). In a temp project with a planted
# AGENTS.md: Ctrl+D shows codex's `# AGENTS.md instructions … <INSTRUCTIONS>`
# fragment BEFORE any turn (the startup seed — dummy backend, so this proves
# the App-side injection is backend-independent), /init submits the bundled
# AGENTS.md-authoring prompt as a normal user turn (codex's one-line
# dispatch), and a mid-turn /init is rejected with the busy toast (codex's
# available_during_task=false). Launched with a LONG pre-stream pause so the
# mid-turn press deterministically lands while the first turn is still
# active (the Phase 20 pattern). ---
S52="${S}_init"
WORK52="$(mktemp -d /tmp/alter-zero-smoke-init-XXXXXX)"
mkdir -p "$WORK52/.git"
printf '# Contributor guide\n\nSmoke sentinel: Umbral-Kite-77.\n' >"$WORK52/AGENTS.md"
# The app runs in the temp cwd (-c), so the binary must be an absolute path.
BIN_ABS52="$(readlink -f "$BIN")"
tmux new-session -d -s "$S52" -x 100 -y 30 -c "$WORK52" \
	"env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=2000 $BIN_ABS52"
sleep 0.5
tmux send-keys -t "$S52" C-d
sleep 0.4
tmux send-keys -t "$S52" Home # the view opens at the bottom; jump to the top
sleep 0.3
init_ctx="$(tmux capture-pane -t "$S52" -p)"
echo "==== captured pane (Ctrl+D shows the AGENTS.md instructions fragment) ===="
printf '%s\n' "$init_ctx"
tmux send-keys -t "$S52" q # back to the conversation
sleep 0.4
tmux send-keys -t "$S52" -l "/init"
sleep 0.3
tmux send-keys -t "$S52" Enter
init_pane=""
for _ in $(seq 1 10); do # the prompt commits as the user message immediately
	init_pane="$(tmux capture-pane -t "$S52" -p -S -60)"
	if printf '%s' "$init_pane" | grep -qF "Generate a file named AGENTS.md"; then
		break
	fi
	sleep 0.1
done
echo "==== captured pane (/init submitted the bundled prompt) ===="
printf '%s\n' "$init_pane"
# Still inside the 2s pre-stream pause: /init again -> the busy toast.
tmux send-keys -t "$S52" -l "/init"
sleep 0.2
tmux send-keys -t "$S52" Enter
init_busy=""
for _ in $(seq 1 15); do # up to ~1.5s, inside the pause
	init_busy="$(tmux capture-pane -t "$S52" -p -S -20)"
	if printf '%s' "$init_busy" | grep -qF "/init is disabled"; then
		break
	fi
	sleep 0.1
done
echo "==== captured pane (mid-turn /init busy toast) ===="
printf '%s\n' "$init_busy"
tmux kill-session -t "$S52" 2>/dev/null
echo "==== Phase 52: /init + the AGENTS.md instructions in the Ctrl+D context ===="
if ! printf '%s' "$init_ctx" | grep -qF "# AGENTS.md instructions"; then
	echo "FAIL: Phase 52 — Ctrl+D lacks the AGENTS.md instructions fragment" >&2
	status=1
fi
if ! printf '%s' "$init_ctx" | grep -qF "Umbral-Kite-77"; then
	echo "FAIL: Phase 52 — the planted AGENTS.md content is missing from the context view" >&2
	status=1
fi
if ! printf '%s' "$init_pane" | grep -qF "Generate a file named AGENTS.md"; then
	echo "FAIL: Phase 52 — /init did not submit the bundled AGENTS.md prompt as the user message" >&2
	status=1
fi
if ! printf '%s' "$init_busy" | grep -qF "/init is disabled while a task is in progress"; then
	echo "FAIL: Phase 52 — mid-turn /init was not rejected with the busy toast" >&2
	status=1
fi
rm -rf "$WORK52" 2>/dev/null

# --- Phase 53: the Agent tool (docs/agent-tool.md) — the dummy's scripted
# two-agent demo. A prompt mentioning "agents" announces a foreground group:
# while it "runs" (AGENT_DELAY) the strip shows the blue `● Running 2 agents…`
# tree with `⎿ Initializing…` per agent AND the footer roster lists `● main` +
# two `◯ general-purpose …` rows; at resolution the committed
# `● 2 agents finished (ctrl+o to expand)` tree lands with `⎿ Done` rows; the
# Ctrl+O transcript expands each as `● Agent({description})` with its
# `⎿ Prompt:` block and `⎿ Done (…)` footer; and after the linger the roster
# rows sweep away. ---
S53="${S}_agents"
tmux new-session -d -s "$S53" -x 100 -y 44 "$APP"
sleep 0.4
tmux send-keys -t "$S53" -l "call agents for the weather"
sleep 0.2
tmux send-keys -t "$S53" Enter
agents_live=""
for _ in $(seq 1 120); do # the group "runs" for AGENT_DELAY (1.6s)
	cap="$(tmux capture-pane -t "$S53" -p)"
	if printf '%s' "$cap" | grep -qF "Running 2 agents…" &&
		printf '%s' "$cap" | grep -qF "● main" &&
		printf '%s' "$cap" | grep -qF "Initializing…"; then
		agents_live="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 53: captured pane (live agent group tree + footer roster) ===="
printf '%s\n' "$agents_live"
agents_done=""
for _ in $(seq 1 200); do # the resolution + the closing text
	cap="$(tmux capture-pane -t "$S53" -p)"
	if printf '%s' "$cap" | grep -qF "2 agents finished (ctrl+o to expand)"; then
		agents_done="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 53: captured pane (committed agent group cell) ===="
printf '%s\n' "$agents_done"
# The Ctrl+O transcript expands each agent with its prompt + Done footer.
tmux send-keys -t "$S53" C-o
sleep 0.5
agents_overlay="$(tmux capture-pane -t "$S53" -p)"
echo "==== Phase 53: captured pane (Ctrl+O agent cells) ===="
printf '%s\n' "$agents_overlay"
tmux send-keys -t "$S53" q
sleep 0.4
# The roster lingers a few seconds after the group settles, then sweeps.
agents_swept=""
for _ in $(seq 1 160); do # AGENT_LINGER is 5s
	cap="$(tmux capture-pane -t "$S53" -p)"
	if ! printf '%s' "$cap" | grep -qF "● main"; then
		agents_swept="$cap"
		break
	fi
	sleep 0.05
done
tmux kill-session -t "$S53" 2>/dev/null
echo "==== Phase 53: the Agent tool — live tree, committed cell, Ctrl+O expansion, roster sweep ===="
if [ -z "$agents_live" ]; then
	echo "FAIL: Phase 53 — the live agent group tree + footer roster never showed" >&2
	status=1
fi
if ! printf '%s' "$agents_done" | grep -qF "├ Fetch current weather and time in Warsaw"; then
	echo "FAIL: Phase 53 — the committed group cell lacks the Warsaw tree row" >&2
	status=1
fi
if ! printf '%s' "$agents_done" | grep -qF "⎿  Done"; then
	echo "FAIL: Phase 53 — the committed group cell lacks the ⎿ Done status rows" >&2
	status=1
fi
if ! printf '%s' "$agents_overlay" | grep -qF "● Agent(Fetch current weather and time in Warsaw)"; then
	echo "FAIL: Phase 53 — the Ctrl+O transcript lacks the expanded Agent cell" >&2
	status=1
fi
if ! printf '%s' "$agents_overlay" | grep -qF "⎿  Prompt:"; then
	echo "FAIL: Phase 53 — the Ctrl+O Agent cell lacks its Prompt: block" >&2
	status=1
fi
if [ -z "$agents_swept" ]; then
	echo "FAIL: Phase 53 — the finished agents never swept off the roster" >&2
	status=1
fi

# --- Phase 54: background agents + the roster selection. A "background agents"
# prompt resolves at once with `● 2 background agents launched (↓ to manage · ctrl+o to expand)`;
# the roster keeps the two running rows, ↓ opens the selection (`❯` on
# `● main`, the `↑/↓ to select · Enter to view` hint in the footer slot), a
# second ↓ moves onto an agent (`Enter to view · x to stop`), and `x` stops it
# — the row leaves at once and the red `Agent "…" was stopped by user` notice
# commits. ---
S54="${S}_bgagents"
tmux new-session -d -s "$S54" -x 100 -y 44 "$APP"
sleep 0.4
tmux send-keys -t "$S54" -l "call background agents for the weather"
sleep 0.2
tmux send-keys -t "$S54" Enter
bg_launched=""
for _ in $(seq 1 250); do
	cap="$(tmux capture-pane -t "$S54" -p)"
	if printf '%s' "$cap" | grep -qF "2 background agents launched (↓ to manage · ctrl+o to expand)" &&
		printf '%s' "$cap" | grep -qF "Done for"; then
		bg_launched="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 54: captured pane (background agents launched) ===="
printf '%s\n' "$bg_launched"
tmux send-keys -t "$S54" Down
sleep 0.3
bg_sel_main="$(tmux capture-pane -t "$S54" -p)"
echo "==== Phase 54: captured pane (❯ on ● main + select hint) ===="
printf '%s\n' "$bg_sel_main"
tmux send-keys -t "$S54" Down
sleep 0.3
bg_sel_agent="$(tmux capture-pane -t "$S54" -p)"
tmux send-keys -t "$S54" -l "x"
bg_stopped=""
for _ in $(seq 1 100); do
	cap="$(tmux capture-pane -t "$S54" -p)"
	if printf '%s' "$cap" | grep -qF "was stopped by user"; then
		bg_stopped="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 54: captured pane (x stopped the agent) ===="
printf '%s\n' "$bg_stopped"
tmux kill-session -t "$S54" 2>/dev/null
echo "==== Phase 54: background agents — launch cell, roster selection, x stop ===="
if [ -z "$bg_launched" ]; then
	echo "FAIL: Phase 54 — the background launch cell never committed" >&2
	status=1
fi
if ! printf '%s' "$bg_sel_main" | grep -qF "❯ ● main"; then
	echo "FAIL: Phase 54 — ↓ did not put the ❯ selection on ● main" >&2
	status=1
fi
if ! printf '%s' "$bg_sel_main" | grep -qF "↑/↓ to select · Enter to view"; then
	echo "FAIL: Phase 54 — the main-row selection hint is missing" >&2
	status=1
fi
if ! printf '%s' "$bg_sel_agent" | grep -qF "Enter to view · x to stop"; then
	echo "FAIL: Phase 54 — the agent-row selection hint is missing" >&2
	status=1
fi
if ! printf '%s' "$bg_sel_agent" | grep -qF "❯ ◯ general-purpose"; then
	echo "FAIL: Phase 54 — the ❯ never moved onto the agent row" >&2
	status=1
fi
if [ -z "$bg_stopped" ]; then
	echo "FAIL: Phase 54 — x did not stop the agent with the red notice" >&2
	status=1
fi

# --- Phase 55: tool permission requests (docs/permissions.md). A "permission"
# prompt makes the dummy raise a scripted `write` request and BLOCK on the gate
# — exactly as a real backend's tool thread does. The composer draft typed
# before it arrives must be stashed and handed back; the prompt must show the
# framed body, the numbered contents, the question, the three options with the
# cyan `❯` on the first, and the hint row; Tab must swap the options for the
# amend field; `2` must approve and remember, so the cell commits and the draft
# returns. ---
S55="${S}_permission"
# A long pre-stream pause so the draft below is unambiguously typed BEFORE the
# request lands — once the prompt is up it owns every key (`a` would answer it).
APP_PERM="env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=2500 $BIN"
tmux new-session -d -s "$S55" -x 100 -y 44 "$APP_PERM"
sleep 0.4
tmux send-keys -t "$S55" -l "permission demo please"
sleep 0.2
tmux send-keys -t "$S55" Enter
sleep 0.4
# Typed WHILE the request is on its way — this draft must survive the prompt.
tmux send-keys -t "$S55" -l "a draft I was typing"
perm_prompt=""
for _ in $(seq 1 250); do
	cap="$(tmux capture-pane -t "$S55" -p)"
	if printf '%s' "$cap" | grep -qF "Do you want to create hello.py?"; then
		perm_prompt="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 55: captured pane (the permission prompt over the stashed draft) ===="
printf '%s\n' "$perm_prompt"
# The hardware cursor: the option list is a menu, not a text field, so the
# frame shows **no cursor at all** — the terminal cursor is the one thing on
# screen that moves by itself, and a kitty cursor-trail animation draws every
# jump it makes. `#{cursor_flag}` is tmux's report of DECTCEM (1 shown, 0
# hidden). Where it *rests* still follows the highlight, straight down the
# options, so its return when the prompt closes starts somewhere sensible
# (docs/permissions.md).
perm_cursor_shown="$(tmux display-message -p -t "$S55" '#{cursor_flag}')"
perm_cursor_1="$(tmux display-message -p -t "$S55" '#{cursor_x} #{cursor_y}')"
perm_marker_row=$(printf '%s\n' "$perm_prompt" | grep -n '❯ 1\.' | head -1 | cut -d: -f1)
tmux send-keys -t "$S55" Down
sleep 0.4
perm_cursor_2="$(tmux display-message -p -t "$S55" '#{cursor_x} #{cursor_y}')"
perm_marker_row_2=$(tmux capture-pane -t "$S55" -p | grep -n '❯ 2\.' | head -1 | cut -d: -f1)
tmux send-keys -t "$S55" Up
sleep 0.4
echo "==== Phase 55: cursor shown=$perm_cursor_shown · seat on option 1 = ($perm_cursor_1) row ${perm_marker_row:-none} · on option 2 = ($perm_cursor_2) row ${perm_marker_row_2:-none} ===="
tmux send-keys -t "$S55" Tab
sleep 0.3
tmux send-keys -t "$S55" -l "use pathlib"
sleep 0.3
# Tab's amend field IS typed into, so the caret comes back with it.
perm_cursor_shown_amend="$(tmux display-message -p -t "$S55" '#{cursor_flag}')"
perm_amend="$(tmux capture-pane -t "$S55" -p)"
echo "==== Phase 55: captured pane (Tab's amend field) ===="
printf '%s\n' "$perm_amend"
tmux send-keys -t "$S55" Escape
sleep 0.3
tmux send-keys -t "$S55" -l "2"
perm_done=""
for _ in $(seq 1 250); do
	cap="$(tmux capture-pane -t "$S55" -p)"
	if printf '%s' "$cap" | grep -qF "Created hello.py" &&
		printf '%s' "$cap" | grep -qF "a draft I was typing"; then
		perm_done="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 55: captured pane (approved; the draft is back) ===="
printf '%s\n' "$perm_done"
# …and the composer's caret comes back with the composer.
perm_cursor_shown_after="$(tmux display-message -p -t "$S55" '#{cursor_flag}')"
# The remembered scope: a second request never asks — the turn runs straight
# through to its summary with no prompt.
tmux send-keys -t "$S55" C-c
sleep 0.2
tmux send-keys -t "$S55" -l "permission demo again"
sleep 0.2
tmux send-keys -t "$S55" Enter
perm_silent=""
for _ in $(seq 1 250); do
	cap="$(tmux capture-pane -t "$S55" -p)"
	if printf '%s' "$cap" | grep -qF "the file is written" &&
		! printf '%s' "$cap" | grep -qF "Do you want to create"; then
		perm_silent="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 55: captured pane (allow-listed — no second prompt) ===="
printf '%s\n' "$perm_silent"
tmux kill-session -t "$S55" 2>/dev/null
echo "==== Phase 55: permission prompt — draft stash/restore, options, amend, remember ===="
if [ -z "$perm_prompt" ]; then
	echo "FAIL: Phase 55 — the permission prompt never showed" >&2
	status=1
fi
if ! printf '%s' "$perm_prompt" | grep -qF "Create file"; then
	echo "FAIL: Phase 55 — the prompt lacks its coloured title" >&2
	status=1
fi
# The call that raised it stays on screen ABOVE the modal — the prompt is a
# question about something visible, not a box out of nowhere — over the same
# dim ⎿ Waiting… a batch sibling shows (Claude Code's look; the approve seam
# runs before ToolStart, so the call genuinely is waiting).
if ! printf '%s' "$perm_prompt" | grep -qF "● Write(hello.py)"; then
	echo "FAIL: Phase 55 — the pending call is hidden behind the prompt" >&2
	status=1
fi
if ! printf '%s' "$perm_prompt" | grep -qF "⎿  Waiting…"; then
	echo "FAIL: Phase 55 — the call under the prompt lacks its ⎿ Waiting… row" >&2
	status=1
fi
if ! printf '%s' "$perm_prompt" | grep -qF "#!/usr/bin/env python3"; then
	echo "FAIL: Phase 55 — the prompt lacks the numbered file body" >&2
	status=1
fi
if ! printf '%s' "$perm_prompt" | grep -qF "❯ 1. Yes"; then
	echo "FAIL: Phase 55 — the ❯ selection is missing from the first option" >&2
	status=1
fi
if ! printf '%s' "$perm_prompt" | grep -qF "2. Yes, allow all edits during this session (a)"; then
	echo "FAIL: Phase 55 — the remember option is missing (or still says shift+tab)" >&2
	status=1
fi
if ! printf '%s' "$perm_prompt" | grep -qF "Esc to cancel · Tab to amend"; then
	echo "FAIL: Phase 55 — the hint row is missing" >&2
	status=1
fi
if printf '%s' "$perm_prompt" | grep -qF "a draft I was typing"; then
	echo "FAIL: Phase 55 — the stashed draft is still on screen under the prompt" >&2
	status=1
fi
# The cursor is not shown over the options — an options list has nothing for
# one to point at, and every move it makes is drawn by a terminal cursor-trail
# animation. It returns for Tab's amend field and for the composer after.
if [ "$perm_cursor_shown" != "0" ]; then
	echo "FAIL: Phase 55 — the terminal still shows a cursor over the prompt's options (cursor_flag=$perm_cursor_shown)" >&2
	status=1
fi
if [ "$perm_cursor_shown_amend" != "1" ]; then
	echo "FAIL: Phase 55 — Tab's amend field is typed into but shows no cursor (cursor_flag=$perm_cursor_shown_amend)" >&2
	status=1
fi
if [ "$perm_cursor_shown_after" != "1" ]; then
	echo "FAIL: Phase 55 — the cursor never came back after the prompt closed (cursor_flag=$perm_cursor_shown_after)" >&2
	status=1
fi
# Its resting seat still follows the highlight: `#{cursor_y}` is 0-based and
# grep -n 1-based, so the row it rests on is the marker row minus one, at the
# one-space inset plus the two-column `❯ `.
if [ "$perm_cursor_1" != "3 $((${perm_marker_row:-0} - 1))" ]; then
	echo "FAIL: Phase 55 — the cursor rests at ($perm_cursor_1), not on the '❯ 1. Yes' row (${perm_marker_row:-none}) at column 3" >&2
	status=1
fi
if [ "$perm_cursor_2" != "3 $((${perm_marker_row_2:-0} - 1))" ]; then
	echo "FAIL: Phase 55 — ↓ moved the selection to row ${perm_marker_row_2:-none} but left the cursor resting at ($perm_cursor_2)" >&2
	status=1
fi
if [ -z "$perm_amend" ] || ! printf '%s' "$perm_amend" | grep -qF "❯ use pathlib"; then
	echo "FAIL: Phase 55 — Tab did not open the amend field" >&2
	status=1
fi
if ! printf '%s' "$perm_amend" | grep -qF "Enter to reject with this feedback"; then
	echo "FAIL: Phase 55 — the amend field's hint row is missing" >&2
	status=1
fi
if [ -z "$perm_done" ]; then
	echo "FAIL: Phase 55 — approving did not commit the cell with the draft restored" >&2
	status=1
fi
if [ -z "$perm_silent" ]; then
	echo "FAIL: Phase 55 — the allow-listed second request asked again" >&2
	status=1
fi

# --- Phase 56: Tab's amend feedback reaches the MODEL and keeps reaching it
# (docs/permissions.md). Rejecting with typed instructions must (a) record them
# on the red cell — the transcript's only trace of what was asked for — and
# (b) put the model-facing denial, feedback included, into the derived LLM
# context, so Ctrl+D shows it and every later turn still carries it. The bug
# this guards: history kept only the one-line cell text, so the instructions
# reached the model for exactly one round and then vanished. ---
S56="${S}_permission_amend"
APP_AMEND="env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=1200 $BIN"
tmux new-session -d -s "$S56" -x 100 -y 44 "$APP_AMEND"
sleep 0.4
tmux send-keys -t "$S56" -l "permission demo please"
sleep 0.2
tmux send-keys -t "$S56" Enter
amend_prompt=""
for _ in $(seq 1 250); do
	cap="$(tmux capture-pane -t "$S56" -p)"
	if printf '%s' "$cap" | grep -qF "Do you want to create hello.py?"; then
		amend_prompt="$cap"
		break
	fi
	sleep 0.05
done
# Tab, type the instructions, Enter — reject WITH feedback.
tmux send-keys -t "$S56" Tab
sleep 0.3
tmux send-keys -t "$S56" -l "use pathlib instead"
sleep 0.3
tmux send-keys -t "$S56" Enter
amend_cell=""
for _ in $(seq 1 250); do
	cap="$(tmux capture-pane -t "$S56" -p -S -60)"
	if printf '%s' "$cap" | grep -qF "User rejected write to hello.py"; then
		amend_cell="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 56: captured pane (the rejected cell carries the instructions) ===="
printf '%s\n' "$amend_cell"
# Wait for the turn to settle, then read the derived context.
for _ in $(seq 1 250); do
	cap="$(tmux capture-pane -t "$S56" -p -S -60)"
	if printf '%s' "$cap" | grep -qF "left the file alone"; then
		break
	fi
	sleep 0.05
done
tmux send-keys -t "$S56" C-d
sleep 0.6
amend_ctx="$(tmux capture-pane -t "$S56" -p -S -60)"
echo "==== Phase 56: captured pane (Ctrl+D — what the model was told) ===="
printf '%s\n' "$amend_ctx"
tmux send-keys -t "$S56" q
sleep 0.3
tmux kill-session -t "$S56" 2>/dev/null
echo "==== Phase 56: Tab amend — instructions on the cell AND in the LLM context ===="
if [ -z "$amend_prompt" ]; then
	echo "FAIL: Phase 56 — the permission prompt never showed" >&2
	status=1
fi
if [ -z "$amend_cell" ]; then
	echo "FAIL: Phase 56 — the amended rejection never committed its red cell" >&2
	status=1
fi
if ! printf '%s' "$amend_cell" | grep -qF "Instructions: use pathlib instead"; then
	echo "FAIL: Phase 56 — the typed instructions are not recorded on the rejected cell" >&2
	status=1
fi
if printf '%s' "$amend_cell" | grep -qF "STOP what you are doing"; then
	echo "FAIL: Phase 56 — the model-facing denial text leaked into the rendered cell" >&2
	status=1
fi
if ! printf '%s' "$amend_ctx" | grep -qF "STOP what you are doing"; then
	echo "FAIL: Phase 56 — the derived context lacks the model-facing denial (it replayed the cell text)" >&2
	status=1
fi
if ! printf '%s' "$amend_ctx" | grep -qF "use pathlib instead"; then
	echo "FAIL: Phase 56 — the amend feedback is missing from the LLM context (the logging bug)" >&2
	status=1
fi

# --- Phase 57: the running bullet's PULSE (docs/tool-pulse.md). A tool in
# flight no longer shows a blue `●` — it shows the permission prompt's grey,
# and in the live region that grey breathes. Colour is only half of it: the
# point is that it MOVES, which no unit test can see. Sample the painted cell
# across frames while a batch runs: the running call's bullet must take more
# than one value, its `⎿ Waiting…` sibling's must not move at all, and the
# resolved cell must still land green. ---
S57="${S}_pulse"
tmux new-session -d -s "$S57" -x 100 -y 34 "$APP"
sleep 0.4
# The dummy's `parallel` turn: three long Bash(ping …) calls, so one is
# running while the others wait — both states on screen at once.
tmux send-keys -t "$S57" -l "parallel"
sleep 0.2
tmux send-keys -t "$S57" Enter
pulse_ready=""
for _ in $(seq 1 250); do
	cap="$(tmux capture-pane -t "$S57" -p)"
	if printf '%s' "$cap" | grep -qF "Waiting…"; then
		pulse_ready="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 57: captured pane (a batch mid-flight) ===="
printf '%s\n' "$pulse_ready"
# The bullet colours, newest-frame-last. `capture-pane -e` keeps the SGR
# sequences; each bullet is `ESC[1m ESC[38;2;R;G;Bm ●`. The running call is
# the first `⎿ Running…` cell and the waiting ones follow, so sampling every
# bullet per frame and diffing frame-to-frame tells us what moved.
pulse_samples=""
for _ in $(seq 1 24); do
	frame="$(tmux capture-pane -t "$S57" -p -e 2>/dev/null |
		grep -oE $'\x1b\\[1m\x1b\\[38;2;[0-9]+;[0-9]+;[0-9]+m●' |
		grep -oE '[0-9]+;[0-9]+;[0-9]+m' | tr -d 'm' | paste -sd, -)"
	pulse_samples="$pulse_samples$frame
"
	sleep 0.08
done
echo "==== Phase 57: sampled bullet colours (one frame per line) ===="
printf '%s' "$pulse_samples"
# Per bullet slot, how many distinct **grey** shades did it take? A breathing
# bullet sweeps through many; a `⎿ Waiting…` sibling sits on exactly one (the
# flat resting grey 138;138;138), and a resolved one leaves grey entirely for
# green/red. Pure white (the assistant bullets) is excluded — the pulse peaks at
# the resting grey and only dips below it, so it never comes near white.
# Note the batch runs its calls IN TURN, so over a two-second sample several
# slots take their own turn breathing; that is the feature, not a fault.
# The "held flat" count only looks at the OPENING frames: the batch runs its
# calls in turn, so a sibling that is queued at the start takes its own turn
# breathing later. Early on it is unambiguously waiting.
pulse_stats="$(printf '%s' "$pulse_samples" | awk -F, '
	NF {
		frames++
		for (i = 1; i <= NF; i++) {
			split($i, c, ";")
			if (c[1] == c[2] && c[2] == c[3] && c[1] + 0 < 250) {
				if (!((i "," $i) in seen)) { seen[i "," $i] = 1; greys[i]++ }
				if (frames <= 8) {
					if (!((i "," $i) in early)) { early[i "," $i] = 1; egreys[i]++ }
					eflat[i] = $i
				}
			}
		}
		if (NF > n) n = NF
	}
	END {
		breathing = 0; resting = 0
		for (i = 1; i <= n; i++) {
			if (greys[i] >= 3) breathing++
			if (egreys[i] == 1 && eflat[i] == "138;138;138") resting++
		}
		print breathing, resting
	}')"
pulse_breathing="${pulse_stats% *}"
pulse_resting="${pulse_stats#* }"
echo "==== Phase 57: bullets that breathed: $pulse_breathing · bullets held flat: $pulse_resting ===="
# Let the turn finish so the resolved colour can be checked.
pulse_done=""
for _ in $(seq 1 400); do
	cap="$(tmux capture-pane -t "$S57" -p -e 2>/dev/null)"
	if printf '%s' "$cap" | grep -qF "Done for"; then
		pulse_done="$cap"
		break
	fi
	sleep 0.1
done
tmux kill-session -t "$S57" 2>/dev/null
echo "==== Phase 57: running bullet pulses grey, waiting stays flat, resolved lands green ===="
if [ -z "$pulse_ready" ]; then
	echo "FAIL: Phase 57 — the parallel batch never showed a running + waiting pair" >&2
	status=1
fi
# No blue bullet anywhere: #61AFEF is 97;175;239.
if printf '%s' "$pulse_samples" | grep -q "97;175;239"; then
	echo "FAIL: Phase 57 — a bullet is still painted the old blue" >&2
	status=1
fi
if [ "${pulse_breathing:-0}" -lt 1 ]; then
	echo "FAIL: Phase 57 — no bullet swept a range of greys (the pulse is not animating)" >&2
	status=1
fi
# …and a queued sibling stayed put the whole time it was queued: one grey, the
# flat resting one. Without this the assertion above would also pass if every
# bullet flickered indiscriminately.
if [ "${pulse_resting:-0}" -lt 1 ]; then
	echo "FAIL: Phase 57 — no ⎿ Waiting… sibling held a flat grey (waiting must not animate)" >&2
	status=1
fi
if [ -z "$pulse_done" ]; then
	echo "FAIL: Phase 57 — the batch never finished" >&2
	status=1
fi
# Green (#3FB950 = 63;185;80) still lands when a call resolves.
if ! printf '%s' "$pulse_done" | grep -q "63;185;80"; then
	echo "FAIL: Phase 57 — a resolved cell lost its green bullet" >&2
	status=1
fi

# --- Phase 58: a permission prompt COVERS the conversation instead of
# scrolling it away (docs/permissions.md). The prompt is the one inline view
# that can be as tall as the whole terminal, and the ordinary content-anchored
# growth serves it badly: opening it from a bottom-seated composer scrolls a
# screenful of chat into the terminal's scrollback, and the collapse back to
# the composer then has nothing to fill the rows it vacates — the box is left
# floating above a band of blank rows (the reported bug). The modal geometry
# never scrolls, and closing the prompt repaints the window in place, so the
# box comes back flush at the bottom with the conversation whole and committed
# exactly once. ---
S58="${S}_permission_flush"
tmux new-session -d -s "$S58" -x 80 -y 44 "$APP"
sleep 0.5
# Ordinary turns first: the conversation has to fill the screen and the box
# settle flush against the bottom — the state the bug needs.
for permflush_msg in "hello there" "tell me more about it" "and a little more"; do
	tmux send-keys -t "$S58" -l "$permflush_msg"
	sleep 0.2
	tmux send-keys -t "$S58" Enter
	sleep 0.6
	for _ in $(seq 1 300); do
		cap="$(tmux capture-pane -t "$S58" -p)"
		if ! printf '%s' "$cap" | grep -qF "esc to interrupt"; then
			break
		fi
		sleep 0.1
	done
	sleep 0.3
done
permflush_before="$(tmux capture-pane -t "$S58" -p)"
echo "==== Phase 58: pane before the prompt (box flush at the bottom) ===="
printf '%s\n' "$permflush_before"
permflush_before_row=$(printf '%s\n' "$permflush_before" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)
# The terminal's own scrollback depth, so the growth can be checked for what it
# really is: a scroll pushes rows into it one-way, and only a repaint can ever
# put them back on screen.
permflush_hist_before="$(tmux display-message -p -t "$S58" '#{history_size}')"
tmux send-keys -t "$S58" -l "permission demo please"
sleep 0.2
tmux send-keys -t "$S58" Enter
permflush_prompt=""
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S58" -p)"
	if printf '%s' "$cap" | grep -qF "Do you want to create hello.py?"; then
		permflush_prompt="$cap"
		break
	fi
	sleep 0.05
done
permflush_hist_prompt="$(tmux display-message -p -t "$S58" '#{history_size}')"
echo "==== Phase 58: the prompt open over the conversation ===="
printf '%s\n' "$permflush_prompt"
tmux send-keys -t "$S58" -l "1"
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S58" -p)"
	if printf '%s' "$cap" | grep -qF "the file is written" &&
		! printf '%s' "$cap" | grep -qF "esc to interrupt"; then
		break
	fi
	sleep 0.05
done
sleep 0.5
permflush_after="$(tmux capture-pane -t "$S58" -p)"
echo "==== Phase 58: pane after answering (the box must be back flush at the bottom) ===="
printf '%s\n' "$permflush_after"
permflush_after_row=$(printf '%s\n' "$permflush_after" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)
# …and the conversation is whole and committed EXACTLY once: the close repaint
# rebuilds the on-screen window from history, so anything the modal had already
# scrolled into scrollback would come back a second time.
permflush_hist="$(tmux capture-pane -t "$S58" -p -S -200)"
permflush_dupes=$(printf '%s\n' "$permflush_hist" | grep -cF '❯ permission demo please')
tmux kill-session -t "$S58" 2>/dev/null
echo "==== Phase 58: a permission prompt covers the conversation and gives it back ===="
if [ "${permflush_before_row:-0}" != "44" ]; then
	echo "FAIL: Phase 58 — the box was not flush at the bottom before the prompt (footer on row ${permflush_before_row:-none} of 44), so the check proves nothing" >&2
	status=1
fi
if [ -z "$permflush_prompt" ]; then
	echo "FAIL: Phase 58 — the permission prompt never showed" >&2
	status=1
fi
# The turn's own `❯ permission demo please` + spacer legitimately scroll two
# rows; the prompt's growth must add none of its own.
permflush_scrolled=$((permflush_hist_prompt - permflush_hist_before))
echo "==== Phase 58: rows the prompt pushed into scrollback: $permflush_scrolled (the user message's 2 are expected) ===="
if [ "$permflush_scrolled" -gt 4 ]; then
	echo "FAIL: Phase 58 — opening the prompt scrolled $permflush_scrolled rows into scrollback: it is growing the region the ordinary way instead of covering the conversation" >&2
	status=1
fi
if [ "${permflush_after_row:-0}" != "44" ]; then
	echo "FAIL: Phase 58 — after answering, the footer sits on row ${permflush_after_row:-none} of the 44-row pane: the box is floating above a band of blank rows the collapsed prompt left behind" >&2
	status=1
fi
if [ "${permflush_dupes:-0}" != "1" ]; then
	echo "FAIL: Phase 58 — '❯ permission demo please' appears $permflush_dupes times in the scrollback: the close repaint duplicated rows the prompt had already scrolled away" >&2
	status=1
fi


# --- Phase 59: a covering prompt REPLAYS the conversation above itself, and
# back-to-back prompts keep it whole (docs/permissions.md). On a full screen
# the modal used to cover the newest rows — the just-sent user message and the
# previous cells vanished while the prompt was up — and a parallel batch's
# second prompt (opened before any draw repaired the first one's covering)
# flushed the resolved cell against the stale full-height viewport, scrolling
# real rows away for good. Now the covering modal spans the screen and paints
# the conversation tail above the prompt, a follow-up prompt keeps doing so
# (the outstanding cover feeds the sizing), the held cell commits exactly
# once, and the close still puts the box back flush at the bottom. The dummy's
# "parallel permission" turn scripts two gated Bash calls with NO pause
# between the first cell's resolution and the second request — the hardest
# timing. ---
S59="${S}_parperm"
tmux new-session -d -s "$S59" -x 80 -y 44 "$APP"
sleep 0.5
# Fill the screen so the composer sits flush at the bottom (the covering
# precondition — a short conversation's prompt fits below and covers nothing).
for parperm_msg in "hello there" "tell me more about it" "and a little more"; do
	tmux send-keys -t "$S59" -l "$parperm_msg"
	sleep 0.2
	tmux send-keys -t "$S59" Enter
	sleep 0.6
	for _ in $(seq 1 300); do
		cap="$(tmux capture-pane -t "$S59" -p)"
		if ! printf '%s' "$cap" | grep -qF "esc to interrupt"; then
			break
		fi
		sleep 0.1
	done
	sleep 0.3
done
tmux send-keys -t "$S59" -l "parallel permission demo"
sleep 0.2
tmux send-keys -t "$S59" Enter
parperm_prompt1=""
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S59" -p)"
	if printf '%s' "$cap" | grep -qF "Do you want to proceed?"; then
		parperm_prompt1="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 59: pane with the first prompt (sudo whoami) open ===="
printf '%s\n' "$parperm_prompt1"
# Approve the first command: its cell resolves and the second request lands in
# the same frame gap (the dummy scripts no pause between them).
tmux send-keys -t "$S59" -l "1"
parperm_prompt2=""
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S59" -p)"
	if printf '%s' "$cap" | grep -qF "Ping google.com 4 times"; then
		parperm_prompt2="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 59: pane with the second prompt (ping) open ===="
printf '%s\n' "$parperm_prompt2"
tmux send-keys -t "$S59" -l "1"
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S59" -p)"
	if printf '%s' "$cap" | grep -qF "Both commands are done" &&
		! printf '%s' "$cap" | grep -qF "esc to interrupt"; then
		break
	fi
	sleep 0.05
done
sleep 0.5
parperm_final="$(tmux capture-pane -t "$S59" -p)"
echo "==== Phase 59: final pane (the conversation must be whole) ===="
printf '%s\n' "$parperm_final"
parperm_footer=$(printf '%s\n' "$parperm_final" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)
parperm_hist="$(tmux capture-pane -t "$S59" -p -S -200)"
parperm_dupes=$(printf '%s\n' "$parperm_hist" | grep -cF '❯ parallel permission demo')
tmux kill-session -t "$S59" 2>/dev/null
echo "==== Phase 59: covering prompts replay the conversation and keep it whole ===="
if [ -z "$parperm_prompt1" ]; then
	echo "FAIL: Phase 59 — the first permission prompt never showed" >&2
	status=1
fi
# While the FIRST prompt is up: the just-sent message, the previous turn, and
# both batch cells (each ⎿ Waiting…) are all still on screen above it.
if ! printf '%s' "$parperm_prompt1" | grep -qF "❯ parallel permission demo"; then
	echo "FAIL: Phase 59 — the just-sent user message is hidden while the first prompt is up" >&2
	status=1
fi
if ! printf '%s' "$parperm_prompt1" | grep -qF "and a little more"; then
	echo "FAIL: Phase 59 — the previous turn is hidden while the first prompt is up" >&2
	status=1
fi
if [ "$(printf '%s\n' "$parperm_prompt1" | grep -cF "⎿  Waiting…")" -lt 2 ]; then
	echo "FAIL: Phase 59 — the batch's two pending calls don't both show ⎿ Waiting…" >&2
	status=1
fi
if [ -z "$parperm_prompt2" ]; then
	echo "FAIL: Phase 59 — the second permission prompt never showed" >&2
	status=1
fi
# While the SECOND prompt is up: the first call's finished cell (committed in
# the gap between the prompts) and the user message are still on screen.
if ! printf '%s' "$parperm_prompt2" | grep -qF "❯ parallel permission demo"; then
	echo "FAIL: Phase 59 — the user message is hidden while the second prompt is up" >&2
	status=1
fi
if ! printf '%s' "$parperm_prompt2" | grep -qF "a terminal is required"; then
	echo "FAIL: Phase 59 — the first call's finished cell is hidden while the second prompt is up" >&2
	status=1
fi
if [ "${parperm_footer:-0}" != "44" ]; then
	echo "FAIL: Phase 59 — after the prompts the footer sits on row ${parperm_footer:-none} of 44 (the box is not back flush at the bottom)" >&2
	status=1
fi
if ! printf '%s' "$parperm_final" | grep -qF "a terminal is required"; then
	echo "FAIL: Phase 59 — the first call's cell is missing from the final conversation" >&2
	status=1
fi
if [ "${parperm_dupes:-0}" != "1" ]; then
	echo "FAIL: Phase 59 — '❯ parallel permission demo' appears $parperm_dupes times in scrollback+screen (a covered or held row was lost or duplicated)" >&2
	status=1
fi


# --- Phase 60: a RESIZE while a permission prompt is open, then the answer
# (docs/permissions.md). The resize purge-rebuilds the screen, which resets
# the modal's covering — the prompt then sits below the rebuilt tail, having
# taken its rows by the rebuild's real scroll. The close used to find no cover
# to hand back, so the collapse stranded the box above a band of blank rows
# (the "newlines under the composer after a resized prompt" bug). Now the
# close purge-rebuilds like the resize did: box flush at the bottom, each
# message committed exactly once. The answer is a reject, whose few committed
# rows can't mask a leftover hole by walking the box back down. ---
S60="${S}_permresize"
tmux new-session -d -s "$S60" -x 80 -y 44 "$APP"
sleep 0.5
for permresize_msg in "hello there" "tell me more about it" "and a little more"; do
	tmux send-keys -t "$S60" -l "$permresize_msg"
	sleep 0.2
	tmux send-keys -t "$S60" Enter
	sleep 0.6
	for _ in $(seq 1 300); do
		cap="$(tmux capture-pane -t "$S60" -p)"
		if ! printf '%s' "$cap" | grep -qF "esc to interrupt"; then
			break
		fi
		sleep 0.1
	done
	sleep 0.3
done
tmux send-keys -t "$S60" -l "permission demo please"
sleep 0.2
tmux send-keys -t "$S60" Enter
permresize_prompt=""
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S60" -p)"
	if printf '%s' "$cap" | grep -qF "Do you want to create hello.py?"; then
		permresize_prompt="$cap"
		break
	fi
	sleep 0.05
done
# Shrink the pane while the prompt is up — the purge-rebuild path — and let
# the redraw settle before answering.
tmux resize-window -t "$S60" -x 76 -y 44
sleep 0.8
permresize_resized="$(tmux capture-pane -t "$S60" -p)"
echo "==== Phase 60: pane after the mid-prompt resize ===="
printf '%s\n' "$permresize_resized"
tmux send-keys -t "$S60" -l "3"
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S60" -p)"
	if printf '%s' "$cap" | grep -qF "left the file alone" &&
		! printf '%s' "$cap" | grep -qF "esc to interrupt"; then
		break
	fi
	sleep 0.05
done
sleep 0.6
permresize_final="$(tmux capture-pane -t "$S60" -p)"
echo "==== Phase 60: final pane (the box must be back flush at the bottom) ===="
printf '%s\n' "$permresize_final"
permresize_footer=$(printf '%s\n' "$permresize_final" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)
permresize_hist="$(tmux capture-pane -t "$S60" -p -S -200)"
permresize_dupes=$(printf '%s\n' "$permresize_hist" | grep -cF '❯ permission demo please')
tmux kill-session -t "$S60" 2>/dev/null
echo "==== Phase 60: a resized prompt's close still lands the box flush at the bottom ===="
if [ -z "$permresize_prompt" ]; then
	echo "FAIL: Phase 60 — the permission prompt never showed" >&2
	status=1
fi
if ! printf '%s' "$permresize_resized" | grep -qF "Do you want to create hello.py?"; then
	echo "FAIL: Phase 60 — the prompt did not survive the resize" >&2
	status=1
fi
if [ "${permresize_footer:-0}" != "44" ]; then
	echo "FAIL: Phase 60 — after answering the resized prompt the footer sits on row ${permresize_footer:-none} of the 44-row pane: the box is floating above a band of blank rows" >&2
	status=1
fi
if [ "${permresize_dupes:-0}" != "1" ]; then
	echo "FAIL: Phase 60 — '❯ permission demo please' appears $permresize_dupes times in scrollback+screen" >&2
	status=1
fi

# --- Phase 61: CLI resume — --continue / --resume {id} / bare --resume, and
# the exit hint (docs/cli.md). Instance 1 records a turn; quitting prints
# 'Resume this session with: {bin} --resume {id}' below the restored terminal,
# the id naming the rollout file (only a session WITH history hints — an empty
# quit prints nothing). '--continue' relaunches straight into the old
# conversation (no picker, repainted inline) and appends to the SAME file;
# '--resume {id}' does the same by id; bare '--resume' boots into the picker;
# and '--continue' over an empty sessions dir fails fast on stderr (exit 1,
# no TUI). The pane must OUTLIVE the app to capture what it prints after
# restore, so each launch is wrapped in a shell that holds the pane open. ---
S61="${S}_cliresume"
CLIR_DIR="$(mktemp -d /tmp/alter-zero-smoke-cliresume-XXXXXX)"
CLIAPP="env $CFG_ENV_NOHIST ALTER_ZERO_SESSIONS_DIR=$CLIR_DIR ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux new-session -d -s "$S61" -x 80 -y 24 "$CLIAPP; echo CLI_APP_EXITED; sleep 60"
sleep 0.4
tmux send-keys -t "$S61" -l "$USER_MSG"
sleep 0.2
tmux send-keys -t "$S61" Enter
for _ in $(seq 1 80); do # instance 1, turn 1 → "Done for"
	if tmux capture-pane -t "$S61" -p -S -40 | grep -qF "Done for"; then
		break
	fi
	sleep 0.15
done
tmux send-keys -t "$S61" C-c # quit instance 1 (empty composer)
for _ in $(seq 1 40); do
	if tmux capture-pane -t "$S61" -p | grep -qF "CLI_APP_EXITED"; then
		break
	fi
	sleep 0.1
done
cli_quit_pane="$(tmux capture-pane -t "$S61" -p -S -80)"
echo "==== Phase 61: pane after quitting a recorded session (the exit hint) ===="
printf '%s\n' "$cli_quit_pane"
# The advertised id must name the recorded rollout file.
cli_hint_id="$(printf '%s\n' "$cli_quit_pane" | sed -n 's/.*--resume \([a-f0-9-]*\).*/\1/p' | tail -1)"
cli_rollout="$(find "$CLIR_DIR" -type f -name 'rollout-*.jsonl' | head -1)"
tmux kill-session -t "$S61" 2>/dev/null
# --continue: straight into the old conversation, appending to the same file.
tmux new-session -d -s "$S61" -x 80 -y 24 "$CLIAPP --continue; echo CLI_APP_EXITED; sleep 60"
cli_continue_pane=""
for _ in $(seq 1 30); do # the loaded conversation repaints at startup
	cli_continue_pane="$(tmux capture-pane -t "$S61" -p -S -80)"
	if printf '%s' "$cli_continue_pane" | grep -qF "$EXPECT_REPLY"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 61: --continue relaunch (old conversation repainted, no picker) ===="
printf '%s\n' "$cli_continue_pane"
tmux send-keys -t "$S61" -l "again please"
sleep 0.2
tmux send-keys -t "$S61" Enter
cli_continue_appended=""
for _ in $(seq 1 80); do # the follow-up (this process's turn 1 → "Done for" #2)
	cli_continue_appended="$(tmux capture-pane -t "$S61" -p -S -60)"
	if [ "$(printf '%s' "$cli_continue_appended" | grep -cF "Done for")" -ge 2 ]; then
		break
	fi
	sleep 0.15
done
sleep 0.3
cli_files_after_continue="$(find "$CLIR_DIR" -type f -name 'rollout-*.jsonl' | wc -l | tr -d ' ')"
tmux kill-session -t "$S61" 2>/dev/null
# --resume {id}: the hint's id resolves to the same session.
tmux new-session -d -s "$S61" -x 80 -y 24 "$CLIAPP --resume $cli_hint_id; echo CLI_APP_EXITED; sleep 60"
cli_resume_id_pane=""
for _ in $(seq 1 30); do
	cli_resume_id_pane="$(tmux capture-pane -t "$S61" -p -S -100)"
	if printf '%s' "$cli_resume_id_pane" | grep -qF "again please"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 61: --resume {id} relaunch ===="
printf '%s\n' "$cli_resume_id_pane"
tmux kill-session -t "$S61" 2>/dev/null
# Bare --resume: the /resume picker as the first screen.
tmux new-session -d -s "$S61" -x 80 -y 24 "$CLIAPP --resume; echo CLI_APP_EXITED; sleep 60"
cli_picker_pane=""
for _ in $(seq 1 30); do
	cli_picker_pane="$(tmux capture-pane -t "$S61" -p)"
	if printf '%s' "$cli_picker_pane" | grep -qF "R E S U M E"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 61: bare --resume (the picker as the first screen) ===="
printf '%s\n' "$cli_picker_pane"
tmux kill-session -t "$S61" 2>/dev/null
# An EMPTY session (no history) must print no hint on quit.
tmux new-session -d -s "$S61" -x 80 -y 24 "$CLIAPP; echo CLI_APP_EXITED; sleep 60"
sleep 0.6
tmux send-keys -t "$S61" C-c
for _ in $(seq 1 40); do
	if tmux capture-pane -t "$S61" -p | grep -qF "CLI_APP_EXITED"; then
		break
	fi
	sleep 0.1
done
cli_empty_quit_pane="$(tmux capture-pane -t "$S61" -p -S -40)"
echo "==== Phase 61: pane after quitting an EMPTY session (no hint expected) ===="
printf '%s\n' "$cli_empty_quit_pane"
tmux kill-session -t "$S61" 2>/dev/null
# --continue with nothing to continue: fail fast on stderr, exit 1, no TUI.
CLIR_EMPTY="$(mktemp -d /tmp/alter-zero-smoke-cliempty-XXXXXX)"
cli_nothing_msg="$(env $CFG_ENV_NOHIST ALTER_ZERO_SESSIONS_DIR="$CLIR_EMPTY" "$BIN" --continue 2>&1)"
cli_nothing_exit=$?
rm -rf "$CLIR_EMPTY" 2>/dev/null
echo "==== Phase 61: --continue with no sessions → '$cli_nothing_msg' (exit $cli_nothing_exit) ===="

# Phase 61: CLI resume — the exit hint, --continue, --resume {id}, bare --resume.
if ! printf '%s' "$cli_quit_pane" | grep -qF "Resume this session with:"; then
	echo "FAIL: Phase 61 — quitting a recorded session printed no 'Resume this session with:' hint" >&2
	status=1
fi
if [ -z "$cli_hint_id" ]; then
	echo "FAIL: Phase 61 — no '--resume {id}' command line found under the hint" >&2
	status=1
fi
case "$(basename "${cli_rollout:-none}")" in
*"$cli_hint_id"*) : ;;
*)
	echo "FAIL: Phase 61 — the hinted id '$cli_hint_id' does not name the rollout file '$(basename "${cli_rollout:-none}")'" >&2
	status=1
	;;
esac
if ! printf '%s' "$cli_continue_pane" | grep -qF "❯ $USER_MSG"; then
	echo "FAIL: Phase 61 — --continue did not repaint the saved user message inline" >&2
	status=1
fi
if ! printf '%s' "$cli_continue_pane" | grep -qF "$EXPECT_REPLY"; then
	echo "FAIL: Phase 61 — --continue did not repaint the saved assistant reply inline" >&2
	status=1
fi
if printf '%s' "$cli_continue_pane" | grep -qF "R E S U M E"; then
	echo "FAIL: Phase 61 — --continue opened the picker instead of loading directly" >&2
	status=1
fi
if ! printf '%s' "$cli_continue_appended" | grep -qF "❯ again please"; then
	echo "FAIL: Phase 61 — the follow-up turn on the --continue'd session never ran" >&2
	status=1
fi
if [ "$cli_files_after_continue" != "1" ]; then
	echo "FAIL: Phase 61 — --continue should append to the SAME rollout file, found $cli_files_after_continue files" >&2
	status=1
fi
if ! printf '%s' "$cli_resume_id_pane" | grep -qF "❯ $USER_MSG"; then
	echo "FAIL: Phase 61 — --resume {id} did not reload the session the hint advertised" >&2
	status=1
fi
if ! printf '%s' "$cli_picker_pane" | grep -qF "R E S U M E"; then
	echo "FAIL: Phase 61 — bare --resume did not boot into the session picker" >&2
	status=1
fi
if ! printf '%s' "$cli_picker_pane" | grep -qF "$USER_MSG"; then
	echo "FAIL: Phase 61 — the startup picker does not list the saved session's preview" >&2
	status=1
fi
if printf '%s' "$cli_empty_quit_pane" | grep -qF "Resume this session with:"; then
	echo "FAIL: Phase 61 — quitting an EMPTY session must not print the resume hint" >&2
	status=1
fi
if [ "$cli_nothing_exit" != "1" ]; then
	echo "FAIL: Phase 61 — --continue with no sessions should exit 1, got $cli_nothing_exit" >&2
	status=1
fi
if ! printf '%s' "$cli_nothing_msg" | grep -qF "No conversation found to continue"; then
	echo "FAIL: Phase 61 — --continue with no sessions printed '$cli_nothing_msg' instead of the fail-fast message" >&2
	status=1
fi

# --- Phase 62: a permission request that arrives UNDER the Ctrl+O overlay,
# answered after the return (docs/permissions.md). The overlay return's reflow
# rebuilds the screen with the prompt already open, which resets the modal's
# covering — the prompt sits below the rebuilt tail, holding its rows by the
# rebuild's real write. The close used to find no cover to hand back, so the
# collapse stranded the box above a band of blank rows (the "newlines at the
# bottom after answering, but only when Ctrl+O was opened first" bug — the
# resize twin Phase 60 guards, reached through the overlay instead). Now any
# rebuild under an open prompt notes itself (term's modal-rebuilt flag) and
# the close purge-rebuilds: box flush at the bottom, each message committed
# exactly once. The startup delay is stretched so Ctrl+O reliably lands in
# the pre-stream pause, BEFORE the request fires. ---
S62="${S}_permoverlay"
PERMOVERLAY_APP="env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=1200 $BIN"
tmux new-session -d -s "$S62" -x 80 -y 44 "$PERMOVERLAY_APP"
sleep 0.5
# Fill the screen so the composer sits flush at the bottom (the bug needs the
# stranded band to be visible under a full screen).
for permoverlay_msg in "hello there" "tell me more about it" "and a little more"; do
	tmux send-keys -t "$S62" -l "$permoverlay_msg"
	sleep 0.2
	tmux send-keys -t "$S62" Enter
	sleep 0.6
	for _ in $(seq 1 300); do
		cap="$(tmux capture-pane -t "$S62" -p)"
		if ! printf '%s' "$cap" | grep -qF "esc to interrupt"; then
			break
		fi
		sleep 0.1
	done
	sleep 0.3
done
permoverlay_before="$(tmux capture-pane -t "$S62" -p)"
permoverlay_before_row=$(printf '%s\n' "$permoverlay_before" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)
tmux send-keys -t "$S62" -l "permission demo please"
sleep 0.2
tmux send-keys -t "$S62" Enter
sleep 0.2
# Into the transcript overlay while the dummy still pauses — the permission
# request then lands with the alternate screen up.
tmux send-keys -t "$S62" C-o
permoverlay_overlay=""
for _ in $(seq 1 200); do
	cap="$(tmux capture-pane -t "$S62" -p)"
	if printf '%s' "$cap" | grep -qF "Write(hello.py)"; then
		permoverlay_overlay="$cap"
		break
	fi
	sleep 0.05
done
sleep 0.3
echo "==== Phase 62: overlay up with the request pending underneath ===="
printf '%s\n' "$permoverlay_overlay" | tail -8
# Back to the inline view: the return repaint must show the waiting prompt.
tmux send-keys -t "$S62" C-o
permoverlay_prompt=""
for _ in $(seq 1 100); do
	cap="$(tmux capture-pane -t "$S62" -p)"
	if printf '%s' "$cap" | grep -qF "Do you want to create hello.py?"; then
		permoverlay_prompt="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 62: the prompt after the overlay return ===="
printf '%s\n' "$permoverlay_prompt"
tmux send-keys -t "$S62" -l "1"
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S62" -p)"
	if printf '%s' "$cap" | grep -qF "the file is written" &&
		! printf '%s' "$cap" | grep -qF "esc to interrupt"; then
		break
	fi
	sleep 0.05
done
sleep 0.6
permoverlay_final="$(tmux capture-pane -t "$S62" -p)"
echo "==== Phase 62: final pane (the box must be back flush at the bottom) ===="
printf '%s\n' "$permoverlay_final"
permoverlay_footer=$(printf '%s\n' "$permoverlay_final" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)
permoverlay_hist="$(tmux capture-pane -t "$S62" -p -S -200)"
permoverlay_dupes=$(printf '%s\n' "$permoverlay_hist" | grep -cF '❯ permission demo please')
tmux kill-session -t "$S62" 2>/dev/null
echo "==== Phase 62: a prompt raised under the overlay still closes flush ===="
if [ "${permoverlay_before_row:-0}" != "44" ]; then
	echo "FAIL: Phase 62 — the box was not flush at the bottom before the turn (footer on row ${permoverlay_before_row:-none} of 44), so the check proves nothing" >&2
	status=1
fi
if [ -z "$permoverlay_overlay" ]; then
	echo "FAIL: Phase 62 — the pending Write call never showed inside the overlay (the request did not land under it)" >&2
	status=1
fi
if [ -z "$permoverlay_prompt" ]; then
	echo "FAIL: Phase 62 — the permission prompt never showed after the overlay return" >&2
	status=1
fi
if [ "${permoverlay_footer:-0}" != "44" ]; then
	echo "FAIL: Phase 62 — after answering, the footer sits on row ${permoverlay_footer:-none} of the 44-row pane: the box is floating above the blank band the collapsed prompt left (the Ctrl+O-first newlines bug)" >&2
	status=1
fi
if [ "${permoverlay_dupes:-0}" != "1" ]; then
	echo "FAIL: Phase 62 — '❯ permission demo please' appears $permoverlay_dupes times in scrollback+screen (the close rebuild lost or duplicated rows)" >&2
	status=1
fi

if [ "$status" -eq 0 ]; then
	echo "PASS: reply + tools streamed to scrollback, the cursor stays visible on the prompt row mid-stream, the input box grows and stays flush at the bottom after a reply, typing bursts render in one repaint, Ctrl+O opens the tool-output view, the slash-command palette opens and runs commands, Esc interrupts a streaming turn, Ctrl+C clears a draft before /quit exits, Up recalls the last sent message for resubmission, ? toggles the shortcuts band, messages submitted mid-turn queue (all shown) and batch-send as the next turn (Esc sends the backlog right away, Alt+Up pulls the last batch back to edit, and Tab queues a message as a separate follow-up turn that runs after the first queue, and a !command queued mid-turn runs locally as its own standalone shell turn after — never sent to the backend as text), the session footer ({model} · {cwd}) sits under the box except while a band is open, every scrollback commit clears+repaints the live region inside one synchronized frame (no flicker), /clear mid-turn kills the generation and blanks the screen (nothing streams in afterwards), a resize — height-only included, mid-stream included — re-presents the conversation at the new size with a single input box, Ctrl+R reverse-searches the input history (typed queries preview matches in the composer, Enter accepts, Esc cancels without quitting), and !commands run locally (the bang is absorbed into a '! cmd' prompt with a Shell mode hint, the run commits as a codex-style exec cell — the dark '! cmd' header with its ⎿ output flush below, ⎿ Running… (Ns) while it runs (no spinner status line — the elapsed rides the preview), no summary — a non-zero exit reports its status, Esc interrupts a long one (resolving ⎿ Interrupted by user with no 'Conversation interrupted' notice), multi-line output shows a 4-line ⎿ preview with a '+N lines (ctrl+o to expand)' hint, and a huge output is capped in memory — no temp file, peak RSS bounded — with a '…' truncation marker at the end of the Ctrl+O view), and the dummy AI pauses before streaming so the status indicator shows first — the just-sent user message counted as ↑ tokens during the pause, flipping to ↓ once the reply streams, and Ctrl+J inserts a newline (the universal Shift+Enter fallback) so the box grows and a plain Enter then submits the multi-line draft, and typing @query opens a file picker below the box (async walk+rank) whose Enter inserts the highlighted path into the composer, and a large bracketed paste collapses to a '[Pasted Content N chars]' placeholder in the composer instead of dumping the raw text (and one Backspace removes the whole placeholder atomically), and Ctrl+V pastes a clipboard image as an '[Image #N]' placeholder (here, headless with no clipboard, it fails gracefully with a red 'Failed to paste image' notice and the composer stays responsive), and a message queued mid-turn shows inside the Ctrl+O transcript view and auto-dispatches there when the turn ends (the overlay follows the new turn live), and /copy copies the last assistant response to the clipboard (an empty conversation reports 'No agent response to copy'; after a reply it confirms 'Copied last message to clipboard' and — arboard having no clipboard here — its OSC 52 fallback lands the reply text in tmux's paste buffer), and Esc Esc backtracks to a previous user message (the first idle Esc arms with an 'esc again to edit previous message' footer hint, the second opens the transcript preview whose hint row shows the backtrack keys, a further Esc steps to the older message, and Enter rewinds the conversation to that point with the message back in the composer — resubmitting it streams a fresh turn to its summary), and /resume picks up a saved session (every conversation records to a rollout JSONL file — session_meta line first, created lazily on the first user message — a later launch's /resume lists it in a full-screen picker with a humanized age and the first-user-message preview, Enter repaints the whole saved conversation inline and appends the turns that follow to the same file, /clear starts a fresh rollout so the next message lands in a new one, and the picker carries codex's Filter/Sort toolbar — 'Filter: [Cwd] All   Sort: [Updated] Created' on the search row, Tab + arrows toggling — with the selected row lit on a full-width background tint), and an Esc interrupt stays prompt even when the backend is slow to observe the cancel — under a stalled backend (ALTER_ZERO_STALL_MS, ignoring the cancel for 3s) that streamed nothing, Esc undoes the no-output turn and settles within a frame (the status line clears and 'hello there' returns to the composer, no 'Conversation interrupted' notice) because the loop detaches the thread and swaps the reply channel instead of join()ing it (the interrupt-lag fix — no UI freeze), and slash-command confirmations and soft rejections surface as transient toasts above the box that self-clear after a few seconds instead of committing scrollback bullets (/copy confirms with a toast that then vanishes; /help and /resume run mid-turn are rejected with a toast; /model and /login now open their inline pickers mid-turn since they only swap the composer, never the running turn), and a resize reflow hides the hardware cursor before it homes/clears the screen and reshows it only at the prompt seat — so a terminal cursor-trail animation (kitty) can't streak from the top when the redraw drags the cursor around, and a mid-stream Ctrl+O round trip keeps the already-streamed partial reply on the restored screen (the repaint carries the stream's committed rows and catches up on what streamed under the overlay exactly once — no vanish, no flicker, no duplicate), and Ctrl+D opens the full-screen context-debug view showing the raw LLM context window (role-tagged entries, the conversation verbatim, tool calls in the provider-native wire format — an assistant '→ name(args)' request plus a 'tool:' result entry) with q returning to the repainted conversation, and the input history PERSISTS across sessions (a message submitted in one process is written to an append-only history.jsonl and, in a fresh process against the same file, Up recalls it and Ctrl+R finds it — both ↑/↓ recall and reverse-search span sessions like codex), and a PARALLEL tool-call batch is visible and clear — the model's calls are announced up front so the running one shows live while the not-yet-run ones show '⎿ Waiting…' in the live region, each committing to scrollback as it finishes (a 'parallel' prompt demos three Bash(ping …) calls at once; the default turn keeps a compact Read+Bash batch; a real backend renders however many parallel calls the model requests — docs/parallel-tools.md), and a running Bash tool STREAMS and TAILS its live output — the last lines under the ⎿ gutter plus a '+N lines (Ns)' footer while it runs, collapsing to the head peek '… +N lines (ctrl+o to expand)' when it finishes (Claude-Code's running-command look — docs/tool-streaming.md), and the Ctrl+O tool-output overlay shows a running bash tool's output LIVE (unlike Claude Code, whose transcript only shows tool output once it finishes) — a running Bash(ping) cell streams into the overlay while its batch siblings still show ⎿ Waiting…, tail-followed to the frontier (docs/tool-streaming.md), and a streamed markdown TABLE previews its whole forming grid in the strip then commits the finished block in one flush at its close — the flush syncing to the collapsed strip height so the box stays flush at the bottom (no blank band beneath it — docs/table-streaming.md), and BACKGROUND SHELLS work end to end — the '(ctrl+b to run in background)' hint is delayed a few seconds (early in a run the command shows 'Running…' but no hint yet, so a fast command never flashes it — Claude-Code-style), Ctrl+B moves a running command to the background (the cell resolves '⎿ Running in the background (↓ to manage)'), the footer counts '· N shells' and ↓ LIGHTS that count on cyan (the rest of the footer intact, no band yet) so Enter is what opens the inline manager band (list → Enter details whose output box tails the live stream → x stops the shell, committing the red 'was stopped by the user' notice) and falls back to 'No tasks currently running' once every shell is gone, and a background shell killed MID-TURN surfaces immediately — the notice commits at the turn's next tool boundary, on screen while the status line still spins, landing above the turn's Done summary instead of after it (the in-flight agent reads the same note from the registry board before its next round, so a model that kills its own background task hears the outcome within the same turn — docs/background.md), and every shell child runs DETACHED from the controlling terminal — a command that opens /dev/tty (sudo's password prompt) errors at once ('No such device or address') inside its cell instead of printing the prompt over the TUI and blocking on the keyboard the event loop owns (the runner spawns through the setsid detach chain — the setsid binary, else the binary's own detached-exec helper mode — crate::subprocess, docs/shell-command.md), and the startup ASCII header banner PERSISTS — shown at launch, surviving a resize round-trip and re-shown after /clear (the Purge rebuilds re-emit it), restored by the Ctrl+O return's InPlace repaint too (it used to vanish there until the next resize or /clear), re-capped to the visible window so a long conversation's return never duplicates it in scrollback, and the Ctrl+O transcript itself opens with the banner at its top (docs/header.md), and CHECKPOINTS reset the CODE, not just the transcript (docs/checkpoint.md) — each turn snapshots the working directory into an isolated git store (never the user's real .git), so an Esc-Esc backtrack to an earlier user message reverts a `!`-mutated file to its state at that point, and a /resume of a saved session restores the working file to that session's final checkpoint even after it was diverged on disk between launches, and Ctrl+O on a resumed CODE-HEAVY session (three ~1000-line numbered HTML Write cells) opens WARM and ATOMIC — the loop-bottom transcript warm pre-renders every committed item so the open assembles instead of re-highlighting, enter_overlay only queues the switch so the first overlay frame lands in the same flush (no capture during the switch is ever a blank screen — the old flushed-blank window kitty's cursor-trail streaked up), the overlay opens tail-following with the resumed reply in view, and the return restores the inline composer (docs/tool-view-performance.md), and checkpoints REFUSE a home-directory cwd — launched with cwd == HOME (even under an explicit ALTER_ZERO_CHECKPOINTS=1) the isolated store is never created, so the session-start snapshot can no longer hash the user's whole home directory into it before the first frame (the 'alter0 hangs in ~' guard — checkpoint::cwd_allows_checkpoints refuses the home dir, its ancestors, and filesystem roots), while a project dir under the same home still checkpoints exactly as before, and /compact runs codex's summarization turn (docs/compact.md) — an empty conversation is rejected with a 'Nothing to compact' toast, a real one streams the summary INVISIBLY (the canned dummy summary never renders) and commits the cyan '● Context compacted' marker cell with the transcript above it untouched (append-only compaction), after which the Ctrl+D context-debug view derives the compacted context: the SUMMARY_PREFIX bridge carrying the summary in place of the old assistant reply, the recent user messages retained under the 20k-token budget, and AUTO-compact + the context gauge work end to end (docs/compact.md) — with a context window known (a model's /v1/models context_length, or the ALTER_ZERO_CONTEXT_WINDOW override) the footer shows a dim '{used}/{window} ({pct}%)' gauge (e.g. 1.3k/160k (0.8%)) fed by real usage frames (tokenizer estimate offline), and one turn past codex's 90%-of-window threshold makes the loop start the summarization turn ON ITS OWN at the idle boundary — the marker cell commits as '● Context compacted · {before} → {after} tokens · auto' with the transcript untouched, one attempt per user turn so an Esc'd, failed, or insufficient compaction never loops, and /init submits codex's bundled AGENTS.md-authoring prompt as a normal user turn (docs/init.md) — the full prompt commits as the user message and a mid-turn /init is rejected with the '/init is disabled while a task is in progress' toast — while the project's AGENTS.md itself LOADS INTO THE CONTEXT as codex's user-instructions fragment (docs/project-doc.md): a planted AGENTS.md shows under '# AGENTS.md instructions' in the Ctrl+D context view before any turn, backend-independent, the fragment leading the derived context and re-read at every turn start so the guide /init just wrote rides the very next turn, and TOOL PERMISSION REQUESTS gate every write/edit/bash (docs/permissions.md) — the tool thread blocks on the gate while an inline modal replaces the whole live region — the call that raised it staying visible above the modal as its '● Write(hello.py)' header over the same dim '⎿ Waiting…' a batch sibling shows (a prompt is a question about something on screen, and the approve seam runs before ToolStart, so the call genuinely is waiting — Claude Code's look) over the coloured 'Create file' title, the target path, the numbered body framed by dashed rules, the question, and '❯ 1. Yes / 2. Yes, allow all edits during this session (a) / 3. No' over an 'Esc to cancel · Tab to amend' hint row), the composer draft typed before the request lands is stashed and handed straight back when it closes, Tab swaps the options for an empty amend field ('❯ …' plus 'Enter to reject with this feedback · Esc to go back') so a rejection can carry instructions — and those instructions are KEPT: they land on the red cell as an 'Instructions: …' line (the transcript's only record of what was asked for) while the model-facing stop-and-wait text, feedback appended, rides the recorded call into the derived context, so Ctrl+D shows what the model was actually told and every later turn still carries it (it used to survive exactly one round — history kept only the one-line cell), and option 2 remembers the scope for the session so the identical request never asks twice, and a tool IN FLIGHT no longer shows a blue bullet — it shows the permission prompt's grey, and in the live region that grey BREATHES dim→bright→dim once a second (Claude-Code's running dot): sampled across frames in a real terminal the running call's bullet takes several values while its '⎿ Waiting…' siblings' stay flat, no bullet is ever painted the old blue, and a call that resolves still lands green, and a permission prompt COVERS the conversation instead of scrolling it away (docs/permissions.md) — opening one from a bottom-seated composer pushes nothing extra into scrollback, and answering it puts the box back FLUSH at the bottom with the conversation whole and each message committed exactly once (the prompt used to scroll a screenful off for good, so its collapse left the box floating above a band of blank rows), and a COVERING prompt REPLAYS the conversation above itself — the modal spans the screen with the newest messages repainted above the question, so opening a prompt on a full screen never hides the message you just sent or the cell that just finished (they used to vanish under the modal until it closed), and a parallel batch's BACK-TO-BACK prompts survive their hardest timing (the dummy's 'parallel permission' turn: two gated Bash calls, the second request landing in the same frame gap as the first cell's commit — that commit is held, not flushed against the stale full-height viewport, so nothing scrolls away mid-cover) with the conversation visible through both prompts, the resolved cell committed exactly once, and the box back flush at the bottom, and a RESIZE while a prompt is open no longer strands the box after the answer — the resize's purge reset the covering, so the close now purge-rebuilds the same way, landing the box flush at the bottom with each message committed exactly once (it used to float above the rows the collapsed prompt vacated), and a permission request that arrives UNDER the Ctrl+O overlay closes flush too — the overlay return's reflow rebuilds the screen with the prompt already open, resetting its covering the same way a mid-prompt resize does, so the close purge-rebuilds off term's modal-rebuilt note: after Ctrl+O → request lands → return → answer, the box is back flush at the bottom with the message committed exactly once (it used to strand above a band of blank rows, the 'newlines at the bottom, but only when Ctrl+O was opened first' bug), and the CLI session flags work end to end (docs/cli.md) — quitting a session that recorded a conversation prints 'Resume this session with: {bin} --resume {id}' below the restored terminal (the id naming the rollout file; an empty session prints no hint), '--continue' relaunches straight into the newest conversation recorded in this cwd (repainted inline under the banner, no picker, appending to the SAME rollout file), '--resume {id}' does the same by id, bare '--resume' boots into the session picker as the first screen, and '--continue' with nothing to continue fails fast on stderr with exit 1 and no TUI"
fi
exit "$status"
