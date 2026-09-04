#!/usr/bin/env bash
# Drive the TUI inside a real terminal (tmux): type a message, let the dummy AI
# stream, then ASSERT the rendered conversation contains the expected lines.
#
# This is the only automated coverage of main.rs (the terminal I/O boundary), so
# it asserts rather than just eyeballing: it polls for the streamed reply (no
# blind fixed sleep racing the 45ms-per-chunk stream) and exits non-zero on
# mismatch, so it can gate in CI or a pre-commit hook.
set -uo pipefail

# Every assertion below is written against the DUMMY backend's canned turns, so
# the suite must not inherit the caller's provider credentials: with a real key
# resolvable the app reaches a live provider instead, and the phases that drive
# the /model picker then race a network model-list fetch (the reported Phase 33
# failure, which reproduces exactly when OPENROUTER_API_KEY is exported and
# vanishes when it is not). Strip every `*_API_KEY` before the first launch —
# the phases that want one set it themselves, per command, which survives this.
for _key_var in $(env | sed -n 's/^\([A-Za-z0-9_]*API_KEY\)=.*/\1/p'); do
	unset "$_key_var"
done
unset _key_var
# The Ollama provider is configured by being *pointed at* rather than keyed
# (docs/ollama.md): a developer's own `OLLAMA_HOST` (or an exported
# `ALTER_ZERO_PROVIDER=ollama`) would make the /model phases fetch from a
# server the suite doesn't run instead of reading `run /login to add one`.
unset OLLAMA_HOST ALTER_ZERO_PROVIDER

BIN="${1:-target/debug/alter-zero}"
S="alterzero_smoke_$$"
USER_MSG="hello there"
# The dummy reply is deterministic per prompt (dummy_response: char-count % 3).
# "hello there" is 11 chars → responses[2], which opens with this phrase.
EXPECT_REPLY="Happy to help"
# Every canned demo reply CLOSES on the same hand-off paragraph — the two
# commands that swap the dummy for a real model (docs/dummy-backend.md). It is
# one shared sentence, pinned by
# `stream::tests::script::the_handoff_paragraph_closes_every_demo_reply`, so
# this one marker means "the turn finished streaming" whatever prompt was sent.
SETTLED_REPLY="Two commands away"
# A demo turn is about 27 rows now (the reply's two paragraphs plus the hand-off,
# and a Read cell that renders the real numbered file body — docs/dummy-backend.md),
# so a finished turn no longer fits in an 80x24 pane: its older rows scroll into
# the terminal's real scrollback, which is exactly where they belong. Assertions
# about *committed* content therefore capture the pane WITH scrollback
# (`capture-pane -S -N`); the ones about layout — where the box sits, what the
# live region shows — still read the visible screen alone.

cleanup() {
	tmux kill-session -t "$S" 2>/dev/null
	tmux kill-session -t "${S}_agentseed" 2>/dev/null
	tmux kill-session -t "${S}_bottom" 2>/dev/null
	tmux kill-session -t "${S}_overlaysilence" 2>/dev/null
	tmux kill-session -t "${S}_agentstream" 2>/dev/null
	tmux kill-session -t "${S}_agentperm" 2>/dev/null
	tmux kill-session -t "${S}_bandpreview" 2>/dev/null
	tmux kill-session -t "${S}_overlaysilence_inline" 2>/dev/null
	tmux kill-session -t "${S}_overlaysilence_Cd" 2>/dev/null
	tmux kill-session -t "${S}_overlaysilence_Co" 2>/dev/null
	tmux kill-session -t "${S}_burst" 2>/dev/null
	tmux kill-session -t "${S}_overlayquit" 2>/dev/null
	tmux kill-session -t "${S}_interrupt" 2>/dev/null
	tmux kill-session -t "${S}_quit" 2>/dev/null
	tmux kill-session -t "${S}_recall" 2>/dev/null
	tmux kill-session -t "${S}_shortcuts" 2>/dev/null
	tmux kill-session -t "${S}_longline" 2>/dev/null
	tmux kill-session -t "${S}_peekrows" 2>/dev/null
	tmux kill-session -t "${S}_peekblank" 2>/dev/null
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
	tmux kill-session -t "${S}_thinking" 2>/dev/null
	tmux kill-session -t "${S}_nothinking" 2>/dev/null
	tmux kill-session -t "${S}_settings" 2>/dev/null
	tmux kill-session -t "${S}_settings2" 2>/dev/null
	tmux kill-session -t "${S}_hooksmenu" 2>/dev/null
	tmux kill-session -t "${S}_curhide" 2>/dev/null
	tmux kill-session -t "${S}_midstream" 2>/dev/null
	tmux kill-session -t "${S}_batch" 2>/dev/null
	tmux kill-session -t "${S}_tableflush" 2>/dev/null
	tmux kill-session -t "${S}_background" 2>/dev/null
	tmux kill-session -t "${S}_bgflow" 2>/dev/null
	tmux kill-session -t "${S}_stripflow" 2>/dev/null
	tmux kill-session -t "${S}_stripstatus" 2>/dev/null
	tmux kill-session -t "${S}_staggered" 2>/dev/null
	tmux kill-session -t "${S}_bgkill" 2>/dev/null
	tmux kill-session -t "${S}_notty" 2>/dev/null
	tmux kill-session -t "${S}_header" 2>/dev/null
	tmux kill-session -t "${S}_mascot" 2>/dev/null
	[ -n "${MASC_CFG:-}" ] && rm -rf "$MASC_CFG" 2>/dev/null
	tmux kill-session -t "${S}_links" 2>/dev/null
	rm -f /tmp/alter-zero-smoke-links-* 2>/dev/null
	tmux kill-session -t "${S}_ctrlofast" 2>/dev/null
	tmux kill-session -t "${S}_compact" 2>/dev/null
	tmux kill-session -t "${S}_autocompact" 2>/dev/null
	tmux kill-session -t "${S}_init" 2>/dev/null
	tmux kill-session -t "${S}_agents" 2>/dev/null
	tmux kill-session -t "${S}_bgagents" 2>/dev/null
	rm -rf /tmp/alter-zero-smoke-init-* 2>/dev/null
	[ -n "${CTRLO_DIR:-}" ] && rm -rf "$CTRLO_DIR" 2>/dev/null
	rm -f /tmp/alter-zero-shell-*.txt 2>/dev/null
	[ -n "${RESUME_DIR:-}" ] && rm -rf "$RESUME_DIR" 2>/dev/null
	[ -n "${SMOKE_CFG:-}" ] && rm -rf "$SMOKE_CFG" 2>/dev/null
	[ -n "${SMOKE_SKILLS:-}" ] && rm -rf "$SMOKE_SKILLS" 2>/dev/null
	tmux kill-session -t "${S}_holefree" 2>/dev/null
	tmux kill-session -t "${S}_holedup" 2>/dev/null
	tmux kill-session -t "${S}_permission" 2>/dev/null
	tmux kill-session -t "${S}_tasks" 2>/dev/null
	tmux kill-session -t "${S}_tasksdone" 2>/dev/null
	tmux kill-session -t "${S}_skills" 2>/dev/null
	tmux kill-session -t "${S}_skillsmenu" 2>/dev/null
	tmux kill-session -t "${S}_permission_amend" 2>/dev/null
	tmux kill-session -t "${S}_permission_flush" 2>/dev/null
	tmux kill-session -t "${S}_parperm" 2>/dev/null
	tmux kill-session -t "${S}_permresize" 2>/dev/null
	tmux kill-session -t "${S}_permoverlay" 2>/dev/null
	tmux kill-session -t "${S}_pulse" 2>/dev/null
	tmux kill-session -t "${S}_cliresume" 2>/dev/null
	tmux kill-session -t "${S}_compactdummy" 2>/dev/null
	[ -n "${CD_DIR:-}" ] && rm -rf "$CD_DIR" 2>/dev/null
	[ -n "${CLIR_DIR:-}" ] && rm -rf "$CLIR_DIR" 2>/dev/null
	tmux kill-session -t "${S}_trust" 2>/dev/null
	tmux kill-session -t "${S}_termkeys" 2>/dev/null
	tmux kill-session -t "${S}_permoverlay" 2>/dev/null
	tmux kill-session -t "${S}_permoverlayfit" 2>/dev/null
	tmux kill-session -t "${S}_permoverlaytall" 2>/dev/null
	[ -n "${TR_CFG:-}" ] && rm -rf "$TR_CFG" 2>/dev/null
	[ -n "${TR_WORK:-}" ] && rm -rf "$TR_WORK" 2>/dev/null
	[ -n "${TR_HOME:-}" ] && rm -rf "$TR_HOME" 2>/dev/null
	tmux kill-session -t "${S}_mcpcli" 2>/dev/null
	[ -n "${MCP84_CFG:-}" ] && rm -rf "$MCP84_CFG" 2>/dev/null
	[ -n "${MCP84_DIR:-}" ] && rm -rf "$MCP84_DIR" 2>/dev/null
	tmux kill-session -t "${S}_mcpera" 2>/dev/null
	[ -n "${MCP88_CFG:-}" ] && rm -rf "$MCP88_CFG" 2>/dev/null
	[ -n "${MCP88_DIR:-}" ] && rm -rf "$MCP88_DIR" 2>/dev/null
	tmux kill-session -t "${S}_cliprompt" 2>/dev/null
	[ -n "${CLIP_DIR:-}" ] && rm -rf "$CLIP_DIR" 2>/dev/null
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
# Skills (docs/skills.md) are discovered from the project's `.claude/skills`
# and the developer's own `~/.claude/skills`, so a machine that HAS skills would
# inject their `<system-reminder>` listing at the top of every context and push
# the rows later phases assert on off the visible screen (Phase 36 caught
# exactly that). ALTER_ZERO_SKILLS_DIR REPLACES the root list, so pointing it at
# an empty directory gives every phase a hermetic, skill-free session while
# leaving the feature itself on — Phase 76's demo is a dummy scenario and needs
# no skill on disk.
SMOKE_SKILLS="$(mktemp -d /tmp/alter-zero-smoke-skills-XXXXXX)"
# Agent definitions (docs/subagents.md) are discovered the same way — from the
# cwd's and the project root's `.alter-zero/agents` as well as the user's — and
# ALTER_ZERO_AGENTS_DIR replaces that whole root list for the same reason: this
# suite's cwd is a real checkout, so a definition someone left in it (a broken
# one especially, which toasts at startup) would otherwise change what every
# phase sees. The two built-ins are seeded into the temp dir at startup, so it
# is not empty — it is *known*.
SMOKE_AGENTS="$(mktemp -d /tmp/alter-zero-smoke-agents-XXXXXX)"
CFG_ENV="ALTER_ZERO_CONFIG_DIR=$SMOKE_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_SKILLS_DIR=$SMOKE_SKILLS ALTER_ZERO_AGENTS_DIR=$SMOKE_AGENTS ALTER_ZERO_PROJECT_CONFIG=0"
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
# walk can't skip rows). The marker is the demo script's `__main__` guard:
# line 14 of 15, past the inline cell's ten-row peek, so finding it proves the
# transcript expands what the cell collapsed.
overlay_deep="$overlay"
for _ in $(seq 1 20); do
	if printf '%s' "$overlay_deep" | grep -qF 'if __name__ == "__main__":'; then
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
# The return must NOT duplicate the header banner: the screen + scrollback
# keep the one committed at launch (the return flushes the overlay-queued
# commits beneath it and never rebuilds — docs/header.md; Phase 81 pins the
# stronger property, that nothing an overlay-covered turn produced is lost).
returned_full="$(tmux capture-pane -t "$S" -p -S -80)"
returned_banner_count=$(printf '%s\n' "$returned_full" | grep -cF "Alter Zero")

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
# the box, capped at 8 rows (/quit, the registry's last, starts off-window); ↑
# wraps the selection to the last row and the window scrolls /quit in
# (menu_window); running /help posts a system notice listing them. ---
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

# ↑ once to the last command: the selection WRAPS at the ends (↑ from the
# first row lands on the last, ↓ from the last on the first — every menu's
# grammar now), so a single ↑ from the top row is the count-independent "go
# to /quit, the registry's last" — no pinned command count for a later
# feature's new command to break (docs/settings.md's /settings, then
# docs/hooks-menu.md's /hooks, then docs/skills.md's /skills each broke the
# old counted walk; the over-press-↓-and-clamp idiom that replaced it died
# with the clamp — 40 ↓ now land at 40 % n). The 8-row window follows the
# selection, so the top rows leave and /quit scrolls in (menu_window).
tmux send-keys -t "$S" Up
sleep 0.3
palette_scrolled="$(tmux capture-pane -t "$S" -p)"
echo "==== captured pane (slash palette scrolled to /quit) ===="
printf '%s\n' "$palette_scrolled"

# Back to the top the same way — one ↓ from the last row wraps to row 0 (the
# window follows the selection up again) so Enter runs /help, not /quit.
tmux send-keys -t "$S" Down
sleep 0.3

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
# Wait until the reply has fully finished (its closing hand-off paragraph is
# committed) AND the screen has stopped changing — so we measure the *settled*
# layout, not a mid-stream frame (where the box legitimately sits at the bottom).
settled_prev=""
for _ in $(seq 1 60); do # up to ~12s
	tmux capture-pane -t "$S2" -p >"$TMP5"
	settled_cur="$(cat "$TMP5")"
	if printf '%s' "$settled_cur" | grep -qF "$SETTLED_REPLY" &&
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
# summary — not the stale live "( ● ) {verb}… (… tokens)" status strip. Commits
# made under the overlay queue on the viewport (invariant 4) and a normal Ctrl+O
# return flushes them, but the quit path used to skip that and exit_overlay
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
for _ in $(seq 1 134); do # up to ~20s
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
for _ in $(seq 1 134); do # up to ~20s: wait for the follow-up turn's summary
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
for _ in $(seq 1 134); do # up to ~20s: wait for turn 1 to finish ("Done for")
	if tmux capture-pane -t "$S7" -p | grep -qF "Done for"; then
		break
	fi
	sleep 0.15
done
tmux send-keys -t "$S7" Up
sleep 0.4
recalled="$(tmux capture-pane -t "$S7" -p -S -200)"
echo "==== captured pane (last message recalled with Up) ===="
printf '%s\n' "$recalled"
recall_up_count=$(printf '%s\n' "$recalled" | grep -cF "❯ $RECALL_MSG")
tmux send-keys -t "$S7" Down
sleep 0.4
recall_down_count=$(tmux capture-pane -t "$S7" -p -S -200 | grep -cF "❯ $RECALL_MSG")
tmux send-keys -t "$S7" Up # recall again …
sleep 0.3
tmux send-keys -t "$S7" Enter # … and resubmit it
resubmitted=""
for _ in $(seq 1 134); do # up to ~20s: wait for turn 2's summary
	resubmitted="$(tmux capture-pane -t "$S7" -p -S -200)"
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

# --- Phase 12: messages submitted WHILE a turn streams go INTO THAT TURN
# (codex's steering, docs/queue.md): each waits like a user message ("  ❯ …",
# two-space inset) *above* the box, and the turn takes them at its next round
# boundary — the dummy's tool boundary — where each becomes a real user bubble
# at column 0 ("❯ world", "❯ again") **while the turn is still running** (the
# status line is still up). So exactly ONE turn runs: turn 1's "Done for"
# summary appears and turn 2's "Finished for" must NOT — a backlog that waited
# for the turn to end would produce it, which is the behaviour this replaced. ---
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
# The delivery itself: both messages committed at column 0 while the status
# line still says the turn is running. A message that only landed after the
# summary is the old wait-for-the-turn behaviour.
queue_midturn=""
for _ in $(seq 1 400); do # up to ~60s: the turn reaches its first tool boundary
	frame="$(tmux capture-pane -t "$S9" -p -S -120)"
	if printf '%s' "$frame" | grep -qE '^❯ world$' &&
		printf '%s' "$frame" | grep -qE '^❯ again$' &&
		printf '%s' "$frame" | grep -qF "esc to interrupt"; then
		queue_midturn="$frame"
		break
	fi
	sleep 0.15
done
echo "==== captured pane (both queued messages taken INTO the running turn) ===="
printf '%s\n' "$queue_midturn"
queue_done=""
for _ in $(seq 1 300); do # up to ~45s: the one turn finishes
	queue_done="$(tmux capture-pane -t "$S9" -p -S -80)"
	if printf '%s' "$queue_done" | grep -qF "Done for"; then
		break
	fi
	sleep 0.15
done
# …and give a would-be second turn time to start before asserting there is none.
sleep 2
queue_done="$(tmux capture-pane -t "$S9" -p -S -80)"
echo "==== captured pane (the turn that read them, finished) ===="
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
for _ in $(seq 1 134); do # up to ~20s: the flushed "world" turn finishes
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
for _ in $(seq 1 134); do # up to ~20s: the whole turn (text + thinking + tools)
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
for _ in $(seq 1 134); do # up to ~20s: wait for the fresh turn's summary
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
for _ in $(seq 1 200); do # up to ~20s: wait for the turn's committed summary
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
resize_regrown="$(tmux capture-pane -t "$S14" -p -S -60)"
echo "==== captured pane (+scrollback) after height grow back to 80x24 ===="
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
for _ in $(seq 1 134); do # up to ~20s: wait for the resized turn's summary
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
for _ in $(seq 1 200); do # up to ~20s: the turn must finish first
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
search_match="$(tmux capture-pane -t "$S15" -p -S -60)"
echo "==== captured pane (query 'alpha' typed — newest match previews) ===="
printf '%s\n' "$search_match"
tmux send-keys -t "$S15" C-r
sleep 0.3
search_older="$(tmux capture-pane -t "$S15" -p -S -60)"
echo "==== captured pane (Ctrl+R again — older match) ===="
printf '%s\n' "$search_older"
tmux send-keys -t "$S15" Enter
sleep 0.3
search_accept="$(tmux capture-pane -t "$S15" -p -S -60)"
echo "==== captured pane (Enter — match accepted as a draft) ===="
printf '%s\n' "$search_accept"
tmux send-keys -t "$S15" C-r
sleep 0.2
tmux send-keys -t "$S15" -l "zzz"
sleep 0.3
search_nomatch="$(tmux capture-pane -t "$S15" -p -S -60)"
echo "==== captured pane (query 'zzz' — no match) ===="
printf '%s\n' "$search_nomatch"
tmux send-keys -t "$S15" Escape
sleep 0.3
search_cancel="$(tmux capture-pane -t "$S15" -p -S -60)"
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
# 21 chars → responses[0], which opens with this phrase.
DELAY_REPLY="Sure thing"
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
# unlike Enter which hands it to the turn already running. Submit "hello there",
# then queue "world" with Enter and "later" with TAB while turn 1 streams: both
# show inset above the box ("  ❯ world", "  ❯ later"), divided by a blank
# boundary. "world" is read by turn 1 itself, and "later" then runs as a
# SEPARATE turn 2 ("Finished for") — the extra turn Phase 12's all-Enter drive
# never produces. ---
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
for _ in $(seq 1 300); do # up to ~45s: turn 1, then the SEPARATE Tab turn 2
	tabqueue="$(tmux capture-pane -t "$S18" -p -S -100)"
	if printf '%s' "$tabqueue" | grep -qF "Finished for"; then
		break
	fi
	sleep 0.15
done
echo "==== captured pane (the Tab follow-up ran as a separate second turn) ===="
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
# text message) and "!echo smoke_queue_ok" (Enter in shell mode, a standalone
# shell turn). Both show inset above the box ("  ❯ world", "  ! echo
# smoke_queue_ok"); then the running turn reads "world" at its round boundary
# and the command runs LOCALLY as the next turn, committing an exec cell
# ("! echo …" header + "⎿ smoke_queue_ok" output) — never a "❯ !echo …" user
# message. ---
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
tmux send-keys -t "$S21" Enter # streaming → into the turn already running
tmux send-keys -t "$S21" -l "!echo smoke_queue_ok"
sleep 0.2
tmux send-keys -t "$S21" Enter # streaming → a STANDALONE shell entry (local, next turn)
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
for _ in $(seq 1 300); do # up to ~45s: turn 1 (reading "world"), then the LOCAL shell turn
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
# query (fetched asynchronously by a background walk+rank worker) in columned
# `→ name  parent/  File|Dir` rows; Enter inserts the highlighted path into the
# composer, replacing the `@token`; and the walk is per-query, so a file created
# after startup shows up too. Launch in a temp dir with known files so the match
# set is deterministic. ---
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
# A file created AFTER startup (what the agent does when asked to create one)
# must appear in a later `@` search: the worker walks the cwd afresh per query
# instead of serving a startup-cached list (docs/file-search.md).
: >"$ATDIR/gamma_new.txt"
tmux send-keys -t "$S22" -l " @gamma"
at_fresh=""
for _ in $(seq 1 30); do # up to ~3s for the fresh walk to list the new file
	at_fresh="$(tmux capture-pane -t "$S22" -p)"
	if printf '%s' "$at_fresh" | grep -qF "gamma_new.txt"; then
		break
	fi
	sleep 0.1
done
echo "==== captured pane (@ picker lists a file created after startup) ===="
printf '%s\n' "$at_fresh"
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

# A bracketed paste into the /model picker's type-to-search FILTERS the list
# (docs/llm.md): a model id is copied far more often than typed, and the paste
# used to be swallowed outright. The dummy backend has no provider key, so the
# picker shows its /login hint — the assertion is that the pasted text lands in
# the search field (it echoes on the `❯` filter row), not in the composer
# draft underneath.
tmux send-keys -t "$S23" -l "/model"
sleep 0.3
tmux send-keys -t "$S23" Enter
sleep 0.6
tmux set-buffer -- "openai/gpt-4o-mini"
tmux paste-buffer -p -t "$S23"
model_paste=""
for _ in $(seq 1 25); do # up to ~2.5s for the filter row to redraw
	model_paste="$(tmux capture-pane -t "$S23" -p)"
	if printf '%s' "$model_paste" | grep -qF "openai/gpt-4o-mini"; then
		break
	fi
	sleep 0.1
done
echo "==== captured pane (bracketed paste into the /model search) ===="
printf '%s\n' "$model_paste"
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
# Now send a message and let the dummy reply finish (its closing hand-off
# committed AND the screen settled, so the final segment is in history).
tmux send-keys -t "$S25" -l "hello there"
sleep 0.2
tmux send-keys -t "$S25" Enter
copy_prev=""
for _ in $(seq 1 60); do # up to ~12s
	copy_cur="$(tmux capture-pane -t "$S25" -p)"
	if printf '%s' "$copy_cur" | grep -qF "$SETTLED_REPLY" && [ "$copy_cur" = "$copy_prev" ]; then
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
# inline strip's inset "  ❯ world" row; when the turn reaches its next round
# boundary UNDER the overlay it takes the message right there (the loop keeps
# draining reply events with the overlay up — invariant 4): the overlay gains
# the real column-0 "❯ world" user entry and the turn runs on to its "Done for"
# summary, all without leaving the overlay. The Ctrl+O return then repaints the
# inline conversation with the whole turn. ---
S29="${S}_queueoverlay"
tmux new-session -d -s "$S29" -x 80 -y 60 "$APP" # 60 rows: two demo turns
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
# The turn reaches its round boundary under the overlay → it takes "world"
# right there, which becomes a real transcript user entry, and runs to its
# summary.
overlay_advanced=""
for _ in $(seq 1 300); do # up to ~45s: the turn reads it, then finishes
	overlay_advanced="$(tmux capture-pane -t "$S29" -p)"
	if printf '%s' "$overlay_advanced" | grep -qF "Done for"; then
		break
	fi
	sleep 0.15
done
echo "==== captured overlay (queued message taken under the overlay) ===="
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
for _ in $(seq 1 134); do # up to ~20s: turn 1 runs to its "Done for" summary
	if tmux capture-pane -t "$S30" -p -S -40 | grep -qF "Done for"; then
		break
	fi
	sleep 0.15
done
tmux send-keys -t "$S30" -l "beta question"
sleep 0.2
tmux send-keys -t "$S30" Enter
for _ in $(seq 1 134); do # turn 2 → "Finished for"
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
for _ in $(seq 1 200); do # up to ~30s: turn 3 → "Completed for"
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
for _ in $(seq 1 134); do # instance 1, turn 1 → "Done for"
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
resume_loaded="$(tmux capture-pane -t "$S31" -p -S -60)"
echo "==== captured pane (Enter — the saved conversation repainted inline) ===="
printf '%s\n' "$resume_loaded"
tmux send-keys -t "$S31" -l "again please"
sleep 0.2
tmux send-keys -t "$S31" Enter
resume_appended=""
for _ in $(seq 1 134); do # the follow-up turn (this process's turn 1 → "Done for" #2)
	resume_appended="$(tmux capture-pane -t "$S31" -p -S -200)"
	if [ "$(printf '%s' "$resume_appended" | grep -cF "Done for")" -ge 2 ]; then
		break
	fi
	sleep 0.15
done
echo "==== captured pane (follow-up turn on the resumed session) ===="
printf '%s\n' "$resume_appended"
resume_files_after_append="$(find "$RESUME_DIR" -type f -name 'rollout-*.jsonl' | wc -l | tr -d ' ')"
# 20 KB, not 2: one turn's records now carry a numbered read and a diff, so a
# small tail no longer reaches back to the user message that opened it.
resume_appended_tail="$(tail -c 20000 "$resume_first_file" 2>/dev/null)"
tmux send-keys -t "$S31" -l "/clear"
sleep 0.3
tmux send-keys -t "$S31" Enter
sleep 0.4
tmux send-keys -t "$S31" -l "fresh session"
sleep 0.2
tmux send-keys -t "$S31" Enter
for _ in $(seq 1 134); do # post-/clear turn (this process's turn 2 → "Finished for")
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
	if printf '%s' "$toast_cur" | grep -qF "$SETTLED_REPLY" && [ "$toast_cur" = "$toast_prev" ]; then
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
# And it replaces the COMPOSER ONLY: the spinner status line stays above it
# (the reported bug was the picker taking the whole region and hiding the
# running turn — docs/llm.md, the ↓ manager band's rule).
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
# Esc closes the picker; /login opens the same way over the same live turn.
tmux send-keys -t "$S33" Escape
sleep 0.3
tmux send-keys -t "$S33" -l "/login"
sleep 0.3
tmux send-keys -t "$S33" Enter
toast_login=""
for _ in $(seq 1 15); do # up to ~1.5s (still inside the 2s pause)
	toast_login="$(tmux capture-pane -t "$S33" -p)"
	# The flow's ROOT, not the provider list: `/login` asks how you sign in
	# first (Phase 103), so the provider step's `Keys are saved to` hint is a
	# step further in and never appears on the page this phase opens.
	if printf '%s' "$toast_login" | grep -qF "Use a subscription"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 33: pane with /login opened mid-turn ===="
printf '%s\n' "$toast_login"
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
for _ in $(seq 1 200); do # up to ~20s for the committed summary (a seated box)
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
midstream_returned="$(tmux capture-pane -t "$S35" -p -S -60)"
echo "==== Phase 35: returned from Ctrl+O mid-stream (thinking pause still open) ===="
printf '%s\n' "$midstream_returned"
# Let the turn finish, then check the reply committed exactly ONCE — the return's
# catch-up must not re-insert rows the screen already holds.
midstream_done=""
for _ in $(seq 1 200); do # up to ~20s
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
for _ in $(seq 1 200); do # let the turn finish so the tools are in history
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
for _ in $(seq 1 200); do # ~20s cap; the table reply streams in about eight
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
# and, that being the last shell, the manager CLOSES itself: the composer and
# the shell-less footer come back with no empty `No tasks currently running`
# page left asking to be dismissed (docs/background.md). ---
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
# the manager closes itself, the composer's prompt row returning in its place.
tmux send-keys -t "$S42" x
bg_stopped_pane=""
for _ in $(seq 1 40); do
	bg_stopped_pane="$(tmux capture-pane -t "$S42" -p -S -40)"
	if printf '%s' "$bg_stopped_pane" | grep -qF "was stopped by the user" \
		&& ! printf '%s' "$bg_stopped_pane" | grep -qF "Shell details"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 42: stopped notice + the band closed behind it ===="
printf '%s\n' "$bg_stopped_pane"
# The band is gone on its own — no Esc needed — and the footer stopped counting.
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
# the footer indicator and Enter opens the manager over the in-flight turn, and
# x stops the shell — which, it being the last one, closes the band on its own
# (docs/background.md). No Esc afterwards: with the band already gone that key
# would reach the composer and interrupt the very turn this phase measures.
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
if ! printf '%s' "$pane" | grep -qE "dummy_model_name · .*manual$"; then
	echo "FAIL: the session footer ('dummy_model_name · … manual' — the mode flush at the right edge) is not shown under the input box" >&2
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
# Idle with a previous user message, Esc begins the backtrack preview — the
# closing hint must say so instead of promising a quit (docs/backtrack.md).
if ! printf '%s' "$overlay" | grep -qF "esc to edit prev"; then
	echo "FAIL: the overlay's closing hint does not tell the truth about Esc (expected 'esc to edit prev' while idle with a previous user message)" >&2
	status=1
fi
if ! printf '%s' "$overlay" | grep -qF "$USER_MSG"; then
	echo "FAIL: tool-output view did not include the user/AI conversation" >&2
	status=1
fi
if ! printf '%s' "$overlay_deep" | grep -qF 'if __name__ == "__main__":'; then
	echo "FAIL: tool-output view did not show the full (expanded) Read output" >&2
	status=1
fi
if [ "${returned_banner_count:-0}" -gt 1 ]; then
	echo "FAIL: the Ctrl+O return duplicated the header banner over a long conversation — expected at most 1 'Alter Zero' in screen+scrollback, got ${returned_banner_count:-0}" >&2
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
# Typing "/" lists the commands below the box (their descriptions are unique to
# the open palette) — the first 8 only, MENU_MAX_ROWS being the cap; everything
# past that starts off-window and ↓ scrolls it in. The assertions below name
# commands by position-independent facts (the 8th is the window's last row,
# /quit is the registry's last entry) rather than by ordinal.
if ! printf '%s' "$palette_open" | grep -qF "List the available commands"; then
	echo "FAIL: typing '/' did not open the command palette (/help missing)" >&2
	status=1
fi
if ! printf '%s' "$palette_open" | grep -qF "Clear the conversation"; then
	echo "FAIL: the command palette did not list /clear" >&2
	status=1
fi
if ! printf '%s' "$palette_open" | grep -qF "Add or update a provider API key"; then
	echo "FAIL: the command palette did not list /login (the 8th command, the window's last row)" >&2
	status=1
fi
if printf '%s' "$palette_open" | grep -qF "Exit the app"; then
	echo "FAIL: the palette shows /quit (the registry's last command) in its first window — the 8-row cap is gone" >&2
	status=1
fi
if ! printf '%s' "$palette_scrolled" | grep -qF "Exit the app"; then
	echo "FAIL: ↓ to the last command did not scroll /quit into the palette window" >&2
	status=1
fi
if printf '%s' "$palette_scrolled" | grep -qF "List the available commands"; then
	echo "FAIL: the scrolled palette still shows /help — the window did not move" >&2
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
if ! printf '%s' "$settled_cur" | grep -qF "$SETTLED_REPLY"; then
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
# status line whose commits were still queued when the overlay went up (the
# quit's return flushes them; it used to skip that and exit onto the strip).
# The strip's precise marker is its 'esc to interrupt' hint — the committed
# turn now legitimately contains 'tokens' (the flushed 'Thought for … tokens'
# cell), which the old broader pattern misread as the strip.
if ! printf '%s' "$overlay_done" | grep -qF "Done for"; then
	echo "FAIL: the turn never finished inside the Ctrl+O overlay (Phase 7 precondition not met — retune the timing)" >&2
	status=1
else
	if ! printf '%s' "$post_quit" | grep -qF "Done for"; then
		echo "FAIL: after quitting (Ctrl+C) from the overlay, the committed 'Done for Ns' summary was not restored to the screen — the quit path left the stale live status strip behind" >&2
		status=1
	fi
	if printf '%s' "$post_quit" | grep -qF "esc to interrupt"; then
		echo "FAIL: after quitting (Ctrl+C) from the overlay, the stale live status line ('… esc to interrupt') was still on screen instead of the settled conversation" >&2
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
if [ -z "$queue_midturn" ]; then
	echo "FAIL: the queued messages never reached the RUNNING turn — '❯ world'/'❯ again' did not commit at column 0 while the status line was still up (docs/queue.md)" >&2
	echo "---- the last frame seen ----" >&2
	tmux capture-pane -t "$S9" -p -S -120 >&2
	status=1
fi
if ! printf '%s' "$queue_done" | grep -qF "❯ world" ||
	! printf '%s' "$queue_done" | grep -qF "❯ again"; then
	echo "FAIL: the queued messages were never sent — '❯ world' and '❯ again' did not both reach scrollback" >&2
	status=1
fi
if ! printf '%s' "$queue_done" | grep -qF "Done for"; then
	echo "FAIL: the turn that read the queued messages never finished — no 'Done for' summary" >&2
	status=1
fi
if printf '%s' "$queue_done" | grep -qF "Finished for"; then
	echo "FAIL: a SECOND turn ran ('Finished for') — the queued messages must be read by the turn already running, not dispatched after it" >&2
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
if ! printf '%s' "$tabqueue" | grep -qF "Finished for"; then
	echo "FAIL: the Tab-queued 'later' did not run as a SEPARATE turn ('Finished for' = turn 2 missing) — Tab must open a follow-up turn, not go into the running one like Enter" >&2
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
# The whole conversation is back once the screen regrows: the repaint must
# rebuild the *whole* tail from history, not just the rows the shrunken screen
# showed. Read with scrollback (the resize reflow purges it and rebuilds from
# history, so what `-S` holds is exactly what this repaint wrote) — a demo turn
# is taller than 24 rows, so "rebuilt" and "on the visible screen" stopped
# meaning the same thing.
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
# The columned row layout (docs/file-search.md): the selected row carries the
# `→` marker, then the name, the `./` parent column, and the `File` kind label.
if ! printf '%s' "$at_open" | grep -qE "→ alpha_smoke\.txt +\./ +File"; then
	echo "FAIL: the @ picker row is not the columned '→ name  ./  File' layout" >&2
	status=1
fi
# The fresh-walk fix: a file created after startup appears in a later search.
if ! printf '%s' "$at_fresh" | grep -qF "gamma_new.txt"; then
	echo "FAIL: a file created after startup never appeared in the @ picker (stale startup index)" >&2
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
# … and a bracketed paste into the /model picker's search reaches the FILTER
# (docs/llm.md) — it used to be swallowed, so nothing happened at all.
if ! printf '%s' "$model_paste" | grep -qF "openai/gpt-4o-mini"; then
	echo "FAIL: a bracketed paste into the /model search never reached the filter (swallowed paste regressed)" >&2
	status=1
fi
if ! printf '%s' "$model_paste" | grep -qF "No API key yet"; then
	echo "FAIL: the /model picker was not open when the paste landed (the phase tested nothing)" >&2
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
if ! printf '%s' "$copy_clip" | grep -qF "$SETTLED_REPLY"; then
	echo "FAIL: /copy's OSC 52 fallback did not put the reply on the clipboard (tmux buffer missing the reply tail)" >&2
	status=1
fi

# Phase 29: the queued message follows into the Ctrl+O overlay and is taken
# there at the turn's round boundary — the overlay never hides (or freezes) the
# queue.
if ! printf '%s' "$queued_overlay" | grep -qF "  ❯ world"; then
	echo "FAIL: the queued message row ('  ❯ world') was missing from the Ctrl+O transcript view" >&2
	status=1
fi
if ! printf '%s' "$overlay_advanced" | grep -qF "T R A N S C R I P T"; then
	echo "FAIL: the overlay was not still open when the running turn took the queued message" >&2
	status=1
fi
if ! printf '%s' "$overlay_world" | grep -qE '^❯ world'; then
	echo "FAIL: the queued message was never taken into the turn under the overlay (no column-0 '❯ world' user entry found via Home/PageDown)" >&2
	status=1
fi
if ! printf '%s' "$overlay_advanced" | grep -qF "Done for"; then
	echo "FAIL: the turn that read the queued message never ran to its 'Done for' summary under the overlay" >&2
	status=1
fi
if ! printf '%s' "$queue_overlay_returned" | grep -qE '^❯ world'; then
	echo "FAIL: after returning from the overlay the taken 'world' message is missing from the inline view" >&2
	status=1
fi
if ! printf '%s' "$queue_overlay_returned" | grep -qF "Done for"; then
	echo "FAIL: after returning from the overlay the turn's summary is missing from the inline view" >&2
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
# Phase 33: …and it replaces the COMPOSER only — the streaming strip's status
# line stays above it, so the picker never hides the turn it was opened beside
# (the reported bug; the ↓ manager band's rule — docs/llm.md).
if ! printf '%s' "$toast_model" | grep -qF "esc to interrupt"; then
	echo "FAIL: Phase 33 the mid-turn /model picker hid the status indicator — it must replace the composer only, keeping the streaming strip above it" >&2
	status=1
fi
# Phase 33: /login opens mid-turn under the same live status line — on its
# method root (the sign-in fork, Phase 103), which is what a mid-turn `/login`
# now shows.
if ! printf '%s' "$toast_login" | grep -qF "Use a subscription"; then
	echo "FAIL: Phase 33 mid-turn /login did not open the inline onboarding flow" >&2
	status=1
fi
if ! printf '%s' "$toast_login" | grep -qF "esc to interrupt"; then
	echo "FAIL: Phase 33 the mid-turn /login flow hid the status indicator — it must replace the composer only" >&2
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
if ! printf '%s' "$ctxdebug_pane" | grep -qF 'read({"path":"about.py"})'; then
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
if printf '%s' "$bg_closed_pane" | grep -qF "No tasks currently running" \
	|| printf '%s' "$bg_closed_pane" | grep -qF "Shell details" \
	|| printf '%s' "$bg_closed_pane" | grep -qF "active shell"; then
	echo "FAIL: Phase 42 — stopping the last shell left the manager band open (it must close itself, not sit on an empty page)" >&2
	status=1
fi
if ! printf '%s' "$bg_closed_pane" | grep -qE 'dummy_model_name · '; then
	echo "FAIL: Phase 42 — the composer/footer did not come back after the band closed" >&2
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
# gradient mascot + title + cwd + hint at the top of scrollback. It is chrome
# (never in `history`), re-emitted on every full repaint — so it survives a
# resize (a width change purges scrollback and rebuilds from history, which the
# header is NOT part of, so it must be re-emitted) and re-shows after /clear (a
# fresh-start banner). The tier-independent title word is the marker (both the
# mascot tier and the narrow badge carry the literal "Alter Zero"); the
# borderless design adds no `─` rule / bare prompt / footer, so Phases 16/17
# stay green.
HEADER_MARK="Alter Zero"
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
for _ in $(seq 1 134); do # up to ~20s — the full dummy turn, tools included
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
# A demo turn is taller than a 24-row screen, so the return's repaint fills
# the screen with its tail and the banner sits just above it in scrollback —
# read both (`-S`). What this phase guards is that the banner is still there
# and still SINGLE; the count assertion below is the real check.
header_roundtrip="$(tmux capture-pane -t "$S_HEADER" -p -S -80)"
header_roundtrip_count=$(printf '%s\n' "$header_roundtrip" | grep -cF "$HEADER_MARK")
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
if [ "${header_roundtrip_count:-0}" != "1" ]; then
	echo "FAIL: Phase 45 — after a Ctrl+O round trip over a real conversation the banner should appear exactly once (saw ${header_roundtrip_count:-0})" >&2
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
for _ in $(seq 1 134); do # text turn 1 → "Done for" (checkpoint {pristine})
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
for _ in $(seq 1 134); do
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
# terminal and hundreds of MB per snapshot (the "alter-zero hangs in ~" bug).
# The
# guard (`checkpoint::cwd_scope`) disables the store when the
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
# A real turn first (every dummy reply ends on the hand-off paragraph),
# settled like Phase 28: the tail committed AND the screen stable.
tmux send-keys -t "$S50" -l "hello there"
sleep 0.2
tmux send-keys -t "$S50" Enter
compact_prev=""
for _ in $(seq 1 60); do # up to ~12s
	compact_cur="$(tmux capture-pane -t "$S50" -p)"
	if printf '%s' "$compact_cur" | grep -qF "$SETTLED_REPLY" && [ "$compact_cur" = "$compact_prev" ]; then
		break
	fi
	compact_prev="$compact_cur"
	sleep 0.2
done
tmux send-keys -t "$S50" -l "/compact"
sleep 0.3
tmux send-keys -t "$S50" Enter
compact_pane=""
for _ in $(seq 1 200); do # up to ~20s (the dummy pause + the summary stream)
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
# The budget covers the WHOLE sequence — the demo turn (which now streams a
# thinking phase too, docs/thinking-stream.md), the auto-compact turn's own
# startup pause, and its summary stream — so it is generous on purpose: at ~10s
# the turn alone could eat it and the phase failed on speed, not behaviour.
for _ in $(seq 1 250); do # up to ~25s
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
# The roster lingers after the group settles, then sweeps.
agents_swept=""
for _ in $(seq 1 900); do # AGENT_LINGER is 30s
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
# — the red `Agent "…" was stopped by user` notice commits AND the row stays
# put, red, for its long stopped linger with the hint swapped to
# `x to clear`; the second `x` is what takes it off. Then the selection's
# **memory**: Esc + ↓ comes back to the row the user last picked instead of
# restarting on `● main`, and Enter into an agent's session view is a pick too
# — the ↓ after it lands on that agent (`docs/agent-tool.md`). ---
S54="${S}_bgagents"
tmux new-session -d -s "$S54" -x 100 -y 44 "$APP"
sleep 0.4
tmux send-keys -t "$S54" -l "call background agents for the weather"
sleep 0.2
tmux send-keys -t "$S54" Enter
bg_launched=""
for _ in $(seq 1 400); do
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
# Enter opens that agent's session view, and the ↓ after it must come back to
# the SAME row — the roster remembers the last picked agent.
tmux send-keys -t "$S54" Enter
sleep 0.6
bg_view="$(tmux capture-pane -t "$S54" -p)"
echo "==== Phase 54: captured pane (the agent session view) ===="
printf '%s\n' "$bg_view"
tmux send-keys -t "$S54" Down
sleep 0.4
bg_resumed="$(tmux capture-pane -t "$S54" -p)"
echo "==== Phase 54: captured pane (↓ resumes on the last picked agent) ===="
printf '%s\n' "$bg_resumed"
# Esc back to the main session, then ↓ again — still the remembered row.
tmux send-keys -t "$S54" Escape
sleep 0.3
tmux send-keys -t "$S54" Escape
sleep 0.6
tmux send-keys -t "$S54" Down
sleep 0.4
bg_resumed_main="$(tmux capture-pane -t "$S54" -p)"
echo "==== Phase 54: captured pane (back in the main session, ↓ still resumes) ===="
printf '%s\n' "$bg_resumed_main"
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
# The stopped row does NOT leave: it lingers red (30s) with the hint swapped
# to the clear, so the user can see what they stopped.
sleep 2
bg_lingering="$(tmux capture-pane -t "$S54" -p)"
echo "==== Phase 54: captured pane (the stopped row lingers · x to clear) ===="
printf '%s\n' "$bg_lingering"
bg_rows_before="$(printf '%s' "$bg_lingering" | grep -cF "general-purpose  Fetch")"
tmux send-keys -t "$S54" -l "x"
sleep 0.6
bg_cleared="$(tmux capture-pane -t "$S54" -p)"
echo "==== Phase 54: captured pane (the second x cleared the row) ===="
printf '%s\n' "$bg_cleared"
bg_rows_after="$(printf '%s' "$bg_cleared" | grep -cF "general-purpose  Fetch")"
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
if ! printf '%s' "$bg_view" | grep -qF "Fetch current weather and time in"; then
	echo "FAIL: Phase 54 — Enter did not open the agent session view" >&2
	status=1
fi
if ! printf '%s' "$bg_resumed" | grep -qF "❯ ● general-purpose"; then
	echo "FAIL: Phase 54 — ↓ inside the view did not resume on the picked agent" >&2
	status=1
fi
if ! printf '%s' "$bg_resumed_main" | grep -qF "❯ ◯ general-purpose"; then
	echo "FAIL: Phase 54 — ↓ in the main session did not resume on the picked agent" >&2
	status=1
fi
if ! printf '%s' "$bg_lingering" | grep -qF "Enter to view · x to clear"; then
	echo "FAIL: Phase 54 — the stopped row did not swap its hint to the clear" >&2
	status=1
fi
if [ "$bg_rows_before" != "2" ]; then
	echo "FAIL: Phase 54 — the stopped row left the roster instead of lingering (rows: $bg_rows_before)" >&2
	status=1
fi
if [ "$bg_rows_after" != "1" ]; then
	echo "FAIL: Phase 54 — the second x did not clear the stopped row (rows: $bg_rows_after)" >&2
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
	if printf '%s' "$cap" | grep -qF "Wrote 8 lines to hello.py" &&
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
# Option 2 on a write prompt is the switch to edit mode, and it PERSISTS per
# project (docs/permissions.md): the file must record this cwd with mode
# "edit" before the cleanup below.
if ! grep -qF '"mode": "edit"' "$SMOKE_CFG/permissions.json" 2>/dev/null; then
	echo "FAIL: Phase 55 — option 2 did not persist the edit mode to permissions.json" >&2
	status=1
fi
# Phase 55's `2` persisted its standing approval (mode=edit for this project)
# into $SMOKE_CFG/permissions.json — drop it so the later permission phases
# (56, 58, 60, 62, 63) still get their prompts.
rm -f "$SMOKE_CFG/permissions.json"
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
if ! printf '%s' "$perm_prompt" | grep -qF "2. Yes, allow all edits during this session (shift+tab)"; then
	echo "FAIL: Phase 55 — the remember option is missing (or still says the old (a) shortcut)" >&2
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
# The footer's right-edge mode flipped with the option-2 mode switch.
if ! printf '%s' "$perm_done" | grep -qE "dummy_model_name · .*edit$"; then
	echo "FAIL: Phase 55 — the footer's right-edge mode did not flip to edit after option 2" >&2
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

# --- Phase 58: the conversation stays REACHABLE while a permission prompt is
# open (docs/permissions.md). The prompt grows like any other region — the
# chat above it scrolls into the terminal's real scrollback, so the user can
# scroll up and re-read what the model said before answering (the covering
# modal used to hold the newest screenful in NO buffer: not on screen, not in
# scrollback — the "terminal scroll is disabled while it asks" bug, worst in
# kitty). The close then purge-rebuilds off the viewport's modal-scrolled
# note, so the box still comes back flush at the bottom with the conversation
# whole and committed exactly once — the invariant this phase has always
# guarded, now reached by scrolling instead of covering. ---
S58="${S}_permission_flush"
tmux new-session -d -s "$S58" -x 80 -y 30 "$APP"
sleep 0.5
# One real turn: its reply is what the user will want to scroll back to while
# the (screen-tall) prompt is up.
tmux send-keys -t "$S58" -l "hello there"
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
sleep 0.3
echo "==== Phase 58: the prompt open (screen-tall on a 30-row pane) ===="
printf '%s\n' "$permflush_prompt"
# THE point of this phase: while the prompt is up, the earlier reply must be
# somewhere the user can scroll to — the screen or the terminal's scrollback
# (capture -S reads both) — and exactly once (no replay double-paint).
permflush_reach="$(tmux capture-pane -t "$S58" -p -S -300)"
permflush_reach_count=$(printf '%s\n' "$permflush_reach" | grep -cF "Happy to help")
echo "==== Phase 58: 'Happy to help' reachable while the prompt is open: $permflush_reach_count time(s) ===="
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
# …and the conversation is whole and committed EXACTLY once: the close's purge
# rebuild replaces screen and scrollback together, so nothing the prompt's
# growth scrolled away can come back a second time.
permflush_hist="$(tmux capture-pane -t "$S58" -p -S -300)"
permflush_dupes=$(printf '%s\n' "$permflush_hist" | grep -cF '❯ permission demo please')
permflush_reply_dupes=$(printf '%s\n' "$permflush_hist" | grep -cF "Happy to help")
tmux kill-session -t "$S58" 2>/dev/null
echo "==== Phase 58: a prompt scrolls the chat into real scrollback and closes flush ===="
if [ -z "$permflush_prompt" ]; then
	echo "FAIL: Phase 58 — the permission prompt never showed" >&2
	status=1
fi
if [ "${permflush_reach_count:-0}" != "1" ]; then
	echo "FAIL: Phase 58 — while the prompt is open the earlier reply appears $permflush_reach_count times in screen+scrollback: the user cannot scroll up to it (the covered rows live in no buffer)" >&2
	status=1
fi
if [ "${permflush_after_row:-0}" != "30" ]; then
	echo "FAIL: Phase 58 — after answering, the footer sits on row ${permflush_after_row:-none} of the 30-row pane: the box is floating above a band of blank rows" >&2
	status=1
fi
if [ "${permflush_dupes:-0}" != "1" ]; then
	echo "FAIL: Phase 58 — '❯ permission demo please' appears $permflush_dupes times in scrollback+screen after the close" >&2
	status=1
fi
if [ "${permflush_reply_dupes:-0}" != "1" ]; then
	echo "FAIL: Phase 58 — the earlier reply appears $permflush_reply_dupes times in scrollback+screen after the close (the purge rebuild lost or doubled it)" >&2
	status=1
fi


# --- Phase 59: the conversation stays VISIBLE through a batch's back-to-back
# prompts (docs/permissions.md). The prompt sits at the bottom like any other
# region, so the just-sent user message, the previous turn, and both batch
# cells are real rows above it — and the first call's resolved cell commits
# above the still-open second prompt the moment it lands (no held queue). The
# close purge-rebuilds, so the final screen is whole: box flush at the
# bottom, each message committed exactly once. The dummy's "parallel
# permission" turn scripts two gated Bash calls with NO pause between the
# first cell's resolution and the second request — the hardest timing. ---
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
# both batch cells (each ⎿ Waiting…) are all still on screen above it. The
# previous turn is checked by its REPLY (its closing hand-off paragraph, right
# above the just-sent message), not by its user message: a demo turn is a
# dozen rows taller now that its Read cell renders the real numbered file body
# (docs/dummy-backend.md), so three of them plus a screen-tall prompt no longer
# fit on a 30-row pane — the older rows scroll into real scrollback, which is
# what Phase 58 checks with `-S`. What matters here is unchanged: the rows
# above the prompt are real, visible conversation, not a covered void.
if ! printf '%s' "$parperm_prompt1" | grep -qF "❯ parallel permission demo"; then
	echo "FAIL: Phase 59 — the just-sent user message is hidden while the first prompt is up" >&2
	status=1
fi
if ! printf '%s' "$parperm_prompt1" | grep -qF "$SETTLED_REPLY"; then
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
	echo "FAIL: Phase 59 — '❯ parallel permission demo' appears $parperm_dupes times in scrollback+screen (the close rebuild lost or duplicated a row)" >&2
	status=1
fi


# --- Phase 60: a RESIZE while a permission prompt is open, then the answer
# (docs/permissions.md). The resize purge-rebuilds the screen, reseating the
# prompt below the rebuilt tail — a one-way move the plain collapse cannot
# undo, so it used to strand the box above a band of blank rows (the
# "newlines under the composer after a resized prompt" bug). The reflow notes
# it (term's modal-scrolled flag) and the close purge-rebuilds: box flush at
# the bottom, each message committed exactly once. The answer is a reject,
# whose few committed rows can't mask a leftover hole by walking the box back
# down. ---
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
for _ in $(seq 1 134); do # instance 1, turn 1 → "Done for"
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
for _ in $(seq 1 134); do # the follow-up (this process's turn 1 → "Done for" #2)
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
# rebuilds the screen with the prompt already open — a one-way reseat whose
# plain collapse used to strand the box above a band of blank rows (the
# "newlines at the bottom after answering, but only when Ctrl+O was opened
# first" bug — the resize twin Phase 60 guards, reached through the overlay
# instead). Any rebuild under an open prompt notes itself (term's
# modal-scrolled flag) and the close purge-rebuilds: box flush at the bottom,
# each message committed exactly once. The startup delay is stretched so
# Ctrl+O reliably lands in the pre-stream pause, BEFORE the request fires. ---
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


# --- Phase 63: STAGGERED back-to-back prompts — the still-open prompt stays
# flush at the screen bottom (docs/permissions.md). The dummy's "staggered
# permission" turn scripts two gated Writes whose prompts differ wildly in
# height: the first's body caps (the prompt fills the terminal), the second's
# is one line — and answering the first commits its cell and opens the second
# in the same frame gap. The pinned modal region shrinks hard, and the one-way
# scrolls that pinned it cannot refill the bottom: the flush used to seat the
# short prompt high and blank everything below, stranding it above a band of
# empty rows for as long as it asked (the reported empty-newlines bug; the
# separate-frame ordering repin-shrinks into the same band). The loop now
# purge-rebuilds the moment a pinned modal frame would seat short of the
# bottom, so the open prompt lands flush with the resolved cell above it. ---
S63="${S}_staggered"
# The prompt's frame rule, as a fixed-string needle (a `─{60}` ERE would bind
# the repeat to the glyph's final UTF-8 byte and never match).
STAG_RULE="$(printf '─%.0s' $(seq 1 60))"
tmux new-session -d -s "$S63" -x 80 -y 44 "$APP"
sleep 0.5
# Fill the screen so the region sits pinned at the bottom (a short
# conversation's floating prompt shrinks over blank rows and shows nothing).
for stagperm_msg in "hello there" "tell me more about it" "and a little more"; do
	tmux send-keys -t "$S63" -l "$stagperm_msg"
	sleep 0.2
	tmux send-keys -t "$S63" Enter
	sleep 0.6
	for _ in $(seq 1 300); do
		cap="$(tmux capture-pane -t "$S63" -p)"
		if ! printf '%s' "$cap" | grep -qF "esc to interrupt"; then
			break
		fi
		sleep 0.1
	done
	sleep 0.3
done
tmux send-keys -t "$S63" -l "staggered permission demo"
sleep 0.2
tmux send-keys -t "$S63" Enter
stagperm_prompt1=""
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S63" -p)"
	if printf '%s' "$cap" | grep -qF "Do you want to create big_module.py?"; then
		stagperm_prompt1="$cap"
		break
	fi
	sleep 0.05
done
sleep 0.3
stagperm_prompt1="$(tmux capture-pane -t "$S63" -p)"
echo "==== Phase 63: the tall (body-capped) first prompt open ===="
printf '%s\n' "$stagperm_prompt1"
# The tall prompt's closing rule sits on the pane's last row (body capped →
# the prompt fills the terminal) — the precondition that pins the region.
stagperm_rule1=$(printf '%s\n' "$stagperm_prompt1" | grep -nF "$STAG_RULE" | tail -1 | cut -d: -f1)
# Approve the tall write: its cell resolves and the tiny prompt lands in the
# same frame gap (the dummy scripts no pause between them).
tmux send-keys -t "$S63" -l "1"
stagperm_prompt2=""
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S63" -p)"
	if printf '%s' "$cap" | grep -qF "Do you want to create tiny_note.py?"; then
		stagperm_prompt2="$cap"
		break
	fi
	sleep 0.05
done
sleep 0.3
stagperm_prompt2="$(tmux capture-pane -t "$S63" -p)"
echo "==== Phase 63: the tiny second prompt open (must be flush at the bottom) ===="
printf '%s\n' "$stagperm_prompt2"
# THE point of this phase: the open prompt's closing rule is the pane's LAST
# row — no band of blank rows underneath while it asks.
stagperm_rule2=$(printf '%s\n' "$stagperm_prompt2" | grep -nF "$STAG_RULE" | tail -1 | cut -d: -f1)
tmux send-keys -t "$S63" -l "1"
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S63" -p)"
	if printf '%s' "$cap" | grep -qF "Both files are written" &&
		! printf '%s' "$cap" | grep -qF "esc to interrupt"; then
		break
	fi
	sleep 0.05
done
sleep 0.5
stagperm_final="$(tmux capture-pane -t "$S63" -p)"
echo "==== Phase 63: final pane (the box must be back flush at the bottom) ===="
printf '%s\n' "$stagperm_final"
stagperm_footer=$(printf '%s\n' "$stagperm_final" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)
stagperm_hist="$(tmux capture-pane -t "$S63" -p -S -300)"
stagperm_dupes=$(printf '%s\n' "$stagperm_hist" | grep -cF '❯ staggered permission demo')
stagperm_cell_dupes=$(printf '%s\n' "$stagperm_hist" | grep -cF 'Wrote 60 lines to big_module.py')
tmux kill-session -t "$S63" 2>/dev/null
echo "==== Phase 63: staggered prompts keep the open prompt flush at the bottom ===="
if [ -z "$stagperm_prompt1" ]; then
	echo "FAIL: Phase 63 — the tall first prompt never showed" >&2
	status=1
fi
if [ "${stagperm_rule1:-0}" != "44" ]; then
	echo "FAIL: Phase 63 — the tall prompt's closing rule sits on row ${stagperm_rule1:-none} of the 44-row pane (the body cap should fill the terminal), so the shrink check proves nothing" >&2
	status=1
fi
if [ -z "$stagperm_prompt2" ]; then
	echo "FAIL: Phase 63 — the tiny second prompt never showed" >&2
	status=1
fi
if [ "${stagperm_rule2:-0}" != "44" ]; then
	echo "FAIL: Phase 63 — while the tiny prompt is open its closing rule sits on row ${stagperm_rule2:-none} of the 44-row pane: the region shrank in place and stranded the prompt above a band of blank rows (the empty-newlines-under-the-prompt bug)" >&2
	status=1
fi
# The resolved tall cell committed above the still-open tiny prompt (visible
# at once, Phase 59's continuity), and the conversation stays whole after.
if ! printf '%s' "$stagperm_prompt2" | grep -qF "Wrote 60 lines to big_module.py"; then
	echo "FAIL: Phase 63 — the tall write's finished cell is hidden while the tiny prompt is up" >&2
	status=1
fi
if [ "${stagperm_footer:-0}" != "44" ]; then
	echo "FAIL: Phase 63 — after the prompts the footer sits on row ${stagperm_footer:-none} of 44 (the box is not back flush at the bottom)" >&2
	status=1
fi
if [ "${stagperm_dupes:-0}" != "1" ]; then
	echo "FAIL: Phase 63 — '❯ staggered permission demo' appears $stagperm_dupes times in scrollback+screen (a rebuild lost or duplicated a row)" >&2
	status=1
fi
if [ "${stagperm_cell_dupes:-0}" != "1" ]; then
	echo "FAIL: Phase 63 — the tall write's cell appears $stagperm_cell_dupes times in scrollback+screen (the mid-prompt rebuild lost or duplicated it)" >&2
	status=1
fi

# --- Phase 64: auto + master permission modes (docs/permissions.md). Shift+Tab
# cycles manual → edit → auto → master; in auto the dummy's "auto permission"
# demo is decided by the offline heuristic classifier — the read-only listing
# runs with a dim '⎿ Allowed by auto mode classifier' row on its finished
# cell, the delete is rejected red 'Denied by auto mode classifier', and NO
# prompt ever opens; the mode persists as "auto" in permissions.json; then in
# master the same demo runs both commands silently (still no prompt, no new
# denial). A fresh config dir so earlier phases' rules can't cover anything.
S64="${S}_automode"
SMOKE_CFG64="$(mktemp -d)"
APP_AUTO="env ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$SMOKE_CFG64 ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=300 $BIN"
tmux new-session -d -s "$S64" -x 100 -y 44 "$APP_AUTO"
sleep 0.4
# Two Shift+Tab steps: manual → edit → auto. The footer's right-edge segment and
# the confirming toast must both say so.
tmux send-keys -t "$S64" BTab
sleep 0.2
tmux send-keys -t "$S64" BTab
sleep 0.3
auto_footer="$(tmux capture-pane -t "$S64" -p)"
echo "==== Phase 64: captured pane (mode cycled to auto) ===="
printf '%s\n' "$auto_footer"
tmux send-keys -t "$S64" -l "auto permission demo"
sleep 0.2
tmux send-keys -t "$S64" Enter
auto_done=""
for _ in $(seq 1 250); do
	cap="$(tmux capture-pane -t "$S64" -p)"
	if printf '%s' "$cap" | grep -qF "Allowed by auto mode classifier" &&
		printf '%s' "$cap" | grep -qF "Denied by auto mode classifier" &&
		printf '%s' "$cap" | grep -qF "the delete was blocked"; then
		auto_done="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 64: captured pane (auto mode — classifier decided, no prompt) ===="
printf '%s\n' "$auto_done"
# Snapshot the persisted mode NOW — the master switch below overwrites it.
auto_json="$(cat "$SMOKE_CFG64/permissions.json" 2>/dev/null)"
# One more Shift+Tab step: auto → master.
tmux send-keys -t "$S64" BTab
sleep 0.3
master_footer="$(tmux capture-pane -t "$S64" -p)"
tmux send-keys -t "$S64" -l "auto permission demo again"
sleep 0.2
tmux send-keys -t "$S64" Enter
master_done=""
for _ in $(seq 1 250); do
	cap="$(tmux capture-pane -t "$S64" -p)"
	if printf '%s' "$cap" | grep -qF "both commands ran to completion"; then
		master_done="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 64: captured pane (master mode — everything ran silently) ===="
printf '%s\n' "$master_done"
auto_scroll="$(tmux capture-pane -t "$S64" -p -S -400)"
tmux kill-session -t "$S64" 2>/dev/null
echo "==== Phase 64: auto mode classifies, master mode bypasses ===="
if ! printf '%s' "$auto_footer" | grep -qE "auto\s*$" ||
	! printf '%s' "$auto_footer" | grep -qF "Mode: auto"; then
	echo "FAIL: Phase 64 — two Shift+Tab steps did not land the footer/toast on auto" >&2
	status=1
fi
if [ -z "$auto_done" ]; then
	echo "FAIL: Phase 64 — the auto-mode demo never resolved both classifier verdicts" >&2
	status=1
fi
if printf '%s' "$auto_scroll" | grep -qF "Do you want to proceed?"; then
	echo "FAIL: Phase 64 — a permission prompt opened in auto/master mode" >&2
	status=1
fi
if ! printf '%s' "$auto_done" | grep -qF "● Bash(ls -la)"; then
	echo "FAIL: Phase 64 — the allowed command's cell is missing" >&2
	status=1
fi
if ! printf '%s' "$auto_done" | grep -qF "⎿  Allowed by auto mode classifier"; then
	echo "FAIL: Phase 64 — the allowed cell lacks its classifier note row" >&2
	status=1
fi
if ! printf '%s' "$auto_done" | grep -qF "Reason:"; then
	echo "FAIL: Phase 64 — the denied cell lacks the classifier's reason line" >&2
	status=1
fi
if ! printf '%s' "$auto_json" | grep -qF '"mode": "auto"'; then
	echo "FAIL: Phase 64 — the auto mode did not persist to permissions.json" >&2
	status=1
fi
if ! grep -qF '"mode": "master"' "$SMOKE_CFG64/permissions.json" 2>/dev/null; then
	echo "FAIL: Phase 64 — the master mode did not persist to permissions.json" >&2
	status=1
fi
if ! printf '%s' "$master_footer" | grep -qE "master\s*$"; then
	echo "FAIL: Phase 64 — the third Shift+Tab step did not land the footer on master" >&2
	status=1
fi
if [ -z "$master_done" ]; then
	echo "FAIL: Phase 64 — the master-mode demo never completed" >&2
	status=1
fi
if [ "$(printf '%s' "$auto_scroll" | grep -cF "Denied by auto mode classifier")" != "1" ]; then
	echo "FAIL: Phase 64 — master mode denied (or re-denied) a command; only auto's single denial may exist" >&2
	status=1
fi
if [ "$(printf '%s' "$auto_scroll" | grep -cF "Allowed by auto mode classifier")" != "1" ]; then
	echo "FAIL: Phase 64 — master mode noted a classifier approval; only auto's single note may exist" >&2
	status=1
fi
rm -rf "$SMOKE_CFG64"

# --- Phase 65: /compact NEVER reaches the network when the session is running
# the DUMMY (docs/compact.md). The trap: `is_usable()` only checks that a key
# resolved, so a configured provider + NO model selection used to build a REAL
# one-off summarization backend for the model name the dummy answers as —
# firing `POST /chat/completions` for "dummy_model_name" and resolving the turn
# red instead of committing the marker. Reproduced offline with a provider whose
# base is the discard port (nothing is ever sent, and if it were the connection
# is refused at once): the footer must show the dummy, and /compact must still
# land the cyan '● Context compacted' cell. ---
S65="${S}_compactdummy"
CD_DIR="$(mktemp -d /tmp/alter-zero-smoke-compactdummy-XXXXXX)"
cat >"$CD_DIR/providers.toml" <<'PROVIDERS'
[providers.deadend]
name = "Dead End"
api_model_base = "http://127.0.0.1:9/v1"

[providers.deadend.kwargs]
api_base = "http://127.0.0.1:9/v1"
PROVIDERS
# A key resolves for the provider, but ALTER_ZERO_MODEL is unset — so the
# session falls back to the dummy while `active_model` is "dummy_model_name".
APP_CD="env ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$CD_DIR/cfg ALTER_ZERO_SESSIONS_DIR=$CD_DIR/sessions ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_PROVIDERS_FILE=$CD_DIR/providers.toml ALTER_ZERO_API_KEY=not-a-real-key ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux new-session -d -s "$S65" -x 80 -y 24 "$APP_CD"
sleep 0.6
compact_dummy_footer="$(tmux capture-pane -t "$S65" -p)"
# One real turn, settled (every dummy reply ends on the hand-off paragraph).
tmux send-keys -t "$S65" -l "hello there"
sleep 0.2
tmux send-keys -t "$S65" Enter
# Waits for the committed "Done for" SUMMARY, not for the reply text: the
# hand-off paragraph starts a second before the stream ends, and /compact is
# rejected with a toast while a turn is still in flight.
for _ in $(seq 1 200); do # up to ~20s
	if tmux capture-pane -t "$S65" -p -S -40 | grep -qE "^Done for [0-9]+s"; then
		break
	fi
	sleep 0.1
done
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
if ! printf '%s' "$compact_dummy_footer" | grep -qF "dummy_model_name"; then
	echo "FAIL: Phase 65 — the session did not fall back to the dummy (the phase tested nothing)" >&2
	status=1
fi
if printf '%s' "$compact_dummy" | grep -qF "request failed"; then
	echo "FAIL: Phase 65 — /compact sent a real request while the session runs the dummy" >&2
	status=1
fi
if ! printf '%s' "$compact_dummy" | grep -qF "Context compacted"; then
	echo "FAIL: Phase 65 — the '● Context compacted' cell never committed on the dummy" >&2
	status=1
fi
tmux kill-session -t "$S65" 2>/dev/null
rm -rf "$CD_DIR"

# --- Phase 66: the THINKING STREAM (docs/thinking-stream.md). The model's
# chain-of-thought used to be counted and thrown away; now it streams live in
# the strip under a `● Thinking…` header over the `⎿` gutter, and COLLAPSES at
# the phase's end into one bullet-less committed
# `Thought for {n} · {t} tokens (ctrl+o to expand)` line — the reasoning text
# itself never reaching scrollback (it expands in Ctrl+O instead). Driven
# against the dummy's canned two-line reasoning: poll for the live block, then
# for the settled cell, and assert the thought's text is nowhere in the
# committed scrollback. Then the same turn with ALTER_ZERO_SHOW_THINKING=0
# must show neither. ---
S66="${S}_thinking"
tmux new-session -d -s "$S66" -x 80 -y 24 "$APP"
sleep 0.5
tmux send-keys -t "$S66" -l "$USER_MSG"
sleep 0.2
tmux send-keys -t "$S66" Enter
# The live block: the header goes up at ThinkingStart and the thought trickles
# in after it (≈2s for the canned reasoning), so poll the VISIBLE screen for the
# TEXT — polling the header alone would win on the frame before the first delta
# and then assert against an empty block.
think_live=""
think_live_header=0
for _ in $(seq 1 150); do # up to ~15s
	think_live="$(tmux capture-pane -t "$S66" -p)"
	if printf '%s' "$think_live" | grep -qF "● Thinking…"; then
		think_live_header=1
	fi
	if printf '%s' "$think_live" | grep -qF "Let me read the file first."; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 66: captured pane (the live thinking block) ===="
printf '%s\n' "$think_live"
if [ "$think_live_header" -ne 1 ]; then
	echo "FAIL: Phase 66 — the live '● Thinking…' header never showed while the model thought" >&2
	status=1
fi
if ! printf '%s' "$think_live" | grep -qF "Let me read the file first."; then
	echo "FAIL: Phase 66 — the chain-of-thought did not stream into the live block" >&2
	status=1
fi
# Invariant 4, LIVE: the block goes up over a finalised segment, so the header
# sits under a blank row instead of butting against the paragraph that was
# streaming (the reported bug — the flush used to wait for the phase's end, so
# the spacer only appeared when the cell collapsed, jolting it down a row).
if ! printf '%s' "$think_live" | grep -B 1 -F "● Thinking…" | head -1 | grep -qE "^[[:space:]]*$"; then
	echo "FAIL: Phase 66 — the live '● Thinking…' header is not preceded by a blank row: the segment before it was not finalised (invariant 4)" >&2
	status=1
fi
# …then the collapsed cell, once the phase ends.
think_done=""
for _ in $(seq 1 200); do # up to ~20s
	think_done="$(tmux capture-pane -t "$S66" -p -S -80)"
	if printf '%s' "$think_done" | grep -qE "^Thought for [0-9]+s"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 66: captured pane + scrollback (the collapsed thought) ===="
printf '%s\n' "$think_done"
if ! printf '%s' "$think_done" | grep -qE "^Thought for [0-9]+s · [0-9]+ tokens \(ctrl\+o to expand\)"; then
	echo "FAIL: Phase 66 — the phase never collapsed into a bullet-less 'Thought for Ns · N tokens (ctrl+o to expand)' line" >&2
	status=1
fi
# …bullet-less: a settled thought is turn meta, so nothing may prefix it.
if printf '%s' "$think_done" | grep -qE "^[^[:space:]]+ Thought for "; then
	echo "FAIL: Phase 66 — the settled line carries a bullet; it must read like 'Done for Ns'" >&2
	status=1
fi
if printf '%s' "$think_done" | grep -qF "Then edit it and run it."; then
	echo "FAIL: Phase 66 — the chain-of-thought reached scrollback; only the collapsed cell may commit" >&2
	status=1
fi
# Invariant 4: a phase that ends MID-REPLY must finalise the assistant text
# before it (the ToolStart dance), so the cell is its own block rather than a
# line spliced into the paragraph that was streaming. The dummy's default turn
# is exactly that shape — text, then thinking — so the row above the cell must
# be blank, and the reply must resume as a fresh `● …` bullet below it.
if ! printf '%s' "$think_done" | grep -B 1 -E "^Thought for" | head -1 | grep -qE "^[[:space:]]*$"; then
	echo "FAIL: Phase 66 — the thought cell was spliced into the streaming reply instead of following a finalised segment (invariant 4)" >&2
	status=1
fi
# Ctrl+O expands it back.
tmux send-keys -t "$S66" C-o
sleep 0.6
think_overlay="$(tmux capture-pane -t "$S66" -p -S -200)"
if ! printf '%s' "$think_overlay" | grep -qF "Then edit it and run it."; then
	echo "==== Phase 66: captured overlay ===="
	printf '%s\n' "$think_overlay"
	echo "FAIL: Phase 66 — the Ctrl+O transcript did not expand the thought's chain-of-thought" >&2
	status=1
fi
tmux send-keys -t "$S66" C-o
sleep 0.3
tmux kill-session -t "$S66" 2>/dev/null

# The off switch: same turn, ALTER_ZERO_SHOW_THINKING=0 → neither the live
# block nor the collapsed cell, and the turn still settles normally.
S66B="${S}_nothinking"
APP_NOTHINK="env $CFG_ENV_NOHIST ALTER_ZERO_SHOW_THINKING=0 ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux new-session -d -s "$S66B" -x 80 -y 24 "$APP_NOTHINK"
sleep 0.5
tmux send-keys -t "$S66B" -l "$USER_MSG"
sleep 0.2
tmux send-keys -t "$S66B" Enter
nothink=""
for _ in $(seq 1 250); do # up to ~25s
	nothink="$(tmux capture-pane -t "$S66B" -p -S -80)"
	if printf '%s' "$nothink" | grep -qE "^Done for [0-9]+s"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 66: captured pane (ALTER_ZERO_SHOW_THINKING=0) ===="
printf '%s\n' "$nothink"
if ! printf '%s' "$nothink" | grep -qE "^Done for [0-9]+s"; then
	echo "FAIL: Phase 66 — the turn never settled with the thinking display off" >&2
	status=1
fi
if printf '%s' "$nothink" | grep -qE "Thinking…|Thought for"; then
	echo "FAIL: Phase 66 — ALTER_ZERO_SHOW_THINKING=0 still showed the model's thinking" >&2
	status=1
fi
tmux kill-session -t "$S66B" 2>/dev/null

# --- Phase 67: the `/settings` MENU (docs/settings.md). The session's knobs —
# until now environment variables you had to know about before launch — listed
# in the `/model` picker's inline frame, searchable, each cycled with
# Enter/Space. Drive it end to end: open it from the palette, check the frame
# and the value column, search, cycle a value, confirm the toast, and close
# back to the composer. Then a SECOND process against the same config home
# must show the changed value — the file persisted it. ---
S67="${S}_settings"
SET_CFG="$(mktemp -d)"
APP_SET="env ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$SET_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux new-session -d -s "$S67" -x 90 -y 30 "$APP_SET"
sleep 0.6
# The palette lists it (a bare `/set` token filters to it).
tmux send-keys -t "$S67" -l "/set"
sleep 0.4
settings_palette="$(tmux capture-pane -t "$S67" -p)"
echo "==== Phase 67: the palette filtered to /settings ===="
printf '%s\n' "$settings_palette"
if ! printf '%s' "$settings_palette" | grep -qF "Open settings menu"; then
	echo "FAIL: Phase 67 — /settings is missing from the slash-command palette" >&2
	status=1
fi
tmux send-keys -t "$S67" Enter
sleep 0.6
settings_open="$(tmux capture-pane -t "$S67" -p)"
echo "==== Phase 67: the settings menu open ===="
printf '%s\n' "$settings_open"
# Only rows that are ALWAYS in the opening window: the list is capped at
# SETTINGS_MENU_MAX_ROWS and scrolls with the selection, so once the knob count
# passed the cap the tail rows stopped showing on open. Same lesson the counter
# below already learned — pinning something that grows with each new feature
# fails here instead of in that feature's own phase. A row past the window is
# reached the way a user reaches it: the type-to-search this phase drives next.
for expect in "Hide thinking" "Error retry" "Permission mode" \
	"Type to search · Enter/Space to change · Esc to cancel"; do
	if ! printf '%s' "$settings_open" | grep -qF "$expect"; then
		echo "FAIL: Phase 67 — the settings menu is missing '$expect'" >&2
		status=1
	fi
done
# …and a row below the window is still reachable by search (proving the cap
# hides rows rather than dropping them).
tmux send-keys -t "$S67" -l "max tool"
sleep 0.4
settings_tail="$(tmux capture-pane -t "$S67" -p)"
echo "==== Phase 67: searched for a row past the window ===="
printf '%s\n' "$settings_tail"
if ! printf '%s' "$settings_tail" | grep -qF "Max tool calls"; then
	echo "FAIL: Phase 67 — a setting past the visible window is unreachable by search" >&2
	status=1
fi
# Clear the query so the search assertions below start from the full list.
for _ in $(seq 1 8); do tmux send-keys -t "$S67" BSpace; done
sleep 0.4
if ! printf '%s' "$settings_open" | grep -qE "→ Hide thinking +false"; then
	echo "FAIL: Phase 67 — the first row is not marked with its value in the value column" >&2
	status=1
fi
# The counter's SHAPE, not a hard-coded total: this phase is about the menu
# chrome, and pinning the row count made every later feature that adds a knob
# fail here instead of in its own phase (the Hooks row did exactly that).
if ! printf '%s' "$settings_open" | grep -qE "\(1/[0-9]+\)"; then
	echo "FAIL: Phase 67 — the (n/total) counter never showed" >&2
	status=1
fi
# Type-to-search narrows to one row; Enter cycles it and toasts the new value.
tmux send-keys -t "$S67" -l "retry"
sleep 0.4
settings_search="$(tmux capture-pane -t "$S67" -p)"
echo "==== Phase 67: searched for 'retry' ===="
printf '%s\n' "$settings_search"
if ! printf '%s' "$settings_search" | grep -qF "(1/1)"; then
	echo "FAIL: Phase 67 — the search did not narrow the list to the one match" >&2
	status=1
fi
if printf '%s' "$settings_search" | grep -qF "Hide thinking"; then
	echo "FAIL: Phase 67 — the search left non-matching settings listed" >&2
	status=1
fi
tmux send-keys -t "$S67" Enter
sleep 0.5
settings_cycled="$(tmux capture-pane -t "$S67" -p)"
echo "==== Phase 67: after Enter cycled the value ===="
printf '%s\n' "$settings_cycled"
if ! printf '%s' "$settings_cycled" | grep -qE "Error retry +5"; then
	echo "FAIL: Phase 67 — Enter did not cycle Error retry from 3 to 5" >&2
	status=1
fi
# Esc clears the query, a second Esc closes back to the composer.
tmux send-keys -t "$S67" Escape
sleep 0.3
tmux send-keys -t "$S67" Escape
sleep 0.5
settings_closed="$(tmux capture-pane -t "$S67" -p)"
echo "==== Phase 67: closed back to the composer ===="
printf '%s\n' "$settings_closed"
if printf '%s' "$settings_closed" | grep -qF "Enter/Space to change"; then
	echo "FAIL: Phase 67 — Esc did not close the settings menu" >&2
	status=1
fi
if ! printf '%s' "$settings_closed" | grep -qF "Error retry: 5"; then
	echo "FAIL: Phase 67 — the change was not confirmed with a toast above the box" >&2
	status=1
fi
if ! printf '%s' "$settings_closed" | grep -qF "❯"; then
	echo "FAIL: Phase 67 — the composer did not come back" >&2
	status=1
fi
tmux send-keys -t "$S67" -l "/quit"
tmux send-keys -t "$S67" Enter
sleep 0.6
tmux kill-session -t "$S67" 2>/dev/null

# It PERSISTED: the file records only what changed, and a fresh process against
# the same config home opens the menu already showing it.
if [ ! -f "$SET_CFG/settings.json" ]; then
	echo "FAIL: Phase 67 — no settings.json was written to the config home" >&2
	status=1
elif ! grep -q '"error_retry": *5' "$SET_CFG/settings.json"; then
	echo "==== Phase 67: settings.json ===="
	cat "$SET_CFG/settings.json"
	echo "FAIL: Phase 67 — settings.json does not record the changed value" >&2
	status=1
elif grep -q 'hide_thinking\|auto_compact' "$SET_CFG/settings.json"; then
	echo "==== Phase 67: settings.json ===="
	cat "$SET_CFG/settings.json"
	echo "FAIL: Phase 67 — settings.json records values the user never changed (defaults must stay off the wire)" >&2
	status=1
fi
S67B="${S}_settings2"
tmux new-session -d -s "$S67B" -x 90 -y 30 "$APP_SET"
sleep 0.6
tmux send-keys -t "$S67B" -l "/settings"
sleep 0.3
tmux send-keys -t "$S67B" Enter
sleep 0.6
settings_restored="$(tmux capture-pane -t "$S67B" -p)"
echo "==== Phase 67: a fresh process shows the saved value ===="
printf '%s\n' "$settings_restored"
if ! printf '%s' "$settings_restored" | grep -qE "Error retry +5"; then
	echo "FAIL: Phase 67 — the saved setting did not survive the restart" >&2
	status=1
fi
tmux kill-session -t "$S67B" 2>/dev/null
rm -rf "$SET_CFG"


# --- Phase 68: the AskUserQuestion modal (docs/ask.md). An "ask … questions"
# prompt makes the dummy raise the three-question demo and BLOCK on the ask
# gate — exactly as the real tool thread does. The modal must show the chip
# strip (the current chip highlighted, answered ones flipping to ☒), the
# numbered options with their descriptions, the multi-select checkboxes with
# their unnumbered Submit row, the side-by-side preview panel with the notes
# line, the review page, and — after Submit answers — the committed green
# "User answered Alter Zero's questions:" cell; the composer draft typed before
# the modal must be stashed and handed back. ---
S68="${S}_ask"
APP_ASK="env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=800 $BIN"
tmux new-session -d -s "$S68" -x 100 -y 44 "$APP_ASK"
sleep 0.4
tmux send-keys -t "$S68" -l "ask me some questions"
sleep 0.2
tmux send-keys -t "$S68" Enter
sleep 0.3
# Typed while the request is on its way — this draft must survive the modal.
tmux send-keys -t "$S68" -l "a draft typed while it asks"
ask_prompt=""
for _ in $(seq 1 250); do
	cap="$(tmux capture-pane -t "$S68" -p)"
	if printf '%s' "$cap" | grep -qF "What's your favorite way to drink coffee?"; then
		ask_prompt="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 68: captured pane (the ask modal over the stashed draft) ===="
printf '%s\n' "$ask_prompt"
# The option page is a menu, not a text field — no hardware cursor (the
# permission prompt's rule).
ask_cursor_shown="$(tmux display-message -p -t "$S68" '#{cursor_flag}')"
# A detour to the Submit page with NOTHING answered: it must lead with the
# amber warning and list no review rows (answered questions only), then Tab
# wraps back to the first question.
tmux send-keys -t "$S68" Right Right Right
sleep 0.4
ask_warning="$(tmux capture-pane -t "$S68" -p)"
echo "==== Phase 68: captured pane (the empty review page warns) ===="
printf '%s\n' "$ask_warning"
tmux send-keys -t "$S68" Tab
sleep 0.3
# 2 picks Latte and advances to the multi-select page.
tmux send-keys -t "$S68" -l "2"
sleep 0.4
ask_multi="$(tmux capture-pane -t "$S68" -p)"
echo "==== Phase 68: captured pane (the multi-select page) ===="
printf '%s\n' "$ask_multi"
# 1 toggles the first checkbox on; then walk down to the unnumbered Submit
# row (option rows 1-3, the Other row, then Submit) and confirm.
tmux send-keys -t "$S68" -l "1"
sleep 0.3
ask_checked="$(tmux capture-pane -t "$S68" -p)"
tmux send-keys -t "$S68" Down Down Down Down
sleep 0.2
tmux send-keys -t "$S68" Enter
sleep 0.4
ask_preview="$(tmux capture-pane -t "$S68" -p)"
echo "==== Phase 68: captured pane (the preview page) ===="
printf '%s\n' "$ask_preview"
# n opens the notes field; the typed note survives Esc back to the options.
tmux send-keys -t "$S68" -l "n"
sleep 0.2
tmux send-keys -t "$S68" -l "smoke note"
sleep 0.2
tmux send-keys -t "$S68" Escape
sleep 0.3
ask_notes="$(tmux capture-pane -t "$S68" -p)"
# 1 picks Arrow function — the last question answered, so the review page.
tmux send-keys -t "$S68" -l "1"
sleep 0.4
ask_review="$(tmux capture-pane -t "$S68" -p)"
echo "==== Phase 68: captured pane (the review page) ===="
printf '%s\n' "$ask_review"
tmux send-keys -t "$S68" Enter
ask_done=""
for _ in $(seq 1 250); do
	cap="$(tmux capture-pane -t "$S68" -p)"
	if printf '%s' "$cap" | grep -qF "User answered Alter Zero's questions:" &&
		printf '%s' "$cap" | grep -qF "a draft typed while it asks"; then
		ask_done="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 68: captured pane (submitted; the draft is back) ===="
printf '%s\n' "$ask_done"
tmux kill-session -t "$S68" 2>/dev/null
echo "==== Phase 68: the AskUserQuestion modal — chips, options, checkboxes, preview, notes, review, the committed cell ===="
if [ -z "$ask_prompt" ]; then
	echo "FAIL: Phase 68 — the ask modal never showed" >&2
	status=1
fi
if ! printf '%s' "$ask_prompt" | grep -qF "☐ Coffee style"; then
	echo "FAIL: Phase 68 — the chip strip is missing the unanswered Coffee style chip" >&2
	status=1
fi
if ! printf '%s' "$ask_prompt" | grep -qF "✔ Submit"; then
	echo "FAIL: Phase 68 — the chip strip is missing the Submit tab" >&2
	status=1
fi
if ! printf '%s' "$ask_prompt" | grep -qF "❯ 1. Black"; then
	echo "FAIL: Phase 68 — the first option is not highlighted" >&2
	status=1
fi
if ! printf '%s' "$ask_prompt" | grep -qF "No milk, no sugar"; then
	echo "FAIL: Phase 68 — the option descriptions are missing" >&2
	status=1
fi
if ! printf '%s' "$ask_prompt" | grep -qF "Type something."; then
	echo "FAIL: Phase 68 — the free-text Other row is missing" >&2
	status=1
fi
if ! printf '%s' "$ask_prompt" | grep -qF "Chat about this"; then
	echo "FAIL: Phase 68 — the Chat about this row is missing" >&2
	status=1
fi
if printf '%s' "$ask_prompt" | grep -qF "a draft typed while it asks"; then
	echo "FAIL: Phase 68 — the composer draft leaked into the modal" >&2
	status=1
fi
if [ "$ask_cursor_shown" != "0" ]; then
	echo "FAIL: Phase 68 — the option menu should hide the hardware cursor (got flag $ask_cursor_shown)" >&2
	status=1
fi
if ! printf '%s' "$ask_multi" | grep -qF "☒ Coffee style"; then
	echo "FAIL: Phase 68 — answering did not flip the chip to ☒" >&2
	status=1
fi
if ! printf '%s' "$ask_multi" | grep -qF "[ ] Preview panel"; then
	echo "FAIL: Phase 68 — the multi-select checkboxes are missing" >&2
	status=1
fi
if ! printf '%s' "$ask_checked" | grep -qF "[✔] Preview panel"; then
	echo "FAIL: Phase 68 — the digit toggle did not check the box" >&2
	status=1
fi
if ! printf '%s' "$ask_preview" | grep -qF "┌" || ! printf '%s' "$ask_preview" | grep -qF "const greet"; then
	echo "FAIL: Phase 68 — the preview panel did not render" >&2
	status=1
fi
if ! printf '%s' "$ask_preview" | grep -qF "Notes: press n to add notes"; then
	echo "FAIL: Phase 68 — the notes placeholder is missing" >&2
	status=1
fi
if ! printf '%s' "$ask_notes" | grep -qF "Notes: smoke note"; then
	echo "FAIL: Phase 68 — the typed note did not survive Esc" >&2
	status=1
fi
if ! printf '%s' "$ask_review" | grep -qF "Review your answers"; then
	echo "FAIL: Phase 68 — the review page did not open" >&2
	status=1
fi
if ! printf '%s' "$ask_review" | grep -qF "→ Latte"; then
	echo "FAIL: Phase 68 — the review page is missing the recorded answer" >&2
	status=1
fi
if ! printf '%s' "$ask_review" | grep -qF "❯ 1. Submit answers"; then
	echo "FAIL: Phase 68 — the Submit answers option is missing" >&2
	status=1
fi
if ! printf '%s' "$ask_warning" | grep -qF "⚠ You have not answered all questions"; then
	echo "FAIL: Phase 68 — the empty review page did not warn" >&2
	status=1
fi
if printf '%s' "$ask_warning" | grep -qF "● What's your favorite way"; then
	echo "FAIL: Phase 68 — the empty review page listed an unanswered question" >&2
	status=1
fi
if printf '%s' "$ask_review" | grep -qF "⚠"; then
	echo "FAIL: Phase 68 — the fully answered review page still warns" >&2
	status=1
fi
if [ -z "$ask_done" ]; then
	echo "FAIL: Phase 68 — the answered cell never committed (or the draft never came back)" >&2
	status=1
# The cell's answer rows word-wrap at this width ("→ Preview / panel"), so
# join the pane's lines and squeeze the gutter indentation before matching.
elif ! printf '%s' "$ask_done" | tr '\n' ' ' | tr -s ' ' | grep -qF "→ Preview panel"; then
	echo "FAIL: Phase 68 — the committed cell is missing the multi-select answer" >&2
	status=1
fi


# --- Phase 69: the task tools' live checklist (docs/task-tools.md) — the
# "todo" demo drives a real TaskStore: the ⎿ ◻ rows render under the status
# line while the turn runs (the blocked suffix included), the spinner wears
# the active task's activeForm, NO task call ever commits a tool cell to the
# conversation, and the Ctrl+O transcript keeps the full per-call record.
tmux new-session -d -s "${S}_tasks" -x 100 -y 30 "$APP"
sleep 0.4
tmux send-keys -t "${S}_tasks" -l "demo the todo tool i want to see how it works"
sleep 0.2
tmux send-keys -t "${S}_tasks" Enter
# Poll the LIVE screen for the checklist and the spinner override while the
# turn streams (the demo paces one task call per TOOL_DELAY, so both states
# hold for whole seconds).
tasks_checklist=""
tasks_verb=""
for _ in $(seq 1 200); do # up to ~20s
	pane="$(tmux capture-pane -t "${S}_tasks" -p)"
	if [ -z "$tasks_checklist" ] && printf '%s' "$pane" | grep -qF "› blocked by #1"; then
		tasks_checklist="$pane"
	fi
	if [ -z "$tasks_verb" ] && printf '%s' "$pane" | grep -qE "(Setting up the project structure|Writing the core logic)…"; then
		tasks_verb="$pane"
	fi
	if [ -n "$tasks_checklist" ] && [ -n "$tasks_verb" ]; then
		break
	fi
	sleep 0.1
done
# Let the turn actually SETTLE before reading the resting screen. The
# hand-off text alone is not that signal — it streams while the turn is
# still running, so polling for it lands on a mid-turn frame whose strip
# still shows the in-turn checklist. Wait for the status line to be gone
# from the VISIBLE screen (the turn is over) with the hand-off already in
# scrollback.
tasks_done=""
tasks_rest=""
for _ in $(seq 1 300); do # up to ~30s
	pane="$(tmux capture-pane -t "${S}_tasks" -p -S -120)"
	screen="$(tmux capture-pane -t "${S}_tasks" -p)"
	if printf '%s' "$pane" | grep -qF "$SETTLED_REPLY" &&
		! printf '%s' "$screen" | grep -qF "esc to interrupt"; then
		# The committed conversation (with scrollback) for the
		# no-cells assertions; the visible screen alone for the resting
		# block — scrollback still holds the mid-turn frames' rows,
		# which are exactly what the resting assertions must not see.
		tasks_done="$pane"
		tasks_rest="$screen"
		break
	fi
	sleep 0.1
done
# The Ctrl+O transcript keeps the record the conversation hides.
tmux send-keys -t "${S}_tasks" C-o
sleep 0.5
tasks_overlay="$(tmux capture-pane -t "${S}_tasks" -p -S -120)"
tmux send-keys -t "${S}_tasks" C-o
sleep 0.3
echo "==== Phase 69: captured pane (mid-turn checklist) ===="
printf '%s\n' "$tasks_checklist" | grep -v "^$" | tail -12
echo "==== Phase 69: captured pane (at rest — the standalone block) ===="
printf '%s\n' "$tasks_rest" | grep -v "^$" | tail -8
tmux kill-session -t "${S}_tasks" 2>/dev/null
echo "==== Phase 69: the task tools' live checklist ===="
if [ -z "$tasks_checklist" ]; then
	echo "FAIL: Phase 69 — the checklist (with its '› blocked by #1' suffix) never rendered under the status line" >&2
	status=1
else
	if ! printf '%s' "$tasks_checklist" | grep -qF "⎿  ◻"; then
		if ! printf '%s' "$tasks_checklist" | grep -qE "⎿  [◻◼✔]"; then
			echo "FAIL: Phase 69 — the checklist rows are missing the ⎿ gutter + status glyph" >&2
			status=1
		fi
	fi
fi
if [ -z "$tasks_verb" ]; then
	echo "FAIL: Phase 69 — the spinner never wore an in-progress task's activeForm" >&2
	status=1
fi
if [ -z "$tasks_done" ]; then
	echo "FAIL: Phase 69 — the tasks demo never settled on the hand-off" >&2
	status=1
else
	if printf '%s' "$tasks_done" | grep -qE "● Task(Create|Update|List|Get)"; then
		echo "FAIL: Phase 69 — a task call committed a tool cell to the conversation (they must render nothing inline)" >&2
		status=1
	fi
	if ! printf '%s' "$tasks_done" | grep -qF "Three tasks created, all pending"; then
		echo "FAIL: Phase 69 — the narration bullets did not commit around the hidden calls" >&2
		status=1
	fi
fi
# The demo ends with work left (#1 done, #2 in progress, #3 pending), so the
# STANDALONE block takes over at rest: the dim count line over the remaining
# rows, above the composer, gutter-less — there is no spinner left to hang
# from (docs/task-tools.md). Asserted against the VISIBLE screen: scrollback
# still holds the mid-turn frames, whose rows do wear the gutter.
if [ -z "$tasks_rest" ]; then
	echo "FAIL: Phase 69 — the turn never settled, so the resting screen was never read" >&2
	status=1
else
	if ! printf '%s' "$tasks_rest" | grep -qF "3 tasks (1 done, 1 in progress, 1 open)"; then
		echo "FAIL: Phase 69 — the resting screen is missing the standalone task count line" >&2
		status=1
	fi
	if ! printf '%s' "$tasks_rest" | grep -qF "◼ Write the core logic"; then
		echo "FAIL: Phase 69 — the resting block is missing the remaining task rows" >&2
		status=1
	fi
	if ! printf '%s' "$tasks_rest" | grep -qF "◻ Add tests › blocked by #2"; then
		echo "FAIL: Phase 69 — the resting block dropped the blocked-by suffix" >&2
		status=1
	fi
	if printf '%s' "$tasks_rest" | grep -qF "⎿  ✔ Set up the project structure"; then
		echo "FAIL: Phase 69 — the resting block still wears the in-turn ⎿ gutter" >&2
		status=1
	fi
fi
if ! printf '%s' "$tasks_overlay" | grep -qF "TaskUpdate(#1 → completed)"; then
	echo "FAIL: Phase 69 — the Ctrl+O transcript is missing the task-call record" >&2
	status=1
fi
if ! printf '%s' "$tasks_overlay" | grep -qF "Updated task #1 status"; then
	echo "FAIL: Phase 69 — the Ctrl+O record is missing the executor's result text" >&2
	status=1
fi


# --- Phase 70: a FINISHED checklist bows out with its turn and is gone for
# good (docs/task-tools.md) — the reported stale-list bug: an all-✔ list kept
# riding the next turn's spinner. "finish the todo demo" plays the twin
# scenario, which walks two tasks to completed.
S70="${S}_tasksdone"
tmux new-session -d -s "$S70" -x 100 -y 30 "$APP"
sleep 0.4
tmux send-keys -t "$S70" -l "finish the todo demo"
sleep 0.2
tmux send-keys -t "$S70" Enter
# Mid-turn: the all-✔ closure shows — the payoff belongs to the turn that
# earned it, so the last task ticks while the status line is still up.
tasks_fin_live=""
for _ in $(seq 1 400); do # up to ~20s
	cap="$(tmux capture-pane -t "$S70" -p)"
	if printf '%s' "$cap" | grep -qF "✔ Run the demo script" &&
		printf '%s' "$cap" | grep -qF "esc to interrupt"; then
		tasks_fin_live="$cap"
		break
	fi
	sleep 0.05
done
# At rest: nothing. Wait for the status line to go (the turn is over), the
# same settle signal Phase 69 uses.
tasks_fin_rest=""
for _ in $(seq 1 300); do # up to ~30s
	cap="$(tmux capture-pane -t "$S70" -p)"
	if printf '%s' "$cap" | grep -qE "Done for [0-9]" &&
		! printf '%s' "$cap" | grep -qF "esc to interrupt"; then
		tasks_fin_rest="$cap"
		break
	fi
	sleep 0.1
done
# The next turn: the retired list must not come back under its spinner.
tmux send-keys -t "$S70" -l "Thanks"
sleep 0.2
tmux send-keys -t "$S70" Enter
tasks_fin_next=""
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S70" -p)"
	if printf '%s' "$cap" | grep -qF "esc to interrupt"; then
		tasks_fin_next="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 70: captured pane (the all-✔ closure, inside its turn) ===="
printf '%s\n' "$tasks_fin_live" | grep -v "^$" | tail -10
echo "==== Phase 70: captured pane (at rest — the list is gone) ===="
printf '%s\n' "$tasks_fin_rest" | grep -v "^$" | tail -8
echo "==== Phase 70: captured pane (the next turn starts clean) ===="
printf '%s\n' "$tasks_fin_next" | grep -v "^$" | tail -8
tmux kill-session -t "$S70" 2>/dev/null
echo "==== Phase 70: a finished checklist retires ===="
if [ -z "$tasks_fin_live" ]; then
	echo "FAIL: Phase 70 — the finished demo never showed its all-✔ checklist inside the turn" >&2
	status=1
fi
if [ -z "$tasks_fin_rest" ]; then
	echo "FAIL: Phase 70 — the finished demo's turn never settled" >&2
	status=1
else
	if printf '%s' "$tasks_fin_rest" | grep -qE "✔ (Create the demo workspace|Run the demo script)"; then
		echo "FAIL: Phase 70 — a finished checklist still showed at rest (it belongs to the turn that finished it)" >&2
		status=1
	fi
	if printf '%s' "$tasks_fin_rest" | grep -qE "[0-9] tasks \("; then
		echo "FAIL: Phase 70 — the standalone panel showed for a list with no work left" >&2
		status=1
	fi
fi
if [ -z "$tasks_fin_next" ]; then
	echo "FAIL: Phase 70 — the follow-up turn never started" >&2
	status=1
elif printf '%s' "$tasks_fin_next" | grep -qE "✔ (Create the demo workspace|Run the demo script)"; then
	echo "FAIL: Phase 70 — the retired checklist came back under the next turn's spinner (the reported stale-list bug)" >&2
	status=1
fi

# --- Phase 71: checkpoints refuse the directories that are not projects, and
# say so (docs/checkpoint.md). Phase 49 covers the home dir; these are the
# three that made alter-zero "take seconds to boot": a SHARED scratch parent
# (`/tmp` — every program's junk, measured 9.7 s to first frame on a 235 MB
# tree), alter-zero's OWN state dir (where the store lives *inside* the work
# tree, so each snapshot hashed the previous ones back in — the reported 2 GB
# `.alter-zero`), and a tree simply past the cost budget (forced here with a
# tiny ALTER_ZERO_CHECKPOINT_MAX_BYTES rather than by writing 128 MiB). Each
# must disable the store AND raise the reason as a toast — a feature that goes
# quiet without saying why is what made this hard to place. ---
S71="${S}_ckscope"
CK71="$(mktemp -d /tmp/alter-zero-smoke-ck71-XXXXXX)"
WORK71="$(mktemp -d /tmp/alter-zero-smoke-work71-XXXXXX)"
printf 'print("hi")\n' >"$WORK71/app.py"
CKAPP71="env $CFG_ENV ALTER_ZERO_CHECKPOINTS=1 ALTER_ZERO_CHECKPOINTS_DIR=$CK71 ALTER_ZERO_SESSIONS_DIR=$CK_SESS ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"

# Launch in $2, wait for the footer, and report the toast row + store entries.
ck71_launch() {
	tmux new-session -d -s "$S71" -x 100 -y 24 -c "$2" "$3"
	ck71_ready=0
	for _ in $(seq 1 60); do
		if tmux capture-pane -t "$S71" -p | grep -qF "dummy_model_name"; then
			ck71_ready=1
			break
		fi
		sleep 0.1
	done
	ck71_pane="$(tmux capture-pane -t "$S71" -p)"
	ck71_store="$(ls -A "$CK71" 2>/dev/null | wc -l)"
	tmux kill-session -t "$S71" 2>/dev/null
	echo "==== Phase 71 ($1): ready=$ck71_ready store_entries=$ck71_store ===="
	printf '%s\n' "$ck71_pane" | grep -F "Checkpoints off" || echo "     (no refusal toast)"
	if [ "$ck71_ready" != 1 ]; then
		echo "FAIL: Phase 71 ($1) — the app did not start" >&2
		status=1
	fi
}

# (a) `/tmp` itself — refused before the store is ever created.
ck71_launch "shared scratch parent" /tmp "$CKAPP71"
if [ "$ck71_store" != 0 ]; then
	echo "FAIL: Phase 71 — launching in /tmp initialized the checkpoint store (a shared scratch dir must never be snapshot)" >&2
	status=1
fi
if ! printf '%s' "$ck71_pane" | grep -qF "shared scratch directory"; then
	echo "FAIL: Phase 71 — no 'Checkpoints off — a shared scratch directory…' toast in /tmp" >&2
	status=1
fi

# (b) alter-zero's own state dir (here $SMOKE_CFG, via ALTER_ZERO_CONFIG_DIR)
# — the self-inclusion case, refused in both directions.
ck71_launch "own state directory" "$SMOKE_CFG" "$CKAPP71"
if [ "$ck71_store" != 0 ]; then
	echo "FAIL: Phase 71 — launching in the state dir initialized the store (the store would snapshot itself)" >&2
	status=1
fi
if ! printf '%s' "$ck71_pane" | grep -qF "own state directory"; then
	echo "FAIL: Phase 71 — no 'Checkpoints off — this is alter-zero's own state directory' toast" >&2
	status=1
fi

# (c) past the cost budget — the general guard, for the huge project no
# denylist can name. The store is init'd (the probe needs it) but must hold
# no commit.
ck71_launch "over the cost budget" "$WORK71" "ALTER_ZERO_CHECKPOINT_MAX_BYTES=1 $CKAPP71"
if ! printf '%s' "$ck71_pane" | grep -qF "too big to snapshot"; then
	echo "FAIL: Phase 71 — no 'Checkpoints off — … too big to snapshot per turn' toast over the budget" >&2
	status=1
fi
ck71_dir="$CK71/$(printf '%s' "$WORK71" | sed 's/[^a-zA-Z0-9]/-/g')"
if git --git-dir="$ck71_dir" rev-parse HEAD >/dev/null 2>&1; then
	echo "FAIL: Phase 71 — an over-budget tree was snapshot anyway (the probe must refuse before 'git add -A')" >&2
	status=1
fi
if printf '%s' "$ck71_pane" | grep -qF "Snapshotting"; then
	echo "FAIL: Phase 71 — a refused tree announced a snapshot it will not take" >&2
	status=1
fi

# (d) the control: the same directory under the default budget checkpoints —
# and, cold, its session-start snapshot ANNOUNCES itself above the banner
# (`Snapshotting 1 file (12 B) for checkpoints…`, docs/checkpoint.md
# "Saying so").
ck71_launch "in-budget project (control)" "$WORK71" "$CKAPP71"
if ! git --git-dir="$ck71_dir" rev-parse HEAD >/dev/null 2>&1; then
	echo "FAIL: Phase 71 — an ordinary project did not snapshot (the guards must be scoped, not a blanket disable)" >&2
	status=1
fi
if printf '%s' "$ck71_pane" | grep -qF "Checkpoints off"; then
	echo "FAIL: Phase 71 — an ordinary project raised a refusal toast" >&2
	status=1
fi
if ! printf '%s' "$ck71_pane" | grep -qF "Snapshotting 1 file ("; then
	echo "FAIL: Phase 71 — the cold session-start snapshot never announced itself (docs/checkpoint.md 'Saying so')" >&2
	status=1
fi

# (e) a checkpoints root pointed INSIDE the project. This is the mechanism
# behind the 2 GB `.alter-zero`: without the anchored self-exclude every
# `git add -A` re-stages the previous snapshots' objects and the tracked set
# compounds turn after turn. It must NOT cost the project its checkpoints —
# the store simply keeps itself out of its own snapshots. Two launches, so a
# second snapshot would have the first one's objects to re-stage.
WORK71B="$(mktemp -d /tmp/alter-zero-smoke-work71b-XXXXXX)"
printf 'print("hi")\n' >"$WORK71B/app.py"
CK71B="$WORK71B/.ck"
CKAPP71B="env $CFG_ENV ALTER_ZERO_CHECKPOINTS=1 ALTER_ZERO_CHECKPOINTS_DIR=$CK71B ALTER_ZERO_SESSIONS_DIR=$CK_SESS ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"
CK71="$CK71B" ck71_launch "store inside the project" "$WORK71B" "$CKAPP71B"
if ! printf '%s' "$ck71_pane" | grep -qF "Snapshotting 1 file ("; then
	echo "FAIL: Phase 71 — the cold first launch never announced its snapshot" >&2
	status=1
fi
CK71="$CK71B" ck71_launch "store inside the project (2)" "$WORK71B" "$CKAPP71B"
# The relaunch finds a warm store with nothing new: the probe reports zero,
# so there is nothing to announce (the files>0 gate keeps relaunches quiet).
if printf '%s' "$ck71_pane" | grep -qF "Snapshotting"; then
	echo "FAIL: Phase 71 — a warm store announced a snapshot with nothing new" >&2
	status=1
fi
ck71b_dir="$CK71B/$(printf '%s' "$WORK71B" | sed 's/[^a-zA-Z0-9]/-/g')"
ck71b_tracked="$(git --git-dir="$ck71b_dir" --work-tree="$WORK71B" ls-files 2>/dev/null)"
echo "==== Phase 71 (store inside the project): tracked=$(printf '%s' "$ck71b_tracked" | tr '\n' ' ') ===="
if ! printf '%s\n' "$ck71b_tracked" | grep -qx "app.py"; then
	echo "FAIL: Phase 71 — a project holding its own checkpoints root lost its snapshots (the self-exclude must cost it nothing)" >&2
	status=1
fi
if printf '%s\n' "$ck71b_tracked" | grep -q "^\.ck/"; then
	echo "FAIL: Phase 71 — the store staged its own objects (the compounding that made .alter-zero 2 GB)" >&2
	status=1
fi

# --- Phase 72: LIFECYCLE HOOKS (docs/hooks.md). The user's own commands wedged
# into the tool loop. The dummy's `hooks` scenario plays both halves of the
# contract against the real rendering path: a `PreToolUse` hook refusing a
# destructive command — the refusal text produced by `llm::hooks::block_texts`,
# the very function the live runner calls, so this cell is byte-for-byte the
# one a real hooks.json makes — and a `PostToolUse` hook annotating a call that
# did run, whose dim `⎿` provenance row is the user-visible trace while the
# context itself rides the model-facing text (never the cell). Assert the
# blocked call resolves with no output of its own, the allowed one keeps its
# command output, and the model-facing paragraph never reaches scrollback. ---
S72="${S}_hooks"
tmux new-session -d -s "$S72" -x 100 -y 30 "$APP"
sleep 0.5
tmux send-keys -t "$S72" -l "show me the hooks demo"
sleep 0.2
tmux send-keys -t "$S72" Enter
hooks_pane=""
for _ in $(seq 1 200); do # up to ~20s
	hooks_pane="$(tmux capture-pane -t "$S72" -p -S -200)"
	if printf '%s' "$hooks_pane" | grep -qF "Done for"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 72: captured pane + scrollback (lifecycle hooks) ===="
printf '%s\n' "$hooks_pane"
if ! printf '%s' "$hooks_pane" | grep -qF "Blocked by hook: no destructive deletes outside ./tmp"; then
	echo "FAIL: Phase 72 — the PreToolUse block never rendered its refusal cell" >&2
	status=1
fi
# The refusal must be the blocked cell's ONLY row: the line right after its
# header is the `⎿ Blocked by hook…`, never command output. (A plain
# `grep -F` of a two-line literal would match either line on its own, so this
# reads the following line explicitly.)
hooks_after_blocked="$(printf '%s\n' "$hooks_pane" | awk '/● Bash\(rm -rf build\/\)/{getline; print; exit}')"
if ! printf '%s' "$hooks_after_blocked" | grep -qF "Blocked by hook"; then
	echo "FAIL: Phase 72 — the blocked call produced output of its own; it must never have run (saw: $hooks_after_blocked)" >&2
	status=1
fi
if ! printf '%s' "$hooks_pane" | grep -qF "Context added by hook"; then
	echo "FAIL: Phase 72 — the PostToolUse hook left no dim provenance row on the cell it annotated" >&2
	status=1
fi
if ! printf '%s' "$hooks_pane" | grep -qF "drwxr-xr-x"; then
	echo "FAIL: Phase 72 — the allowed call lost its own output" >&2
	status=1
fi
# The long stop-and-wait instruction is what the MODEL reads
# (ToolCall::context_output). It must not be committed to the conversation —
# the one-line cell is the transcript's record.
if printf '%s' "$hooks_pane" | grep -qF "A configured lifecycle hook blocked this tool call."; then
	echo "FAIL: Phase 72 — the model-facing hook text was committed to scrollback; only the short cell line may show" >&2
	status=1
fi
# A Stop hook's feedback note is CELL-LESS inline (docs/hooks.md): the demo
# ends with one, and it must not have painted a row in the conversation.
if printf '%s' "$hooks_pane" | grep -qF "Stop hook feedback"; then
	echo "FAIL: Phase 72 — the hook note leaked into the inline conversation" >&2
	status=1
fi
tmux send-keys -t "$S72" C-o
sleep 0.6
tmux send-keys -t "$S72" Home
sleep 0.3
hooks_overlay="$(tmux capture-pane -t "$S72" -p)"
echo "==== Phase 72: captured overlay top (the hook block in the transcript) ===="
printf '%s\n' "$hooks_overlay"
if ! printf '%s' "$hooks_overlay" | grep -qF "Blocked by hook"; then
	echo "FAIL: Phase 72 — the Ctrl+O transcript lost the hook's refusal" >&2
	status=1
fi
tmux send-keys -t "$S72" End
sleep 0.3
hooks_overlay_tail="$(tmux capture-pane -t "$S72" -p)"
echo "==== Phase 72: captured overlay tail (the hook note) ===="
printf '%s\n' "$hooks_overlay_tail"
if ! printf '%s' "$hooks_overlay_tail" | grep -qF "Stop hook feedback"; then
	echo "FAIL: Phase 72 — the Ctrl+O transcript lost the hook note" >&2
	status=1
fi
tmux send-keys -t "$S72" q
sleep 0.3
tmux kill-session -t "$S72" 2>/dev/null || true

# --- Phase 73: a UserPromptSubmit hook BLOCKS the prompt (docs/hooks.md). The
# dummy's prompt-block scenario scripts the single PromptBlocked event; the
# loop's arm rolls the submission back out of history AND scrollback (the pop +
# purge repaint — the recorder's shrink-rewrite erases it from the rollout
# too), returns the text to the composer, and commits the red reason-only
# notice. The one remaining "❯ hook demo…" line on screen must be the
# composer's own — exactly one occurrence, not two. ---
S73="${S}_promptblock"
tmux new-session -d -s "$S73" -x 100 -y 30 "$APP"
sleep 0.5
tmux send-keys -t "$S73" -l "hook demo: block my prompt"
sleep 0.2
tmux send-keys -t "$S73" Enter
block_pane=""
for _ in $(seq 1 100); do # up to ~10s
	block_pane="$(tmux capture-pane -t "$S73" -p -S -200)"
	if printf '%s' "$block_pane" | grep -qF "blocked the prompt"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 73: captured pane (prompt blocked by hook) ===="
printf '%s\n' "$block_pane"
if ! printf '%s' "$block_pane" | grep -qF "UserPromptSubmit hook blocked the prompt"; then
	echo "FAIL: Phase 73 — the block notice never rendered" >&2
	status=1
fi
if ! printf '%s' "$block_pane" | grep -qF "Reason: no prompts about hooks"; then
	echo "FAIL: Phase 73 — the notice lost the hook's reason" >&2
	status=1
fi
block_echoes="$(printf '%s\n' "$block_pane" | grep -cF "hook demo: block my prompt" || true)"
if [ "$block_echoes" -ne 1 ]; then
	echo "FAIL: Phase 73 — expected exactly the composer's copy of the blocked text, saw $block_echoes (the sent-message echo must be rolled back, the composer must get the draft back)" >&2
	status=1
fi
tmux send-keys -t "$S73" C-c C-c
sleep 0.3
tmux kill-session -t "$S73" 2>/dev/null || true

# --- Phase 74: a BLOCKED prompt is erased from the ROLLOUT too (docs/hooks.md).
# The recorder used to key its truncation rewrite on history length alone, and
# block_prompt removes the submission AND records the notice — length holds
# still, so the file kept the censored prompt, lost the notice, and a
# --continue fed the secret straight back into the model's context (the
# resume-leak an independent review caught). The recorder now keys on
# App::history_generation. Round trip: block → quit → --continue → the prompt
# text must be gone from the resumed transcript AND the rollout file, while
# the red notice survives both. ---
S74="${S}_blockleak"
BL_DIR="$(mktemp -d /tmp/alter-zero-smoke-blockleak-XXXXXX)"
BLAPP="env $CFG_ENV_NOHIST ALTER_ZERO_SESSIONS_DIR=$BL_DIR ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux new-session -d -s "$S74" -x 100 -y 30 "$BLAPP; echo CLI_APP_EXITED; sleep 60"
sleep 0.5
# A real turn first: --continue resumes conversations, and a session whose
# every prompt was blocked holds none — the leak only ever mattered on a
# session with something in it.
tmux send-keys -t "$S74" -l "$USER_MSG"
sleep 0.2
tmux send-keys -t "$S74" Enter
for _ in $(seq 1 134); do
	if tmux capture-pane -t "$S74" -p -S -40 | grep -qF "Done for"; then
		break
	fi
	sleep 0.15
done
tmux send-keys -t "$S74" -l "hook demo: block my prompt"
sleep 0.2
tmux send-keys -t "$S74" Enter
for _ in $(seq 1 100); do
	if tmux capture-pane -t "$S74" -p -S -200 | grep -qF "blocked the prompt"; then
		break
	fi
	sleep 0.1
done
# The block restored the draft to the composer: Ctrl+C once clears it, again quits.
tmux send-keys -t "$S74" C-c
sleep 0.3
tmux send-keys -t "$S74" C-c
for _ in $(seq 1 40); do
	if tmux capture-pane -t "$S74" -p | grep -qF "CLI_APP_EXITED"; then
		break
	fi
	sleep 0.1
done
bl_rollout="$(find "$BL_DIR" -type f -name 'rollout-*.jsonl' | head -1)"
echo "==== Phase 74: rollout file after the blocked prompt ===="
if [ -n "$bl_rollout" ]; then cat "$bl_rollout"; else echo "(no rollout file)"; fi
if [ -z "$bl_rollout" ]; then
	echo "FAIL: Phase 74 — no rollout was recorded (the notice should have created one)" >&2
	status=1
else
	if grep -qF "hook demo: block my prompt" "$bl_rollout"; then
		echo "FAIL: Phase 74 — the censored prompt is still in the rollout on disk" >&2
		status=1
	fi
	if ! grep -qF "blocked the prompt" "$bl_rollout"; then
		echo "FAIL: Phase 74 — the block notice never reached the rollout" >&2
		status=1
	fi
fi
tmux kill-session -t "$S74" 2>/dev/null || true
# --continue: the resumed transcript must carry the notice, never the prompt.
tmux new-session -d -s "$S74" -x 100 -y 30 "$BLAPP --continue; echo CLI_APP_EXITED; sleep 60"
bl_resumed=""
for _ in $(seq 1 40); do
	bl_resumed="$(tmux capture-pane -t "$S74" -p -S -120)"
	if printf '%s' "$bl_resumed" | grep -qF "blocked the prompt"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 74: --continue after a blocked prompt (no leak) ===="
printf '%s\n' "$bl_resumed"
if printf '%s' "$bl_resumed" | grep -qF "hook demo: block my prompt"; then
	echo "FAIL: Phase 74 — the censored prompt came back in the resumed transcript" >&2
	status=1
fi
if ! printf '%s' "$bl_resumed" | grep -qF "blocked the prompt"; then
	echo "FAIL: Phase 74 — the resumed transcript lost the block notice" >&2
	status=1
fi
tmux send-keys -t "$S74" C-c
sleep 0.3
tmux kill-session -t "$S74" 2>/dev/null || true
rm -rf "$BL_DIR"

# --- Phase 75: the read-only /hooks MENU (docs/hooks-menu.md). Claude Code's
# /hooks browser over a planted hooks.json: the palette lists the command, it
# opens the inline framed events list ('Hooks', '{N} hooks configured', the ℹ
# read-only banner, the eleven events with counts + summaries in an aligned
# column, a ↓ overflow marker past the five-row window), Enter drills into
# matchers → hooks → details (the aligned field block, the REAL command
# word-wrapped in a rounded box, the modify note), Esc walks back one level
# at a time, and closing restores the composer. ---
S75="${S}_hooksmenu"
HK_CFG="$(mktemp -d)"
cat >"$HK_CFG/hooks.json" <<'HOOKS75'
{
  "hooks": {
    "PreToolUse": [
      { "matcher": "Bash",
        "hooks": [ { "type": "command",
          "command": "jq -re '.tool_input.command | test(\"rm -rf\") | not' >/dev/null || { echo 'no recursive deletes' >&2; exit 2; }" } ] }
    ],
    "Stop": [
      { "hooks": [ { "type": "command", "command": "./notify.sh" } ] }
    ]
  }
}
HOOKS75
APP_HK="env ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$HK_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux new-session -d -s "$S75" -x 100 -y 35 "$APP_HK"
sleep 0.6
tmux send-keys -t "$S75" -l "/hooks"
sleep 0.4
hooks_palette="$(tmux capture-pane -t "$S75" -p)"
echo "==== Phase 75: the palette filtered to /hooks ===="
printf '%s\n' "$hooks_palette"
if ! printf '%s' "$hooks_palette" | grep -qF "Browse the configured lifecycle hooks"; then
	echo "FAIL: Phase 75 — /hooks is missing from the slash-command palette" >&2
	status=1
fi
tmux send-keys -t "$S75" Enter
sleep 0.6
hooks_events="$(tmux capture-pane -t "$S75" -p)"
echo "==== Phase 75: the events level ===="
printf '%s\n' "$hooks_events"
for expect in "Hooks" "2 hooks configured" "This menu is read-only" \
	"PreToolUse (1)" "Before tool execution" "↓ 5." \
	"Enter to confirm · Esc to cancel"; do
	if ! printf '%s' "$hooks_events" | grep -qF "$expect"; then
		echo "FAIL: Phase 75 — the events level is missing '$expect'" >&2
		status=1
	fi
done
if ! printf '%s' "$hooks_events" | grep -qF "❯ 1."; then
	echo "FAIL: Phase 75 — the first event row is not marked selected" >&2
	status=1
fi
# Enter drills into PreToolUse's matchers.
tmux send-keys -t "$S75" Enter
sleep 0.4
hooks_matchers="$(tmux capture-pane -t "$S75" -p)"
echo "==== Phase 75: the matchers level ===="
printf '%s\n' "$hooks_matchers"
for expect in "PreToolUse - Matchers" "Input to command is the tool call" \
	"[User] Bash" "1 hook"; do
	if ! printf '%s' "$hooks_matchers" | grep -qF "$expect"; then
		echo "FAIL: Phase 75 — the matchers level is missing '$expect'" >&2
		status=1
	fi
done
# Enter again: the matcher's hooks.
tmux send-keys -t "$S75" Enter
sleep 0.4
hooks_list="$(tmux capture-pane -t "$S75" -p)"
echo "==== Phase 75: the hooks level ===="
printf '%s\n' "$hooks_list"
for expect in "PreToolUse - Matcher: Bash" "[command] jq -re" "User Settings"; do
	if ! printf '%s' "$hooks_list" | grep -qF "$expect"; then
		echo "FAIL: Phase 75 — the hooks level is missing '$expect'" >&2
		status=1
	fi
done
# Enter once more: the read-only detail page, the command whole in its box.
tmux send-keys -t "$S75" Enter
sleep 0.4
hooks_detail="$(tmux capture-pane -t "$S75" -p)"
echo "==== Phase 75: the detail page ===="
printf '%s\n' "$hooks_detail"
for expect in "Hook details" "Event:    PreToolUse" "Matcher:  Bash" \
	"Type:     command" "Source:   User settings (" "Command:" \
	"│ jq -re" "To modify or remove this hook" \
	"Esc to go back"; do
	if ! printf '%s' "$hooks_detail" | grep -qF "$expect"; then
		echo "FAIL: Phase 75 — the detail page is missing '$expect'" >&2
		status=1
	fi
done
# The command survives WHOLE in the box — but word-wrapped, so a phrase can
# split across box rows ('no recursive / deletes'). Strip the borders, join
# the rows, and assert on the reassembled text (the unit test's approach).
hooks_box="$(printf '%s\n' "$hooks_detail" | sed -n 's/^  │ \(.*\)│[[:space:]]*$/\1/p' \
	| sed 's/[[:space:]]*$//' | tr '\n' ' ')"
if ! printf '%s' "$hooks_box" | grep -qF "no recursive deletes"; then
	echo "FAIL: Phase 75 — the boxed command lost 'no recursive deletes' (got: $hooks_box)" >&2
	status=1
fi
if printf '%s' "$hooks_detail" | grep -qF "Enter to confirm"; then
	echo "FAIL: Phase 75 — the detail page offers Enter (it has nothing to confirm)" >&2
	status=1
fi
# Esc walks back one level at a time; a fourth Esc closes to the composer.
tmux send-keys -t "$S75" Escape
sleep 0.3
hooks_back="$(tmux capture-pane -t "$S75" -p)"
if ! printf '%s' "$hooks_back" | grep -qF "PreToolUse - Matcher: Bash"; then
	echo "FAIL: Phase 75 — Esc from the details did not return to the hooks level" >&2
	status=1
fi
tmux send-keys -t "$S75" Escape
sleep 0.2
tmux send-keys -t "$S75" Escape
sleep 0.2
tmux send-keys -t "$S75" Escape
sleep 0.5
hooks_closed="$(tmux capture-pane -t "$S75" -p)"
echo "==== Phase 75: closed back to the composer ===="
printf '%s\n' "$hooks_closed"
if printf '%s' "$hooks_closed" | grep -qF "This menu is read-only"; then
	echo "FAIL: Phase 75 — Esc from the events level did not close the menu" >&2
	status=1
fi
if ! printf '%s' "$hooks_closed" | grep -qF "dummy_model_name"; then
	echo "FAIL: Phase 75 — the composer (and its footer) did not come back" >&2
	status=1
fi
tmux kill-session -t "$S75" 2>/dev/null || true
rm -rf "$HK_CFG"

# --- Phase 76: the `Skill` tool loads an authored SKILL.md (docs/skills.md).
# The cell is the WHOLE visible surface — `● Skill(dataviz)` over one green
# `⎿  Successfully loaded skill` row — while the model reads the skill's entire
# body. That split is the feature: a 400-line skill would otherwise dump itself
# into the transcript every time it is used. Ctrl+D must show the body (it is
# what was really sent) and Ctrl+O must expand it, while the inline conversation
# shows neither. ---
S76="${S}_skills"
tmux new-session -d -s "$S76" -x 100 -y 30 "$APP; echo CLI_APP_EXITED; sleep 60"
sleep 0.5
tmux send-keys -t "$S76" -l "load a skill for me"
sleep 0.2
tmux send-keys -t "$S76" Enter
for _ in $(seq 1 160); do
	if tmux capture-pane -t "$S76" -p -S -80 | grep -qF "Done for"; then
		break
	fi
	sleep 0.15
done
skills_pane="$(tmux capture-pane -t "$S76" -p -S -80)"
echo "==== Phase 76: the skill cell ===="
printf '%s\n' "$skills_pane"
if ! printf '%s' "$skills_pane" | grep -qE "● Skill\(dataviz\)"; then
	echo "FAIL: Phase 76 — no '● Skill(dataviz)' cell header" >&2
	status=1
fi
if ! printf '%s' "$skills_pane" | grep -qF "Successfully loaded skill"; then
	echo "FAIL: Phase 76 — the cell is missing its 'Successfully loaded skill' row" >&2
	status=1
fi
# The body is the MODEL's, not the transcript's: none of it may reach the
# inline conversation.
if printf '%s' "$skills_pane" | grep -qF "Base directory for this skill"; then
	echo "FAIL: Phase 76 — the skill body leaked into the inline conversation" >&2
	status=1
fi
if printf '%s' "$skills_pane" | grep -qF "references/palette.md"; then
	echo "FAIL: Phase 76 — the skill body's text leaked into the inline conversation" >&2
	status=1
fi
# Ctrl+D: the derived context must carry the body, because that IS what the
# model was sent (the ToolOutcome::context split, docs/skills.md).
tmux send-keys -t "$S76" C-d
sleep 0.6
skills_ctx="$(tmux capture-pane -t "$S76" -p -S -400)"
echo "==== Phase 76: the Ctrl+D context ===="
printf '%s\n' "$skills_ctx"
for expect in "Base directory for this skill" "references/palette.md"; do
	if ! printf '%s' "$skills_ctx" | grep -qF "$expect"; then
		echo "FAIL: Phase 76 — the context view is missing '$expect'" >&2
		status=1
	fi
done
tmux send-keys -t "$S76" q
sleep 0.5
# Ctrl+O: the transcript keeps the CELL, not the body. The two-text split's
# rule everywhere here (a rejected call's transcript shows its red display
# line, not the model-facing instruction): the transcript is what happened,
# Ctrl+D is what was sent. It is also the reference's behaviour, and it keeps
# a 100 KiB skill out of the render cache.
tmux send-keys -t "$S76" C-o
sleep 0.6
skills_tx="$(tmux capture-pane -t "$S76" -p -S -400)"
echo "==== Phase 76: the Ctrl+O transcript ===="
printf '%s\n' "$skills_tx"
if ! printf '%s' "$skills_tx" | grep -qE "● Skill\(dataviz\)"; then
	echo "FAIL: Phase 76 — the transcript lost the skill cell" >&2
	status=1
fi
if ! printf '%s' "$skills_tx" | grep -qF "Successfully loaded skill"; then
	echo "FAIL: Phase 76 — the transcript lost the cell's resolved row" >&2
	status=1
fi
if printf '%s' "$skills_tx" | grep -qF "Base directory for this skill"; then
	echo "FAIL: Phase 76 — the transcript dumped the skill body (that belongs to ctrl+d)" >&2
	status=1
fi
tmux send-keys -t "$S76" q
sleep 0.5
skills_back="$(tmux capture-pane -t "$S76" -p)"
if ! printf '%s' "$skills_back" | grep -qF "dummy_model_name"; then
	echo "FAIL: Phase 76 — the composer did not come back after the overlays" >&2
	status=1
fi
tmux kill-session -t "$S76" 2>/dev/null || true

# --- Phase 77: the `/skills` MENU (docs/skills.md). The `/settings` menu's
# twin — same frame, same grammar — over the discovered skills, each one
# enable/disable-able. Drive it end to end against a REAL skill directory
# (the rest of the suite runs skill-free, so this phase points
# ALTER_ZERO_SKILLS_DIR at one it plants): open it from the palette, check the
# rows and the value column, disable one, confirm the toast and the flipped
# value, and prove it PERSISTED — a second process against the same config
# home must open the menu already showing it off. ---
S77="${S}_skillsmenu"
SK_CFG="$(mktemp -d)"
SK_DIR="$(mktemp -d)"
mkdir -p "$SK_DIR/alpha-skill" "$SK_DIR/beta-skill"
cat >"$SK_DIR/alpha-skill/SKILL.md" <<'SKILL'
---
name: alpha-skill
description: The first demo skill, for the smoke suite only
---
Alpha body.
SKILL
cat >"$SK_DIR/beta-skill/SKILL.md" <<'SKILL'
---
name: beta-skill
description: The second demo skill, for the smoke suite only
---
Beta body.
SKILL
APP_SK="env ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$SK_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_SKILLS_DIR=$SK_DIR ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux new-session -d -s "$S77" -x 90 -y 32 "$APP_SK"
sleep 0.6
# The palette lists it (a bare `/skil` token filters to it).
tmux send-keys -t "$S77" -l "/skil"
sleep 0.4
skills_palette="$(tmux capture-pane -t "$S77" -p)"
echo "==== Phase 77: the palette filtered to /skills ===="
printf '%s\n' "$skills_palette"
if ! printf '%s' "$skills_palette" | grep -qF "Browse skills and enable or disable each one"; then
	echo "FAIL: Phase 77 — /skills is missing from the slash-command palette" >&2
	status=1
fi
tmux send-keys -t "$S77" Enter
sleep 0.6
skills_open="$(tmux capture-pane -t "$S77" -p)"
echo "==== Phase 77: the skills menu open ===="
printf '%s\n' "$skills_open"
for expect in "alpha-skill" "beta-skill" "(1/2)" \
	"The first demo skill, for the smoke suite only" \
	"Type to search · Enter/Space to enable/disable · Esc to cancel"; do
	if ! printf '%s' "$skills_open" | grep -qF "$expect"; then
		echo "FAIL: Phase 77 — the skills menu is missing '$expect'" >&2
		status=1
	fi
done
if ! printf '%s' "$skills_open" | grep -qE "→ alpha-skill +enabled"; then
	echo "FAIL: Phase 77 — the first row is not marked with its value in the value column" >&2
	status=1
fi
# Enter disables the highlighted skill: the row flips and a toast confirms.
tmux send-keys -t "$S77" Enter
sleep 0.6
skills_off="$(tmux capture-pane -t "$S77" -p)"
echo "==== Phase 77: after Enter disabled the highlighted skill ===="
printf '%s\n' "$skills_off"
if ! printf '%s' "$skills_off" | grep -qE "alpha-skill +disabled"; then
	echo "FAIL: Phase 77 — Enter did not flip alpha-skill to disabled" >&2
	status=1
fi
if ! printf '%s' "$skills_off" | grep -qF "Skill alpha-skill: disabled"; then
	echo "FAIL: Phase 77 — the toggle was not confirmed with a toast above the box" >&2
	status=1
fi
if ! printf '%s' "$skills_off" | grep -qE "beta-skill +enabled"; then
	echo "FAIL: Phase 77 — the toggle moved a sibling row's value too" >&2
	status=1
fi
# The menu stays open across toggles; Esc closes back to the composer.
tmux send-keys -t "$S77" Escape
sleep 0.5
skills_closed="$(tmux capture-pane -t "$S77" -p)"
if printf '%s' "$skills_closed" | grep -qF "Enter/Space to enable/disable"; then
	echo "FAIL: Phase 77 — Esc did not close the skills menu" >&2
	status=1
fi
if ! printf '%s' "$skills_closed" | grep -qF "dummy_model_name"; then
	echo "FAIL: Phase 77 — the composer (and its footer) did not come back" >&2
	status=1
fi
tmux kill-session -t "$S77" 2>/dev/null
echo "==== Phase 77: skills.json on disk ===="
cat "$SK_CFG/skills.json" 2>/dev/null || echo "(no file)"
if ! grep -qF "alpha-skill" "$SK_CFG/skills.json" 2>/dev/null; then
	echo "FAIL: Phase 77 — the disabled skill was not persisted to skills.json" >&2
	status=1
fi
# A SECOND process against the same config home opens the menu already off.
tmux new-session -d -s "$S77" -x 90 -y 32 "$APP_SK"
sleep 0.6
tmux send-keys -t "$S77" -l "/skills"
sleep 0.3
tmux send-keys -t "$S77" Enter
sleep 0.6
skills_reopened="$(tmux capture-pane -t "$S77" -p)"
echo "==== Phase 77: a fresh process shows the persisted state ===="
printf '%s\n' "$skills_reopened"
if ! printf '%s' "$skills_reopened" | grep -qE "alpha-skill +disabled"; then
	echo "FAIL: Phase 77 — the disabled skill did not survive a restart" >&2
	status=1
fi
if ! printf '%s' "$skills_reopened" | grep -qE "beta-skill +enabled"; then
	echo "FAIL: Phase 77 — the restart lost the ENABLED skill's state" >&2
	status=1
fi
tmux kill-session -t "$S77" 2>/dev/null
rm -rf "$SK_CFG" "$SK_DIR"

# --- Phase 78: skills are RE-WALKED every turn (docs/skills.md). Discovery
# used to run once at startup, so a skill added mid-session — or one the agent
# had just written for you — stayed invisible until a restart. Boot against an
# EMPTY skills dir, plant a SKILL.md while the session is running, take one
# turn, and both surfaces must have it: the `<system-reminder>` listing in the
# derived context (Ctrl+D) and the `/skills` menu. The listing is the sharp
# one — it rides `skills_offered`, which reads the Skills row's availability,
# so a rescan that re-rendered it BEFORE re-deriving availability shipped the
# listing a turn late (it looked exactly like the rescan not working). ---
S78="${S}_skillrescan"
RS_CFG="$(mktemp -d)"
RS_DIR="$(mktemp -d)"
APP_RS="env ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$RS_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_SKILLS_DIR=$RS_DIR ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux new-session -d -s "$S78" -x 90 -y 32 "$APP_RS"
sleep 0.6
# Nothing on disk yet: the menu opens on the where-would-one-go placeholder.
tmux send-keys -t "$S78" -l "/skills"
sleep 0.3
tmux send-keys -t "$S78" Enter
sleep 0.6
rescan_empty="$(tmux capture-pane -t "$S78" -p)"
echo "==== Phase 78: the menu before any skill exists ===="
printf '%s\n' "$rescan_empty"
if ! printf '%s' "$rescan_empty" | grep -qF "No skills found. Add one at:"; then
	echo "FAIL: Phase 78 — an empty skills dir did not open on the placeholder" >&2
	status=1
fi
tmux send-keys -t "$S78" Escape
sleep 0.4
# …now plant one while the session is running.
mkdir -p "$RS_DIR/late-skill"
cat >"$RS_DIR/late-skill/SKILL.md" <<'SKILL'
---
name: late-skill
description: Planted mid-session by the smoke suite
---
Late body.
SKILL
# One turn re-walks the roots at its start.
tmux send-keys -t "$S78" -l "hello there"
sleep 0.2
tmux send-keys -t "$S78" Enter
for _ in $(seq 1 160); do
	if tmux capture-pane -t "$S78" -p -S -80 | grep -qF "Done for"; then
		break
	fi
	sleep 0.15
done
# Ctrl+D: the listing must name it in THIS turn's context, not the next one's.
# The view opens tail-following, and the listing is a LEADING fragment, so jump
# to the top before reading it.
tmux send-keys -t "$S78" C-d
sleep 0.8
tmux send-keys -t "$S78" Home
sleep 0.5
rescan_ctx="$(tmux capture-pane -t "$S78" -p)"
echo "==== Phase 78: the derived context after the rescan ===="
printf '%s\n' "$rescan_ctx" | grep -A4 "skills are available" || true
for expect in "The following skills are available for use with the Skill tool" \
	"late-skill: Planted mid-session by the smoke suite"; do
	if ! printf '%s' "$rescan_ctx" | grep -qF "$expect"; then
		echo "FAIL: Phase 78 — the mid-session skill is missing from the context: '$expect'" >&2
		status=1
	fi
done
tmux send-keys -t "$S78" q
sleep 0.6
# …and the menu lists it without a restart.
tmux send-keys -t "$S78" -l "/skills"
sleep 0.3
tmux send-keys -t "$S78" Enter
sleep 0.6
rescan_menu="$(tmux capture-pane -t "$S78" -p)"
echo "==== Phase 78: the menu after the rescan ===="
printf '%s\n' "$rescan_menu"
if ! printf '%s' "$rescan_menu" | grep -qE "late-skill +enabled"; then
	echo "FAIL: Phase 78 — the mid-session skill never reached the /skills menu" >&2
	status=1
fi
tmux kill-session -t "$S78" 2>/dev/null
rm -rf "$RS_CFG" "$RS_DIR"

# --- Phase 79: the `$` SKILL PICKER (docs/skill-mentions.md). Codex's skill
# mentions as a fourth band: typing `$` in the composer lists the discovered
# skills (name + description columns), typing narrows on the name, Tab
# completes the mention IN PLACE — `$alpha-skill ` stays in the draft, sigil
# kept — and submitting a message that carries a mention plays the skill load
# (offline: the dummy's skills demo; live: the Skill tool description makes
# the model call the `skill` tool). ---
S79="${S}_skillmention"
SM_CFG="$(mktemp -d)"
SM_DIR="$(mktemp -d)"
mkdir -p "$SM_DIR/alpha-skill" "$SM_DIR/beta-skill"
cat >"$SM_DIR/alpha-skill/SKILL.md" <<'SKILL'
---
name: alpha-skill
description: The first demo skill, for the smoke suite only
---
Alpha body.
SKILL
cat >"$SM_DIR/beta-skill/SKILL.md" <<'SKILL'
---
name: beta-skill
description: The second demo skill, for the smoke suite only
---
Beta body.
SKILL
APP_SM="env ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$SM_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_SKILLS_DIR=$SM_DIR ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux new-session -d -s "$S79" -x 90 -y 32 "$APP_SM"
sleep 0.6
# A bare `$` lists every discovered skill with its description.
tmux send-keys -t "$S79" -l '$'
sleep 0.4
mention_all="$(tmux capture-pane -t "$S79" -p)"
echo "==== Phase 79: the band on a bare \$ ===="
printf '%s\n' "$mention_all"
for expect in "alpha-skill" "The first demo skill" "beta-skill" "The second demo skill"; do
	if ! printf '%s' "$mention_all" | grep -qF "$expect"; then
		echo "FAIL: Phase 79 — the \$ band is missing '$expect'" >&2
		status=1
	fi
done
# Typing narrows on the name…
tmux send-keys -t "$S79" -l "alp"
sleep 0.4
mention_narrow="$(tmux capture-pane -t "$S79" -p)"
echo "==== Phase 79: the band narrowed to \$alp ===="
printf '%s\n' "$mention_narrow"
if ! printf '%s' "$mention_narrow" | grep -qF "alpha-skill"; then
	echo "FAIL: Phase 79 — the narrowed band lost the match" >&2
	status=1
fi
if printf '%s' "$mention_narrow" | grep -qF "beta-skill"; then
	echo "FAIL: Phase 79 — 'alp' still lists beta-skill" >&2
	status=1
fi
# …and Tab completes the mention in place, sigil kept, ready to keep typing.
tmux send-keys -t "$S79" Tab
sleep 0.4
mention_done="$(tmux capture-pane -t "$S79" -p)"
echo "==== Phase 79: the completed mention ===="
printf '%s\n' "$mention_done"
if ! printf '%s' "$mention_done" | grep -qF '❯ $alpha-skill'; then
	echo "FAIL: Phase 79 — Tab did not complete the mention into the composer" >&2
	status=1
fi
if ! printf '%s' "$mention_done" | grep -qF "dummy_model_name"; then
	echo "FAIL: Phase 79 — the footer did not come back after the band closed" >&2
	status=1
fi
# Submitting the mention plays the skill load — the dummy's skills demo
# answers it (the cell, never the body), like a live model calling `skill`.
tmux send-keys -t "$S79" -l "load it please"
sleep 0.2
tmux send-keys -t "$S79" Enter
for _ in $(seq 1 160); do
	if tmux capture-pane -t "$S79" -p -S -80 | grep -qF "Done for"; then
		break
	fi
	sleep 0.15
done
mention_turn="$(tmux capture-pane -t "$S79" -p -S -80)"
echo "==== Phase 79: the submitted mention's turn ===="
printf '%s\n' "$mention_turn"
if ! printf '%s' "$mention_turn" | grep -qE "● Skill\(dataviz\)"; then
	echo "FAIL: Phase 79 — the submitted mention played no skill load" >&2
	status=1
fi
if ! printf '%s' "$mention_turn" | grep -qF "Successfully loaded skill"; then
	echo "FAIL: Phase 79 — the skill cell is missing its loaded row" >&2
	status=1
fi
tmux kill-session -t "$S79" 2>/dev/null
rm -rf "$SM_CFG" "$SM_DIR"


# --- Phase 80: the /mcp MANAGER (docs/mcp.md). Claude Code's MCP surface
# driven offline end to end against a SCRIPTED stdio server (a sh script
# answering the deterministic ids of a MODERN 2026-07-28 server: 1 = the
# server/discover probe, 2 = tools/list — no initialize anywhere): the
# palette lists /mcp, the manager opens on the grouped server list, the
# startup connect resolves the row to '✔ connected · 1 tool', Enter walks
# list → server detail (facts + actions, the negotiated Protocol row
# included) → tools → the tool detail naming the wire name the model calls,
# and Esc walks all the way back out with the composer restored. ---
S80="${S}_mcp"
MCP_CFG="$(mktemp -d)"
MCP_DIR="$(mktemp -d)"
cat >"$MCP_DIR/server.sh" <<'MCPSRV'
#!/bin/sh
cat > /dev/null &
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"ttlMs":60000,"cacheScope":"public","_meta":{"io.modelcontextprotocol/serverInfo":{"name":"fixture","version":"1.0"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"resultType":"complete","tools":[{"name":"echo_text","description":"Echo the text back.","inputSchema":{"type":"object","properties":{"text":{"type":"string","description":"What to echo."}},"required":["text"]}}],"ttlMs":60000,"cacheScope":"public"}}'
sleep 60
MCPSRV
chmod +x "$MCP_DIR/server.sh"
cat >"$MCP_CFG/mcp.json" <<MCPJSON
{"mcpServers": {"fixture": {"type": "stdio", "command": "sh", "args": ["$MCP_DIR/server.sh"]}}}
MCPJSON
APP_MCP="env ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$MCP_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux new-session -d -s "$S80" -x 100 -y 36 "$APP_MCP"
sleep 0.8
tmux send-keys -t "$S80" -l "/mcp"
sleep 0.4
mcp_palette="$(tmux capture-pane -t "$S80" -p)"
echo "==== Phase 80: the palette filtered to /mcp ===="
printf '%s\n' "$mcp_palette"
if ! printf '%s' "$mcp_palette" | grep -qF "Manage MCP servers"; then
	echo "FAIL: Phase 80 — /mcp is missing from the slash-command palette" >&2
	status=1
fi
tmux send-keys -t "$S80" Enter
# The startup connect runs on a worker thread; poll for the resolved row.
for _ in $(seq 1 60); do
	if tmux capture-pane -t "$S80" -p | grep -qF "✔ connected"; then
		break
	fi
	sleep 0.25
done
mcp_list="$(tmux capture-pane -t "$S80" -p)"
echo "==== Phase 80: the server list ===="
printf '%s\n' "$mcp_list"
for expect in "Manage MCP servers" "1 server" "User MCPs" \
	"fixture · ✔ connected · 1 tool" \
	"↑/↓ to navigate · Enter to confirm · Esc to cancel"; do
	if ! printf '%s' "$mcp_list" | grep -qF "$expect"; then
		echo "FAIL: Phase 80 — the server list is missing '$expect'" >&2
		status=1
	fi
done
tmux send-keys -t "$S80" Enter
sleep 0.5
mcp_detail="$(tmux capture-pane -t "$S80" -p)"
echo "==== Phase 80: the server detail ===="
printf '%s\n' "$mcp_detail"
for expect in "Fixture MCP Server" "Status:" "✔ connected" \
	"Protocol:" "2026-07-28" \
	"Command:" "server.sh" "Config location:" "Capabilities:" "tools" \
	"Tools:" "1 tool" \
	"1. View tools" "2. Reconnect" "3. Disable"; do
	if ! printf '%s' "$mcp_detail" | grep -qF "$expect"; then
		echo "FAIL: Phase 80 — the server detail is missing '$expect'" >&2
		status=1
	fi
done
# The count belongs to the LIST row; the detail page has a `Tools:` row of
# its own, so saying it on the Status row too is a duplicate (docs/mcp.md).
if printf '%s' "$mcp_detail" | grep -qF "· 1 tool"; then
	echo "FAIL: Phase 80 — the detail's Status row repeats the tool count" >&2
	status=1
fi
# A stdio server has no auth story — and a modern one that never asked for
# credentials must not wear a '✘ not authenticated' row (the deepwiki bug).
if printf '%s' "$mcp_detail" | grep -qF "Auth:"; then
	echo "FAIL: Phase 80 — the detail page shows an Auth row for a server with no auth story" >&2
	status=1
fi
tmux send-keys -t "$S80" Enter
sleep 0.5
mcp_tools="$(tmux capture-pane -t "$S80" -p)"
echo "==== Phase 80: the tools list ===="
printf '%s\n' "$mcp_tools"
for expect in "Tools for fixture" "1 tool" "1. echo_text"; do
	if ! printf '%s' "$mcp_tools" | grep -qF "$expect"; then
		echo "FAIL: Phase 80 — the tools list is missing '$expect'" >&2
		status=1
	fi
done
tmux send-keys -t "$S80" Enter
sleep 0.5
mcp_tool="$(tmux capture-pane -t "$S80" -p)"
echo "==== Phase 80: the tool detail ===="
printf '%s\n' "$mcp_tool"
for expect in "Tool name:" "echo_text" "Full name:" "mcp__fixture__echo_text" \
	"Description:" "Echo the text back." "Parameters:" \
	"text (required): string - What to echo." "Esc to go back"; do
	if ! printf '%s' "$mcp_tool" | grep -qF "$expect"; then
		echo "FAIL: Phase 80 — the tool detail is missing '$expect'" >&2
		status=1
	fi
done
# Esc walks back out: tool → tools → server → list → closed (footer back).
tmux send-keys -t "$S80" Escape; sleep 0.3
tmux send-keys -t "$S80" Escape; sleep 0.3
tmux send-keys -t "$S80" Escape; sleep 0.3
tmux send-keys -t "$S80" Escape; sleep 0.5
mcp_closed="$(tmux capture-pane -t "$S80" -p)"
echo "==== Phase 80: closed back to the composer ===="
printf '%s\n' "$mcp_closed"
if printf '%s' "$mcp_closed" | grep -qF "Manage MCP servers"; then
	echo "FAIL: Phase 80 — Esc did not close the /mcp manager" >&2
	status=1
fi
if ! printf '%s' "$mcp_closed" | grep -qF "dummy_model_name"; then
	echo "FAIL: Phase 80 — the session footer did not come back after closing /mcp" >&2
	status=1
fi
tmux kill-session -t "$S80" 2>/dev/null
rm -rf "$MCP_CFG" "$MCP_DIR"

# --- Phase 81: a turn that FINISHES under the Ctrl+O overlay loses nothing.
# Commits made while an alternate-screen overlay is up queue on the viewport
# and the return flushes them above the live region (invariant 4) — the
# retired history-window rebuild re-emitted at most one screenful, which
# silently dropped the rest of an overlay-covered turn from the terminal (the
# user-reported "cells hidden until a resize" / "can't scroll back to the
# reply" scrollback hole). Submit, open the overlay before the reply streams,
# let the WHOLE turn (text + Read/Edit/Bash cells + summary) finish under it,
# return, and assert every part reached the terminal's screen+scrollback —
# the user bubble included (it sat on the visible screen when the overlay
# opened, and the old return's window rewrite used to overwrite it). ---
S81="${S}_holefree"
tmux new-session -d -s "$S81" -x 80 -y 24 "$APP"
sleep 0.4
tmux send-keys -t "$S81" -l "$USER_MSG"
sleep 0.2
tmux send-keys -t "$S81" Enter
sleep 0.3 # the bubble commits; the reply has not started (startup delay)
tmux send-keys -t "$S81" C-o
sleep 0.3
for _ in $(seq 1 100); do # the turn finishes while the overlay is up
	if tmux capture-pane -t "$S81" -p | grep -qF "$SETTLED_REPLY"; then
		break
	fi
	sleep 0.15
done
sleep 0.8 # StreamDone + the Done-for summary land under the overlay
tmux send-keys -t "$S81" C-o
sleep 0.8
hole_free="$(tmux capture-pane -t "$S81" -p -S -)"
echo "==== Phase 81: returned after the turn finished under the overlay ===="
printf '%s\n' "$hole_free" | tail -60
for marker in "❯ $USER_MSG" "$EXPECT_REPLY" "Read(about.py)" "Edit(about.py)" "Bash(python3 about.py)" "$SETTLED_REPLY" "Done for"; do
	if ! printf '%s' "$hole_free" | grep -qF "$marker"; then
		echo "FAIL: Phase 81 — '$marker' never reached the terminal after the overlay return (the scrollback hole)" >&2
		status=1
	fi
done
hole_screen="$(tmux capture-pane -t "$S81" -p)"
if ! printf '%s' "$hole_screen" | grep -q "^❯"; then
	echo "FAIL: Phase 81 — the composer is missing from the returned screen" >&2
	status=1
fi
if ! printf '%s' "$hole_screen" | grep -qE "dummy_model_name · .*manual$"; then
	echo "FAIL: Phase 81 — the session footer is missing from the returned screen" >&2
	status=1
fi
tmux kill-session -t "$S81" 2>/dev/null

# --- Phase 82: mid-stream overlay round-trips neither lose NOR DOUBLE rows,
# and a resize under the overlay still purge-rebuilds cleanly. The return
# flushes exactly the not-yet-flushed queue; bouncing through the overlay
# three times while the reply streams must leave every committed row in the
# terminal exactly once, and the resize-under-overlay return (the one case
# that still rebuilds — the emulator reflowed the main screen underneath)
# must not duplicate them either. ---
S82="${S}_holedup"
tmux new-session -d -s "$S82" -x 80 -y 24 "$APP"
sleep 0.4
tmux send-keys -t "$S82" -l "$USER_MSG"
sleep 0.2
tmux send-keys -t "$S82" Enter
for _ in $(seq 1 40); do # wait for the stream to begin
	if tmux capture-pane -t "$S82" -p | grep -qF "$EXPECT_REPLY"; then
		break
	fi
	sleep 0.1
done
for _ in 1 2 3; do # bounce while it streams
	tmux send-keys -t "$S82" C-o
	sleep 0.6
	tmux send-keys -t "$S82" C-o
	sleep 0.4
done
tmux send-keys -t "$S82" C-o # a resize lands UNDER the overlay…
sleep 0.3
tmux resize-window -t "$S82" -x 70 -y 20 2>/dev/null
sleep 0.4
tmux send-keys -t "$S82" C-o # …so this return purge-rebuilds
sleep 0.5
for _ in $(seq 1 100); do
	if tmux capture-pane -t "$S82" -p -S - | grep -qF "Done for"; then
		break
	fi
	sleep 0.15
done
sleep 0.5
dedup="$(tmux capture-pane -t "$S82" -p -S -)"
echo "==== Phase 82: after three mid-stream round-trips + a resize under the overlay ===="
printf '%s\n' "$dedup" | tail -40
for marker in "❯ $USER_MSG" "$EXPECT_REPLY" "Read(about.py)" "Edit(about.py)" "Bash(python3 about.py)" "$SETTLED_REPLY" "Done for"; do
	count=$(printf '%s\n' "$dedup" | grep -cF "$marker")
	if [ "$count" -ne 1 ]; then
		echo "FAIL: Phase 82 — '$marker' appears $count times after overlay round-trips (expected exactly 1)" >&2
		status=1
	fi
done
tmux kill-session -t "$S82" 2>/dev/null


# --- Phase 83: the project-level .alter-zero config layer behind the /trust
# gate (docs/project-config.md). A temp project (no .git — the root falls
# back to the cwd) carries .alter-zero/hooks.json and .mcp.json (the Phase
# 80 scripted stdio fixture). First launch, the layer ON over its own fresh
# config home: the startup toast points at /trust, /mcp lists the project
# server '⚠ untrusted' (default-deny — never launched), /trust reviews the
# hook command and the server target VERBATIM over the approve option, and
# approving activates LIVE — the server connects with no restart and the
# /hooks browser shows the merged Stop hook. A relaunch on the same config
# home starts already-trusted: /trust reads 'Status: trusted' offering only
# the revoke, and the server connects unprompted. ---
S83="${S}_trust"
TR_CFG="$(mktemp -d)"
TR_WORK="$(mktemp -d)"
mkdir -p "$TR_WORK/.alter-zero"
cat >"$TR_WORK/server.sh" <<'TRSRV'
#!/bin/sh
cat > /dev/null &
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"ttlMs":60000,"cacheScope":"public","_meta":{"io.modelcontextprotocol/serverInfo":{"name":"projfix","version":"1.0"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"resultType":"complete","tools":[{"name":"echo_text","description":"Echo the text back.","inputSchema":{"type":"object","properties":{"text":{"type":"string","description":"What to echo."}},"required":["text"]}}],"ttlMs":60000,"cacheScope":"public"}}'
sleep 60
TRSRV
chmod +x "$TR_WORK/server.sh"
cat >"$TR_WORK/.mcp.json" <<TRJSON
{"mcpServers": {"projfix": {"type": "stdio", "command": "sh", "args": ["$TR_WORK/server.sh"]}}}
TRJSON
cat >"$TR_WORK/.alter-zero/hooks.json" <<'TRHOOKS'
{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "./fmt.sh"}]}]}}
TRHOOKS
# The phase runs in a temp cwd (-c "$TR_WORK"), so the binary path must be
# absolute — the Phase 46 BIN_ABS rule; a relative $BIN would resolve inside
# the temp project and never launch.
TR_BIN="$(readlink -f "$BIN")"
APP_TR="env ALTER_ZERO_PROJECT_CONFIG=1 ALTER_ZERO_CONFIG_DIR=$TR_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $TR_BIN"
tmux new-session -d -s "$S83" -x 100 -y 36 -c "$TR_WORK" "$APP_TR"
# The pending toast rides the first frames and self-clears — poll for it.
tr_toast=""
for _ in $(seq 1 25); do
	tr_toast="$(tmux capture-pane -t "$S83" -p)"
	if printf '%s' "$tr_toast" | grep -qF "/trust to review"; then
		break
	fi
	sleep 0.2
done
echo "==== Phase 83: the pending-config startup toast ===="
printf '%s\n' "$tr_toast"
if ! printf '%s' "$tr_toast" | grep -qF "Project .alter-zero config found — /trust to review"; then
	echo "FAIL: Phase 83 — the pending project config raised no startup toast" >&2
	status=1
fi
# Default-deny: /mcp lists the project server untrusted, never connected.
tmux send-keys -t "$S83" -l "/mcp"
sleep 0.4
tmux send-keys -t "$S83" Enter
sleep 0.6
tr_mcp="$(tmux capture-pane -t "$S83" -p)"
echo "==== Phase 83: /mcp holds the untrusted project server ===="
printf '%s\n' "$tr_mcp"
for expect in "Project MCPs" "projfix · ⚠ untrusted"; do
	if ! printf '%s' "$tr_mcp" | grep -qF "$expect"; then
		echo "FAIL: Phase 83 — /mcp is missing '$expect' before approval" >&2
		status=1
	fi
done
tmux send-keys -t "$S83" Escape
sleep 0.4
# The /trust review names the root, both files, and what would run.
tmux send-keys -t "$S83" -l "/trust"
sleep 0.4
tmux send-keys -t "$S83" Enter
sleep 0.5
tr_review="$(tmux capture-pane -t "$S83" -p)"
echo "==== Phase 83: the /trust review ===="
printf '%s\n' "$tr_review"
for expect in "Project trust —" "Status: not trusted" \
	"Hooks — " "pending approval" "Stop: ./fmt.sh" \
	"MCP servers — " "projfix: sh $TR_WORK/server.sh" \
	"1. Trust this project's config"; do
	if ! printf '%s' "$tr_review" | grep -qF "$expect"; then
		echo "FAIL: Phase 83 — the /trust review is missing '$expect'" >&2
		status=1
	fi
done
# Approve: Enter on option 1 records trust.json and activates live.
tmux send-keys -t "$S83" Enter
sleep 0.5
tr_after="$(tmux capture-pane -t "$S83" -p)"
echo "==== Phase 83: after the approval ===="
printf '%s\n' "$tr_after"
if ! printf '%s' "$tr_after" | grep -qF "Trusted this project's config"; then
	echo "FAIL: Phase 83 — approving raised no confirmation toast" >&2
	status=1
fi
# Live activation, no restart: the project server connects…
tmux send-keys -t "$S83" -l "/mcp"
sleep 0.4
tmux send-keys -t "$S83" Enter
for _ in $(seq 1 60); do
	if tmux capture-pane -t "$S83" -p | grep -qF "✔ connected"; then
		break
	fi
	sleep 0.25
done
tr_live="$(tmux capture-pane -t "$S83" -p)"
echo "==== Phase 83: the approved server connected live ===="
printf '%s\n' "$tr_live"
if ! printf '%s' "$tr_live" | grep -qF "projfix · ✔ connected · 1 tool"; then
	echo "FAIL: Phase 83 — the approved project server did not connect live" >&2
	status=1
fi
tmux send-keys -t "$S83" Escape
sleep 0.4
# …and the merged project hook shows in the /hooks browser.
tmux send-keys -t "$S83" -l "/hooks"
sleep 0.4
tmux send-keys -t "$S83" Enter
sleep 0.5
tr_hooks="$(tmux capture-pane -t "$S83" -p)"
echo "==== Phase 83: the merged project hook in /hooks ===="
printf '%s\n' "$tr_hooks"
if ! printf '%s' "$tr_hooks" | grep -qF "1 hook configured"; then
	echo "FAIL: Phase 83 — /hooks does not count the merged project hook" >&2
	status=1
fi
# Stop sits past the five-row event window — the digit jumps straight into
# its handler list, which names the project file's command.
tmux send-keys -t "$S83" -l "7"
sleep 0.5
tr_stop="$(tmux capture-pane -t "$S83" -p)"
echo "==== Phase 83: the Stop handler list ===="
printf '%s\n' "$tr_stop"
if ! printf '%s' "$tr_stop" | grep -qF "./fmt.sh"; then
	echo "FAIL: Phase 83 — the merged Stop hook does not list ./fmt.sh" >&2
	status=1
fi
tmux send-keys -t "$S83" Escape
sleep 0.3
tmux send-keys -t "$S83" Escape
sleep 0.3
tmux kill-session -t "$S83" 2>/dev/null
# Relaunch on the same config home: the trust persisted.
tmux new-session -d -s "$S83" -x 100 -y 36 -c "$TR_WORK" "$APP_TR"
sleep 0.8
tmux send-keys -t "$S83" -l "/trust"
sleep 0.4
tmux send-keys -t "$S83" Enter
sleep 0.5
tr_persist="$(tmux capture-pane -t "$S83" -p)"
echo "==== Phase 83: /trust after a relaunch ===="
printf '%s\n' "$tr_persist"
for expect in "Status: trusted" "1. Revoke trust"; do
	if ! printf '%s' "$tr_persist" | grep -qF "$expect"; then
		echo "FAIL: Phase 83 — the relaunch lost the recorded trust ('$expect' missing)" >&2
		status=1
	fi
done
if printf '%s' "$tr_persist" | grep -qF "Trust this project's config"; then
	echo "FAIL: Phase 83 — a trusted, unchanged project still offers the approval" >&2
	status=1
fi
tmux send-keys -t "$S83" Escape
sleep 0.3
tmux send-keys -t "$S83" -l "/mcp"
sleep 0.4
tmux send-keys -t "$S83" Enter
for _ in $(seq 1 60); do
	if tmux capture-pane -t "$S83" -p | grep -qF "✔ connected"; then
		break
	fi
	sleep 0.25
done
tr_boot="$(tmux capture-pane -t "$S83" -p)"
echo "==== Phase 83: the trusted server connects unprompted at startup ===="
printf '%s\n' "$tr_boot"
if ! printf '%s' "$tr_boot" | grep -qF "projfix · ✔ connected · 1 tool"; then
	echo "FAIL: Phase 83 — the trusted project server did not connect at the relaunch" >&2
	status=1
fi
tmux kill-session -t "$S83" 2>/dev/null
# The home directory is never a project (docs/project-config.md): a cwd with
# no .git falls back to itself as the root, and launched in ~ that made
# {root}/.alter-zero the user's own config home — the layer rediscovered the
# user's files as pending "project config" and asked the user to trust
# themself (the reported bug). A fake HOME carrying user-level hooks must
# raise no pending toast, still load them as user hooks, and /trust explains.
TR_HOME="$(mktemp -d)"
mkdir -p "$TR_HOME/.alter-zero"
cat >"$TR_HOME/.alter-zero/hooks.json" <<'TRHOME'
{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "./fmt.sh"}]}]}}
TRHOME
tmux new-session -d -s "$S83" -x 100 -y 36 -c "$TR_HOME" \
	"env HOME=$TR_HOME ALTER_ZERO_PROJECT_CONFIG=1 ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $TR_BIN"
sleep 1.5
tr_home_pane="$(tmux capture-pane -t "$S83" -p)"
echo "==== Phase 83: launched in the home directory ===="
printf '%s\n' "$tr_home_pane"
if printf '%s' "$tr_home_pane" | grep -qF "/trust to review"; then
	echo "FAIL: Phase 83 — the user's own config home raised the project trust toast" >&2
	status=1
fi
tmux send-keys -t "$S83" -l "/hooks"
sleep 0.4
tmux send-keys -t "$S83" Enter
sleep 0.5
if ! tmux capture-pane -t "$S83" -p | grep -qF "1 hook configured"; then
	echo "FAIL: Phase 83 — the home config no longer loads as user-level hooks" >&2
	status=1
fi
tmux send-keys -t "$S83" Escape
sleep 0.4
tmux send-keys -t "$S83" -l "/trust"
sleep 0.4
tmux send-keys -t "$S83" Enter
sleep 0.5
if ! tmux capture-pane -t "$S83" -p | grep -qF "The home directory is not a project"; then
	echo "FAIL: Phase 83 — /trust in the home directory does not explain itself" >&2
	status=1
fi
tmux kill-session -t "$S83" 2>/dev/null
rm -rf "$TR_CFG" "$TR_WORK" "$TR_HOME"

# --- Phase 84: the mcp CLI SUBCOMMAND (docs/mcp-cli.md). The install path
# end to end, before any TUI: `mcp add fixture -- sh server.sh` writes the
# scripted stdio fixture into a temp USER mcp.json (exit 0, the Added line
# + the File: path), `mcp list` reports it, a duplicate add exits 1 naming
# the mcp remove escape hatch, a grammar error exits 2 with the mcp usage
# as its trailer — then the TUI boots against that SAME file and /mcp
# resolves the row '✔ connected · 1 tool': the file the CLI writes is the
# file the session reads. ---
S84="${S}_mcpcli"
MCP84_CFG="$(mktemp -d)"
MCP84_DIR="$(mktemp -d)"
cat >"$MCP84_DIR/server.sh" <<'MCPSRV84'
#!/bin/sh
cat > /dev/null &
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"ttlMs":60000,"cacheScope":"public","_meta":{"io.modelcontextprotocol/serverInfo":{"name":"fixture","version":"1.0"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"resultType":"complete","tools":[{"name":"echo_text","description":"Echo the text back.","inputSchema":{"type":"object","properties":{"text":{"type":"string","description":"What to echo."}},"required":["text"]}}],"ttlMs":60000,"cacheScope":"public"}}'
sleep 60
MCPSRV84
chmod +x "$MCP84_DIR/server.sh"
mcp84_add="$(env ALTER_ZERO_MCP_FILE="$MCP84_CFG/mcp.json" "$BIN" mcp add fixture -- sh "$MCP84_DIR/server.sh" 2>&1)"
mcp84_add_exit=$?
echo "==== Phase 84: mcp add ===="
printf '%s\n' "$mcp84_add"
if [ "$mcp84_add_exit" -ne 0 ]; then
	echo "FAIL: Phase 84 — mcp add exited $mcp84_add_exit" >&2
	status=1
fi
for expect in 'Added stdio MCP server "fixture"' "File: $MCP84_CFG/mcp.json"; do
	if ! printf '%s' "$mcp84_add" | grep -qF "$expect"; then
		echo "FAIL: Phase 84 — mcp add output is missing '$expect'" >&2
		status=1
	fi
done
mcp84_list="$(env ALTER_ZERO_MCP_FILE="$MCP84_CFG/mcp.json" "$BIN" mcp list 2>&1)"
echo "==== Phase 84: mcp list ===="
printf '%s\n' "$mcp84_list"
if ! printf '%s' "$mcp84_list" | grep -qF "fixture: sh $MCP84_DIR/server.sh (stdio)"; then
	echo "FAIL: Phase 84 — mcp list does not report the installed server" >&2
	status=1
fi
mcp84_dup="$(env ALTER_ZERO_MCP_FILE="$MCP84_CFG/mcp.json" "$BIN" mcp add fixture --url https://dup 2>&1)"
mcp84_dup_exit=$?
if [ "$mcp84_dup_exit" -ne 1 ]; then
	echo "FAIL: Phase 84 — a duplicate add exited $mcp84_dup_exit (want 1)" >&2
	status=1
fi
for expect in 'MCP server "fixture" already exists' "mcp remove fixture"; do
	if ! printf '%s' "$mcp84_dup" | grep -qF "$expect"; then
		echo "FAIL: Phase 84 — the duplicate-add error is missing '$expect'" >&2
		status=1
	fi
done
mcp84_bad="$(env ALTER_ZERO_MCP_FILE="$MCP84_CFG/mcp.json" "$BIN" mcp add lonely 2>&1)"
mcp84_bad_exit=$?
if [ "$mcp84_bad_exit" -ne 2 ]; then
	echo "FAIL: Phase 84 — a grammar error exited $mcp84_bad_exit (want 2)" >&2
	status=1
fi
if ! printf '%s' "$mcp84_bad" | grep -qF "alter-zero mcp — manage MCP servers"; then
	echo "FAIL: Phase 84 — the grammar error is missing the mcp usage trailer" >&2
	status=1
fi
APP_MCP84="env ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$MCP84_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux new-session -d -s "$S84" -x 100 -y 36 "$APP_MCP84"
sleep 0.8
tmux send-keys -t "$S84" -l "/mcp"
sleep 0.4
tmux send-keys -t "$S84" Enter
for _ in $(seq 1 60); do
	if tmux capture-pane -t "$S84" -p | grep -qF "✔ connected"; then
		break
	fi
	sleep 0.25
done
mcp84_tui="$(tmux capture-pane -t "$S84" -p)"
echo "==== Phase 84: the TUI reads the CLI-written file ===="
printf '%s\n' "$mcp84_tui"
if ! printf '%s' "$mcp84_tui" | grep -qF "fixture · ✔ connected · 1 tool"; then
	echo "FAIL: Phase 84 — /mcp does not show the CLI-installed server connected" >&2
	status=1
fi
tmux kill-session -t "$S84" 2>/dev/null
rm -rf "$MCP84_CFG" "$MCP84_DIR"

# --- Phase 85: the VIEW FLOW (docs/view-flow.md). A framed view's page taller
# than the terminal — here a /hooks detail whose command wraps to dozens of box
# rows — must not clip at the bottom: the paint bottom-anchors (the hint and
# the closing rule stay on screen) and the skipped top FLOWS into the
# terminal's real scrollback, so the whole page reads via the terminal's own
# scrolling. Navigating back purge-rebuilds — no stale flowed row survives —
# and closing the menu restores the composer. ---
S85="${S}_viewflow"
FL_CFG="$(mktemp -d)"
FL_CMD="echo FLOW_TOP_OF_COMMAND $(printf 'lorem-ipsum-filler-%03d ' $(seq 1 220))FLOW_DEEP_MARKER_85 done"
printf '{ "hooks": { "PreToolUse": [ { "matcher": "Bash", "hooks": [ { "type": "command", "command": "%s" } ] } ] } }' "$FL_CMD" >"$FL_CFG/hooks.json"
APP_FL="env ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$FL_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux new-session -d -s "$S85" -x 100 -y 24 "$APP_FL"
sleep 0.6
tmux send-keys -t "$S85" -l "/hooks"
sleep 0.4
tmux send-keys -t "$S85" Enter
sleep 0.5
tmux send-keys -t "$S85" Enter # PreToolUse -> matchers
sleep 0.4
tmux send-keys -t "$S85" Enter # matcher -> hooks
sleep 0.4
tmux send-keys -t "$S85" Enter # hook -> the screen-tall detail page
sleep 0.8
flow_pane="$(tmux capture-pane -t "$S85" -p)"
flow_full="$(tmux capture-pane -t "$S85" -p -S -200)"
echo "==== Phase 85: the screen-tall detail page (visible pane) ===="
printf '%s\n' "$flow_pane"
# The visible screen keeps the page's TAIL: the hint and the closing rule.
if ! printf '%s' "$flow_pane" | grep -qF "Esc to go back"; then
	echo "FAIL: Phase 85 — the detail tail (Esc to go back) is not on screen" >&2
	status=1
fi
if ! printf '%s\n' "$flow_pane" | awk 'END { exit ($0 ~ /──/) ? 0 : 1 }'; then
	echo "FAIL: Phase 85 — the bottom rule is not the last screen row" >&2
	status=1
fi
# …and genuinely overflowed: the page top is NOT on the visible screen…
if printf '%s' "$flow_pane" | grep -qF "Hook details"; then
	echo "FAIL: Phase 85 — the page fits the pane; the fixture must overflow for this phase to test the flow" >&2
	status=1
fi
# …but IS in the terminal's real scrollback, whole: the title, the field
# block, and the boxed command's first words all reachable by scrolling up.
for expect in "Hook details" "Event:    PreToolUse" "FLOW_TOP_OF_COMMAND" "FLOW_DEEP_MARKER_85"; do
	if ! printf '%s' "$flow_full" | grep -qF "$expect"; then
		echo "FAIL: Phase 85 — the flowed page is missing '$expect' from scrollback+screen" >&2
		status=1
	fi
done
# Esc back to the hooks level: the shrink purge-rebuilds, so the flowed rows
# vanish from scrollback (the deep marker can't hide in the list's one-row
# truncated command) and the small page paints in place.
tmux send-keys -t "$S85" Escape
sleep 0.6
flow_back="$(tmux capture-pane -t "$S85" -p -S -400)"
echo "==== Phase 85: back at the hooks level after the flow ===="
printf '%s\n' "$flow_back" | tail -24
if ! printf '%s' "$flow_back" | grep -qF "PreToolUse - Matcher: Bash"; then
	echo "FAIL: Phase 85 — Esc from the flowed detail did not return to the hooks level" >&2
	status=1
fi
if printf '%s' "$flow_back" | grep -qF "FLOW_DEEP_MARKER_85"; then
	echo "FAIL: Phase 85 — stale flowed rows survived the back-navigation purge" >&2
	status=1
fi
# Esc the rest of the way out: the composer and its footer return, and no
# trace of the menu or the flowed page is left anywhere in the terminal.
tmux send-keys -t "$S85" Escape
sleep 0.2
tmux send-keys -t "$S85" Escape
sleep 0.2
tmux send-keys -t "$S85" Escape
sleep 0.6
flow_closed="$(tmux capture-pane -t "$S85" -p -S -400)"
echo "==== Phase 85: closed back to the composer ===="
printf '%s\n' "$flow_closed" | tail -12
if ! printf '%s' "$flow_closed" | grep -qF "dummy_model_name"; then
	echo "FAIL: Phase 85 — the composer (and its footer) did not come back" >&2
	status=1
fi
for stale in "Hook details" "FLOW_TOP_OF_COMMAND" "This menu is read-only"; do
	if printf '%s' "$flow_closed" | grep -qF "$stale"; then
		echo "FAIL: Phase 85 — '$stale' survived the close (the purge should have wiped it)" >&2
		status=1
	fi
done
tmux kill-session -t "$S85" 2>/dev/null || true
rm -rf "$FL_CFG"


# --- Phase 86: the `/mascot` picker (docs/mascot.md). The `/settings`
# family's frame over the mascot catalog with a LIVE banner preview: the page
# previews the highlighted mascot through the header's own builder, Enter
# switches the startup banner in place (the selection purge-rebuilds, so the
# chrome at the top of scrollback redraws at once), the switch is confirmed
# with a toast and persisted to {config}/mascot.json — and a second process
# against the same config home must LAUNCH with the switched mascot. ---
S86="${S}_mascot"
MASC_CFG="$(mktemp -d)"
APP_MASCOT="env ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$MASC_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_SKILLS_DIR=$SMOKE_SKILLS ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux new-session -d -s "$S86" -x 80 -y 30 "$APP_MASCOT"
sleep 0.8
mascot_boot="$(tmux capture-pane -t "$S86" -p)"
echo "==== Phase 86: the startup banner (default crest) ===="
printf '%s\n' "$mascot_boot"
# The default banner: crest's crown row beside the bold title row.
if ! printf '%s' "$mascot_boot" | grep -qF "▙▄▙▄▟▄▟"; then
	echo "FAIL: Phase 86 — the default crest mascot is missing from the startup banner" >&2
	status=1
fi
if ! printf '%s' "$mascot_boot" | grep -qF "Alter Zero (v"; then
	echo "FAIL: Phase 86 — the banner's title row is missing" >&2
	status=1
fi
tmux send-keys -t "$S86" -l "/mascot"
sleep 0.4
mascot_palette="$(tmux capture-pane -t "$S86" -p)"
echo "==== Phase 86: the palette filtered to /mascot ===="
printf '%s\n' "$mascot_palette"
if ! printf '%s' "$mascot_palette" | grep -qF "Choose the banner mascot"; then
	echo "FAIL: Phase 86 — /mascot is missing from the slash-command palette" >&2
	status=1
fi
tmux send-keys -t "$S86" Enter
sleep 0.5
mascot_open="$(tmux capture-pane -t "$S86" -p)"
echo "==== Phase 86: the picker open (crest highlighted + previewed) ===="
printf '%s\n' "$mascot_open"
for expect in "→ crest ✓" "sprout" "gem" "(1/6)" \
	"A crested hatchling flaring its frill" \
	"Type to search · Enter to choose · Esc to cancel"; do
	if ! printf '%s' "$mascot_open" | grep -qF "$expect"; then
		echo "FAIL: Phase 86 — the open picker is missing '$expect'" >&2
		status=1
	fi
done
# ↓ to bloom: the preview follows the selection live (bloom's petal row shows,
# crest's crown row leaves the preview slot — the startup banner above keeps
# its own crest, so the check is scoped to the picker's preview rows).
tmux send-keys -t "$S86" Down
sleep 0.4
mascot_bloom="$(tmux capture-pane -t "$S86" -p)"
echo "==== Phase 86: ↓ previews bloom ===="
printf '%s\n' "$mascot_bloom"
if ! printf '%s' "$mascot_bloom" | grep -qF "▀█▄███▄█▀"; then
	echo "FAIL: Phase 86 — moving the selection did not preview bloom's art" >&2
	status=1
fi
if ! printf '%s' "$mascot_bloom" | grep -qF "(2/6)"; then
	echo "FAIL: Phase 86 — the counter did not follow the selection" >&2
	status=1
fi
# Type-to-search narrows to sprout; Enter switches the banner.
tmux send-keys -t "$S86" -l "spr"
sleep 0.3
tmux send-keys -t "$S86" Enter
sleep 0.8
mascot_after="$(tmux capture-pane -t "$S86" -p -S -40)"
echo "==== Phase 86: after Enter — the banner redrawn + the toast ===="
printf '%s\n' "$mascot_after"
if ! printf '%s' "$mascot_after" | grep -qF "▝▛▛▀▜▜▘"; then
	echo "FAIL: Phase 86 — the banner did not redraw with sprout after Enter" >&2
	status=1
fi
if printf '%s' "$mascot_after" | grep -qF "▙▄▙▄▟▄▟"; then
	echo "FAIL: Phase 86 — the old crest banner survived the switch's purge rebuild" >&2
	status=1
fi
if ! printf '%s' "$mascot_after" | grep -qF "Mascot: sprout"; then
	echo "FAIL: Phase 86 — the switch was not confirmed with a toast" >&2
	status=1
fi
if ! grep -qF '"mascot": "sprout"' "$MASC_CFG/mascot.json" 2>/dev/null; then
	echo "FAIL: Phase 86 — mascot.json was not written (or holds the wrong mascot)" >&2
	status=1
fi
tmux send-keys -t "$S86" -l "/quit"
sleep 0.2
tmux send-keys -t "$S86" Enter
sleep 0.6
tmux kill-session -t "$S86" 2>/dev/null
# The persistence half: a fresh process against the same config home boots
# with sprout in the banner (the bootstrap seed, docs/mascot.md).
tmux new-session -d -s "$S86" -x 80 -y 30 "$APP_MASCOT"
sleep 0.8
mascot_relaunch="$(tmux capture-pane -t "$S86" -p)"
echo "==== Phase 86: a fresh launch keeps the saved mascot ===="
printf '%s\n' "$mascot_relaunch"
if ! printf '%s' "$mascot_relaunch" | grep -qF "▝▛▛▀▜▜▘"; then
	echo "FAIL: Phase 86 — the saved mascot did not survive a relaunch" >&2
	status=1
fi
tmux kill-session -t "$S86" 2>/dev/null
rm -rf "$MASC_CFG"


# --- Phase 87: CLICKABLE LINKS (docs/links.md). A URL wider than its row
# hard-breaks across display rows — this 26-col pane splits the reply's
# https://github.com/linuztx exactly like the report — and the terminal's own
# per-row URL detection then opened only the first fragment on click. Every
# painted fragment must ride an OSC 8 hyperlink carrying the FULL target. The
# escape renders as nothing, so the assertion reads the RAW byte stream
# (pipe-pane, captured before tmux interprets it); the link-id carrier must
# never leak as a real underline colour (SGR 58); and a falsy
# ALTER_ZERO_HYPERLINKS must emit no escape while the visible wrap stays
# identical. ---
S87="${S}_links"
LINKS_RAW="$(mktemp /tmp/alter-zero-smoke-links-XXXXXX)"
tmux new-session -d -s "$S87" -x 26 -y 40 "$APP"
tmux pipe-pane -t "$S87" -o "cat >> $LINKS_RAW"
sleep 0.4
tmux send-keys -t "$S87" -l "$USER_MSG"
sleep 0.2
tmux send-keys -t "$S87" Enter
# Poll the raw stream for the hyperlink open — it lands when the reply's URL
# commits — then give the rest of the turn a beat to settle.
for _ in $(seq 1 120); do
	if grep -aqF "]8;id=az" "$LINKS_RAW"; then
		break
	fi
	sleep 0.1
done
sleep 1.0
links_pane="$(tmux capture-pane -t "$S87" -p -S -200)"
echo "==== Phase 87: the 26-col pane wraps the URL (tail) ===="
printf '%s\n' "$links_pane" | tail -30
# The visible text is what it always was: the URL hard-breaks at the 24-col
# content width, so NO single row holds it whole — the split prefix row is
# there and the unbroken URL is not.
if ! printf '%s' "$links_pane" | grep -qF "https://github.com/linuz"; then
	echo "FAIL: Phase 87 — the reply's wrapped URL prefix is missing from the pane" >&2
	status=1
fi
if printf '%s' "$links_pane" | grep -qF "https://github.com/linuztx"; then
	echo "FAIL: Phase 87 — the URL fits one row; narrow the pane so this phase tests the split" >&2
	status=1
fi
# The raw stream carries what the screen cannot show: an OSC 8 open whose URI
# is the WHOLE URL (ESC ] 8 ; id=azN ; url ESC \), plus its close.
if ! grep -aqE $']8;id=az[0-9]+;https://github\\.com/linuztx\x1b' "$LINKS_RAW"; then
	echo "FAIL: Phase 87 — no OSC 8 open carries the full URL in the raw stream" >&2
	status=1
fi
if ! grep -aqF $'\x1b]8;;\x1b' "$LINKS_RAW"; then
	echo "FAIL: Phase 87 — the OSC 8 close is missing from the raw stream" >&2
	status=1
fi
# The id carrier (an RGB underline colour) is stripped at the paint boundary —
# a leaked SGR 58 would draw garbage underlines on supporting terminals.
if grep -aqF $'\x1b[58;' "$LINKS_RAW"; then
	echo "FAIL: Phase 87 — the link-id carrier leaked as an SGR 58 underline colour" >&2
	status=1
fi
tmux kill-session -t "$S87" 2>/dev/null

# The env gate: ALTER_ZERO_HYPERLINKS=0 emits no escapes — and the visible
# wrap is identical, so turning links off can never change the layout.
LINKS_RAW_OFF="$(mktemp /tmp/alter-zero-smoke-links-XXXXXX)"
tmux new-session -d -s "$S87" -x 26 -y 40 "env ALTER_ZERO_HYPERLINKS=0 $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux pipe-pane -t "$S87" -o "cat >> $LINKS_RAW_OFF"
sleep 0.4
tmux send-keys -t "$S87" -l "$USER_MSG"
sleep 0.2
tmux send-keys -t "$S87" Enter
links_off_pane=""
for _ in $(seq 1 120); do
	links_off_pane="$(tmux capture-pane -t "$S87" -p -S -200)"
	if printf '%s' "$links_off_pane" | grep -qF "https://github.com/linuz"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 87: gate off — same wrap, no escapes ===="
printf '%s\n' "$links_off_pane" | tail -12
if ! printf '%s' "$links_off_pane" | grep -qF "https://github.com/linuz"; then
	echo "FAIL: Phase 87 — the wrapped URL is missing with the gate off" >&2
	status=1
fi
if grep -aqF "]8;" "$LINKS_RAW_OFF"; then
	echo "FAIL: Phase 87 — ALTER_ZERO_HYPERLINKS=0 still emitted OSC 8" >&2
	status=1
fi
if grep -aqF $'\x1b[58;' "$LINKS_RAW_OFF"; then
	echo "FAIL: Phase 87 — the carrier leaked as SGR 58 with the gate off" >&2
	status=1
fi
tmux kill-session -t "$S87" 2>/dev/null
rm -f "$LINKS_RAW" "$LINKS_RAW_OFF"


# --- Phase 88: the PROTOCOL REVISION IS THE SERVER'S OWN, re-derived at
# every launch (docs/mcp.md). The reported bug: a DUAL-ERA server — one that
# serves the modern 2026-07-28 `server/discover` *and* answers the legacy
# `initialize` handshake, which is what every real dual-era server does —
# read `2025-11-25` on the /mcp detail page forever, because a remembered
# `legacy` verdict skipped the probe and the handshake it went to instead
# succeeded, so the wrong guess never failed and never corrected itself.
# The era cache is retired: this launches against a dual-era scripted server
# with a stale `mcp-era.json` sitting in the config dir claiming legacy, and
# the detail page must read the server's live revision — and the retired
# cache file must be swept away rather than left to mislead. ---
S88="${S}_mcpera"
MCP88_CFG="$(mktemp -d)"
MCP88_DIR="$(mktemp -d)"
cat >"$MCP88_DIR/server.sh" <<'MCPDUAL'
#!/bin/sh
# A DUAL-ERA server: it answers whichever era the client opens with.
while IFS= read -r line; do
	id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
	case "$line" in
	*'"method":"server/discover"'*)
		printf '{"jsonrpc":"2.0","id":%s,"result":{"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"ttlMs":60000,"cacheScope":"public","_meta":{"io.modelcontextprotocol/serverInfo":{"name":"dual","version":"1.0"}}}}\n' "$id" ;;
	*'"method":"initialize"'*)
		printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"dual","version":"1.0"}}}\n' "$id" ;;
	*'"method":"tools/list"'*)
		printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo_text","description":"Echo the text back.","inputSchema":{"type":"object","properties":{}}}]}}\n' "$id" ;;
	esac
done
MCPDUAL
chmod +x "$MCP88_DIR/server.sh"
cat >"$MCP88_CFG/mcp.json" <<MCP88JSON
{"mcpServers": {"dual": {"type": "stdio", "command": "sh", "args": ["$MCP88_DIR/server.sh"]}}}
MCP88JSON
# The stale verdict a previous build would have left behind, keyed exactly
# as it keyed it (the server's command line).
cat >"$MCP88_CFG/mcp-era.json" <<MCP88ERA
{"servers": {"sh $MCP88_DIR/server.sh": {"era": "legacy", "version": "2025-11-25"}}}
MCP88ERA
APP_MCP88="env ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$MCP88_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux new-session -d -s "$S88" -x 100 -y 36 "$APP_MCP88"
sleep 1.2
tmux send-keys -t "$S88" -l "/mcp"
sleep 0.4
tmux send-keys -t "$S88" Enter
sleep 1.2
tmux send-keys -t "$S88" Enter
sleep 0.6
mcp88_detail="$(tmux capture-pane -t "$S88" -p)"
echo "==== Phase 88: the dual-era server's detail page ===="
printf '%s\n' "$mcp88_detail"
for expect in "Dual MCP Server" "✔ connected" "Protocol:" "2026-07-28"; do
	if ! printf '%s' "$mcp88_detail" | grep -qF "$expect"; then
		echo "FAIL: Phase 88 — the detail page is missing '$expect'" >&2
		status=1
	fi
done
if printf '%s' "$mcp88_detail" | grep -qF "2025-11-25"; then
	echo "FAIL: Phase 88 — the stale cached revision is on the detail page" >&2
	status=1
fi
if [ -e "$MCP88_CFG/mcp-era.json" ]; then
	echo "FAIL: Phase 88 — the retired era cache survived the launch" >&2
	status=1
fi
tmux kill-session -t "$S88" 2>/dev/null
rm -rf "$MCP88_CFG" "$MCP88_DIR"

# --- Phase 89: the COMPOSER'S TERMINAL SHORTCUTS (docs/textarea.md). Cursor
# positions are made visible by typing after each motion: Ctrl+A/E jump the
# line ends, idle Ctrl+B steps left (nothing backgroundable is running),
# Alt+B walks a word back, Ctrl+W rubs out a unix word, Ctrl+U kills the
# line, Ctrl+H backspaces. All in the draft — nothing is ever submitted. ---
S89="${S}_termkeys"
tmux new-session -d -s "$S89" -x 100 -y 24 "$APP"
sleep 0.5
tmux send-keys -t "$S89" -l "alpha beta"
sleep 0.2
tmux send-keys -t "$S89" C-a
tmux send-keys -t "$S89" -l "zero "
sleep 0.2
tk_home="$(tmux capture-pane -t "$S89" -p)"
tmux send-keys -t "$S89" C-e
tmux send-keys -t "$S89" -l " delta"
sleep 0.2
tk_end="$(tmux capture-pane -t "$S89" -p)"
tmux send-keys -t "$S89" M-b
tmux send-keys -t "$S89" -l "X"
sleep 0.2
tk_word="$(tmux capture-pane -t "$S89" -p)"
tmux send-keys -t "$S89" C-e
tmux send-keys -t "$S89" C-w
sleep 0.2
tk_killw="$(tmux capture-pane -t "$S89" -p)"
tmux send-keys -t "$S89" C-u
sleep 0.2
tk_killu="$(tmux capture-pane -t "$S89" -p)"
tmux send-keys -t "$S89" -l "ab"
tmux send-keys -t "$S89" C-b
tmux send-keys -t "$S89" -l "Y"
sleep 0.2
tk_left="$(tmux capture-pane -t "$S89" -p)"
tmux send-keys -t "$S89" C-e
tmux send-keys -t "$S89" C-h
sleep 0.2
tk_bsp="$(tmux capture-pane -t "$S89" -p)"
tmux kill-session -t "$S89" 2>/dev/null
echo "==== Phase 89: terminal editing shortcuts in the composer ===="
if ! printf '%s' "$tk_home" | grep -qF "zero alpha beta"; then
	echo "FAIL: Phase 89 — Ctrl+A did not move to the line start (typed text not at the front)" >&2
	status=1
fi
if ! printf '%s' "$tk_end" | grep -qF "zero alpha beta delta"; then
	echo "FAIL: Phase 89 — Ctrl+E did not move to the line end" >&2
	status=1
fi
if ! printf '%s' "$tk_word" | grep -qF "zero alpha beta Xdelta"; then
	echo "FAIL: Phase 89 — Alt+B did not step back one word" >&2
	status=1
fi
if ! printf '%s' "$tk_killw" | grep -qF "zero alpha beta" ||
	printf '%s' "$tk_killw" | grep -qF "Xdelta"; then
	echo "FAIL: Phase 89 — Ctrl+W did not rub out the last word" >&2
	status=1
fi
if printf '%s' "$tk_killu" | grep -qF "zero alpha"; then
	echo "FAIL: Phase 89 — Ctrl+U did not kill the line" >&2
	status=1
fi
if ! printf '%s' "$tk_left" | grep -qF "aYb"; then
	echo "FAIL: Phase 89 — idle Ctrl+B did not step the cursor left" >&2
	status=1
fi
if ! printf '%s' "$tk_bsp" | grep -qF "aY" || printf '%s' "$tk_bsp" | grep -qF "aYb"; then
	echo "FAIL: Phase 89 — Ctrl+H did not backspace" >&2
	status=1
fi


# --- Phase 90: the READ-ONLY OVERLAYS stay reachable from a permission prompt
# (docs/permissions.md). The prompt is modal — it owns every key so the blocked
# tool thread can't be answered by accident — but Ctrl+O (the transcript) and
# Ctrl+D (the raw context) are exactly how you decide what to answer, so they
# are the two keys it lets through. Both must open over the open prompt, and
# both round trips must be a **no-op on the terminal**: the region is the one
# inline view that can be as tall as the screen and its growth scrolls chat
# one-way into real scrollback, so a return that repainted at the wrong seat
# would strand the prompt above a blank band or double the flowed rows. Driven
# at three heights — a region floating with rows to spare below it, one seated
# flush at the screen bottom, and one flush whose page FLOWS its top into real
# scrollback (docs/view-flow.md); each asserts the seat it is named for, so a
# case can never quietly degenerate into a copy of another.
# The screen AND the scrollback are compared byte for byte before and after —
# and where the region is meant to sit flush at the screen bottom the runner
# ASSERTS that first, since a stranding bug is invisible on a half-empty screen
# and the comparison would then prove nothing. Both exits are driven: the
# transcript is left by **Esc** — the key that would otherwise arm the
# edit-previous backtrack, so the trip exercises the guard keeping a parked
# tool thread's history un-rewindable (docs/backtrack.md) — and the context
# view by its own Ctrl+D. Phase 62 makes the same check in the other
# direction, with the overlay already up when the request lands. ---
po_round_trip() { # session, rows, cue, label, seat(flush | floating)
	local S90="$1" rows="$2" cue="$3" label="$4" seat="$5"
	tmux new-session -d -s "$S90" -x 100 -y "$rows" "$APP_PERM"
	sleep 0.4
	tmux send-keys -t "$S90" -l "$cue"
	sleep 0.2
	tmux send-keys -t "$S90" Enter
	local po_open=""
	for _ in $(seq 1 250); do
		if tmux capture-pane -t "$S90" -p | grep -qF "Do you want to create"; then
			po_open=1
			break
		fi
		sleep 0.05
	done
	sleep 0.4
	if [ -z "$po_open" ]; then
		echo "FAIL: Phase 90 ($label) — the permission prompt never showed" >&2
		status=1
		tmux kill-session -t "$S90" 2>/dev/null
		return
	fi
	local before before_sb in_o back_o in_d back_o_sb after_sb
	local before_rule back_rule
	before="$(tmux capture-pane -t "$S90" -p)"
	before_sb="$(tmux capture-pane -t "$S90" -p -S -100)"
	# Where the prompt's closing rule sits before the trip. Each case asserts
	# the seat it is *named* for — a flush case that had drifted off the
	# bottom would make "it came back flush" vacuous, and a floating case that
	# had grown flush would silently become a third copy of the flush one.
	before_rule=$(printf '%s\n' "$before" | grep -nF '────' | tail -1 | cut -d: -f1)
	if [ "$seat" = flush ] && [ "${before_rule:-0}" != "$rows" ]; then
		echo "FAIL: Phase 90 ($label) — the prompt is not flush at the bottom before the trip (closing rule on row ${before_rule:-none} of $rows), so the return check proves nothing" >&2
		status=1
	fi
	if [ "$seat" = floating ] && [ "${before_rule:-0}" = "$rows" ]; then
		echo "FAIL: Phase 90 ($label) — the prompt already reaches row $rows, so this is not the floating geometry it is meant to cover" >&2
		status=1
	fi
	# Ctrl+O — the transcript, on the alternate screen, over the open prompt.
	tmux send-keys -t "$S90" C-o
	sleep 0.7
	in_o="$(tmux capture-pane -t "$S90" -p)"
	# …left with **Esc**, the key that would otherwise arm the edit-previous
	# preview: under an open prompt it must simply close the overlay.
	tmux send-keys -t "$S90" Escape
	sleep 0.9
	back_o="$(tmux capture-pane -t "$S90" -p)"
	back_o_sb="$(tmux capture-pane -t "$S90" -p -S -100)"
	back_rule=$(printf '%s\n' "$back_o" | grep -nF '────' | tail -1 | cut -d: -f1)
	if [ "$seat" = flush ] && [ "${back_rule:-0}" != "$rows" ]; then
		echo "FAIL: Phase 90 ($label) — after the return the prompt's closing rule sits on row ${back_rule:-none} of $rows: it is stranded above a band of blank rows" >&2
		status=1
	fi
	# Ctrl+D — the raw LLM context, the same dance.
	tmux send-keys -t "$S90" C-d
	sleep 0.7
	in_d="$(tmux capture-pane -t "$S90" -p)"
	tmux send-keys -t "$S90" C-d
	sleep 0.9
	after_sb="$(tmux capture-pane -t "$S90" -p -S -100)"
	local back_d
	back_d="$(tmux capture-pane -t "$S90" -p)"
	echo "==== Phase 90 ($label): captured pane (the transcript over the prompt) ===="
	printf '%s\n' "$in_o"
	if ! printf '%s' "$in_o" | grep -qF "T R A N S C R I P T"; then
		echo "FAIL: Phase 90 ($label) — Ctrl+O did not open the transcript over the prompt" >&2
		status=1
	fi
	# The call being asked about is what the transcript must show — the whole
	# point of looking is reading what you are approving.
	if ! printf '%s' "$in_o" | grep -qF "Waiting…"; then
		echo "FAIL: Phase 90 ($label) — the transcript hides the call the prompt is asking about" >&2
		status=1
	fi
	# The Esc that left it must NOT have armed the backtrack preview — a rewind
	# would truncate the history a parked tool thread is still waiting on — and
	# the hint row promises exactly that, off the same `modal_open` predicate.
	if ! printf '%s' "$in_o" | grep -qF "q/esc/ctrl+o to quit"; then
		echo "FAIL: Phase 90 ($label) — the overlay offers 'esc to edit prev' under an open prompt: Esc would arm a rewind of the history a parked tool thread is waiting on" >&2
		status=1
	fi
	if ! printf '%s' "$back_o" | grep -qF "Do you want to create"; then
		echo "FAIL: Phase 90 ($label) — Esc from the overlay did not land back on the still-open prompt" >&2
		status=1
	fi
	if ! printf '%s' "$in_d" | grep -qF "C O N T E X T"; then
		echo "FAIL: Phase 90 ($label) — Ctrl+D did not open the context view over the prompt" >&2
		status=1
	fi
	if [ "$before" != "$back_o" ]; then
		echo "FAIL: Phase 90 ($label) — the Ctrl+O round trip changed the screen under the prompt" >&2
		diff <(printf '%s\n' "$before") <(printf '%s\n' "$back_o") >&2
		status=1
	fi
	if [ "$back_o" != "$back_d" ]; then
		echo "FAIL: Phase 90 ($label) — the Ctrl+D round trip changed the screen under the prompt" >&2
		diff <(printf '%s\n' "$back_o") <(printf '%s\n' "$back_d") >&2
		status=1
	fi
	if [ "$before_sb" != "$back_o_sb" ] || [ "$before_sb" != "$after_sb" ]; then
		echo "FAIL: Phase 90 ($label) — the round trips lost or doubled rows in the scrollback" >&2
		diff <(printf '%s\n' "$before_sb") <(printf '%s\n' "$after_sb") >&2
		status=1
	fi
	# …and the prompt is still answerable afterwards, draft restore included.
	tmux send-keys -t "$S90" -l "1"
	local po_done=""
	for _ in $(seq 1 250); do
		if tmux capture-pane -t "$S90" -p -S -80 | grep -qF "Wrote 8 lines to hello.py"; then
			po_done=1
			break
		fi
		sleep 0.05
	done
	if [ -z "$po_done" ]; then
		echo "FAIL: Phase 90 ($label) — the prompt no longer resolves after the overlay round trips" >&2
		status=1
	fi
	tmux kill-session -t "$S90" 2>/dev/null
}
# 44 rows: the whole conversation and the prompt fit with rows to spare, so the
# region floats — nothing has scrolled one-way, and the return must not start.
po_round_trip "${S}_permoverlay" 44 "permission demo please" "region floating" floating
# 30 rows: everything still fits the page, but the region now reaches the
# screen bottom — the seat a mis-timed return strands above a blank band.
po_round_trip "${S}_permoverlayfit" 30 "permission demo please" "page fits, flush" flush
# 18 rows: the page is taller than the terminal, so its top FLOWS into real
# scrollback (docs/view-flow.md) — the geometry the close's purge rebuild
# exists for, and the only one that exercises the flow signature across the
# overlay round trip.
po_round_trip "${S}_permoverlaytall" 18 "permission demo please" "page flows" flush
echo "==== Phase 90: Ctrl+O / Ctrl+D open over a permission prompt and give the screen back unchanged ===="


# --- Phase 91: the SESSION TEMP TREE (docs/scratchpad.md). One per-user,
# per-session root holds two named leaves: `scratchpad/` — created before the
# first frame, so the system prompt can name a directory that exists — and
# `tasks/`, where a background shell tees its interim output. TMPDIR points the
# whole tree at a throwaway dir so this phase can find it unambiguously (and so
# a concurrent session of the developer's own is never mistaken for it). ---
S91="${S}_scratchpad"
SP_TMP="$(mktemp -d /tmp/alter-zero-smoke-pad-XXXXXX)"
SPAPP="env $CFG_ENV_NOHIST TMPDIR=$SP_TMP ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"
tmux new-session -d -s "$S91" -x 80 -y 24 "$SPAPP"
sp_dir=""
for _ in $(seq 1 60); do
	sp_dir="$(find "$SP_TMP" -maxdepth 3 -type d -name scratchpad 2>/dev/null | head -1)"
	if [ -n "$sp_dir" ]; then break; fi
	sleep 0.15
done
# A backgrounded `!` command tees into the tasks leaf beside it.
tmux send-keys -t "$S91" -l "!sleep 30"
sleep 0.2
tmux send-keys -t "$S91" Enter
sleep 0.6
tmux send-keys -t "$S91" C-b # move the running command to the background
sp_out=""
for _ in $(seq 1 60); do
	sp_out="$(find "$SP_TMP" -maxdepth 4 -path '*/tasks/*.output' 2>/dev/null | head -1)"
	if [ -n "$sp_out" ]; then break; fi
	sleep 0.15
done
sp_pane="$(tmux capture-pane -t "$S91" -p)"
tmux kill-session -t "$S91" 2>/dev/null
echo "==== Phase 91: scratchpad='${sp_dir#"$SP_TMP"}', task output='${sp_out#"$SP_TMP"}' ===="
if [ -z "$sp_dir" ]; then
	echo "FAIL: Phase 91 — no scratchpad directory was created under the session root" >&2
	status=1
elif ! printf '%s' "$sp_dir" | grep -qE "^$SP_TMP/alter-zero-[0-9]+/[^/]+/scratchpad$"; then
	echo "FAIL: Phase 91 — the scratchpad is not at {tmp}/alter-zero-{uid}/{session}/scratchpad (got '$sp_dir')" >&2
	status=1
fi
if [ -z "$sp_out" ]; then
	echo "FAIL: Phase 91 — a backgrounded command left no interim output file" >&2
	status=1
elif [ "$(dirname "$(dirname "$sp_out")")" != "$(dirname "$sp_dir")" ]; then
	echo "FAIL: Phase 91 — the tasks dir is not the scratchpad's sibling (output '$sp_out', scratchpad '$sp_dir')" >&2
	status=1
fi
if ! printf '%s' "$sp_pane" | grep -qF "Running in the background"; then
	echo "FAIL: Phase 91 — Ctrl+B did not move the command to the background" >&2
	status=1
fi
rm -rf "$SP_TMP" 2>/dev/null

# --- Phase 92: a VERY LONG output line is bounded and counted honestly
# (docs/long-lines.md). One 750-char line — a minified blob, a `curl` JSON body
# — used to spend the whole peek ceiling on itself (twelve rows of wrapped
# noise) under a hint claiming ONE line was hidden. It now shows
# TOOL_LINE_MAX_ROWS rows closed by a `…`, and the hint counts the display ROWS
# the expansion adds. At 80 columns the `  ⎿  ` gutter leaves 75, so 750 chars
# is exactly 10 rows: 3 shown, 7 hidden. Ctrl+O still holds all ten. ---
S92="${S}_longline"
tmux new-session -d -s "$S92" -x 80 -y 24 "$APP"
sleep 0.4
tmux send-keys -t "$S92" -l "!printf 'x%.0s' \$(seq 1 750)"
sleep 0.2
tmux send-keys -t "$S92" Enter
ll_pane=""
for _ in $(seq 1 60); do # up to ~6s
	ll_pane="$(tmux capture-pane -t "$S92" -p -S -40)"
	if printf '%s' "$ll_pane" | grep -qF "lines (ctrl+o to expand)"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 92: captured pane (a 750-char single line, clipped and counted) ===="
printf '%s\n' "$ll_pane"
ll_rows="$(printf '%s' "$ll_pane" | grep -c 'xxxxxxxx')"
if [ "$ll_rows" -ne 3 ]; then
	echo "FAIL: Phase 92 — the 750-char line painted $ll_rows rows inline, not the 3-row per-line budget" >&2
	status=1
fi
if ! printf '%s' "$ll_pane" | grep -qE 'x…$'; then
	echo "FAIL: Phase 92 — the clipped row carries no … marker, so it reads as a line that simply ended" >&2
	status=1
fi
if ! printf '%s' "$ll_pane" | grep -qF "… +7 lines (ctrl+o to expand)"; then
	echo "FAIL: Phase 92 — the hint does not count the 7 hidden display rows (a source-line count would say '+1 lines')" >&2
	status=1
fi
# Ctrl+O: the expansion is where the whole line lives — all ten rows, unmarked.
tmux send-keys -t "$S92" C-o
sleep 0.5
ll_view="$(tmux capture-pane -t "$S92" -p)"
echo "==== Phase 92: captured pane (Ctrl+O — the line whole) ===="
printf '%s\n' "$ll_view"
ll_full="$(printf '%s' "$ll_view" | grep -c 'xxxxxxxx')"
if [ "$ll_full" -lt 10 ]; then
	echo "FAIL: Phase 92 — the transcript shows only $ll_full of the line's 10 rows" >&2
	status=1
fi
if printf '%s' "$ll_view" | grep -qE 'x…$'; then
	echo "FAIL: Phase 92 — the expansion clipped the line too, leaving the text nowhere" >&2
	status=1
fi
tmux send-keys -t "$S92" C-o
sleep 0.3
tmux kill-session -t "$S92" 2>/dev/null

# The other half of the same rule, and the reported one (docs/long-lines.md
# "Rows, not lines"): FOUR wrapping lines — a `curl | grep` of a web page —
# where every single line is inside its own 3-row budget and the CELL was still
# ten rows of noise, because the block was budgeted in source lines. The peek is
# bounded in display ROWS now (TOOL_PEEK_ROWS = 4), so four 150-char lines (2
# rows each at the 75-column gutter) show the first two and hide the rest. ---
S92B="${S}_peekrows"
tmux new-session -d -s "$S92B" -x 80 -y 24 "$APP"
sleep 0.4
tmux send-keys -t "$S92B" -l "!for c in a b c d; do printf \"\$c%.0s\" \$(seq 1 150); echo; done"
sleep 0.2
tmux send-keys -t "$S92B" Enter
pr_pane=""
for _ in $(seq 1 60); do # up to ~6s
	pr_pane="$(tmux capture-pane -t "$S92B" -p -S -40)"
	if printf '%s' "$pr_pane" | grep -qF "lines (ctrl+o to expand)"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 92: captured pane (four wrapping lines, bounded in rows) ===="
printf '%s\n' "$pr_pane"
pr_rows="$(printf '%s' "$pr_pane" | grep -cE 'aaaaaaaa|bbbbbbbb|cccccccc|dddddddd')"
if [ "$pr_rows" -ne 4 ]; then
	echo "FAIL: Phase 92 — the four-line output painted $pr_rows rows inline, not the 4-row peek ceiling" >&2
	status=1
fi
if printf '%s' "$pr_pane" | grep -qF "cccccccc"; then
	echo "FAIL: Phase 92 — the third line shows inline: the block ceiling is not bounding the cell" >&2
	status=1
fi
if ! printf '%s' "$pr_pane" | grep -qF "… +4 lines (ctrl+o to expand)"; then
	echo "FAIL: Phase 92 — the hint does not count the 4 hidden display rows" >&2
	status=1
fi
# Ctrl+O still holds every row — the cell is bounded, the output is not lost.
tmux send-keys -t "$S92B" C-o
sleep 0.5
pr_view="$(tmux capture-pane -t "$S92B" -p)"
echo "==== Phase 92: captured pane (Ctrl+O — all four lines) ===="
printf '%s\n' "$pr_view"
pr_full="$(printf '%s' "$pr_view" | grep -cE 'aaaaaaaa|bbbbbbbb|cccccccc|dddddddd')"
if [ "$pr_full" -lt 8 ]; then
	echo "FAIL: Phase 92 — the transcript shows only $pr_full of the output's 8 rows" >&2
	status=1
fi
tmux send-keys -t "$S92B" C-o
sleep 0.3
tmux kill-session -t "$S92B" 2>/dev/null

# The third part of the same rule (docs/long-lines.md "The peek is the output's
# first block"): a BLANK line costs a full row of a four-row cell and says
# nothing. Leading blanks are skipped and the first blank after the content
# closes the peek, so an output shaped `\n\nfirst\nsecond\n\nhidden` shows
# exactly `first` + `second` — no empty gutter row above them, and no fragment
# of the next block below — while the hint still counts every hidden row
# (the two leading blanks, the closing blank, and `hidden` = 4). ---
S92C="${S}_peekblank"
tmux new-session -d -s "$S92C" -x 80 -y 24 "$APP"
sleep 0.4
tmux send-keys -t "$S92C" -l "!printf '\n\nfirst\nsecond\n\nhidden\n'"
sleep 0.2
tmux send-keys -t "$S92C" Enter
pb_pane=""
for _ in $(seq 1 60); do # up to ~6s
	pb_pane="$(tmux capture-pane -t "$S92C" -p -S -40)"
	if printf '%s' "$pb_pane" | grep -qF "lines (ctrl+o to expand)"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 92: captured pane (the peek is the output's first block) ===="
printf '%s\n' "$pb_pane"
if ! printf '%s' "$pb_pane" | grep -qE '⎿ +first'; then
	echo "FAIL: Phase 92 — the peek does not open on the first non-blank line" >&2
	status=1
fi
if ! printf '%s' "$pb_pane" | grep -qE '^ +second$'; then
	echo "FAIL: Phase 92 — the first block's second line is missing from the peek" >&2
	status=1
fi
# Anchored to a gutter row: the echoed `! printf …` header names `hidden` too.
if printf '%s' "$pb_pane" | grep -qE '^ +hidden$'; then
	echo "FAIL: Phase 92 — the peek hopped the blank line into the next block" >&2
	status=1
fi
if ! printf '%s' "$pb_pane" | grep -qF "… +4 lines (ctrl+o to expand)"; then
	echo "FAIL: Phase 92 — the hint does not count the skipped blank rows" >&2
	status=1
fi
# Ctrl+O still holds the blanks and the block below them — the cell is a peek,
# not a filter.
tmux send-keys -t "$S92C" C-o
sleep 0.5
pb_view="$(tmux capture-pane -t "$S92C" -p)"
echo "==== Phase 92: captured pane (Ctrl+O — the whole output, blanks included) ===="
printf '%s\n' "$pb_view"
if ! printf '%s' "$pb_view" | grep -qE '^ +hidden$'; then
	echo "FAIL: Phase 92 — the transcript dropped the block the peek hid" >&2
	status=1
fi
tmux send-keys -t "$S92C" C-o
sleep 0.3
tmux kill-session -t "$S92C" 2>/dev/null


# --- Phase 93: the Ctrl+D view's CLASSIFIER PAGE (docs/permissions.md). Auto
# mode's reviewer reads a bounded task context — the recent user requests plus
# one line per action — and that context is the one input to a verdict the
# user cannot otherwise see: the request is silent and the cell shows only the
# outcome. It is Tab away inside the Ctrl+D view, sharing its chrome. Four
# claims: Ctrl+D opens on the LLM window, TAB flips to the classifier page
# (its own title + the mode note, plus a hint pointing back), Tab flips back,
# and the whole round trip is a NO-OP on the terminal — screen and scrollback
# byte-identical, the overlay-return invariant every alternate-screen view
# owes (invariant 4). The dummy keeps no classifier log (its offline auto demo
# answers from the pure `auto_verdict` heuristic), so the body is the
# placeholder here; the live block is covered by the classifier unit tests and
# tests/live_openrouter.rs. ---
S93="${S}_classifier"
tmux new-session -d -s "$S93" -x 90 -y 24 "$APP"
sleep 0.4
tmux send-keys -t "$S93" -l "hello"
sleep 0.2
tmux send-keys -t "$S93" Enter
cg_done=""
for _ in $(seq 1 120); do # up to ~12s
	if tmux capture-pane -t "$S93" -p | grep -qE "Done for [0-9]+"; then
		cg_done=1
		break
	fi
	sleep 0.1
done
if [ -z "$cg_done" ]; then
	echo "FAIL: Phase 93 — the reply never finished, so the round trip proves nothing" >&2
	status=1
fi
sleep 0.3
cg_before="$(tmux capture-pane -t "$S93" -p)"
cg_scroll_before="$(tmux capture-pane -t "$S93" -p -S -60)"
tmux send-keys -t "$S93" C-d
sleep 0.6
cg_llm="$(tmux capture-pane -t "$S93" -p)"
if ! printf '%s' "$cg_llm" | grep -qF "C O N T E X T"; then
	echo "FAIL: Phase 93 — Ctrl+D did not open on the LLM context page" >&2
	status=1
fi
if ! printf '%s' "$cg_llm" | grep -qF "tab for classifier context"; then
	echo "FAIL: Phase 93 — the LLM page never advertises its other half" >&2
	status=1
fi
tmux send-keys -t "$S93" Tab
sleep 0.5
cg_view="$(tmux capture-pane -t "$S93" -p)"
echo "==== Phase 93: captured pane (Ctrl+D, Tab — the classifier page) ===="
printf '%s\n' "$cg_view"
if ! printf '%s' "$cg_view" | grep -qF "C L A S S I F I E R"; then
	echo "FAIL: Phase 93 — Tab did not flip to the classifier page" >&2
	status=1
fi
if ! printf '%s' "$cg_view" | grep -qF "tab for llm context"; then
	echo "FAIL: Phase 93 — the classifier page never points back" >&2
	status=1
fi
# The mode note always shows, so the page can never read as "the classifier is
# deciding this" when it is not — here the suite's default manual mode, whose
# note says the log is recorded but consulted only in auto.
if ! printf '%s' "$cg_view" | grep -qE "auto mode|disabled"; then
	echo "FAIL: Phase 93 — no mode note above the block" >&2
	status=1
fi
tmux send-keys -t "$S93" Tab
sleep 0.5
if ! tmux capture-pane -t "$S93" -p | grep -qF "C O N T E X T"; then
	echo "FAIL: Phase 93 — Tab did not flip back to the LLM context page" >&2
	status=1
fi
tmux send-keys -t "$S93" C-d
sleep 0.6
cg_after="$(tmux capture-pane -t "$S93" -p)"
cg_scroll_after="$(tmux capture-pane -t "$S93" -p -S -60)"
if [ "$cg_before" != "$cg_after" ]; then
	echo "FAIL: Phase 93 — the Ctrl+D round trip changed the screen" >&2
	printf 'before:\n%s\nafter:\n%s\n' "$cg_before" "$cg_after" >&2
	status=1
fi
if [ "$cg_scroll_before" != "$cg_scroll_after" ]; then
	echo "FAIL: Phase 93 — the Ctrl+D round trip lost or doubled scrollback rows" >&2
	status=1
fi
# …and the page persists across opens: Ctrl+D comes back to the classifier
# page when that is where you left it, and q closes from there.
tmux send-keys -t "$S93" C-d
sleep 0.4
tmux send-keys -t "$S93" Tab
sleep 0.4
tmux send-keys -t "$S93" C-d
sleep 0.4
tmux send-keys -t "$S93" C-d
sleep 0.5
if ! tmux capture-pane -t "$S93" -p | grep -qF "C L A S S I F I E R"; then
	echo "FAIL: Phase 93 — the view did not reopen on the page it was left on" >&2
	status=1
fi
tmux send-keys -t "$S93" -l "q"
sleep 0.5
if tmux capture-pane -t "$S93" -p | grep -qF "C L A S S I F I E R"; then
	echo "FAIL: Phase 93 — q did not close the view from the classifier page" >&2
	status=1
fi
tmux kill-session -t "$S93" 2>/dev/null

# --- Phase 94: an alternate-screen overlay is SILENT on a page that has not
# changed, so its text can be selected and copied WHILE A TURN RUNS
# (docs/overlay-repaint.md). A terminal drops the user's mouse selection the
# moment the cells under it are rewritten, and the overlay used to
# re-serialize every cell of the screen on every frame — up to 120 a second
# while a turn streamed, floored at ~31 by the status animation's clock chain
# — so text in Ctrl+O / Ctrl+D could not be copied until the turn finished
# (the reported bug). `draw_overlay` now diffs against the frame already on
# the alternate screen and emits nothing at all when nothing moved, and the
# draw tick stops re-arming the clock chain under an overlay (none of what it
# animates is on screen there). Measured, not eyeballed: `tmux pipe-pane`
# captures every byte the app writes to the pane. The turn is a 30s local
# `!sleep`, so it is unambiguously ACTIVE for the whole window and the pane's
# own liveness is asserted after each measurement. The INLINE control must
# write something — otherwise a zero would only prove the meter is broken —
# and each overlay must then write nothing while still coming back to life on
# the next keypress. ---
S94="${S}_overlaysilence"
op_bytes() { # keys… → bytes written to the pane over 2s of an active turn
	local sess="$1"
	shift
	tmux kill-session -t "$sess" 2>/dev/null
	tmux new-session -d -s "$sess" -x 100 -y 30 "$APP"
	sleep 0.7
	tmux send-keys -t "$sess" -l '!sleep 30'
	sleep 0.3
	tmux send-keys -t "$sess" Enter
	local started=""
	for _ in $(seq 1 80); do
		if tmux capture-pane -t "$sess" -p | grep -qF "Running…"; then
			started=1
			break
		fi
		sleep 0.1
	done
	if [ -z "$started" ]; then
		echo "-1"
		return
	fi
	local k
	for k in "$@"; do
		tmux send-keys -t "$sess" "$k"
		sleep 0.5
	done
	sleep 0.3
	local log="${TMPDIR:-/tmp}/alterzero_smoke_bytes_$$_${sess##*_}"
	: >"$log"
	tmux pipe-pane -t "$sess" -o "cat >> $log"
	sleep 2
	tmux pipe-pane -t "$sess"
	wc -c <"$log"
	rm -f "$log"
}
inline_bytes="$(op_bytes "${S94}_inline")"
# The meter itself: the inline view animates its status line, so it MUST write.
if [ "${inline_bytes:-0}" -le 0 ]; then
	echo "FAIL: Phase 94 — the inline control wrote $inline_bytes bytes during an active turn, so the byte meter proves nothing" >&2
	status=1
fi
for probe in "C-d:the Ctrl+D context view" "C-o:the Ctrl+O transcript"; do
	key="${probe%%:*}"
	what="${probe#*:}"
	got="$(op_bytes "${S94}_${key//-/}" "$key")"
	echo "==== Phase 94: $what wrote $got bytes over 2s of an active turn (inline control: $inline_bytes) ===="
	if [ "${got:-1}" -lt 0 ]; then
		echo "FAIL: Phase 94 — the shell turn never started under $what" >&2
		status=1
	elif [ "${got:-1}" -ne 0 ]; then
		echo "FAIL: Phase 94 — $what rewrote the terminal ($got bytes) over an unchanged page: a mouse selection there is dropped, so its text cannot be copied mid-turn" >&2
		status=1
	fi
done
# …and silence is not death: the page still repaints the moment its content
# changes. Open the transcript mid-turn, let the reply stream under it, and
# assert the view actually moved — the diff baseline going stale would show
# here as a frozen page.
tmux kill-session -t "$S94" 2>/dev/null
tmux new-session -d -s "$S94" -x 100 -y 30 "$APP"
sleep 0.7
tmux send-keys -t "$S94" -l "$USER_MSG"
sleep 0.3
tmux send-keys -t "$S94" Enter
sleep 0.4
tmux send-keys -t "$S94" C-o # open before the dummy's pre-stream pause ends
sleep 0.5
live_a="$(tmux capture-pane -t "$S94" -p)"
for _ in $(seq 1 120); do # …and let the reply stream under it
	if tmux capture-pane -t "$S94" -p | grep -qF "$EXPECT_REPLY"; then
		break
	fi
	sleep 0.1
done
sleep 1.2
live_b="$(tmux capture-pane -t "$S94" -p)"
echo "==== Phase 94: the transcript after the reply streamed under it ===="
printf '%s\n' "$live_b" | sed -n '1,14p'
if ! printf '%s' "$live_b" | grep -qF "T R A N S C R I P T"; then
	echo "FAIL: Phase 94 — the transcript is not up" >&2
	status=1
fi
if [ "$live_a" = "$live_b" ]; then
	echo "FAIL: Phase 94 — the overlay froze: the reply streamed underneath and the page never repainted (a stale diff baseline)" >&2
	status=1
fi
if ! printf '%s' "$live_b" | grep -qF "$EXPECT_REPLY"; then
	echo "FAIL: Phase 94 — the streamed reply never reached the open transcript" >&2
	status=1
fi
# …and the OTHER half, which the byte count cannot see: that the incremental
# paints leave the *right* cells on screen. Let the whole turn settle under the
# overlay (hundreds of diff frames), scroll away from the tail and back (more
# diffs), then resize away and straight back — a burst whose net size is the one
# the baseline records, which is why `resized` drops that baseline outright
# instead of trusting the area check (whether the two events actually coalesce
# into one frame is up to the scheduler, so this is a best-effort reproduction
# and a permanent guard on the post-resize repaint path either way). Finally
# close and REOPEN: `enter_overlay` clears the alternate screen, so that frame
# is a pure full repaint of the same content at the same scroll seat (End
# re-engaged tail-follow, and an open re-arms it). The two screens must match
# exactly — any mismatch is a cell the diff path left stale.
for _ in $(seq 1 120); do # let the turn finish under the overlay
	if tmux capture-pane -t "$S94" -p | grep -qF "$SETTLED_REPLY"; then
		break
	fi
	sleep 0.15
done
sleep 1.0 # StreamDone + the Done-for summary land under it
for _ in 1 2 3; do
	tmux send-keys -t "$S94" PageUp
	sleep 0.15
done
tmux send-keys -t "$S94" End
sleep 0.5
tmux resize-window -t "$S94" -x 88 -y 26 2>/dev/null
tmux resize-window -t "$S94" -x 100 -y 30 2>/dev/null
sleep 0.8
overlay_incremental="$(tmux capture-pane -t "$S94" -p)"
tmux send-keys -t "$S94" -l "q" # close (q, not Esc: idle Esc arms the backtrack)
sleep 0.7
tmux send-keys -t "$S94" C-o # …and reopen: a clear + full repaint
sleep 0.9
overlay_full="$(tmux capture-pane -t "$S94" -p)"
if ! printf '%s' "$overlay_full" | grep -qF "T R A N S C R I P T"; then
	echo "FAIL: Phase 94 — the transcript did not reopen, so the stale-cell check proves nothing" >&2
	status=1
elif [ "$overlay_incremental" != "$overlay_full" ]; then
	echo "FAIL: Phase 94 — the incrementally-painted overlay differs from a full repaint of the same content: the diff left a stale cell on the alternate screen" >&2
	echo "---- incrementally painted ----" >&2
	printf '%s\n' "$overlay_incremental" >&2
	echo "---- full repaint ----" >&2
	printf '%s\n' "$overlay_full" >&2
	status=1
fi
# …and the clock chain the draw tick stopped re-arming must come BACK on the
# return: it seeds from the Submit keypress and would otherwise stay broken for
# the rest of the turn, freezing the inline timer and the spinner's sweep. Round
# trip through the overlay mid-turn, then assert the inline view is writing
# again and its elapsed counter is advancing.
tmux kill-session -t "$S94" 2>/dev/null
tmux new-session -d -s "$S94" -x 100 -y 30 "$APP"
sleep 0.7
tmux send-keys -t "$S94" -l '!sleep 30'
sleep 0.3
tmux send-keys -t "$S94" Enter
reseed_go=""
for _ in $(seq 1 80); do
	if tmux capture-pane -t "$S94" -p | grep -qF "Running…"; then
		reseed_go=1
		break
	fi
	sleep 0.1
done
if [ -z "$reseed_go" ]; then
	echo "FAIL: Phase 94 — the shell turn never started for the re-seed check" >&2
	status=1
fi
tmux send-keys -t "$S94" C-o # into the overlay (the chain stops)…
sleep 0.8
tmux send-keys -t "$S94" C-o # …and back out (it must re-seed)
sleep 0.8
reseed_log="${TMPDIR:-/tmp}/alterzero_smoke_reseed_$$"
: >"$reseed_log"
tmux pipe-pane -t "$S94" -o "cat >> $reseed_log"
sleep 2.5
tmux pipe-pane -t "$S94"
reseed_bytes="$(wc -c <"$reseed_log")"
rm -f "$reseed_log"
echo "==== Phase 94: inline wrote $reseed_bytes bytes over 2.5s after an overlay round trip mid-turn ===="
if [ "${reseed_bytes:-0}" -le 0 ]; then
	echo "FAIL: Phase 94 — the animation chain never re-seeded after the overlay return: the inline status line is frozen for the rest of the turn" >&2
	status=1
fi
if ! tmux capture-pane -t "$S94" -p | grep -qE "Running… \([0-9]+s\)"; then
	echo "FAIL: Phase 94 — the live region's elapsed counter is not advancing after the overlay return" >&2
	status=1
fi
tmux kill-session -t "$S94" 2>/dev/null
echo "==== Phase 94: the overlays are silent on an unchanged page and repaint the moment one changes ===="

# --- Phase 95: the AGENT SESSION VIEW streams like the main one
# (docs/agent-view-streaming.md). The view has its own streaming strip, and it
# used to render the last row of a BATCH render of the agent's buffer — while
# its commits went through a `StreamRender`, which withholds a forming table
# WHOLE. So a subagent streaming a table showed one row (the block's closing
# border) and every row above it was on screen nowhere: the reported
# "streaming disappears inside the subagent TUI". The `agent-stream` demo
# launches one background subagent and plays its own round on the subagent
# channel — a thinking phase, then that table — so the whole thing is drivable
# offline. Walk the roster into its session and assert (a) the live
# `● Thinking…` block shows there, and (b) the forming GRID is in the strip
# mid-stream, not just its border. ---
S95="${S}_agentstream"
tmux new-session -d -s "$S95" -x 100 -y 34 "$APP"
sleep 0.7
tmux send-keys -t "$S95" -l "launch a subagent that streams a table"
sleep 0.3
tmux send-keys -t "$S95" Enter
agent_row=""
for _ in $(seq 1 200); do # the background launch puts its row on the roster
	if tmux capture-pane -t "$S95" -p | grep -qF "Stream a comparison table"; then
		agent_row=1
		break
	fi
	sleep 0.1
done
if [ -z "$agent_row" ]; then
	echo "FAIL: Phase 95 — the subagent demo never put its row on the footer roster" >&2
	status=1
fi
# ↓ opens the roster on `● main`, a second ↓ steps onto the agent, Enter opens
# its session view (the demo's pre-roll leaves time for exactly this).
tmux send-keys -t "$S95" Down
sleep 0.2
tmux send-keys -t "$S95" Down
sleep 0.2
tmux send-keys -t "$S95" Enter
sleep 0.3
agent_view="$(tmux capture-pane -t "$S95" -p)"
echo "==== Phase 95: the agent session view ===="
printf '%s\n' "$agent_view" | sed -n '1,20p'
if ! printf '%s' "$agent_view" | grep -qF "Compare four languages"; then
	echo "FAIL: Phase 95 — the agent session view did not open on the agent's own transcript" >&2
	status=1
fi
# (a) the live thinking block, and (b) the forming grid — both while the
# agent's own status line still says it is working, i.e. before the block
# commits. A broken strip shows `└────┴…┘` alone at (b).
agent_thinking=""
agent_grid=""
for _ in $(seq 1 250); do
	frame="$(tmux capture-pane -t "$S95" -p)"
	if [ -z "$agent_thinking" ] && printf '%s' "$frame" | grep -qF "● Thinking…"; then
		agent_thinking="$frame"
	fi
	if printf '%s' "$frame" | grep -qF "Working…" &&
		printf '%s' "$frame" | grep -qF "│ Python" &&
		printf '%s' "$frame" | grep -qF "│ Rust"; then
		agent_grid="$frame"
		break
	fi
	sleep 0.1
done
if [ -z "$agent_thinking" ]; then
	echo "FAIL: Phase 95 — the subagent's live '● Thinking…' block never showed in its session view" >&2
	status=1
else
	echo "==== Phase 95: the subagent's live thinking block ===="
	printf '%s\n' "$agent_thinking" | grep -A 4 "● Thinking…" | sed -n '1,5p'
fi
if [ -z "$agent_grid" ]; then
	echo "FAIL: Phase 95 — the forming table never showed in the agent view's strip: the streaming rows are on screen nowhere (the reported bug)" >&2
	echo "---- the last frame seen ----" >&2
	tmux capture-pane -t "$S95" -p >&2
	status=1
else
	echo "==== Phase 95: the forming grid in the agent view's strip ===="
	printf '%s\n' "$agent_grid" | grep -E "│|┌|└" | sed -n '1,12p'
fi
# …and its **parallel `write` batch** commits its cells at once, with no
# resize. The file tools resolve through the two-text split
# (`StreamEvent::ToolAnswered`), which the view's commit arm did not list, so
# the cells reached the agent's transcript and stopped there — a resize
# rebuilt the view from history and they all appeared at once (the reported
# bug). Assert them in the pane *before* anything resizes it.
for _ in $(seq 1 250); do
	if tmux capture-pane -t "$S95" -p -S -200 | grep -qF "Wrote 3 lines to notes/sources.md"; then
		break
	fi
	sleep 0.1
done
agent_writes="$(tmux capture-pane -t "$S95" -p -S -200)"
for want in "Write(notes/languages.md)" "Wrote 3 lines to notes/languages.md" \
	"Write(notes/sources.md)" "Wrote 3 lines to notes/sources.md"; do
	if ! printf '%s' "$agent_writes" | grep -qF "$want"; then
		echo "FAIL: Phase 95 — the subagent's parallel write batch never committed '$want' to scrollback (it needs a resize to appear)" >&2
		status=1
	fi
done
echo "==== Phase 95: the subagent's committed write cells ===="
printf '%s\n' "$agent_writes" | grep -A 3 -F "Write(notes/" | sed -n '1,10p'
# …and the table commits exactly once when it closes.
for _ in $(seq 1 200); do
	if tmux capture-pane -t "$S95" -p | grep -qF "Four rows, one grid."; then
		break
	fi
	sleep 0.1
done
sleep 0.6
agent_settled="$(tmux capture-pane -t "$S95" -p -S -200)"
grid_rows="$(printf '%s' "$agent_settled" | grep -cF "│ Python")"
if [ "${grid_rows:-0}" -ne 1 ]; then
	echo "FAIL: Phase 95 — the committed table appears $grid_rows times in scrollback (expected exactly 1)" >&2
	status=1
fi
if ! printf '%s' "$agent_settled" | grep -qF "Thought for"; then
	echo "FAIL: Phase 95 — the subagent's settled 'Thought for …' cell is missing from its transcript" >&2
	status=1
fi
tmux kill-session -t "$S95" 2>/dev/null
echo "==== Phase 95: the agent session view streams its frontier like the main one ===="



# --- Phase 96: the agent session view has the SAME mid-turn queue the main one
# has (docs/queue.md) — one mechanism, two levels. Open the demo subagent's
# session while it works, type a message: it waits inset above the box
# ("  ❯ also add Elixir") exactly like the main session's, and the agent's own
# loop takes it at its next ROUND boundary (its parallel `write` batch), where
# it commits at column 0 on **that agent's** transcript while the agent is
# still running. It must never touch the main conversation. Before this the
# view recorded the message the instant it was typed — claiming the agent had
# read something it had not — and showed no pending state at all. ---
S96="${S}_agentqueue"
tmux new-session -d -s "$S96" -x 100 -y 34 "$APP"
sleep 0.7
tmux send-keys -t "$S96" -l "launch a subagent that streams a table"
sleep 0.3
tmux send-keys -t "$S96" Enter
for _ in $(seq 1 200); do # the background launch puts its row on the roster
	if tmux capture-pane -t "$S96" -p | grep -qF "Stream a comparison table"; then
		break
	fi
	sleep 0.1
done
# ↓ opens the roster on `● main`, a second ↓ steps onto the agent, Enter opens
# its session view (the demo's pre-roll leaves time for exactly this).
tmux send-keys -t "$S96" Down
sleep 0.15
tmux send-keys -t "$S96" Down
sleep 0.15
tmux send-keys -t "$S96" Enter
sleep 0.3
tmux send-keys -t "$S96" -l "also add Elixir"
sleep 0.2
tmux send-keys -t "$S96" Enter
agent_pending=""
for _ in $(seq 1 40); do # the message waits inset above the box, not committed
	frame="$(tmux capture-pane -t "$S96" -p)"
	if printf '%s' "$frame" | grep -qF "  ❯ also add Elixir"; then
		agent_pending="$frame"
		break
	fi
	sleep 0.1
done
if [ -z "$agent_pending" ]; then
	echo "FAIL: Phase 96 — a message typed into a running agent's session never showed pending ('  ❯ also add Elixir' inset above the box)" >&2
	echo "---- the last frame seen ----" >&2
	tmux capture-pane -t "$S96" -p >&2
	status=1
else
	echo "==== Phase 96: the agent's own queued row ===="
	printf '%s\n' "$agent_pending" | grep -F "❯ also add Elixir"
fi
agent_delivered=""
for _ in $(seq 1 300); do # its loop takes it at the round boundary, still running
	frame="$(tmux capture-pane -t "$S96" -p -S -200)"
	if printf '%s' "$frame" | grep -qE '^❯ also add Elixir$' &&
		printf '%s' "$frame" | grep -qF "esc to interrupt"; then
		agent_delivered="$frame"
		break
	fi
	sleep 0.1
done
if [ -z "$agent_delivered" ]; then
	echo "FAIL: Phase 96 — the agent never took the queued message into its running turn ('❯ also add Elixir' at column 0 while its status line was still up)" >&2
	echo "---- the last frame seen ----" >&2
	tmux capture-pane -t "$S96" -p -S -200 >&2
	status=1
else
	echo "==== Phase 96: taken into the agent's own turn, after its write batch ===="
	printf '%s\n' "$agent_delivered" | grep -E "Wrote 3 lines to notes/sources.md|^❯ also add Elixir$"
	# It lands on the AGENT's transcript — under its launch prompt, after the
	# batch that ended the round — not on the main conversation's.
	if ! printf '%s' "$agent_delivered" | grep -qF "Compare four languages"; then
		echo "FAIL: Phase 96 — the delivered message is not on the agent's own transcript (its launch prompt is gone from the view)" >&2
		status=1
	fi
fi
# Esc back to the main conversation: its purge-rebuild must show no trace of a
# message that belonged to the agent's conversation.
tmux send-keys -t "$S96" Escape
sleep 0.6
main_after="$(tmux capture-pane -t "$S96" -p -S -200)"
if printf '%s' "$main_after" | grep -qF "also add Elixir"; then
	echo "FAIL: Phase 96 — the agent's message leaked into the MAIN conversation on return" >&2
	printf '%s\n' "$main_after" >&2
	status=1
fi
tmux kill-session -t "$S96" 2>/dev/null
echo "==== Phase 96: the subagent session's mid-turn queue matches the main one ===="


# --- Phase 97: Alt+Up pulls back a message the running turn has NOT read yet
# (docs/queue.md). Submit "hello there", Enter "oops typo" mid-stream — it waits
# inset above the box — then Alt+Up: only the shared queue knows whether the
# loop has taken it, so the boundary pops it and hands it back. The row goes and
# the text returns to the composer as an editable draft, with the turn still
# running. (The drive presses Alt+Up well before the turn's first tool
# resolution, which is the only point that could have taken it.) ---
S97="${S}_unsteer"
tmux new-session -d -s "$S97" -x 80 -y 24 "$APP"
sleep 0.4
tmux send-keys -t "$S97" -l "hello there"
sleep 0.2
tmux send-keys -t "$S97" Enter
for _ in $(seq 1 40); do # up to ~4s: wait until turn 1 is visibly streaming
	if tmux capture-pane -t "$S97" -p | grep -qF "Happy"; then
		break
	fi
	sleep 0.1
done
tmux send-keys -t "$S97" -l "oops typo"
sleep 0.2
tmux send-keys -t "$S97" Enter
unsteer_pending=""
for _ in $(seq 1 30); do # the message waits inset above the box
	unsteer_pending="$(tmux capture-pane -t "$S97" -p)"
	if printf '%s' "$unsteer_pending" | grep -qF "  ❯ oops typo"; then
		break
	fi
	sleep 0.1
done
tmux send-keys -t "$S97" M-Up
sleep 0.5
unsteer_after="$(tmux capture-pane -t "$S97" -p)"
echo "==== Phase 97: pulled back into the composer ===="
printf '%s\n' "$unsteer_after"
if ! printf '%s' "$unsteer_pending" | grep -qF "  ❯ oops typo"; then
	echo "FAIL: Phase 97 — the message never showed pending above the box" >&2
	status=1
fi
if printf '%s' "$unsteer_after" | grep -qF "  ❯ oops typo"; then
	echo "FAIL: Phase 97 — Alt+Up left the pending row up; it must go with the pull-back" >&2
	status=1
fi
if ! printf '%s' "$unsteer_after" | grep -qE '^❯ oops typo'; then
	echo "FAIL: Phase 97 — Alt+Up did not return the message to the composer as a draft" >&2
	status=1
fi
if ! printf '%s' "$unsteer_after" | grep -qF "esc to interrupt"; then
	echo "FAIL: Phase 97 — the turn stopped: Alt+Up must take the message back without touching it" >&2
	status=1
fi
tmux kill-session -t "$S97" 2>/dev/null
echo "==== Phase 97: an unread message comes back on Alt+Up ===="



# --- Phase 98: a subagent's PERMISSION PROMPT is about the conversation ON
# SCREEN (docs/permissions.md, docs/agent-view-streaming.md). In manual mode,
# standing inside a lone FOREGROUND subagent's session view, its `bash` request
# opened over the LEAD's live `● Agent(…)` / `⎿ Working…` cell — the main
# strip's lone-agent tree, which that screen does not show — hiding the agent's
# own `● Bash(ls -la)` / `⎿ Waiting…` and every waiting sibling of its parallel
# batch. The `agent-permission` demo launches one foreground subagent that
# announces two `bash` calls and asks at the shared gate before each: walk into
# its session and assert the prompt's context is ITS cells, not the lead's —
# then leave and assert the MAIN view still shows the lead's cell, which is
# what is on screen there. ---
S98="${S}_agentperm"
tmux new-session -d -s "$S98" -x 100 -y 34 "$APP"
sleep 0.7
tmux send-keys -t "$S98" -l "subagent permission demo"
sleep 0.3
tmux send-keys -t "$S98" Enter
agentperm_row=""
for _ in $(seq 1 200); do # the foreground launch puts its row on the roster
	if tmux capture-pane -t "$S98" -p | grep -qF "Run ls -la via subagent"; then
		agentperm_row=1
		break
	fi
	sleep 0.1
done
if [ -z "$agentperm_row" ]; then
	echo "FAIL: Phase 98 — the gated subagent demo never put its row on the footer roster" >&2
	status=1
fi
# The lead's own cell IS right in the main view — a lone foreground launch
# wears the tool-cell look there (docs/agent-tool.md).
agentperm_main="$(tmux capture-pane -t "$S98" -p)"
if ! printf '%s' "$agentperm_main" | grep -qF "● Agent(Run ls -la via subagent)"; then
	echo "FAIL: Phase 98 — the main view is missing the lead's live agent cell" >&2
	status=1
fi
# ↓ opens the roster on `● main`, a second ↓ steps onto the agent, Enter opens
# its session — all inside the demo's pre-roll, before its first request.
tmux send-keys -t "$S98" Down
sleep 0.2
tmux send-keys -t "$S98" Down
sleep 0.2
tmux send-keys -t "$S98" Enter
agentperm_entered=""
for _ in $(seq 1 60); do # the view is up once its rule carries the description
	if tmux capture-pane -t "$S98" -p | grep -qF "─ Run ls -la via subagent ─"; then
		agentperm_entered=1
		break
	fi
	sleep 0.1
done
if [ -z "$agentperm_entered" ]; then
	echo "FAIL: Phase 98 — never reached the subagent's session view (the demo's pre-roll must outlast the ↓ ↓ Enter walk)" >&2
	status=1
fi
agentperm_view=""
for _ in $(seq 1 200); do # …and its first `bash` call raises the prompt
	agentperm_view="$(tmux capture-pane -t "$S98" -p)"
	if printf '%s' "$agentperm_view" | grep -qF "Do you want to proceed?"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 98: the prompt inside the subagent's session view ===="
printf '%s\n' "$agentperm_view"
if ! printf '%s' "$agentperm_view" | grep -qF "● Bash(ls -la)"; then
	echo "FAIL: Phase 98 — the prompt does not show the AGENT's own call above it" >&2
	status=1
fi
if ! printf '%s' "$agentperm_view" | grep -qF "● Bash(pwd)"; then
	echo "FAIL: Phase 98 — the batch's waiting sibling is missing from the prompt's context" >&2
	status=1
fi
if [ "$(printf '%s' "$agentperm_view" | grep -cF "⎿  Waiting…")" -ne 2 ]; then
	echo "FAIL: Phase 98 — both queued cells must read '⎿ Waiting…' above the prompt" >&2
	status=1
fi
if printf '%s' "$agentperm_view" | grep -qF "● Agent(Run ls -la via subagent)"; then
	echo "FAIL: Phase 98 — the LEAD's agent cell covered the agent's own cells (the reported bug)" >&2
	status=1
fi
if ! printf '%s' "$agentperm_view" | grep -qF "Bash command · from the general-purpose agent"; then
	echo "FAIL: Phase 98 — the prompt does not say which subagent asked" >&2
	status=1
fi
# Answer it (option 1), let the batch drain, then Esc back to the main view:
# the lead's cell is on screen there, and the agent's own cells are not.
tmux send-keys -t "$S98" Enter
sleep 0.6
for _ in $(seq 1 200); do # the second call raises its own prompt
	if tmux capture-pane -t "$S98" -p | grep -qF "don't ask again for: pwd"; then
		break
	fi
	sleep 0.1
done
agentperm_second="$(tmux capture-pane -t "$S98" -p)"
if ! printf '%s' "$agentperm_second" | grep -qF "● Bash(pwd)"; then
	echo "FAIL: Phase 98 — the second prompt lost the call it is about" >&2
	status=1
fi
tmux send-keys -t "$S98" Enter
sleep 0.8
tmux send-keys -t "$S98" Escape
sleep 0.8
agentperm_back="$(tmux capture-pane -t "$S98" -p)"
echo "==== Phase 98: back in the main view ===="
printf '%s\n' "$agentperm_back"
if ! printf '%s' "$agentperm_back" | grep -qF "subagent permission demo"; then
	echo "FAIL: Phase 98 — Esc did not return to the main conversation" >&2
	status=1
fi
tmux kill-session -t "$S98" 2>/dev/null
echo "==== Phase 98: a subagent's prompt asks about the screen it opens on ===="


# --- Phase 99: a BAND OPENED UNDER A TALL STREAMING PREVIEW keeps the composer
# (docs/table-streaming.md "The preview slot is budgeted"). The strip's preview
# was capped against a FIXED chrome allowance — the box, the status line, a
# footer — so a forming table always spent the region's whole slack. Press `/`
# (or `?`, or `@`) mid-table on a small terminal and the band asked for rows
# the terminal did not have: the constraint solver spent them on the strip and
# the textarea disappeared until the turn ended — the reported bug. The budget
# now pays every other row the region owes first and the preview tail-follows
# into what is left, so the box, the band and the status line are all on screen
# together. Driven at the reported size with each of the three bands. ---
S99="${S}_bandpreview"
for band_key in "/" "?" "@s"; do
	tmux kill-session -t "$S99" 2>/dev/null
	tmux new-session -d -s "$S99" -x 51 -y 24 "$APP"
	sleep 0.7
	tmux send-keys -t "$S99" -l "response again but now with table"
	sleep 0.2
	tmux send-keys -t "$S99" Enter
	# Wait until the block is genuinely taller than the region's slack: the
	# demo table's fifth record is well past it at 51 columns.
	band_ready=""
	for _ in $(seq 1 250); do
		if tmux capture-pane -t "$S99" -p | grep -qF "Shell Command"; then
			band_ready=1
			break
		fi
		sleep 0.1
	done
	if [ -z "$band_ready" ]; then
		echo "FAIL: Phase 99 — the table demo never streamed past the region's slack" >&2
		status=1
	fi
	tmux send-keys -t "$S99" -l "$band_key"
	sleep 0.5
	band_pane="$(tmux capture-pane -t "$S99" -p)"
	echo "==== Phase 99: '$band_key' opened mid-table on a 51x24 pane ===="
	printf '%s\n' "$band_pane"
	# The composer: its prompt row, and both of the box's rules around it.
	if ! printf '%s' "$band_pane" | grep -qE '^❯'; then
		echo "FAIL: Phase 99 — '$band_key' mid-table squeezed the composer off the screen" >&2
		status=1
	fi
	# The rules are counted in awk's byte mode (the suite runs in a POSIX
	# locale, where `─+` would quantify the glyph's last byte): a rule row is
	# one that is nothing but `─`.
	band_rules="$(printf '%s\n' "$band_pane" |
		awk '{ bare = $0; gsub(/─/, "", bare); if (length($0) > 0 && length(bare) == 0) n++ } END { print n + 0 }')"
	if [ "$band_rules" -ne 2 ]; then
		echo "FAIL: Phase 99 — the box lost a rule under the '$band_key' band ($band_rules of 2)" >&2
		status=1
	fi
	# …and neither the band nor the turn's status line was traded away for it.
	if ! printf '%s' "$band_pane" | grep -qF "Working…"; then
		echo "FAIL: Phase 99 — the status line went missing under the '$band_key' band" >&2
		status=1
	fi
	case "$band_key" in
	"/") band_marker="/help" ;;
	"?") band_marker="for commands" ;;
	*) band_marker="Dir" ;;
	esac
	if ! printf '%s' "$band_pane" | grep -qF "$band_marker"; then
		echo "FAIL: Phase 99 — the '$band_key' band did not open" >&2
		status=1
	fi
	# The preview yielded rather than the composer: the forming block is still
	# there, tail-following into the rows that are left.
	if ! printf '%s' "$band_pane" | grep -qE "(Code Snippet|│)"; then
		echo "FAIL: Phase 99 — the forming table vanished from the strip entirely" >&2
		status=1
	fi
done
tmux kill-session -t "$S99" 2>/dev/null
echo "==== Phase 99: a band opened under a tall streaming preview keeps the composer ===="


# --- Phase 100: the built-in AGENT DEFINITIONS are seeded as real, editable
# files (docs/subagents.md). `general-purpose` and `explore` are `agents/*.md`
# under the user config home now, not a `match` in the binary — so the first
# launch has to WRITE them (with the comments that teach the `tools:`/`model:`
# keys), a later launch must never clobber one the user edited (seeding that
# overwrote would silently discard their own agent on every restart), and a
# deleted default must come back, since the `agent` schema's default type has
# to resolve. Boundary I/O, so it is checked here rather than in a unit test. ---
S100="${S}_agentseed"
AG_CFG="$(mktemp -d)"
# The seed lands in the config home's `agents/`; plant a file that will not
# parse there first, so the same launch has to seed AND report.
mkdir -p "$AG_CFG/agents"
printf 'just a body, no frontmatter at all\n' >"$AG_CFG/agents/broken.md"
APP_AG="env ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$AG_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_SKILLS_DIR=$SMOKE_SKILLS ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux kill-session -t "$S100" 2>/dev/null
tmux new-session -d -s "$S100" -x 100 -y 24 "$APP_AG"
sleep 1.2
# A definition that will not parse says so, and says why: silence here is what
# makes "the model says my agent type is unknown" and "I typo'd the
# frontmatter" read as two unrelated problems.
agent_toast="$(tmux capture-pane -t "$S100" -p)"
echo "==== Phase 100: the startup toast names the definition that would not parse ===="
printf '%s\n' "$agent_toast"
if ! printf '%s' "$agent_toast" | grep -qF "broken.md"; then
	echo "FAIL: Phase 100 — a definition that will not parse said nothing at startup" >&2
	status=1
fi
if ! printf '%s' "$agent_toast" | grep -qF "frontmatter"; then
	echo "FAIL: Phase 100 — the toast does not say WHY the definition was refused" >&2
	status=1
fi
tmux send-keys -t "$S100" -l "/quit"
sleep 0.2
tmux send-keys -t "$S100" Enter
sleep 0.6
echo "==== Phase 100: seeded agent definitions ===="
ls -1 "$AG_CFG/agents" 2>&1
for seeded in general-purpose explore; do
	if [ ! -f "$AG_CFG/agents/$seeded.md" ]; then
		echo "FAIL: Phase 100 — the first launch did not seed $seeded.md" >&2
		status=1
	fi
done
# The frontmatter the model reads, and the comments the *user* reads: a file
# that documents neither key is a file nobody can edit with confidence.
if ! grep -q "^name: general-purpose" "$AG_CFG/agents/general-purpose.md" 2>/dev/null; then
	echo "FAIL: Phase 100 — the seeded general-purpose.md has no name field" >&2
	status=1
fi
if ! grep -q "^# \`tools:\`" "$AG_CFG/agents/general-purpose.md" 2>/dev/null; then
	echo "FAIL: Phase 100 — the seeded default does not document the tools key" >&2
	status=1
fi
# explore is the read-only one: its allowlist leaves Write and Edit out.
if ! grep -q "^tools: Bash, Read, Skill, mcp__\*" "$AG_CFG/agents/explore.md" 2>/dev/null; then
	echo "FAIL: Phase 100 — explore.md is not the read-only allowlist" >&2
	status=1
fi

if ! grep -qF "no frontmatter at all" "$AG_CFG/agents/broken.md" 2>/dev/null; then
	echo "FAIL: Phase 100 — the seed overwrote a file that was already there" >&2
	status=1
fi
# Edit one, delete the other, relaunch: the edit survives and the deletion is
# repaired.
printf '%s\n' "---" "name: explore" "description: MINE-NOT-YOURS." "---" >"$AG_CFG/agents/explore.md"
rm -f "$AG_CFG/agents/general-purpose.md"
tmux kill-session -t "$S100" 2>/dev/null
tmux new-session -d -s "$S100" -x 100 -y 24 "$APP_AG"
sleep 1.2
tmux send-keys -t "$S100" -l "/quit"
sleep 0.2
tmux send-keys -t "$S100" Enter
sleep 0.6
tmux kill-session -t "$S100" 2>/dev/null
if ! grep -q "MINE-NOT-YOURS" "$AG_CFG/agents/explore.md" 2>/dev/null; then
	echo "FAIL: Phase 100 — the relaunch clobbered an edited agent definition" >&2
	status=1
fi
if [ ! -f "$AG_CFG/agents/general-purpose.md" ]; then
	echo "FAIL: Phase 100 — a deleted default was not re-seeded on the next launch" >&2
	status=1
fi
rm -rf "$AG_CFG"
echo "==== Phase 100: the built-in agent definitions seed once, survive an edit, come back if deleted, and a broken one is reported ===="


# --- Phase 101: TAB in an agent session view queues a follow-up turn for THAT
# AGENT (docs/queue.md) — the main session's Tab, one level down. Before this
# the Tab arm read the *lead's* `is_streaming()` and pushed onto the *lead's*
# queue, so a message typed into a subagent ran as a follow-up turn of the main
# conversation once the lead's turn ended: the reported "it sends the msg to
# main agent TUI". Open the demo subagent's session while it works, Tab two
# messages — both wait inset above the box, blank-divided — and each then runs
# as its OWN continuation turn on that agent's transcript, in order, once its
# loop settles. The main conversation must never see either. ---
S101="${S}_agenttab"
tmux new-session -d -s "$S101" -x 100 -y 34 "$APP"
sleep 0.7
tmux send-keys -t "$S101" -l "launch a subagent that streams a table"
sleep 0.3
tmux send-keys -t "$S101" Enter
for _ in $(seq 1 200); do # the background launch puts its row on the roster
	if tmux capture-pane -t "$S101" -p | grep -qF "Stream a comparison table"; then
		break
	fi
	sleep 0.1
done
# ↓ ↓ Enter opens the agent's session view (Phase 96's walk).
tmux send-keys -t "$S101" Down
sleep 0.15
tmux send-keys -t "$S101" Down
sleep 0.15
tmux send-keys -t "$S101" Enter
sleep 0.3
tmux send-keys -t "$S101" -l "first follow up"
sleep 0.2
tmux send-keys -t "$S101" Tab
sleep 0.2
tmux send-keys -t "$S101" -l "second follow up"
sleep 0.2
tmux send-keys -t "$S101" Tab
sleep 0.3
# Alt+Up there edits **that agent's** last follow-up, never the main session's
# backlog (which is what it used to reach, alongside Tab): the second row goes
# and its text comes back as the draft. Tab re-queues it for the run below.
tmux send-keys -t "$S101" M-Up
sleep 0.4
agenttab_recall="$(tmux capture-pane -t "$S101" -p)"
if ! printf '%s' "$agenttab_recall" | grep -qE '^❯ second follow up'; then
	echo "FAIL: Phase 101 — Alt+Up did not pull the agent's last follow-up back into the composer" >&2
	echo "---- the frame ----" >&2
	printf '%s\n' "$agenttab_recall" >&2
	status=1
fi
if printf '%s' "$agenttab_recall" | grep -qF "  ❯ second follow up"; then
	echo "FAIL: Phase 101 — Alt+Up left the pending row up; it must go with the pull-back" >&2
	status=1
fi
if ! printf '%s' "$agenttab_recall" | grep -qF "  ❯ first follow up"; then
	echo "FAIL: Phase 101 — Alt+Up took more than the last follow-up; the earlier one must stay queued" >&2
	status=1
fi
echo "==== Phase 101: Alt+Up edits the agent's own last follow-up ===="
tmux send-keys -t "$S101" Tab
agenttab_pending=""
for _ in $(seq 1 40); do # both wait inset above the box, neither committed
	frame="$(tmux capture-pane -t "$S101" -p)"
	if printf '%s' "$frame" | grep -qF "  ❯ first follow up" &&
		printf '%s' "$frame" | grep -qF "  ❯ second follow up"; then
		agenttab_pending="$frame"
		break
	fi
	sleep 0.1
done
if [ -z "$agenttab_pending" ]; then
	echo "FAIL: Phase 101 — Tab in an agent session view did not queue the drafts on that agent (no inset '  ❯ first follow up' / '  ❯ second follow up' rows)" >&2
	echo "---- the last frame seen ----" >&2
	tmux capture-pane -t "$S101" -p >&2
	status=1
else
	echo "==== Phase 101: both Tab follow-ups pending on the agent's own queue ===="
	printf '%s\n' "$agenttab_pending" | grep -F "❯ first follow up"
	printf '%s\n' "$agenttab_pending" | grep -F "❯ second follow up"
fi
# Its loop settles, then each follow-up runs as its own continuation turn: the
# first commits at column 0 on the AGENT's transcript while the second is still
# waiting inset behind it.
agenttab_first=""
for _ in $(seq 1 400); do
	frame="$(tmux capture-pane -t "$S101" -p -S -200)"
	if printf '%s' "$frame" | grep -qE '^❯ first follow up$'; then
		agenttab_first="$frame"
		break
	fi
	sleep 0.1
done
if [ -z "$agenttab_first" ]; then
	echo "FAIL: Phase 101 — the agent never ran its first Tab follow-up as a turn of its own" >&2
	echo "---- the last frame seen ----" >&2
	tmux capture-pane -t "$S101" -p -S -200 >&2
	status=1
else
	echo "==== Phase 101: the first follow-up ran on the agent's own transcript ===="
	printf '%s\n' "$agenttab_first" | grep -E '^❯ first follow up$'
	# It lands under the agent's launch prompt, not the main conversation's.
	if ! printf '%s' "$agenttab_first" | grep -qF "Compare four languages"; then
		echo "FAIL: Phase 101 — the follow-up is not on the agent's own transcript (its launch prompt is gone from the view)" >&2
		status=1
	fi
fi
agenttab_second=""
for _ in $(seq 1 400); do
	frame="$(tmux capture-pane -t "$S101" -p -S -200)"
	if printf '%s' "$frame" | grep -qE '^❯ second follow up$'; then
		agenttab_second="$frame"
		break
	fi
	sleep 0.1
done
if [ -z "$agenttab_second" ]; then
	echo "FAIL: Phase 101 — the second Tab follow-up never ran; the queue must drain one entry per settle" >&2
	status=1
else
	echo "==== Phase 101: the second follow-up ran after it, its own turn ===="
	printf '%s\n' "$agenttab_second" | grep -E '^❯ second follow up$'
fi
# Esc back to the main conversation: its purge-rebuild must show no trace of
# either — they belonged to the agent's conversation, and the main session's
# own queue was never touched.
tmux send-keys -t "$S101" Escape
sleep 0.8
agenttab_main="$(tmux capture-pane -t "$S101" -p -S -200)"
if printf '%s' "$agenttab_main" | grep -qF "follow up"; then
	echo "FAIL: Phase 101 — a message typed into the subagent leaked into the main conversation" >&2
	echo "---- the main view ----" >&2
	printf '%s\n' "$agenttab_main" >&2
	status=1
fi
tmux kill-session -t "$S101" 2>/dev/null
echo "==== Phase 101: Tab in a subagent's session queues that agent's own follow-up turns ===="


# --- Phase 102: the built-in SKILL is seeded as a real, editable directory
# (docs/skills.md). `skill-creator` is a `skills/<name>/SKILL.md` under the user
# config home, not a string in the binary, so the same three things are
# filesystem behaviour no unit test can see: the first launch has to WRITE it
# — the whole directory, its `reference.md` included, since the body sends the
# model there for the substitution tokens a body cannot spell out — a later
# launch must never clobber a copy the user edited, and a deleted file has to
# come back. The fourth is the difference from the agent definitions: an
# ALTER_ZERO_SKILLS_DIR override is NEVER seeded into (it says "these are the
# skills, and only these" — and this suite's own hermetic sessions depend on
# that dir staying exactly as empty as it was made). ---
S102="${S}_skillseed"
SK_CFG="$(mktemp -d)"
SK_HOME="$(mktemp -d)"
# No ALTER_ZERO_SKILLS_DIR here — the override is what this phase must see
# switched off. HOME is redirected instead so the walk's `~/.claude/skills` row
# finds nothing and the run stays hermetic on a machine that has real skills.
APP_SK="env ALTER_ZERO_PROJECT_CONFIG=0 HOME=$SK_HOME ALTER_ZERO_CONFIG_DIR=$SK_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_AGENTS_DIR=$SMOKE_AGENTS ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux kill-session -t "$S102" 2>/dev/null
tmux new-session -d -s "$S102" -x 100 -y 24 "$APP_SK"
sleep 1.2
# The seed runs before the walk, so the session that installed it can use it —
# a skill that appeared only on the *second* launch would be missing from
# exactly the session that just installed the app.
tmux send-keys -t "$S102" -l "/skills"
sleep 0.3
tmux send-keys -t "$S102" Enter
sleep 0.7
skillseed_menu="$(tmux capture-pane -t "$S102" -p)"
echo "==== Phase 102: /skills on the first launch ===="
printf '%s\n' "$skillseed_menu"
if ! printf '%s' "$skillseed_menu" | grep -qF "skill-creator"; then
	echo "FAIL: Phase 102 — the first launch's /skills does not list the built-in" >&2
	status=1
fi
tmux send-keys -t "$S102" Escape
sleep 0.3
tmux send-keys -t "$S102" -l "/quit"
sleep 0.2
tmux send-keys -t "$S102" Enter
sleep 0.6
tmux kill-session -t "$S102" 2>/dev/null
echo "==== Phase 102: seeded skill directory ===="
ls -1 "$SK_CFG/skills/skill-creator" 2>&1
for seeded in SKILL.md reference.md; do
	if [ ! -f "$SK_CFG/skills/skill-creator/$seeded" ]; then
		echo "FAIL: Phase 102 — the first launch did not seed skill-creator/$seeded" >&2
		status=1
	fi
done
if ! grep -q "^name: skill-creator" "$SK_CFG/skills/skill-creator/SKILL.md" 2>/dev/null; then
	echo "FAIL: Phase 102 — the seeded SKILL.md has no name field" >&2
	status=1
fi
# Edit one file, delete the other, relaunch: the edit survives and the deletion
# is repaired — a seed that overwrote would discard the user's own copy on
# every restart.
printf '%s\n' "---" "name: skill-creator" "description: MINE-NOT-YOURS." "---" >"$SK_CFG/skills/skill-creator/SKILL.md"
rm -f "$SK_CFG/skills/skill-creator/reference.md"
tmux new-session -d -s "$S102" -x 100 -y 24 "$APP_SK"
sleep 1.2
tmux send-keys -t "$S102" -l "/quit"
sleep 0.2
tmux send-keys -t "$S102" Enter
sleep 0.6
tmux kill-session -t "$S102" 2>/dev/null
if ! grep -q "MINE-NOT-YOURS" "$SK_CFG/skills/skill-creator/SKILL.md" 2>/dev/null; then
	echo "FAIL: Phase 102 — the relaunch clobbered an edited SKILL.md" >&2
	status=1
fi
if [ ! -f "$SK_CFG/skills/skill-creator/reference.md" ]; then
	echo "FAIL: Phase 102 — a deleted built-in file did not come back on the next launch" >&2
	status=1
fi
# And the override is left alone: an explicit skills root is the user's set,
# and every other phase in this suite runs against one that must stay empty.
SK_OVERRIDE="$(mktemp -d)"
SK_CFG2="$(mktemp -d)"
tmux new-session -d -s "$S102" -x 100 -y 24 "env ALTER_ZERO_PROJECT_CONFIG=0 HOME=$SK_HOME ALTER_ZERO_CONFIG_DIR=$SK_CFG2 ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_SKILLS_DIR=$SK_OVERRIDE ALTER_ZERO_AGENTS_DIR=$SMOKE_AGENTS ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
sleep 1.2
tmux send-keys -t "$S102" -l "/quit"
sleep 0.2
tmux send-keys -t "$S102" Enter
sleep 0.6
tmux kill-session -t "$S102" 2>/dev/null
if [ -n "$(ls -A "$SK_OVERRIDE" 2>/dev/null)" ]; then
	echo "FAIL: Phase 102 — the seed wrote into an ALTER_ZERO_SKILLS_DIR override" >&2
	ls -1 "$SK_OVERRIDE" >&2
	status=1
fi
if [ -e "$SK_CFG2/skills" ]; then
	echo "FAIL: Phase 102 — the override run seeded the config home behind its back" >&2
	status=1
fi
rm -rf "$SK_CFG" "$SK_CFG2" "$SK_HOME" "$SK_OVERRIDE"
echo "==== Phase 102: the built-in skill seeds once, survives an edit, comes back if deleted, and never touches an override root ===="


# --- Phase 103: the `/login` SIGN-IN FORK (docs/copilot.md). `/login` no
# longer opens on the provider list — it asks how you sign in first, because
# GitHub Copilot is a subscription rather than a key you paste. Walk the whole
# tree in one session: the method root, down into the subscription list and
# back, down into the API-key list and back, and out. The two lists must be
# disjoint — a subscription offered a key field, or Copilot missing from the
# subscriptions, is the bug this phase exists for — and Esc must step *back*
# from either list rather than closing the flow, with only the root's Esc
# closing it. The device page itself is not driven here (it would talk to
# github.com); Phase 33 already covers `/login` opening mid-turn. ---
S103="${S}_loginfork"
tmux new-session -d -s "$S103" -x 90 -y 30 "$APP"
sleep 0.6
tmux send-keys -t "$S103" -l "/login"
sleep 0.2
tmux send-keys -t "$S103" Enter
sleep 0.5
login_root="$(tmux capture-pane -t "$S103" -p)"
echo "==== Phase 103: /login opens on the method root ===="
printf '%s\n' "$login_root"
for want in "Use a subscription" "Use an API key" "escape/ctrl+c cancel"; do
	if ! printf '%s' "$login_root" | grep -qF "$want"; then
		echo "FAIL: Phase 103 — the /login root did not show \"$want\"" >&2
		status=1
	fi
done
if printf '%s' "$login_root" | grep -qF "Keys are saved to"; then
	echo "FAIL: Phase 103 — the root opened straight onto the provider list" >&2
	status=1
fi

# Enter on the highlighted first row opens the subscription list.
tmux send-keys -t "$S103" Enter
sleep 0.4
login_subs="$(tmux capture-pane -t "$S103" -p)"
echo "==== Phase 103: the subscription list ===="
printf '%s\n' "$login_subs"
# The rows are names and their ✓, not sentences: the one-line descriptions
# used to trail each name and were dropped so the three lists read as one
# shape. What the list must still show is every subscription, by name.
for want in "Anthropic Console" "GitHub Copilot" "OpenAI (ChatGPT)" "enter sign in"; do
	if ! printf '%s' "$login_subs" | grep -qF "$want"; then
		echo "FAIL: Phase 103 — the subscription list did not show \"$want\"" >&2
		status=1
	fi
done
if printf '%s' "$login_subs" | grep -qF "Sign in with your"; then
	echo "FAIL: Phase 103 — a row still trails its description" >&2
	status=1
fi
# Every row says whether it is reachable. The suite scrubs the key store, so
# each one must read as unconfigured — and say so, rather than leaving the
# reader to notice a missing mark.
if ! printf '%s' "$login_subs" | grep -qF "◯ unconfigured"; then
	echo "FAIL: Phase 103 — a subscription row does not report its configured status" >&2
	status=1
fi
# …and the list opens straight onto its filter: the heading that used to
# repeat the row that opened it is gone.
if printf '%s' "$login_subs" | grep -qF "Use a subscription"; then
	echo "FAIL: Phase 103 — the subscription list still carries a heading" >&2
	status=1
fi

# Esc steps BACK to the root, not out of the flow.
tmux send-keys -t "$S103" Escape
sleep 0.4
login_back="$(tmux capture-pane -t "$S103" -p)"
if ! printf '%s' "$login_back" | grep -qF "Use a subscription"; then
	echo "FAIL: Phase 103 — Esc on the subscription list left the flow instead of stepping back" >&2
	printf '%s\n' "$login_back" >&2
	status=1
fi

# Down + Enter takes the other branch: the API-key providers, and ONLY those.
tmux send-keys -t "$S103" Down
sleep 0.2
tmux send-keys -t "$S103" Enter
sleep 0.4
login_keys="$(tmux capture-pane -t "$S103" -p)"
echo "==== Phase 103: the API-key provider list ===="
printf '%s\n' "$login_keys"
for want in "Agent Zero API" "OpenRouter" "Keys are saved to" "esc back"; do
	if ! printf '%s' "$login_keys" | grep -qF "$want"; then
		echo "FAIL: Phase 103 — the API-key list did not show \"$want\"" >&2
		status=1
	fi
done
if printf '%s' "$login_keys" | grep -qF "GitHub Copilot"; then
	echo "FAIL: Phase 103 — a subscription provider was offered a key field" >&2
	status=1
fi
# Nor does a row trail its env var. The step's own hint names the file every
# key lands in, and the save toast names the variable — on the row it only
# pushed the names apart.
if printf '%s' "$login_keys" | grep -qF "_API_KEY]"; then
	echo "FAIL: Phase 103 — a provider row still trails its env var" >&2
	status=1
fi
if ! printf '%s' "$login_keys" | grep -qF "◯ unconfigured"; then
	echo "FAIL: Phase 103 — a provider row does not report its configured status" >&2
	status=1
fi
if printf '%s' "$login_keys" | grep -qF "Use an API key"; then
	echo "FAIL: Phase 103 — the provider list still carries a heading" >&2
	status=1
fi

# Enter on the highlighted provider opens the key step — which introduces the
# provider it is asking a secret for and links the page that secret is made
# on, read from `providers.toml` (docs/llm.md). A page that asks for a key and
# says nothing about where to get one is the thing this replaces.
tmux send-keys -t "$S103" Enter
sleep 0.4
login_key_step="$(tmux capture-pane -t "$S103" -p)"
echo "==== Phase 103: the key step introduces its provider ===="
printf '%s\n' "$login_key_step"
for want in "Enter your Agent Zero API key" "Venice.ai" "Create a key at" "agent-zero.ai"; do
	if ! printf '%s' "$login_key_step" | grep -qF "$want"; then
		echo "FAIL: Phase 103 — the key step did not show \"$want\"" >&2
		status=1
	fi
done
# The block wraps rather than clipping: the description's own last word has to
# survive, or the sentence stops mid-thought.
if ! printf '%s' "$login_key_step" | grep -qF "holders."; then
	echo "FAIL: Phase 103 — the key step clipped its description" >&2
	status=1
fi
# Esc steps back to the provider list, as it always did.
tmux send-keys -t "$S103" Escape
sleep 0.3
if ! tmux capture-pane -t "$S103" -p | grep -qF "Keys are saved to"; then
	echo "FAIL: Phase 103 — Esc on the key step did not return to the provider list" >&2
	status=1
fi

# Esc back to the root, then Esc again closes the flow and the composer returns.
tmux send-keys -t "$S103" Escape
sleep 0.3
tmux send-keys -t "$S103" Escape
sleep 0.4
login_closed="$(tmux capture-pane -t "$S103" -p)"
if printf '%s' "$login_closed" | grep -qF "Use a subscription"; then
	echo "FAIL: Phase 103 — Esc on the root did not close the flow" >&2
	printf '%s\n' "$login_closed" >&2
	status=1
fi
tmux kill-session -t "$S103" 2>/dev/null
echo "==== Phase 103: the /login sign-in fork walks both halves and closes at its root ===="

# --- Phase 104: the BROWSER sign-in page (docs/chatgpt.md). The second
# subscription's sign-in is not a device code — it is a link the user opens,
# and the browser redirects back to a loopback listener. That page is drivable
# offline (building the URL and binding 127.0.0.1:1455 touch no network at
# all), so unlike Copilot's device page it can be walked here. What this
# phase pins is that the page is worded for a LINK rather than for a code: the
# `Open …` verb, the authorize URL itself, the row saying the window continues
# by itself, and a hint offering `c copy link`. A page that silently fell back
# to the device wording would tell the user to type a one-time code that does
# not exist. ---
S104="${S}_chatgptlogin"
tmux new-session -d -s "$S104" -x 100 -y 30 "$APP"
sleep 0.6
tmux send-keys -t "$S104" -l "/login"
sleep 0.2
tmux send-keys -t "$S104" Enter
sleep 0.5
# Enter opens the subscription list, then the row is picked by NAME rather
# than by counting Downs: the list is alphabetical by provider id, so every
# subscription added ahead of `openai_chatgpt` used to shift this phase onto
# the wrong sign-in page — and the failure reads as "the browser page fell
# back to the device wording", which blames the code under test rather than
# the walk. `chatgpt` matches this row's id, name and description, and no
# other row's anything.
tmux send-keys -t "$S104" Enter
sleep 0.4
tmux send-keys -t "$S104" -l "chatgpt"
sleep 0.3
tmux send-keys -t "$S104" Enter
sleep 1.2
chatgpt_page="$(tmux capture-pane -t "$S104" -p)"
echo "==== Phase 104: the ChatGPT browser sign-in page ===="
printf '%s\n' "$chatgpt_page"
for want in "Sign in to OpenAI (ChatGPT)" "this window continues by itself" \
	"c copy link" "Waiting for the browser"; do
	if ! printf '%s' "$chatgpt_page" | grep -qF "$want"; then
		echo "FAIL: Phase 104 — the browser sign-in page did not show \"$want\"" >&2
		status=1
	fi
done
# The URL is one unbreakable word, so it wraps across rows at any width — the
# parameter assertions read the pane with the wrapping squeezed out (a URL
# contains no spaces, so nothing real is lost). These are the parameters
# OpenAI refuses the flow without, plus the allow-listed redirect and the
# `offline_access` scope that is what earns a refresh token at all.
chatgpt_url="$(printf '%s' "$chatgpt_page" | tr -d ' \n')"
for want in "https://auth.openai.com/oauth/authorize?response_type=code" \
	"client_id=app_EMoamEEZ73f0CkXaXp7hrann" \
	"redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback" \
	"offline_access" "code_challenge_method=S256" "originator=codex_cli_rs"; do
	if ! printf '%s' "$chatgpt_url" | grep -qF "$want"; then
		echo "FAIL: Phase 104 — the authorize URL is missing \"$want\"" >&2
		status=1
	fi
done
# The URL opens its own row: it is a clickable OSC 8 hyperlink
# (docs/links.md), and a verb in front of one only pushes the target off the
# start of the row it should begin.
if ! printf '%s' "$chatgpt_page" | grep -qE '^ *https://auth\.openai\.com/oauth/authorize'; then
	echo "FAIL: Phase 104 — the authorize URL does not start its own row" >&2
	status=1
fi
# And it must NOT wear the device page's clothes, or a verb the link replaced.
for unwanted in "enter this one-time code" "c copy code" "Open https://"; do
	if printf '%s' "$chatgpt_page" | grep -qF "$unwanted"; then
		echo "FAIL: Phase 104 — the browser page fell back to the device-code wording (\"$unwanted\")" >&2
		status=1
	fi
done
# Esc cancels the sign-in back to the subscription list, releasing the port.
tmux send-keys -t "$S104" Escape
sleep 0.5
chatgpt_back="$(tmux capture-pane -t "$S104" -p)"
if ! printf '%s' "$chatgpt_back" | grep -qF "OpenAI (ChatGPT)"; then
	echo "FAIL: Phase 104 — Esc on the sign-in page did not return to the subscription list" >&2
	printf '%s\n' "$chatgpt_back" >&2
	status=1
fi
if printf '%s' "$chatgpt_back" | grep -qF "Waiting for the browser"; then
	echo "FAIL: Phase 104 — Esc left the sign-in page up" >&2
	status=1
fi
tmux kill-session -t "$S104" 2>/dev/null
echo "==== Phase 104: the browser sign-in page is worded for a link, not a code ===="


# --- Phase 105: the ↓ MANAGER BAND FLOWS ITS PAGE TOP (docs/view-flow.md,
# docs/background.md). On a terminal shorter than the details page, the band
# bottom-anchors — and the rows the anchor skips used to be dropped into NO
# buffer at all: the conversation ran straight into a headless output box, and
# scrolling the terminal up never found the `Shell details` title, the status
# or the command (the reported bug). They flow into real scrollback now, like
# every other framed view — while the page itself keeps live-tailing, because
# this one flow is signed on the SHELL rather than on its ticking rows
# (`FlowSign::Frozen`), so the frozen top costs no purge rebuild per frame.
# Closing the band purges the flowed rows and leaves the conversation intact. ---
S105="${S}_bgflow"
BGF_SCRIPT="$SMOKE_CFG/bgflow.sh"
cat >"$BGF_SCRIPT" <<'EOS'
i=0
while [ $i -lt 900 ]; do
	echo flowline$i
	i=$((i + 1))
	sleep 0.05
done
EOS
# 18 rows: shorter than the details page (26 rows at this width), so the top
# rule, the title and the whole field block are what the anchor skips.
tmux new-session -d -s "$S105" -x 100 -y 18 "$APP"
sleep 0.5
# A committed cell above the band, so the close can be checked to leave the
# conversation — the thing the user scrolls up for — untouched.
tmux send-keys -t "$S105" -l "!echo FLOWCONV_MARKER_105"
sleep 0.2
tmux send-keys -t "$S105" Enter
sleep 0.6
tmux send-keys -t "$S105" -l "!sh $BGF_SCRIPT"
sleep 0.2
tmux send-keys -t "$S105" Enter
# Wait past TOOL_BACKGROUND_HINT_DELAY, then hand the run to the registry.
for _ in $(seq 1 80); do
	if tmux capture-pane -t "$S105" -p | grep -qF "(ctrl+b to run in background)"; then
		break
	fi
	sleep 0.1
done
tmux send-keys -t "$S105" C-b
sleep 0.5
tmux send-keys -t "$S105" Down # focus the footer's shell indicator
sleep 0.3
tmux send-keys -t "$S105" Enter # open the list
sleep 0.4
tmux send-keys -t "$S105" Enter # open the details page
bgf_pane=""
for _ in $(seq 1 40); do
	bgf_pane="$(tmux capture-pane -t "$S105" -p)"
	if printf '%s' "$bgf_pane" | grep -qF "flowline"; then
		break
	fi
	sleep 0.1
done
bgf_full="$(tmux capture-pane -t "$S105" -p -S -200)"
echo "==== Phase 105: the screen-tall details page (visible pane) ===="
printf '%s\n' "$bgf_pane"
# The visible screen keeps the page's TAIL — the hints and the closing rule.
if ! printf '%s' "$bgf_pane" | grep -qF "to go back"; then
	echo "FAIL: Phase 105 — the details tail (← to go back …) is not on screen" >&2
	status=1
fi
if ! printf '%s\n' "$bgf_pane" | awk 'END { exit ($0 ~ /──/) ? 0 : 1 }'; then
	echo "FAIL: Phase 105 — the bottom rule is not the last screen row" >&2
	status=1
fi
# …and the page genuinely overflowed: its top is NOT on the visible screen…
if printf '%s' "$bgf_pane" | grep -qF "Shell details"; then
	echo "FAIL: Phase 105 — the page fits the pane; the fixture must overflow for this phase to test the flow" >&2
	status=1
fi
# …but IS in the terminal's real scrollback, whole — the bug this phase guards.
for expect in "Shell details" "Status:" "Runtime:" "Command:"; do
	if ! printf '%s' "$bgf_full" | grep -qF "$expect"; then
		echo "FAIL: Phase 105 — the flowed page top is missing '$expect' from scrollback+screen" >&2
		printf '%s\n' "$bgf_full" >&2
		status=1
	fi
done
# The flow FREEZES: the page keeps tailing live (the box advances) while the
# flowed rows stay put — committed once, never re-flowed per tick, and never
# purge-rebuilt out from under the conversation above them.
bgf_seq_before="$(printf '%s\n' "$bgf_pane" | grep -oE 'flowline[0-9]+' | tail -1)"
bgf_seq_after="$bgf_seq_before"
for _ in $(seq 1 40); do
	bgf_later="$(tmux capture-pane -t "$S105" -p)"
	bgf_seq_after="$(printf '%s\n' "$bgf_later" | grep -oE 'flowline[0-9]+' | tail -1)"
	if [ -n "$bgf_seq_after" ] && [ "$bgf_seq_after" != "$bgf_seq_before" ]; then
		break
	fi
	sleep 0.2
done
if [ "$bgf_seq_after" = "$bgf_seq_before" ]; then
	echo "FAIL: Phase 105 — the details box stopped tailing under the flow ($bgf_seq_before)" >&2
	status=1
fi
bgf_held="$(tmux capture-pane -t "$S105" -p -S -200)"
echo "==== Phase 105: the flowed top after the page has ticked on ===="
printf '%s\n' "$bgf_held" | grep -n "Shell details" || true
bgf_titles="$(printf '%s\n' "$bgf_held" | grep -cF "Shell details" || true)"
if [ "$bgf_titles" != "1" ]; then
	echo "FAIL: Phase 105 — the flowed title is in scrollback $bgf_titles times, expected exactly 1" >&2
	printf '%s\n' "$bgf_held" >&2
	status=1
fi
if ! printf '%s' "$bgf_held" | grep -qF "FLOWCONV_MARKER_105"; then
	echo "FAIL: Phase 105 — the conversation above the flowed page was lost from scrollback" >&2
	status=1
fi
# Esc closes the band: the flowed rows purge, the conversation stays.
tmux send-keys -t "$S105" Escape
sleep 0.8
bgf_closed="$(tmux capture-pane -t "$S105" -p -S -200)"
echo "==== Phase 105: closed back to the composer ===="
printf '%s\n' "$bgf_closed" | tail -14
if printf '%s' "$bgf_closed" | grep -qF "Shell details"; then
	echo "FAIL: Phase 105 — stale flowed rows survived the close (the purge should have wiped them)" >&2
	status=1
fi
if ! printf '%s' "$bgf_closed" | grep -qF "FLOWCONV_MARKER_105"; then
	echo "FAIL: Phase 105 — the conversation did not survive the flow-exit rebuild" >&2
	status=1
fi
if ! printf '%s' "$bgf_closed" | grep -qF "Running in the background"; then
	echo "FAIL: Phase 105 — the backgrounded cell did not survive the flow-exit rebuild" >&2
	status=1
fi
tmux kill-session -t "$S105" 2>/dev/null
rm -f "$BGF_SCRIPT"



# --- Phase 106: the STRIP FLOW (docs/strip-flow.md). The streaming strip is
# the live region's only elastic content, and the rows it could not afford
# were thrown away: on a short terminal a running command lost its
# `+N lines (Ns)` footer and its ctrl+b hint first, then its output rows, then
# its header, and past that the spinner status line — none of it in any
# buffer. The strip bottom-anchors now and the rows it cannot paint FLOW into
# the terminal's real scrollback, frozen: the strip keeps its newest rows and
# its head stays readable by scrolling up. The turn's end purges the frozen
# rows and commits the real cells exactly once. ---
S106="${S}_stripflow"
# 12 rows: room for the composer, the footer and the status line, but not for
# a parallel batch's three cells — the regime the bug report was taken in.
tmux new-session -d -s "$S106" -x 80 -y 12 "$APP"
sleep 0.6
tmux send-keys -t "$S106" -l "run three pings in parallel"
sleep 0.2
tmux send-keys -t "$S106" Enter
sf_pane=""
sf_full=""
for _ in $(seq 1 400); do # the batch is announced a couple of seconds in
	sf_pane="$(tmux capture-pane -t "$S106" -p)"
	if printf '%s' "$sf_pane" | grep -qF "esc to interrupt" \
		&& printf '%s' "$sf_pane" | grep -qF "Waiting…"; then
		sf_full="$(tmux capture-pane -t "$S106" -p -S -80)"
		break
	fi
	sleep 0.05
done
echo "==== Phase 106: the squeezed strip mid-batch (visible pane) ===="
printf '%s\n' "$sf_pane"
# The anchor keeps the strip's TAIL — the last queued sibling and, below it,
# the spinner status line, which a starved strip used to clip away.
for expect in "Bash(ping -c 20 x.invalid)" "esc to interrupt"; do
	if ! printf '%s' "$sf_pane" | grep -qF "$expect"; then
		echo "FAIL: Phase 106 — '$expect' is not on the squeezed screen" >&2
		status=1
	fi
done
# …and the strip genuinely overflowed: the running call at its head is NOT on
# the visible screen…
if printf '%s' "$sf_pane" | grep -qF "Bash(ping -c 20 google.com)"; then
	echo "FAIL: Phase 106 — the strip fits the pane; the fixture must overflow for this phase to test the flow" >&2
	status=1
fi
# …but IS in the terminal's real scrollback, running row and all, with the
# conversation still above it. Exactly once: a flow re-committed per frame
# would be a purge rebuild at 30fps.
echo "==== Phase 106: the flowed strip head in scrollback ===="
printf '%s\n' "$sf_full" | tail -18
for expect in "Bash(ping -c 20 google.com)" "run three pings in parallel"; do
	if ! printf '%s' "$sf_full" | grep -qF "$expect"; then
		echo "FAIL: Phase 106 — the flowed strip head is missing '$expect' from scrollback+screen" >&2
		printf '%s\n' "$sf_full" >&2
		status=1
	fi
done
# Its running row too — matched as a whole cell row (`⎿  Running…` alone),
# never as the substring the demo's own narration also contains.
if ! printf '%s\n' "$sf_full" | grep -qE '^[[:space:]]*⎿[[:space:]]+Running…[[:space:]]*$'; then
	echo "FAIL: Phase 106 — the flowed head lost the running call's '⎿ Running…' row" >&2
	printf '%s\n' "$sf_full" >&2
	status=1
fi
sf_heads="$(printf '%s\n' "$sf_full" | grep -cF "Bash(ping -c 20 google.com)" || true)"
if [ "$sf_heads" != "1" ]; then
	echo "FAIL: Phase 106 — the flowed head is in scrollback $sf_heads times, expected exactly 1" >&2
	status=1
fi
# The turn ends: the flow clears, the frozen rows are purged, and the three
# cells commit — once each — with nothing of the strip left behind.
sf_done=""
for _ in $(seq 1 300); do
	sf_done="$(tmux capture-pane -t "$S106" -p -S -120)"
	if printf '%s' "$sf_done" | grep -qE 'Done for [0-9]+s'; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 106: the turn resolved, the frozen rows purged ===="
printf '%s\n' "$sf_done" | tail -16
if printf '%s' "$sf_done" | grep -qF "esc to interrupt"; then
	echo "FAIL: Phase 106 — the frozen status line survived the turn (the purge should have wiped the strip)" >&2
	status=1
fi
# The live cell rows, as whole rows: this demo's closing narration *quotes*
# `⎿ Waiting…` mid-sentence, so a substring match would fail on the prose.
for stale in "Waiting…" "Running…"; do
	if printf '%s\n' "$sf_done" | grep -qE "^[[:space:]]*⎿[[:space:]]+${stale}[[:space:]]*\$"; then
		echo "FAIL: Phase 106 — a frozen '⎿ $stale' row survived the turn (the purge should have wiped the strip)" >&2
		status=1
	fi
done
for cell in "Bash(ping -c 20 google.com)" "Bash(ping -c 20 facebook.com)" "Bash(ping -c 20 x.invalid)"; do
	sf_n="$(printf '%s\n' "$sf_done" | grep -cF "$cell" || true)"
	if [ "$sf_n" != "1" ]; then
		echo "FAIL: Phase 106 — '$cell' committed $sf_n times, expected exactly 1" >&2
		status=1
	fi
done
if ! printf '%s' "$sf_done" | grep -qF "run three pings in parallel"; then
	echo "FAIL: Phase 106 — the conversation did not survive the flow-exit rebuild" >&2
	status=1
fi
tmux kill-session -t "$S106" 2>/dev/null

# Phase 106b: the STATUS LINE itself. Past the preview slot `live_layout`
# starves the strip, and the spinner row was simply not painted — the elapsed
# and the token tally gone with it. It freezes into scrollback now
# ("freeze the status indicator if it's not visible in the active window").
# A wedged backend (ALTER_ZERO_STALL_MS) holds the turn open so the check is
# not a race.
S106B="${S}_stripstatus"
APP_STALL="env $CFG_ENV_NOHIST ALTER_ZERO_STALL_MS=60000 $BIN"
tmux new-session -d -s "$S106B" -x 80 -y 4 "$APP_STALL"
sleep 0.8
tmux send-keys -t "$S106B" -l "stall please"
sleep 0.2
tmux send-keys -t "$S106B" Enter
sleep 2
sf_st_pane="$(tmux capture-pane -t "$S106B" -p)"
sf_st_full="$(tmux capture-pane -t "$S106B" -p -S -40)"
echo "==== Phase 106b: a 4-row terminal — the status line is off-screen ===="
printf '%s\n' "$sf_st_pane"
if printf '%s' "$sf_st_pane" | grep -qF "esc to interrupt"; then
	echo "FAIL: Phase 106b — the region fits the status line; the fixture must starve the strip" >&2
	status=1
fi
echo "==== Phase 106b: …but frozen in scrollback ===="
printf '%s\n' "$sf_st_full"
if ! printf '%s' "$sf_st_full" | grep -qF "esc to interrupt"; then
	echo "FAIL: Phase 106b — the starved status line vanished instead of freezing into scrollback" >&2
	status=1
fi
tmux kill-session -t "$S106B" 2>/dev/null



# --- Phase 107: INLINE IMAGES (docs/images.md). A handcrafted rollout carries
# an image `read` of a real PNG in the cwd; resuming it must draw the picture
# in the terminal — flush at the left margin, one blank row under the cell —
# and Ctrl+O must show the same picture in the transcript. The protocol is
# pinned to half-blocks (real coloured cells, so `capture-pane` can see them)
# and the cell size to 5x10, so the footprint is deterministic: a 100x60 PNG
# is exactly 20 columns by 6 rows. Then `/settings` **Show images** is cycled
# off, which must purge-rebuild the conversation WITHOUT the picture. ---
S107="${S}_images"
IMG_DIR="$(mktemp -d /tmp/alter-zero-smoke-images-XXXXXX)"
IMG_SESS="$(mktemp -d /tmp/alter-zero-smoke-imgsess-XXXXXX)"
img_day="$IMG_SESS/2026/07/23"
mkdir -p "$img_day"
base64 -d >"$IMG_DIR/shot.png" <<'PNG64'
iVBORw0KGgoAAAANSUhEUgAAAGQAAAA8CAIAAAAfXYiZAAADYUlEQVR4Ae3AA6AkWZbG8f937o3I
zKdyS2Oubdu2bdu2bdu2bWmMnpZKr54yMyLu+Xa3anqmhztr1a/aLvqjQguy0IIstCALLchCK7Qg
Cy3IQguy0IIstEILstCCLLQgCy3IQguy0AotyEILstCCLLQgC63Qgiy0IAstyEILstCCLLRCC7LQ
giy0IAstyEIrtCALLchCC7LQgiy0IAut0IIstCALLchCC7KQBQIKBBQIKBBQIKBAgYACAQUCCgQU
qOr+JCpX/QsAqFSu+pcBUKk80P7+6x3f/lWuei4AVDoeaPvkb5SOq54bAJXKVf8yACqVF+Kuu976
5ht+gqsAqFReiBtu+elSuQoAKh1X/csAqFRedE984ns85lHfw/9DAFQqL7pHvdj3ReX/IwAqlav+
ZQBUOv7N/vzPP/QVX/7r+f8AgErl3+zlX/kbo/L/AgCVylX/MgAqlf8ov/3bn/h6r/1F/J8EQKXj
P8prv8GXlo7/mwCoVK76lwFQqfwn+bmf+7y3fotP5f8GACqV/yRv8TafUSr/RwBQ6bjqXwZApfJf
4wd/8Kvf/V0/kv+lAKhU/mu863t+dFT+twKgUrnqXwZApeO/xbd8y3d8yAe9D/9bAFCp/Lf4oA97
v6j8rwFApXLVvwyASuV/gq/4ih/5xI97e/7HAqDS8T/Bx33yO5WO/7kAqFSu+pcBUKn8D/TZn/2L
n/fZb8T/HABUKv8Dffbnv2mp/A8CQKXjqn8ZAJXK/3wf93G/91Vf8Wr8NwKgUvmf7yu+5jWi8t8J
gErlqn8ZAJWO/3U+8AP/+tu/9SX5rwRApfK/zrd+50tH5b8UAJXKVf8yACqV/+3e9V2f8iM/+BD+
UwFQ6fjf7gd/7OGl4z8XAJXKVf8yACqV/2Pe4i3u/cWfO81/LAAqlf9jfu6Xri2V/2AAVDqu+pcB
UKn83/bar330u789598JgErl/7bf/v2NqPx7AVCpXPUvA6DS8f/Ky72c/+ovxL8WAJXK/yt/8TeK
yr8aAJXKVf8yACqV/88e9Sg/5YniXwRApeP/syc+TaXjXwZApXLVvwyASuWqZ7nhBt97l3heAFQq
Vz3LXfepVJ4PACodV/3LAKhUrnpBtrd9uC8AACqVq16Q/aWiAgBApXLVvwyAfwRqc6dvjMnyQgAA
AABJRU5ErkJggg==
PNG64
img_file="$img_day/rollout-2026-07-23T10-00-00-10710710.jsonl"
{
	printf '{"timestamp":"2026-07-23T10:00:00.000Z","type":"session_meta","payload":{"id":"smoke-107","timestamp":"2026-07-23T10:00:00.000Z","cwd":"%s","model":"dummy_model_name","originator":"alter-zero","version":"0.1.0"}}\n' "$IMG_DIR"
	printf '{"timestamp":"2026-07-23T10:00:01.000Z","type":"message","payload":{"role":"user","text":"look at shot.png","timestamp":"10:00 AM"}}\n'
	printf '{"timestamp":"2026-07-23T10:00:02.000Z","type":"tool","payload":{"name":"Read","args":"%s/shot.png","ok":true,"output":"Read image (PNG, 100x60, 2 KB)","timestamp":"10:00 AM","shell":false,"truncated":false,"arguments":"{\\"path\\":\\"%s/shot.png\\"}"}}\n' "$IMG_DIR" "$IMG_DIR"
	printf '{"timestamp":"2026-07-23T10:00:03.000Z","type":"message","payload":{"role":"assistant","text":"A colour gradient with a white diagonal.","timestamp":"10:00 AM"}}\n'
} >"$img_file"
IMG_ENV="$CFG_ENV_NOHIST ALTER_ZERO_SESSIONS_DIR=$IMG_SESS ALTER_ZERO_IMAGE_PROTOCOL=halfblocks ALTER_ZERO_IMAGE_CELL_SIZE=5x10 ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS"
tmux new-session -d -s "$S107" -x 100 -y 30 -c "$IMG_DIR" "env $IMG_ENV $BIN_ABS"
sleep 0.6
tmux send-keys -t "$S107" -l "/resume"
sleep 0.3
tmux send-keys -t "$S107" Enter
img_listed=0
for _ in $(seq 1 40); do
	if tmux capture-pane -t "$S107" -p | grep -qF "look at shot.png"; then
		img_listed=1
		break
	fi
	sleep 0.1
done
tmux send-keys -t "$S107" Enter # load it
img_loaded=0
for _ in $(seq 1 60); do
	if tmux capture-pane -t "$S107" -p -S -60 | grep -qF "Read image (PNG, 100x60"; then
		img_loaded=1
		break
	fi
	sleep 0.1
done
sleep 0.6
img_pane="$(tmux capture-pane -t "$S107" -p -S -60)"
# The picture's rows are the non-blank ones between the `Read image` cell and
# the reply that follows it — counted structurally rather than by matching the
# block glyphs, which `grep`'s byte-oriented classes can't size in the C
# locale. A 100x60 PNG at a 5x10 cell is exactly 6 rows.
img_between() {
	printf '%s\n' "$1" | awk '
		/Read image \(PNG, 100x60/ { seen = 1; next }
		seen && /A colour gradient/ { exit }
		seen { print }
	'
}
img_rows="$(img_between "$img_pane" | grep -cve '^[[:space:]]*$')"
# …and they start at column 0: not indented into the cell's `⎿` gutter.
img_flush=0
if [ -n "$(img_between "$img_pane" | grep -ve '^[[:space:]]*$' | grep -ve '^[[:space:]]')" ]; then
	img_flush=1
fi
# …directly under the cell, one blank row between (the picture is not indented
# into the `⎿` gutter, and it is not glued to it either).
img_gap=0
if printf '%s\n' "$img_pane" | grep -A 2 -F "Read image (PNG, 100x60" | sed -n '2p' | grep -qE '^[[:space:]]*$'; then
	img_gap=1
fi
echo "==== Phase 107: inline image — listed=$img_listed loaded=$img_loaded rows=$img_rows flush=$img_flush gap=$img_gap ===="
printf '%s\n' "$img_pane" | grep -v '^$' | tail -12
# Ctrl+O shows the same picture in the full-screen transcript.
tmux send-keys -t "$S107" C-o
img_overlay=0
for _ in $(seq 1 40); do
	if [ "$(img_between "$(tmux capture-pane -t "$S107" -p)" | grep -cve '^[[:space:]]*$')" -ge 6 ]; then
		img_overlay=1
		break
	fi
	sleep 0.1
done
tmux send-keys -t "$S107" C-o
sleep 0.5
# /settings → Show images → off: the rebuild must drop the picture.
tmux send-keys -t "$S107" -l "/settings"
sleep 0.3
tmux send-keys -t "$S107" Enter
sleep 0.5
tmux send-keys -t "$S107" Down
sleep 0.2
img_row_seen=0
if tmux capture-pane -t "$S107" -p | grep -qE 'Show images +true'; then
	img_row_seen=1
fi
tmux send-keys -t "$S107" Enter
sleep 1.0
tmux send-keys -t "$S107" Escape
sleep 0.8
img_after="$(tmux capture-pane -t "$S107" -p -S -60)"
img_gone=1
if [ "$(img_between "$img_after" | grep -cve '^[[:space:]]*$')" -ne 0 ]; then
	img_gone=0
fi
img_cell_kept=0
if printf '%s' "$img_after" | grep -qF "Read image (PNG, 100x60"; then
	img_cell_kept=1
fi
echo "==== Phase 107: overlay=$img_overlay row_seen=$img_row_seen gone_after_toggle=$img_gone cell_kept=$img_cell_kept ===="
# …and back ON: the rebuild must redraw the picture — and the setting must
# not stay off, because it persists to settings.json in the config home the
# whole suite shares, where it would blank every picture the phases after
# this one (107b, 107c) expect to see.
tmux send-keys -t "$S107" -l "/settings"
sleep 0.3
tmux send-keys -t "$S107" Enter
sleep 0.5
tmux send-keys -t "$S107" Down
sleep 0.2
img_row_off=0
if tmux capture-pane -t "$S107" -p | grep -qE 'Show images +false'; then
	img_row_off=1
fi
tmux send-keys -t "$S107" Enter
sleep 1.0
tmux send-keys -t "$S107" Escape
sleep 0.8
img_back_rows="$(img_between "$(tmux capture-pane -t "$S107" -p -S -60)" | grep -cve '^[[:space:]]*$')"
echo "==== Phase 107: Show images back on — row_off_seen=$img_row_off rows=$img_back_rows ===="
tmux kill-session -t "$S107" 2>/dev/null
rm -rf "$IMG_DIR" "$IMG_SESS"
if [ "$img_listed" != 1 ] || [ "$img_loaded" != 1 ]; then
	echo "FAIL: Phase 107 precondition — the seeded image session did not list/load (listed=$img_listed loaded=$img_loaded)" >&2
	status=1
fi
if [ "$img_rows" -ne 6 ]; then
	echo "FAIL: Phase 107 — expected 6 picture rows for a 100x60 image at a 5x10 cell, found $img_rows" >&2
	status=1
fi
if [ "$img_flush" != 1 ]; then
	echo "FAIL: Phase 107 — the picture is indented; it must start at column 0" >&2
	status=1
fi
if [ "$img_gap" != 1 ]; then
	echo "FAIL: Phase 107 — no blank row between the tool cell and the picture" >&2
	status=1
fi
if [ "$img_overlay" != 1 ]; then
	echo "FAIL: Phase 107 — the Ctrl+O transcript did not draw the picture" >&2
	status=1
fi
if [ "$img_row_seen" != 1 ]; then
	echo "FAIL: Phase 107 — /settings has no live `Show images` row" >&2
	status=1
fi
if [ "$img_gone" != 1 ]; then
	echo "FAIL: Phase 107 — turning Show images off left the picture on screen" >&2
	status=1
fi
if [ "$img_cell_kept" != 1 ]; then
	echo "FAIL: Phase 107 — the rebuild lost the tool cell along with the picture" >&2
	status=1
fi
if [ "$img_row_off" != 1 ]; then
	echo "FAIL: Phase 107 — /settings did not show Show images as false after the toggle" >&2
	status=1
fi
if [ "$img_back_rows" -ne 6 ]; then
	echo "FAIL: Phase 107 — turning Show images back on did not redraw the picture (rows=$img_back_rows)" >&2
	status=1
fi

# --- Phase 107b: the Ctrl+O overlay must TRANSMIT the picture, not just place
# it (docs/images.md). A graphics placement belongs to the screen it was made
# on, and the kitty protocol transmits its image — creating that placement —
# exactly once per encoded protocol object. Carrying one across the switch to
# the alternate screen therefore painted the transcript with unicode
# placeholders pointing at a placement that only existed on the primary
# screen: reserved rows, and nothing in them. `capture-pane` cannot see that
# (the placeholders ARE ordinary cells), so this phase reads the raw byte
# stream with `pipe-pane` and asserts a kitty transmit — `ESC _ G … a=T` —
# lands on *each* side of the switch. ---
S107B="${S}_imagetransmit"
IMG_DIR2="$(mktemp -d /tmp/alter-zero-smoke-imagetx-XXXXXX)"
IMG_SESS2="$(mktemp -d /tmp/alter-zero-smoke-imgtxsess-XXXXXX)"
IMG_RAW="$(mktemp -d /tmp/alter-zero-smoke-imgraw-XXXXXX)/raw.bin"
mkdir -p "$IMG_SESS2/2026/07/23"
base64 -d >"$IMG_DIR2/shot.png" <<'PNG64'
iVBORw0KGgoAAAANSUhEUgAAAGQAAAA8CAIAAAAfXYiZAAADYUlEQVR4Ae3AA6AkWZbG8f937o3I
zKdyS2Oubdu2bdu2bdu2bWmMnpZKr54yMyLu+Xa3anqmhztr1a/aLvqjQguy0IIstCALLchCK7Qg
Cy3IQguy0IIstEILstCCLLQgCy3IQguy0AotyEILstCCLLQgC63Qgiy0IAstyEILstCCLLRCC7LQ
giy0IAstyEIrtCALLchCC7LQgiy0IAut0IIstCALLchCC7KQBQIKBBQIKBBQIKBAgYACAQUCCgQU
qOr+JCpX/QsAqFSu+pcBUKk80P7+6x3f/lWuei4AVDoeaPvkb5SOq54bAJXKVf8yACqVF+Kuu976
5ht+gqsAqFReiBtu+elSuQoAKh1X/csAqFRedE984ns85lHfw/9DAFQqL7pHvdj3ReX/IwAqlav+
ZQBUOv7N/vzPP/QVX/7r+f8AgErl3+zlX/kbo/L/AgCVylX/MgAqlf8ov/3bn/h6r/1F/J8EQKXj
P8prv8GXlo7/mwCoVK76lwFQqfwn+bmf+7y3fotP5f8GACqV/yRv8TafUSr/RwBQ6bjqXwZApfJf
4wd/8Kvf/V0/kv+lAKhU/mu863t+dFT+twKgUrnqXwZApeO/xbd8y3d8yAe9D/9bAFCp/Lf4oA97
v6j8rwFApXLVvwyASuV/gq/4ih/5xI97e/7HAqDS8T/Bx33yO5WO/7kAqFSu+pcBUKn8D/TZn/2L
n/fZb8T/HABUKv8Dffbnv2mp/A8CQKXjqn8ZAJXK/3wf93G/91Vf8Wr8NwKgUvmf7yu+5jWi8t8J
gErlqn8ZAJWO/3U+8AP/+tu/9SX5rwRApfK/zrd+50tH5b8UAJXKVf8yACqV/+3e9V2f8iM/+BD+
UwFQ6fjf7gd/7OGl4z8XAJXKVf8yACqV/2Pe4i3u/cWfO81/LAAqlf9jfu6Xri2V/2AAVDqu+pcB
UKn83/bar330u789598JgErl/7bf/v2NqPx7AVCpXPUvA6DS8f/Ky72c/+ovxL8WAJXK/yt/8TeK
yr8aAJXKVf8yACqV/88e9Sg/5YniXwRApeP/syc+TaXjXwZApXLVvwyASuWqZ7nhBt97l3heAFQq
Vz3LXfepVJ4PACodV/3LAKhUrnpBtrd9uC8AACqVq16Q/aWiAgBApXLVvwyAfwRqc6dvjMnyQgAA
AABJRU5ErkJggg==
PNG64
img_file2="$IMG_SESS2/2026/07/23/rollout-2026-07-23T10-00-00-10710711.jsonl"
{
	printf '{"timestamp":"2026-07-23T10:00:00.000Z","type":"session_meta","payload":{"id":"smoke-107b","timestamp":"2026-07-23T10:00:00.000Z","cwd":"%s","model":"dummy_model_name","originator":"alter-zero","version":"0.1.0"}}\n' "$IMG_DIR2"
	printf '{"timestamp":"2026-07-23T10:00:01.000Z","type":"message","payload":{"role":"user","text":"look at shot.png","timestamp":"10:00 AM"}}\n'
	printf '{"timestamp":"2026-07-23T10:00:02.000Z","type":"tool","payload":{"name":"Read","args":"%s/shot.png","ok":true,"output":"Read image (PNG, 100x60, 2 KB)","timestamp":"10:00 AM","shell":false,"truncated":false,"arguments":"{\\"path\\":\\"%s/shot.png\\"}"}}\n' "$IMG_DIR2" "$IMG_DIR2"
	printf '{"timestamp":"2026-07-23T10:00:03.000Z","type":"message","payload":{"role":"assistant","text":"A colour gradient with a white diagonal.","timestamp":"10:00 AM"}}\n'
} >"$img_file2"
IMG_ENV2="$CFG_ENV_NOHIST ALTER_ZERO_SESSIONS_DIR=$IMG_SESS2 ALTER_ZERO_IMAGE_PROTOCOL=kitty ALTER_ZERO_IMAGE_CELL_SIZE=5x10 ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS"
tmux new-session -d -s "$S107B" -x 100 -y 30 -c "$IMG_DIR2" "env $IMG_ENV2 $BIN_ABS"
sleep 0.6
tmux pipe-pane -t "$S107B" -o "cat >> $IMG_RAW"
sleep 0.3
tmux send-keys -t "$S107B" -l "/resume"
sleep 0.3
tmux send-keys -t "$S107B" Enter
sleep 1.2
tmux send-keys -t "$S107B" Enter # load the seeded session
img_tx_inline=0
for _ in $(seq 1 60); do
	if tmux capture-pane -t "$S107B" -p -S -60 | grep -qF "Read image (PNG, 100x60"; then
		img_tx_inline=1
		break
	fi
	sleep 0.1
done
sleep 0.8
img_split="$(stat -c%s "$IMG_RAW" 2>/dev/null || echo 0)"
tmux send-keys -t "$S107B" C-o
sleep 1.5
# A kitty transmit is `ESC _ G <controls incl. a=T> ; <base64> ESC \` — count
# them on each side of the byte offset the switch happened at.
img_kitty_tx() { grep -ao "$(printf '\033')_G[^;]*a=T" 2>/dev/null | wc -l; }
img_tx_before="$(head -c "$img_split" "$IMG_RAW" | img_kitty_tx)"
img_tx_after="$(tail -c +$((img_split + 1)) "$IMG_RAW" | img_kitty_tx)"
# …and it is uploaded ONCE per screen, not once per keypress. A picture is
# megabytes on the wire (raw RGBA — 4.5 MB of base64 for a 120x35-cell one),
# so re-uploading it on the way out, or on every reopen, is the flicker the
# per-screen cache exists to avoid. Coming back costs nothing because the
# overlay never touched the primary screen's store; reopening costs nothing
# because a virtually-placed image survives the 1049 switch.
img_split_back="$(stat -c%s "$IMG_RAW" 2>/dev/null || echo 0)"
tmux send-keys -t "$S107B" C-o
sleep 1.5
img_tx_back="$(tail -c +$((img_split_back + 1)) "$IMG_RAW" | img_kitty_tx)"
img_split_again="$(stat -c%s "$IMG_RAW" 2>/dev/null || echo 0)"
tmux send-keys -t "$S107B" C-o
sleep 1.5
img_tx_again="$(tail -c +$((img_split_again + 1)) "$IMG_RAW" | img_kitty_tx)"
echo "==== Phase 107b: kitty transmits — inline=$img_tx_before overlay=$img_tx_after back=$img_tx_back reopen=$img_tx_again (cell seen=$img_tx_inline) ===="
tmux kill-session -t "$S107B" 2>/dev/null
rm -rf "$IMG_DIR2" "$IMG_SESS2" "$(dirname "$IMG_RAW")"
if [ "$img_tx_inline" != 1 ]; then
	echo "FAIL: Phase 107b precondition — the seeded image session never loaded" >&2
	status=1
fi
if [ "$img_tx_before" -lt 1 ]; then
	echo "FAIL: Phase 107b — the inline commit never transmitted the picture to the terminal" >&2
	status=1
fi
if [ "$img_tx_after" -lt 1 ]; then
	echo "FAIL: Phase 107b — the Ctrl+O overlay placed the picture without transmitting it for the alternate screen; it draws nothing there" >&2
	status=1
fi
if [ "$img_tx_back" -ne 0 ]; then
	echo "FAIL: Phase 107b — returning from the overlay re-uploaded the picture ($img_tx_back transmits); the primary screen's encoding must survive the round trip" >&2
	status=1
fi
if [ "$img_tx_again" -ne 0 ]; then
	echo "FAIL: Phase 107b — reopening the overlay re-uploaded the picture ($img_tx_again transmits); a virtually-placed image survives the 1049 switch, so each screen uploads once" >&2
	status=1
fi

# --- Phase 107c: a picture TALLER THAN THE PAGER still draws its visible part
# (docs/images.md). Ctrl+O opens pinned to the bottom, so a tall picture starts
# *above* the window and its first visible row carries a non-zero row index —
# which the first version had no way to draw from, leaving a screenful of
# reserved-but-empty rows on the most ordinary open there is. A 100x600 PNG at
# a 5x10 cell on a 40-column terminal is 19 rows against an 11-row pager body,
# and the seeded session ENDS on the image, so the bottom-pinned open lands in
# the middle of it. ---
S107C="${S}_imageclip"
IMG_DIR3="$(mktemp -d /tmp/alter-zero-smoke-imageclip-XXXXXX)"
IMG_SESS3="$(mktemp -d /tmp/alter-zero-smoke-imgclipsess-XXXXXX)"
mkdir -p "$IMG_SESS3/2026/07/23"
base64 -d >"$IMG_DIR3/tall.png" <<'PNG64'
iVBORw0KGgoAAAANSUhEUgAAAGQAAAJYCAIAAAAbp9T8AAAcfUlEQVR4Ae3AA6AkWZbG8f937o3I
zKdyS2Oubdu2bdu2bdu2bWmMnpZKr54yMyLu+Xa3anqmhztr1a/aLvqjQguy0IIstCALLchCK7Qg
Cy3IQguy0IIstEILstCCLLQgCy3IQguy0AotyEILstCCLLQgC63Qgiy0IAstyEILstCCLLRCC7LQ
giy0IAstyEIrtCALLchCC7LQgiy0IAut0IIstCALLchCC7KQBQIKBBQIKBBQIKBAgYACAQUCCgQU
COlPuOpfBEBw1YsCgIq46l8GQHDViwKAirjqXwZAAK29Cle9cAAEUMofcdULB0BFXPUvAyC46kUB
QEVc9S8DILjqRQFARQDD8Orz/ne56gUBIACg73+fq14IAIKrXhQAVMRV/zIAgqteFABUxFX/MgCC
+61Wr8lVLwgAFXHFfPG7RVz1/AEQXPWiACC46kUBQEVc9S8DILjqRQFARTzL4eHrbG/+Blc9LwCC
B9jc/C2uer4AqIir/mUABFe9KAAIrnpRAFARV/3LAAie0/7+63HV8wKgIh5oe+c3irjquQEQXPWi
AKAirvqXARBc9aIAILjqRQFAIBAIBAKxe+kNEQgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAI
BAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCETyP48d/
laueCwAVcdW/DIDgqhcFABVx1b8MgOCqFwUAwfNz4cIbc9UDAVARz+vkqV8u4qpnAyC46kUBQEVc
9S8DILjqRQFAcNWLAoBAIBAIBAKBOHvuzRAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQC
gUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAhE8AKcOfMLXPUs
AFTEVf8yAIKrXhQAVMRV/zIAgqteFAAEL9i9974FV10BQEW8INde93NFXAUAQHDViwKAirjqXwZA
cNWLAoCKuOpfBkDwQt1111tzFQBA8ELdcMNPcxUAQEVc9S8DILjqRQFARVz1LwMguOpFAUAgEAgE
AoFAIBCI2+94OwQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAg
EAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBCI4F9y880/wVUABFe9KACoiKv+ZQAEV70oAKiI
q/5lAAQvgmc84x34fw6AivgXPejBP1bE/2sABFe9KAAIrnpRAFARV/3LAAiuelEAEAgEAoFAIBAI
BAKBeNrT3xmBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQC
gUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEIjgRfPQh/4w/58BUBFX/csACK56UQAQXPWiAKAirvqX
ARC8yJ7ylHfl/y0AKuJF9PBH/GAR/08BEFz1ogAguOpFAUBFXPUvAyC46kUBQCAQCAQCgUAgEAgE
AoF44pPeA4FAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKB
QCAQCAQCgUAgEAgEAoFAIBAIBAKBQCCCf41HPer7+P8JgIq46l8GQHDViwKA4KoXBQAVcdW/DIDg
X+nxj38v/h8CoCL+VR7z2O8p4v8dAIKrXhQAVMRV/zIAgqteFAAEV70oAAgEAoFAIBAIBAKBQCAQ
CMTf/8P7IhAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQ
CAQCgUAgEAgEAoFAIBAIBAKBQCAQwb/ei7/4d/L/DQAVcdW/DIDgqhcFABVx1b8MgOCqFwUAwb/J
3/7t+/P/CgAV8W/wki/17UX8PwJAcNWLAoCKuOpfBkBw1YsCgIq46l8GQPBv9Vd/9UH8/wFA8G/1
Mi/zLfz/AUBFXPUvAyC46kUBQEVc9S8DILjqRQFAIBAIBAKBQCAQCAQCgUAgEAjEn//FhyIQCAQC
gUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKB
QCAQCAQCgQj+HV7+5b+R/ycACK56UQBQEVf9ywAIrnpRAFARV/3LAAj+ff70Tz+c/w8ACP59XvEV
v57/DwCoiKv+ZQAEV70oAKiIq/5lAARXvSgACAQCgUAgEAgEAoFAIBAIBAKBQCD+6I8/CoFAIBAI
BAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgE
AoFAIBDBv9urvMrX8H8eAMFVLwoAKuKqfxkAwVUvCgAq4qp/GQDBf4Q/+IOP4f82ACri3+/VXv2r
ivi/DIDgqhcFAMFVLwoAKuKqfxkAwVUvCgACgUAgEAgEAoFAIBAIBAKBQCAQCATid3/v4xEIBAKB
QCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFA
IBAIRPAf5DVf88v5PwyAirjqXwZAcNWLAoDgqhcFABVx1b8MgOA/zm//9ifyfxUAFfEf5bVf50uL
+L8JgOCqFwUAFXHVvwyA4KoXBQDBVS8KAAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKB+I3f/BQEAoFA
IBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAg
EAhE8B/q9V7vi/g/CYCKuOpfBkBw1YsCgIq46l8GQHDViwKA4D/ar/3ap/F/DwAV8R/rDd7wC4r4
vwaA4KoXBQAVcdW/DIDgqhcFAMFVLwoAAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIH75Vz4TgUAg
EAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQ
CETwn+CN3/hz+T8GgIq46l8GQHDViwKAirjqXwZAcNWLAoDgP8cv/uJn838JABXxn+FN3+yzi/i/
A4DgqhcFABVx1b8MgOCqFwUAFXHVvwyA4D/Nz/3c5/F/BgDBf5q3eIvP4P8MACriqn8ZAMFVLwoA
KuKqfxkAwVUvCgACgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAjET//MFyIQCAQCgUAgEAgEAoFA
IBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIRPCf6a3f+lP5vwGA
4KoXBQAVcdW/DIDgqhcFABVx1b8MgOA/2U/+5BfzfwAAFfGf6m3f7pOL+F8PgOCqFwUAwVUvCgAq
4qp/GQDBVS8KAAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQC8WM//mUIBAKBQCAQCAQCgUAg
EAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAhE8J/vHd7hE/jfDoCK
uOpfBkBw1YsCgOCqFwUAFXHVvwyA4L/Ej/zIV/C/GgAV8V/gnd7544r4XwyA4KoXBQDBVS8KACri
qn8ZAMFVLwoAAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQPzgD301AoFAIBAIBAKBQCAQ
CAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCETwX+Vd3/Wj+d8LgIq4
6l8GQHDViwKA4KoXBQAVcdW/DIDgv9D3f//X8r8UABXxX+bd3+Mji/hfCYDgqhcFABVx1b8MgOCq
FwUAwVUvCgACgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBDf873fgEAgEAgEAoFAIBAI
BAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIRPBf673e68P43wiAirjq
XwZAcNWLAoCKuOpfBkBw1YsCgOC/3Hd91zfxvw4AFfFf7H3e90OK+F8GgOCqFwUAFXHVvwyA4KoX
BQAVcdW/DIDgv8O3f/u38r8LAMF/h/d//w/kfxcAKuKqfxkAwVUvCgAq4qp/GQDBVS8KAAKBQCAQ
CAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgE4lu+9TsQCAQCgUAgEAgEAoFAIBAIBAKBQCAQ
CAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAhE8N/kgz7o/fhfBIDgqhcFABVx1b8MgOCqFwUA
FXHVvwyA4L/PN33Td/G/BQDBf58P+ZD34X8LACriqn8ZAMFVLwoAKuKqfxkAwVUvCgACgUAgEAgE
AoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKB+Ppv+F4EAoFAIBAIBAKBQCAQCAQCgUAgEAgE
AoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIRPDf6sM//D35XwGA4KoXBQAVcdW/DIDgqhcFABVx
1b8MgOC/29d+7ffzPx8AFfHf6yM/6t2L+J8OgOCqFwUAwVUvCgAq4qp/GQDBVS8KAAKBQCAQCAQC
gUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIL7qq38IgUAgEAgEAoFAIBAIBAKBQCAQCAQC
gUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAhE8D/Ax3zMu/A/HAAVcdW/DIDgqhcFAMFVLwoAKuKq
fxkAwf8MX/EVP8L/ZABUxP8EH/fx71TE/1wABFe9KACoiKv+ZQAEV70oAAiuelEAEAgEAoFAIBAI
BAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUB86Zf9OAKBQCAQCAQCgUAgEAgEAoFAIBAI
BAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCCC/zE+8RPfnv+xAKiIq/5lAARXvSgAqIir/mUABFe9
KAAI/if54i/+Sf5nAqAi/uf45E952yL+JwIguOpFAUBFXPUvAyC46kUBQHDViwKAQCAQCAQCgUAg
EAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKB+IIv/BkEAoFAIBAIBAKBQCAQCAQCgUAg
EAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAhH8D/Npn/ZW/A8EQEVc9S8DILjqRQFARVz1LwMguOpF
AUDwP8/nfd7P8T8NABXxP81nfOZbFPE/CwDBVS8KACriqn8ZAMFVLwoAKuKqfxkAwf9In/3Zv8j/
KAAE/yN99me/Kf+jAFARV/3LAAiuelEAUBFX/csACK56UQAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQ
CAQCgUAgEAgEAoFAIBAIBAKBQCAQCATiMz7zVxAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQ
CAQCgUAgEAgEAoFAIIL/qT7v896I/zkACK56UQBQEVf9ywAIrnpRAFARV/3LAAj+B/u0T/s1/ocA
oCL+x/qCL3yDIv5HACC46kUBQHDViwKAirjqXwZAcNWLAoBAIBAIBAKBQCAQCAQCgUAgEAgEAoFA
IBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIxCd/ym8iEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFA
IBAIBAKBQCAQCAQCEfzP9sVf/Lr8TwBARVz1LwMguOpFAUBw1YsCgIq46l8GQPA/3id+4m/z3w6A
ivgf7ku/7LWL+G8GQHDViwKA4KoXBQAVcdW/DIDgqhcFAIFAIBAIBAKBQCAQCAQCgUAgEAgEAoFA
IBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBOLjPv73EAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFA
IBAIBAKBQCAQCAQi+N/gK77iNfjvBUBFXPUvAyC46kUBQHDViwKAirjqXwZA8L/Ex3zMH/DfCICK
+F/hq7761Yr4bwNAcNWLAoCKuOpfBkBw1YsCgOCqFwUAgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAg
EAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoH4yI/6YwQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAg
EAgEAoFAIBAIBCL43+Nrv/aV+e8CQEVc9S8DILjqRQFARVz1LwMguOpFAUDwv8qHf/if8t8CgIr4
X+Trv+EVi/hvAEBw1YsCgIq46l8GQHDViwKAirjqXwZA8L/Nh3zIX/BfD4Dgf5tv+qaX478eABVx
1b8MgOCqFwUAFXHVvwyA4KoXBQCBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFA
IBAIBAKBQCAQCAQCgUAgEB/4QX+NQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEIvhf
6Fu/9aX5LwZAcNWLAoCKuOpfBkBw1YsCgIq46l8GQPC/0/u//9/yXwmA4H+nb//2l+S/EgAVcdW/
DIDgqhcFABVx1b8MgOCqFwUAgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQ
CAQCgUAgEAgEAoFAIBAIxPu87z8gEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBCL4X+u7
vuvF+C8DQHDViwKAirjqXwZAcNWLAoCKuOpfBkDwv9l7vdfj+a8BQEX87/U93/uYIv4rABBc9aIA
ILjqRQFARVz1LwMguOpFAUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQC
gUAgEAgEAoFAIBAIBAKBQLz7ezwJgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFABP/Lff/3
P5L/AgBUxFX/MgCCq14UAARXvSgAqIir/mUABP/7veu7PoX/bABUxP92P/hDDy/iPxcAwVUvCgAq
4qp/GQDBVS8KAIKrXhQABAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAI
BAKBQCAQCAQCgUAgEAgEAvFO7/x0BAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCETwf8KP/MhD
+E8FQEVc9S8DILjqRQFARVz1LwMguOpFAUDwf8U7vMMz+M8DQEX83/BjP/6gIv6zABBc9aIAoCKu
+pcBEFz1ogAguOpFAUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAg
EAgEAoFAIBAIBAKBQCAQCMTbvt0dCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAggv9DfvInb+I/
CQAVcdW/DIDgqhcFABVx1b8MgOCqFwUAwf8tb/3Wd/GfAYCK+L/kp3/mhiL+4wEQXPWiAKAirvqX
ARBc9aIAoCKu+pcBEPyf8xZvcS//4QAI/s/5uZ+7lv9wAFTEVf8yAIKrXhQAVMRV/zIAgqteFAAE
AoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQC
gUAgEIg3fbNzCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCEfxf9Iu/eJr/WAAEV70oAKiIq/5lAARX
vSgAqIir/mUABP9HvfEbX+A/EAAV8X/SL//KySL+wwAQXPWiACC46kUBQEVc9S8DILjqRQFAIBAI
BAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgE
AoFAIN7gDS8hEAgEAoFAIBAIBAKBQCAQCAQCgUAgEMH/Xb/2a8f4jwJARVz1LwMguOpFAUBw1YsC
gIq46l8GQPB/2uu93j7/IQCoiP/DfuM3t4v4DwBAcNWLAoDgqhcFABVx1b8MgOCqFwUAgUAgEAgE
AoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQC
gUAgXvt1jhAIBAKBQCAQCAQCgUAgEAgEAoFAIIL/6377tzf49wOgIq76lwEQXPWiACC46kUBQEVc
9S8DIPh/4DVfc8W/EwAV8X/e7/7evIh/FwCCq14UAFTEVf8yAIKrXhQABFe9KAAIBAKBQCAQCAQC
gUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKB
QLzaq48IBAKBQCAQCAQCgUAgEAgEAoFABP8//MEfdPx7AFARV/3LAAiuelEAUBFX/csACK56UQAQ
/L/xKq/S+DcDoCL+n/ijPy5F/BsBEFz1ogCgIq76lwEQXPWiAKAirvqXARD8f/KKr2j+bQAI/j/5
0z8V/zYAVMRV/zIAgqteFABUxFX/MgCCq14UAAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgE
AoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAjEy728EQgEAoFAIBAIBAKB
QCAQCETw/8xf/IX4NwAguOpFAUBFXPUvAyC46kUBQEVc9S8DIPj/52VexvxrARD8//NXfyX+tQCo
iKv+ZQAEV70oAKiIq/5lAARXvSgAqIj/h17yJf33fytedAAE/y/97d+KfxUAgqteFABUxFX/MgCC
q14UAFTEVf8yAIL/r178xc2LDoCK+P/p7/9BRbyoAAiuelEAEFz1ogCgIq76lwEQXPWiAKAi/t96
zGP8xMeLFwUAwf9jj3+8eBEBUBFX/csACK56UQAQXPWiAKAirvqXARD8//aoR5kXBQAV8f/ZE5+k
Iv5lAARXvSgAqIir/mUABFe9KAAIrnpRAFAR/889/OF+2lPECwdA8P/eU54i/kUAVMRV/zIAgqte
FABUxFX/MgCCq14UAARXwUMfal44ACriqqc9XUW8MAAEV70oAKiIq/5lAARXvSgACK56UQBQEVcB
D3qQb3+GeEEACK667BnPEC8EABVx1b8MgOCqFwUAFXHVvwyA4KoXBQDBVfe7+WbzggBQEVddcfsd
KuL5AyC46kUBQEVc9S8DILjqRQFARVz1LwMguOoBbrjBPF8ABFc9wF13iecLgIq46l8GQHDViwKA
irjqXwZAcNWLAoCKuOqBrr3WZ+8VzwWA4KrndO+94nkBEFz1ogCgIq76lwEQXPWiAKAirvqXARBc
9TzOnDHPBYCKuOq5nD2nIp4DAMFVLwoAgqteFABUxFX/MgCCq14UAFTEVc/r5EnvXhDPAkBw1fNz
4YJ4IAAq4qp/GQDBVS8KAIKrXhQAVMRV/zIAgqtegOPHzbMAUBFXPV+7l1TEMwEQXPWiACC46kUB
QEVc9S8DILjqRQFARVz1gmxv+3BfAAAEV71g+/viCgAq4qp/GQDBVS8KAIKrXhQAVMRV/zIAgqte
qM1NAwBQEVe9EIdHKgIAgqteFABUxFX/MgCCq14UAARXvSgAqIirXrj53MNKQHDVv2S1EgBUxFX/
MgCCq14UAFTEVf8yAIKrXhQABFe9CPreQEVc9S8aRhURXPWiAKAirvqXARBc9aIAoCKu+pcBULnq
RQGluHLViwJaU0Vc9S8DILjqRQFARVz1LwMguOpFAcA/Ao9HoT6C64xuAAAAAElFTkSuQmCC
PNG64
img_file3="$IMG_SESS3/2026/07/23/rollout-2026-07-23T10-00-00-10710712.jsonl"
{
	printf '{"timestamp":"2026-07-23T10:00:00.000Z","type":"session_meta","payload":{"id":"smoke-107c","timestamp":"2026-07-23T10:00:00.000Z","cwd":"%s","model":"dummy_model_name","originator":"alter-zero","version":"0.1.0"}}\n' "$IMG_DIR3"
	printf '{"timestamp":"2026-07-23T10:00:01.000Z","type":"message","payload":{"role":"user","text":"look at tall.png","timestamp":"10:00 AM"}}\n'
	printf '{"timestamp":"2026-07-23T10:00:02.000Z","type":"tool","payload":{"name":"Read","args":"%s/tall.png","ok":true,"output":"Read image (PNG, 100x600, 9 KB)","timestamp":"10:00 AM","shell":false,"truncated":false,"arguments":"{\\"path\\":\\"%s/tall.png\\"}"}}\n' "$IMG_DIR3" "$IMG_DIR3"
} >"$img_file3"
IMG_ENV3="$CFG_ENV_NOHIST ALTER_ZERO_SESSIONS_DIR=$IMG_SESS3 ALTER_ZERO_IMAGE_PROTOCOL=halfblocks ALTER_ZERO_IMAGE_CELL_SIZE=5x10 ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS"
tmux new-session -d -s "$S107C" -x 40 -y 16 -c "$IMG_DIR3" "env $IMG_ENV3 $BIN_ABS"
sleep 0.6
tmux send-keys -t "$S107C" -l "/resume"
sleep 0.3
tmux send-keys -t "$S107C" Enter
sleep 1.2
tmux send-keys -t "$S107C" Enter # load it
img_clip_loaded=0
for _ in $(seq 1 60); do
	if tmux capture-pane -t "$S107C" -p -S -60 | grep -qF "Read image (PNG, 100x600"; then
		img_clip_loaded=1
		break
	fi
	sleep 0.1
done
sleep 0.6
# A picture row is a non-blank line carrying none of the chrome's letters,
# digits or percent sign — the half-block glyphs are multibyte, which the C
# locale's character classes never match.
img_clip_inline="$(tmux capture-pane -t "$S107C" -p -S -60 | grep -v '[A-Za-z0-9%]' | grep -cve '^[[:space:]]*$')"
tmux send-keys -t "$S107C" C-o
sleep 1.5
img_clip_overlay="$(tmux capture-pane -t "$S107C" -p | grep -v '[A-Za-z0-9%]' | grep -cve '^[[:space:]]*$')"
echo "==== Phase 107c: tall picture — inline_rows=$img_clip_inline pager_rows=$img_clip_overlay ===="
tmux capture-pane -t "$S107C" -p | tail -4
tmux kill-session -t "$S107C" 2>/dev/null
rm -rf "$IMG_DIR3" "$IMG_SESS3"
if [ "$img_clip_loaded" != 1 ]; then
	echo "FAIL: Phase 107c precondition — the seeded tall-image session never loaded" >&2
	status=1
fi
if [ "$img_clip_inline" -lt 15 ]; then
	echo "FAIL: Phase 107c precondition — the fixture is not taller than the pager (inline drew only $img_clip_inline rows)" >&2
	status=1
fi
if [ "$img_clip_overlay" -lt 8 ]; then
	echo "FAIL: Phase 107c — the bottom-pinned Ctrl+O pager drew $img_clip_overlay rows of a picture whose head is above the window; a cut block must still draw its visible part" >&2
	status=1
fi


# --- Phase 108: the [PROMPT] CLI shortcut and the --help page (docs/cli.md).
# `alter-zero "hello there"` boots STRAIGHT into that turn — the ❯ bubble, the
# streamed reply and the Done summary appear with no key pressed — and its
# quit still prints the resume hint; `--resume {id} "again please"` reloads
# the transcript and runs the prompt as the next turn in the SAME rollout
# file; `-c "…"` does the same for the newest session here. And the help
# page: on the pane's tty it opens on 'Alter Zero' in the bold-cyan heading
# escape and carries the [PROMPT] argument, through a pipe the same page
# holds no escape at all, and a grammar error prints the clap-shaped trailer
# with exit 2. The pane must OUTLIVE the app to capture what it prints after
# restore, so each launch is wrapped in a shell that holds the pane open. ---
S108="${S}_cliprompt"
CLIP_DIR="$(mktemp -d /tmp/alter-zero-smoke-cliprompt-XXXXXX)"
CLIPAPP="env $CFG_ENV_NOHIST ALTER_ZERO_SESSIONS_DIR=$CLIP_DIR ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux new-session -d -s "$S108" -x 80 -y 24 "$CLIPAPP \"$USER_MSG\"; echo CLI_APP_EXITED; sleep 60"
clip_first_pane=""
for _ in $(seq 1 134); do # the shortcut's turn → "Done for", no key pressed
	clip_first_pane="$(tmux capture-pane -t "$S108" -p -S -60)"
	if printf '%s' "$clip_first_pane" | grep -qF "Done for"; then
		break
	fi
	sleep 0.15
done
echo "==== Phase 108: alter-zero \"$USER_MSG\" — the turn ran from the command line ===="
printf '%s\n' "$clip_first_pane"
tmux send-keys -t "$S108" C-c # quit (empty composer)
for _ in $(seq 1 40); do
	if tmux capture-pane -t "$S108" -p | grep -qF "CLI_APP_EXITED"; then
		break
	fi
	sleep 0.1
done
clip_quit_pane="$(tmux capture-pane -t "$S108" -p -S -80)"
clip_hint_id="$(printf '%s\n' "$clip_quit_pane" | sed -n 's/.*--resume \([a-f0-9-]*\).*/\1/p' | tail -1)"
tmux kill-session -t "$S108" 2>/dev/null
# --resume {id} "prompt": the transcript reloads and the prompt runs next.
tmux new-session -d -s "$S108" -x 80 -y 24 "$CLIPAPP --resume $clip_hint_id \"again please\"; echo CLI_APP_EXITED; sleep 60"
clip_resume_pane=""
for _ in $(seq 1 134); do # the loaded turn's summary + the new turn's → 2× "Done for"
	clip_resume_pane="$(tmux capture-pane -t "$S108" -p -S -100)"
	if [ "$(printf '%s' "$clip_resume_pane" | grep -cF "Done for")" -ge 2 ]; then
		break
	fi
	sleep 0.15
done
sleep 0.3
echo "==== Phase 108: --resume {id} \"again please\" (reloaded, then the prompt's turn) ===="
printf '%s\n' "$clip_resume_pane"
tmux kill-session -t "$S108" 2>/dev/null
# -c "prompt": the newest session here, plus a third turn.
tmux new-session -d -s "$S108" -x 80 -y 24 "$CLIPAPP -c \"and once more\"; echo CLI_APP_EXITED; sleep 60"
clip_continue_pane=""
for _ in $(seq 1 134); do
	clip_continue_pane="$(tmux capture-pane -t "$S108" -p -S -140)"
	if [ "$(printf '%s' "$clip_continue_pane" | grep -cF "Done for")" -ge 3 ]; then
		break
	fi
	sleep 0.15
done
sleep 0.3
clip_files="$(find "$CLIP_DIR" -type f -name 'rollout-*.jsonl' | wc -l | tr -d ' ')"
echo "==== Phase 108: -c \"and once more\" (rollout files: $clip_files) ===="
printf '%s\n' "$clip_continue_pane" | tail -20
tmux kill-session -t "$S108" 2>/dev/null
# The help page on a real tty: the raw pane (escapes kept — tmux re-encodes
# the binary's `ESC[1;36m` as its own `ESC[1m ESC[36m`, so the check is on
# the cyan `36m` landing right before the word) opens on the bold-cyan title.
tmux new-session -d -s "$S108" -x 100 -y 40 "$BIN --help; echo CLI_APP_EXITED; sleep 60"
for _ in $(seq 1 40); do
	if tmux capture-pane -t "$S108" -p | grep -qF "CLI_APP_EXITED"; then
		break
	fi
	sleep 0.1
done
clip_help_raw="$(tmux capture-pane -t "$S108" -p -e)"
clip_help_text="$(tmux capture-pane -t "$S108" -p)"
echo "==== Phase 108: --help on the pane's tty ===="
printf '%s\n' "$clip_help_text" | head -24
tmux kill-session -t "$S108" 2>/dev/null
# …and through a pipe: the same words, no escape anywhere.
clip_help_piped="$("$BIN" --help 2>&1)"
clip_help_piped_exit=$?
clip_usage_err="$("$BIN" fix "the bug" 2>&1)"
clip_usage_err_exit=$?
echo "==== Phase 108: a grammar error → exit $clip_usage_err_exit ===="
printf '%s\n' "$clip_usage_err"
rm -rf "$CLIP_DIR" 2>/dev/null

# Phase 108: the [PROMPT] shortcut and the --help page.
if ! printf '%s' "$clip_first_pane" | grep -qF "❯ $USER_MSG"; then
	echo "FAIL: Phase 108 — the command-line prompt was not committed as the user bubble" >&2
	status=1
fi
if ! printf '%s' "$clip_first_pane" | grep -qF "$EXPECT_REPLY"; then
	echo "FAIL: Phase 108 — the command-line prompt's turn never streamed its reply" >&2
	status=1
fi
if ! printf '%s' "$clip_first_pane" | grep -qF "Done for"; then
	echo "FAIL: Phase 108 — the command-line prompt's turn never settled" >&2
	status=1
fi
if ! printf '%s' "$clip_quit_pane" | grep -qF "Resume this session with:"; then
	echo "FAIL: Phase 108 — quitting the shortcut's session printed no resume hint" >&2
	status=1
fi
if [ -z "$clip_hint_id" ]; then
	echo "FAIL: Phase 108 — no '--resume {id}' line under the hint" >&2
	status=1
fi
if ! printf '%s' "$clip_resume_pane" | grep -qF "❯ $USER_MSG"; then
	echo "FAIL: Phase 108 — --resume {id} \"prompt\" did not reload the first turn" >&2
	status=1
fi
if ! printf '%s' "$clip_resume_pane" | grep -qF "❯ again please"; then
	echo "FAIL: Phase 108 — --resume {id} \"prompt\" did not run the prompt as the next turn" >&2
	status=1
fi
if [ "$(printf '%s' "$clip_resume_pane" | grep -cF "Done for")" -lt 2 ]; then
	echo "FAIL: Phase 108 — the resumed session's prompt turn never settled" >&2
	status=1
fi
if ! printf '%s' "$clip_continue_pane" | grep -qF "❯ and once more"; then
	echo "FAIL: Phase 108 — -c \"prompt\" did not run the prompt as the next turn" >&2
	status=1
fi
if [ "$clip_files" != "1" ]; then
	echo "FAIL: Phase 108 — the prompt turns should append to the SAME rollout file, found $clip_files files" >&2
	status=1
fi
if [ "$(printf '%s\n' "$clip_help_text" | head -1)" != "Alter Zero" ]; then
	echo "FAIL: Phase 108 — --help on a tty does not open on 'Alter Zero' (got '$(printf '%s\n' "$clip_help_text" | head -1)')" >&2
	status=1
fi
if ! printf '%s' "$clip_help_raw" | grep -qF "36mAlter Zero"; then
	echo "FAIL: Phase 108 — --help on a tty does not wear the bold-cyan heading escape on its title" >&2
	status=1
fi
if ! printf '%s' "$clip_help_raw" | grep -qF "36mUsage:"; then
	echo "FAIL: Phase 108 — --help on a tty does not wear the heading escape on 'Usage:'" >&2
	status=1
fi
if ! printf '%s' "$clip_help_text" | grep -qF "[PROMPT]"; then
	echo "FAIL: Phase 108 — --help does not name the [PROMPT] argument" >&2
	status=1
fi
if [ "$clip_help_piped_exit" != "0" ] || [ "$(printf '%s\n' "$clip_help_piped" | head -1)" != "Alter Zero" ]; then
	echo "FAIL: Phase 108 — piped --help should exit 0 opening on 'Alter Zero' (exit $clip_help_piped_exit)" >&2
	status=1
fi
if printf '%s' "$clip_help_piped" | grep -qF "$(printf '\033')"; then
	echo "FAIL: Phase 108 — piped --help carries an escape sequence" >&2
	status=1
fi
if [ "$clip_usage_err_exit" != "2" ]; then
	echo "FAIL: Phase 108 — a grammar error should exit 2, got $clip_usage_err_exit" >&2
	status=1
fi
if ! printf '%s' "$clip_usage_err" | grep -qF "error: unexpected argument: the bug"; then
	echo "FAIL: Phase 108 — the grammar error does not lead with clap's 'error:' line naming the culprit" >&2
	status=1
fi
if ! printf '%s' "$clip_usage_err" | grep -qF "For more information, try '--help'."; then
	echo "FAIL: Phase 108 — the grammar error does not close on the --help pointer" >&2
	status=1
fi


if [ "$status" -eq 0 ]; then
	echo "PASS: reply + tools streamed to scrollback, the cursor stays visible on the prompt row mid-stream, the input box grows and stays flush at the bottom after a reply, typing bursts render in one repaint, Ctrl+O opens the tool-output view, the slash-command palette opens and runs commands, Esc interrupts a streaming turn, Ctrl+C clears a draft before /quit exits, Up recalls the last sent message for resubmission, ? toggles the shortcuts band, messages submitted mid-turn wait inset above the box and are read by the turn ALREADY RUNNING at its next round boundary — in a subagent's session view exactly as in the main one (Esc sends back what the turn never read, Alt+Up pulls the last follow-up back to edit — or, with none left, a message the turn has not read yet, Tab queues a message as a separate follow-up turn that runs after, and a !command queued mid-turn runs locally as its own standalone shell turn — never sent to the backend as text), the session footer ({model} · {cwd}) sits under the box except while a band is open, every scrollback commit clears+repaints the live region inside one synchronized frame (no flicker), /clear mid-turn kills the generation and blanks the screen (nothing streams in afterwards), a resize — height-only included, mid-stream included — re-presents the conversation at the new size with a single input box, Ctrl+R reverse-searches the input history (typed queries preview matches in the composer, Enter accepts, Esc cancels without quitting), and !commands run locally (the bang is absorbed into a '! cmd' prompt with a Shell mode hint, the run commits as a codex-style exec cell — the dark '! cmd' header with its ⎿ output flush below, ⎿ Running… (Ns) while it runs (no spinner status line — the elapsed rides the preview), no summary — a non-zero exit reports its status, Esc interrupts a long one (resolving ⎿ Interrupted by user with no 'Conversation interrupted' notice), multi-line output shows a 4-line ⎿ preview with a '+N lines (ctrl+o to expand)' hint, and a huge output is capped in memory — no temp file, peak RSS bounded — with a '…' truncation marker at the end of the Ctrl+O view), and the dummy AI pauses before streaming so the status indicator shows first — the just-sent user message counted as ↑ tokens during the pause, flipping to ↓ once the reply streams, and Ctrl+J inserts a newline (the universal Shift+Enter fallback) so the box grows and a plain Enter then submits the multi-line draft, and typing @query opens a file picker below the box (async walk+rank) whose Enter inserts the highlighted path into the composer, and a large bracketed paste collapses to a '[Pasted Content N chars]' placeholder in the composer instead of dumping the raw text (and one Backspace removes the whole placeholder atomically), and Ctrl+V pastes a clipboard image as an '[Image #N]' placeholder (here, headless with no clipboard, it fails gracefully with a red 'Failed to paste image' notice and the composer stays responsive), and a message queued mid-turn shows inside the Ctrl+O transcript view and auto-dispatches there when the turn ends (the overlay follows the new turn live), and /copy copies the last assistant response to the clipboard (an empty conversation reports 'No agent response to copy'; after a reply it confirms 'Copied last message to clipboard' and — arboard having no clipboard here — its OSC 52 fallback lands the reply text in tmux's paste buffer), and Esc Esc backtracks to a previous user message (the first idle Esc arms with an 'esc again to edit previous message' footer hint, the second opens the transcript preview whose hint row shows the backtrack keys, a further Esc steps to the older message, and Enter rewinds the conversation to that point with the message back in the composer — resubmitting it streams a fresh turn to its summary), and /resume picks up a saved session (every conversation records to a rollout JSONL file — session_meta line first, created lazily on the first user message — a later launch's /resume lists it in a full-screen picker with a humanized age and the first-user-message preview, Enter repaints the whole saved conversation inline and appends the turns that follow to the same file, /clear starts a fresh rollout so the next message lands in a new one, and the picker carries codex's Filter/Sort toolbar — 'Filter: [Cwd] All   Sort: [Updated] Created' on the search row, Tab + arrows toggling — with the selected row lit on a full-width background tint), and an Esc interrupt stays prompt even when the backend is slow to observe the cancel — under a stalled backend (ALTER_ZERO_STALL_MS, ignoring the cancel for 3s) that streamed nothing, Esc undoes the no-output turn and settles within a frame (the status line clears and 'hello there' returns to the composer, no 'Conversation interrupted' notice) because the loop detaches the thread and swaps the reply channel instead of join()ing it (the interrupt-lag fix — no UI freeze), and slash-command confirmations and soft rejections surface as transient toasts above the box that self-clear after a few seconds instead of committing scrollback bullets (/copy confirms with a toast that then vanishes; /help and /resume run mid-turn are rejected with a toast; /model and /login now open their inline pickers mid-turn since they only swap the composer, never the running turn), and a resize reflow hides the hardware cursor before it homes/clears the screen and reshows it only at the prompt seat — so a terminal cursor-trail animation (kitty) can't streak from the top when the redraw drags the cursor around, and a mid-stream Ctrl+O round trip keeps the already-streamed partial reply on the restored screen (the repaint carries the stream's committed rows and catches up on what streamed under the overlay exactly once — no vanish, no flicker, no duplicate), and Ctrl+D opens the full-screen context-debug view showing the raw LLM context window (role-tagged entries, the conversation verbatim, tool calls in the provider-native wire format — an assistant '→ name(args)' request plus a 'tool:' result entry) with q returning to the repainted conversation, and the input history PERSISTS across sessions (a message submitted in one process is written to an append-only history.jsonl and, in a fresh process against the same file, Up recalls it and Ctrl+R finds it — both ↑/↓ recall and reverse-search span sessions like codex), and a PARALLEL tool-call batch is visible and clear — the model's calls are announced up front so the running one shows live while the not-yet-run ones show '⎿ Waiting…' in the live region, each committing to scrollback as it finishes (a 'parallel' prompt demos three Bash(ping …) calls at once; the default turn keeps a compact Read+Bash batch; a real backend renders however many parallel calls the model requests — docs/parallel-tools.md), and a running Bash tool STREAMS and TAILS its live output — the last lines under the ⎿ gutter plus a '+N lines (Ns)' footer while it runs, collapsing to the head peek '… +N lines (ctrl+o to expand)' when it finishes (Claude-Code's running-command look — docs/tool-streaming.md), and the Ctrl+O tool-output overlay shows a running bash tool's output LIVE (unlike Claude Code, whose transcript only shows tool output once it finishes) — a running Bash(ping) cell streams into the overlay while its batch siblings still show ⎿ Waiting…, tail-followed to the frontier (docs/tool-streaming.md), and a streamed markdown TABLE previews its whole forming grid in the strip then commits the finished block in one flush at its close — the flush syncing to the collapsed strip height so the box stays flush at the bottom (no blank band beneath it — docs/table-streaming.md), and BACKGROUND SHELLS work end to end — the '(ctrl+b to run in background)' hint is delayed a few seconds (early in a run the command shows 'Running…' but no hint yet, so a fast command never flashes it — Claude-Code-style), Ctrl+B moves a running command to the background (the cell resolves '⎿ Running in the background (↓ to manage)'), the footer counts '· N shells' and ↓ LIGHTS that count on cyan (the rest of the footer intact, no band yet) so Enter is what opens the inline manager band (list → Enter details whose output box tails the live stream → x stops the shell, committing the red 'was stopped by the user' notice) and closes itself once every shell is gone (no empty page left behind), and a background shell killed MID-TURN surfaces immediately — the notice commits at the turn's next tool boundary, on screen while the status line still spins, landing above the turn's Done summary instead of after it (the in-flight agent reads the same note from the registry board before its next round, so a model that kills its own background task hears the outcome within the same turn — docs/background.md), and every shell child runs DETACHED from the controlling terminal — a command that opens /dev/tty (sudo's password prompt) errors at once ('No such device or address') inside its cell instead of printing the prompt over the TUI and blocking on the keyboard the event loop owns (the runner spawns through the setsid detach chain — the setsid binary, else the binary's own detached-exec helper mode — crate::subprocess, docs/shell-command.md), and the startup ASCII header banner PERSISTS — shown at launch, surviving a resize round-trip and re-shown after /clear (the Purge rebuilds re-emit it), untouched by a Ctrl+O return (which flushes the overlay-queued commits beneath it instead of rewriting the screen, so a long conversation's return never duplicates it in scrollback), and the Ctrl+O transcript itself opens with the banner at its top (docs/header.md), and CHECKPOINTS reset the CODE, not just the transcript (docs/checkpoint.md) — each turn snapshots the working directory into an isolated git store (never the user's real .git), so an Esc-Esc backtrack to an earlier user message reverts a '!'-mutated file to its state at that point, and a /resume of a saved session restores the working file to that session's final checkpoint even after it was diverged on disk between launches, and Ctrl+O on a resumed CODE-HEAVY session (three ~1000-line numbered HTML Write cells) opens WARM and ATOMIC — the loop-bottom transcript warm pre-renders every committed item so the open assembles instead of re-highlighting, enter_overlay only queues the switch so the first overlay frame lands in the same flush (no capture during the switch is ever a blank screen — the old flushed-blank window kitty's cursor-trail streaked up), the overlay opens tail-following with the resumed reply in view, and the return restores the inline composer (docs/tool-view-performance.md), and checkpoints REFUSE a home-directory cwd — launched with cwd == HOME (even under an explicit ALTER_ZERO_CHECKPOINTS=1) the isolated store is never created, so the session-start snapshot can no longer hash the user's whole home directory into it before the first frame (the 'alter-zero hangs in ~' guard — checkpoint::cwd_scope refuses the home dir, its ancestors, and filesystem roots), while a project dir under the same home still checkpoints exactly as before, and /compact runs codex's summarization turn (docs/compact.md) — an empty conversation is rejected with a 'Nothing to compact' toast, a real one streams the summary INVISIBLY (the canned dummy summary never renders) and commits the cyan '● Context compacted' marker cell with the transcript above it untouched (append-only compaction), after which the Ctrl+D context-debug view derives the compacted context: the SUMMARY_PREFIX bridge carrying the summary in place of the old assistant reply, the recent user messages retained under the 20k-token budget, and AUTO-compact + the context gauge work end to end (docs/compact.md) — with a context window known (a model's /v1/models context_length, or the ALTER_ZERO_CONTEXT_WINDOW override) the footer shows a dim '{used}/{window} ({pct}%)' gauge (e.g. 1.3k/160k (0.8%)) fed by real usage frames (tokenizer estimate offline), and one turn past codex's 90%-of-window threshold makes the loop start the summarization turn ON ITS OWN at the idle boundary — the marker cell commits as '● Context compacted · {before} → {after} tokens · auto' with the transcript untouched, one attempt per user turn so an Esc'd, failed, or insufficient compaction never loops, and /init submits codex's bundled AGENTS.md-authoring prompt as a normal user turn (docs/init.md) — the full prompt commits as the user message and a mid-turn /init is rejected with the '/init is disabled while a task is in progress' toast — while the project's AGENTS.md itself LOADS INTO THE CONTEXT as codex's user-instructions fragment (docs/project-doc.md): a planted AGENTS.md shows under '# AGENTS.md instructions' in the Ctrl+D context view before any turn, backend-independent, the fragment leading the derived context and re-read at every turn start so the guide /init just wrote rides the very next turn, and TOOL PERMISSION REQUESTS gate every write/edit/bash (docs/permissions.md) — the tool thread blocks on the gate while an inline modal replaces the whole live region — the call that raised it staying visible above the modal as its '● Write(hello.py)' header over the same dim '⎿ Waiting…' a batch sibling shows (a prompt is a question about something on screen, and the approve seam runs before ToolStart, so the call genuinely is waiting — Claude Code's look) over the coloured 'Create file' title, the target path, the numbered body framed by dashed rules, the question, and '❯ 1. Yes / 2. Yes, allow all edits during this session (shift+tab) / 3. No' over an 'Esc to cancel · Tab to amend' hint row), the composer draft typed before the request lands is stashed and handed straight back when it closes, Tab swaps the options for an empty amend field ('❯ …' plus 'Enter to reject with this feedback · Esc to go back') so a rejection can carry instructions — and those instructions are KEPT: they land on the red cell as an 'Instructions: …' line (the transcript's only record of what was asked for) while the model-facing stop-and-wait text, feedback appended, rides the recorded call into the derived context, so Ctrl+D shows what the model was actually told and every later turn still carries it (it used to survive exactly one round — history kept only the one-line cell), and option 2 remembers the scope so the identical request never asks twice (a command rule shows as '{prefix} *' and persists per project in permissions.json; a file prompt's option 2 is the switch to edit mode), and a tool IN FLIGHT no longer shows a blue bullet — it shows the permission prompt's grey, and in the live region that grey BREATHES dim→bright→dim once a second (Claude-Code's running dot): sampled across frames in a real terminal the running call's bullet takes several values while its '⎿ Waiting…' siblings' stay flat, no bullet is ever painted the old blue, and a call that resolves still lands green, and the conversation stays REACHABLE while a permission prompt is open (docs/permissions.md) — the prompt grows like any other region, the chat above it scrolling into the terminal's REAL scrollback, so the user can scroll up and re-read what the model said before answering (the covering modal used to hold the newest screenful in NO buffer — not on screen, not in scrollback — which read as 'terminal scroll is disabled while it asks', worst in kitty), and answering purge-rebuilds off term's modal-scrolled note so the box lands back FLUSH at the bottom with the conversation whole and each message committed exactly once, and a parallel batch's BACK-TO-BACK prompts survive their hardest timing (the dummy's 'parallel permission' turn: two gated Bash calls, the second request landing in the same frame gap as the first cell's commit — that cell commits above the still-open second prompt, visible at once) with the conversation visible through both prompts as real rows, the resolved cell committed exactly once, and the box back flush at the bottom, and STAGGERED back-to-back prompts of wildly different heights (the dummy's 'staggered permission' turn: a screen-tall body-capped Write answered into a one-line one) keep the STILL-OPEN prompt flush at the screen bottom — the loop purge-rebuilds the pinned modal region the moment its next frame would seat short of the bottom (ui::modal_needs_rebuild over term's painted-bottom note), so no band of blank rows ever shows under an open prompt (the reported empty-newlines-under-the-prompt bug) and the resolved tall cell stays visible above it, committed exactly once, and a RESIZE while a prompt is open no longer strands the box after the answer — the resize's purge reseats the prompt one-way, so the close purge-rebuilds the same way, landing the box flush at the bottom with each message committed exactly once (it used to float above the rows the collapsed prompt vacated), and a permission request that arrives UNDER the Ctrl+O overlay closes flush too — the overlay return seats the open prompt over the restored screen (its growth and the queued commits' flush are one-way moves like the resize's), so the close purge-rebuilds off the same note: after Ctrl+O → request lands → return → answer, the box is back flush at the bottom with the message committed exactly once (it used to strand above a band of blank rows, the 'newlines at the bottom, but only when Ctrl+O was opened first' bug), and the CLI session flags work end to end (docs/cli.md) — quitting a session that recorded a conversation prints 'Resume this session with: {bin} --resume {id}' below the restored terminal (the id naming the rollout file; an empty session prints no hint), '--continue' relaunches straight into the newest conversation recorded in this cwd (repainted inline under the banner, no picker, appending to the SAME rollout file), '--resume {id}' does the same by id, bare '--resume' boots into the session picker as the first screen, and '--continue' with nothing to continue fails fast on stderr with exit 1 and no TUI, and the AUTO and MASTER permission modes work end to end (docs/permissions.md) — Shift+Tab cycles manual → edit → auto → master (the footer's right-edge segment and the confirming toast tracking each step, the mode persisting per project in permissions.json), in auto mode a bash command is decided by the auto mode classifier instead of the user (offline: the deterministic heuristic; live: a silent LLM check on the session's provider) — the safe listing runs with a dim '⎿ Allowed by auto mode classifier' row appended to its finished cell while the dangerous delete resolves red 'Denied by auto mode classifier' with its Reason line, no prompt ever opening — and in master mode everything runs silently: no prompt, no classifier, no notes, and /compact NEVER reaches the network while the session is running the DUMMY (docs/compact.md) — a configured provider with no model selected leaves the footer on dummy_model_name, and the summarization turn stays on the dummy's canned script (the '● Context compacted' cell commits, no request is sent): gating the one-off backend on a *usable config* only proved a key had resolved, so it used to POST /chat/completions for 'dummy_model_name' and resolve the turn red, and the THINKING STREAM shows the model's reasoning (docs/thinking-stream.md) — while a phase runs it wears the tool cell's shape — a breathing '●' bullet over the chain-of-thought in the '⎿' gutter, tail-following the newest rows — and at the phase's end it COLLAPSES into one bullet-less committed 'Thought for {n} · {t} tokens (ctrl+o to expand)' line (nothing is happening any more, so what is left reads like 'Done for Ns'), the reasoning text itself never reaching scrollback and the cell never spliced into the paragraph that was streaming (invariant 4), while Ctrl+O expands the thought back and ALTER_ZERO_SHOW_THINKING=0 restores the pre-feature behaviour exactly: neither the live block nor the collapsed line, and the turn still settles to its Done summary, and the /settings MENU exposes the session's knobs (docs/settings.md) — the palette lists 'Open settings menu', the command opens the /model picker's inline frame listing every setting with its live value in an aligned column, an (n/total) counter, the highlighted row's description, and the 'Type to search · Enter/Space to change · Esc to cancel' hint; typing narrows the list, Enter cycles the highlighted value (Error retry 3 → 5) and confirms with a transient toast, Esc clears the query then closes back to the composer, and the change PERSISTS to ~/.alter-zero/settings.json as a diff from the defaults (only what the user touched is written) so a fresh process opens the menu already showing it, and the AskUserQuestion modal asks the user mid-turn (docs/ask.md) — an 'ask me some questions' prompt makes the dummy raise the three-question demo and block on the ask gate exactly as the real tool thread does: the modal replaces the live region framed by rules with the chip strip (the current chip highlighted on cyan, an answered question flipping its ☐ to ☒, the ✔ Submit tab at the end), the numbered options with dim descriptions and the auto-added Type something. and Chat about this rows, the composer draft typed before it stashed and handed back on close, the option menu hiding the hardware cursor like the permission prompt, a digit answering a single-select and advancing to the next tab, the multi-select page toggling [✔] checkboxes and confirming via its own unnumbered Submit row, the preview question rendering the side-by-side bordered panel with the focused option's content over the Notes line (n opens the notes field and the typed note survives Esc), the review page listing every → answer over ❯ 1. Submit answers / 2. Cancel, and Submit committing the green '● User answered Alter Zero's questions:' cell with each · question → answer row — the model reading the answers JSON, and the TASK TOOLS' live checklist works end to end (docs/task-tools.md) — the 'todo' demo drives a real TaskStore through create → link → work → complete: the ⎿ ◻/◼/✔ rows render directly under the status line while the turn runs (a blocked task naming its open blockers '› blocked by #1'), the spinner wears the in-progress task's activeForm instead of a stock verb, NOT ONE task call commits a tool cell to the conversation (the narration bullets flow around the hidden calls), while at rest the standalone '3 tasks (1 done, 1 in progress, 1 open)' count line takes over above the composer with the remaining rows under it — gutter-less, since there is no spinner left to hang from — and the Ctrl+O transcript keeps every call's full record ('● TaskUpdate(#1 → completed)' over the executor's 'Updated task #1 status') — while a FINISHED checklist bows out with the turn that finished it: the all-✔ closure shows to that turn's end, nothing shows at rest, and the next turn's spinner comes up clean because the retired list is deleted for good rather than hidden (the reported stale-list bug), and checkpoints REFUSE every directory that is not a project and SAY SO (docs/checkpoint.md) — the same 'alter-zero takes seconds to boot' bug from its three other sides: launching in a shared scratch parent (/tmp — every program's junk, measured 9.7 s to first frame on a 235 MB tree) or in alter-zero's OWN state dir (where the store lives inside the work tree, so each snapshot hashed the previous ones back in — the reported 2 GB .alter-zero) never even creates the store, and a tree past the cost budget is caught by the pre-flight probe — which enumerates what 'git add -A' would stage with 'git ls-files', honouring .gitignore, instead of hashing it (~800× cheaper) — so no snapshot is taken; each raises a one-row 'Checkpoints off — {reason}' toast rather than going quiet, while an ordinary in-budget project still snapshots with no toast at all, and a checkpoints root pointed INSIDE a project costs that project nothing — the store appends an anchored, metacharacter-escaped info/exclude line for itself, so across two launches the tracked set is app.py alone and never the store's own objects (without it every 'git add -A' re-stages the previous snapshots and the tree compounds 59→130→269→528 files over four turns, which is how a .alter-zero reaches 2 GB), and LIFECYCLE HOOKS run the user's own commands inside the tool loop (docs/hooks.md) — a PreToolUse hook's refusal resolves the call as a red '⎿ Blocked by hook: {reason}' cell with no output of its own (the command never ran) while the longer stop-and-wait instruction the MODEL reads stays off the screen, a PostToolUse hook leaves its dim '⎿ Context added by hook' provenance row under the output of the call that did run, and the Ctrl+O transcript keeps the refusal — while the loop-path events now fire too: a Stop hook's continuation feedback stays cell-less inline with the Ctrl+O transcript recording it under '● Stop hook', and a UserPromptSubmit hook's block rolls the submission back out of scrollback (exactly the composer's restored copy of the text remains) under the red reason-only notice — and the censorship holds on disk: the blocked prompt is erased from the rollout too (the recorder keys its rewrite on history_generation, not length), so a --continue resumes to the notice with the censored text nowhere in the transcript or the file, and the read-only /hooks MENU browses the configured lifecycle hooks (docs/hooks-menu.md) — Claude Code's /hooks ported whole: the palette runs it into the inline framed events list ('Hooks' over '{N} hooks configured', the ℹ read-only banner, all eleven events with their counts and summaries in an aligned column, a five-row selection-centered window wearing ↑/↓ overflow markers), Enter drills into an event's matchers ('[User] Bash' over its hook count), a matcher's hooks ('[command] {cmd}' over 'User Settings'), and one hook's read-only details — the aligned Event/Matcher/Type/Source field block, the REAL command word-wrapped whole in a rounded dim box, and the edit-hooks.json direction — while Esc walks back one level at a time and closes from the top, the composer and footer restored, and the SKILL tool loads an authored SKILL.md (docs/skills.md) — the whole visible surface is '● Skill(dataviz)' over one green '⎿  Successfully loaded skill' row while the model reads the skill's entire body: none of that body reaches the inline conversation, Ctrl+D shows it in the derived context (it is what was actually sent — the ToolOutcome::context split the ask tool already had), while Ctrl+O keeps just the cell — the two-text split's rule everywhere here, the transcript being what happened and Ctrl+D what was sent, and the /skills MENU enables or disables each skill individually (docs/skills.md) — the /settings menu's twin, listed in the palette, opening on the same framed rows with each skill's name over an enabled/disabled value column, its own description under the list and the 'Enter/Space to enable/disable' hint; Enter flips the highlighted skill (its value column and a confirming toast both moving, its siblings untouched), the change persists to skills.json under this project's path, and a SECOND process against the same config home opens the menu already showing it off, and skills are RE-WALKED at every turn start so one planted MID-SESSION needs no restart — booted against an empty skills dir the menu opens on its where-would-one-go placeholder, and after a SKILL.md is planted and one turn taken the very next Ctrl+D already carries the <system-reminder> listing that names it (availability is re-derived before the listing, or it would ship a turn late) and /skills lists it enabled, and the \$ SKILL PICKER completes mentions in the composer (docs/skill-mentions.md) — codex's \$ mentions as a fourth band: a bare \$ lists every discovered skill with its description in aligned columns, typing narrows on the name, Tab completes '\$alpha-skill ' in place (the sigil kept — it is what marks the mention in the submitted text) with the footer back once the band closes, and submitting a message that carries a mention plays the skill load end to end (offline the dummy's skills demo answers it — the '● Skill(dataviz)' cell over 'Successfully loaded skill', never the body — exactly as a live model does via the Skill tool description's guidance, verified in tests/live_openrouter.rs), and the /mcp MANAGER walks the MCP servers end to end (docs/mcp.md) — the palette runs it into the grouped server list (scope headings naming each config file, the startup connect resolving a scripted stdio server's row to 'fixture · ✔ connected · 1 tool'), Enter opens the server detail (Status/Command/Config location/Capabilities facts over the state's numbered actions), its View tools the tools list, and Enter again the tool detail naming the full wire name the model calls ('Full name: mcp__fixture__echo_text') with the parameter listing derived from the input schema, while Esc walks back one page at a time and closes from the list with the composer and footer restored, and commits made while an alternate-screen overlay is up QUEUE on the viewport and the return FLUSHES them (invariant 4) — a turn that finishes entirely under Ctrl+O keeps its every row (the user bubble, the reply, the Read/Edit/Bash cells, the Done summary) in the terminal's screen+scrollback (the retired window rebuild re-emitted at most one screenful, silently dropping the rest — the reported 'cells hidden until a resize' / 'cannot scroll back to the reply' hole), three mid-stream overlay round-trips plus a resize under the overlay leave every committed row in the terminal exactly once, and the Ctrl+D context view serves its window from a signature-keyed cache so scrolling it mid-turn costs O(viewport) instead of rebuilding the whole derived context twice per animation frame (the 'Ctrl+D freezes the TUI' report), and the mcp CLI SUBCOMMAND installs servers from the command line (docs/mcp-cli.md) — 'mcp add fixture -- sh server.sh' writes the user mcp.json before any TUI exists (exit 0, the Added line over the File: path), 'mcp list' reports it, a duplicate add exits 1 naming the 'mcp remove' escape hatch, a grammar error exits 2 with the mcp usage as its trailer, and a TUI booted against that same file resolves /mcp's row to 'fixture · ✔ connected · 1 tool' — the file the CLI writes is the file the session reads, and the terminal editing shortcuts land (Shift+Tab cycles the permission mode inside and outside a prompt, and the readline motion/kill keys edit the composer), and the two READ-ONLY OVERLAYS stay reachable from an open permission prompt (Ctrl+O shows the transcript with the asked-about call still Waiting on it, Ctrl+D the raw context, the overlay's Esc offered as a quit rather than a backtrack — and taken as one, landing back on the same prompt — while both round trips leave screen and scrollback byte-identical at all three seats: a floating region, one flush at the screen bottom, and one flush whose page flows into scrollback, the prompt still resolving afterwards), and the SESSION TEMP TREE is laid out under one per-user, per-session root (docs/scratchpad.md) — the agent's scratchpad/ created before the first frame at {tmp}/alter-zero-{uid}/{session}/scratchpad, and a backgrounded command's interim {id}.output teed into the tasks/ leaf beside it, and an alternate-screen overlay is SILENT on a page that has not changed — measured with tmux pipe-pane, the Ctrl+O transcript and the Ctrl+D context view write ZERO bytes over two seconds of an active turn (the inline control writes throughout), so a mouse selection over them survives and their text can be copied WHILE a turn streams instead of only after it finishes, while the page still repaints the moment its own content changes, and the BUILT-IN AGENT DEFINITIONS are seeded into the user config home as real editable files (docs/subagents.md) — general-purpose.md and explore.md written on the first launch with the comments that teach the tools:/model: keys, an edited one never clobbered by a later launch, and a deleted default re-created, since the agent schema's default type has to resolve — while a definition that will not parse raises a startup toast naming the file and the reason, and /login ASKS HOW YOU SIGN IN before it asks for anything else (docs/copilot.md) — the method root offering Use a subscription / Use an API key, the subscription list carrying GitHub Copilot with its own description and the API-key list carrying only the providers you paste a key for, Esc stepping BACK from either list rather than closing the flow, and only the root's Esc closing it"
fi
exit "$status"
