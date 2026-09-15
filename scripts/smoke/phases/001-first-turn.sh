#!/usr/bin/env bash
# Phase 1 — the first turn — reply + tools to scrollback, the growing box, the Ctrl+O view, the slash palette

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# ---- drive: Phase 1 ----
launch "$S" 80 24
submit "$S" "$USER_MSG"

# Poll for the streamed reply rather than sleeping a fixed amount.
pane="$(wait_pane 5 "$S" -S -60 -- -F "$EXPECT_REPLY")" # up to ~5s

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

expect_has "$pane" -F "❯ $USER_MSG" "user message line '❯ $USER_MSG' not echoed to scrollback"
expect_has "$pane" -F "$EXPECT_REPLY" "streamed reply '$EXPECT_REPLY' not found"
# While the reply streams, the live status line shows a running token count (the
# pane was captured mid-stream above, so text — and so tokens — is flowing).
expect_has "$pane" -F "tokens" "the live status line (token count) was not shown while streaming"
# The session-context footer ("{model} · {cwd}", docs/footer.md) sits under the
# box from startup — including mid-stream, when this pane was captured. The cwd
# half varies by environment, so assert on the model name + separator.
expect_has "$pane" -E "dummy_model_name · .*manual$" "the session footer ('dummy_model_name · … manual' — the mode flush at the right edge) is not shown under the input box"
# The cursor stays visible ON THE PROMPT ROW while the reply streams (probed
# live during Phase 1): the box keeps focus mid-turn so typing/queueing has a
# blinking cursor, codex-style. Before the fix the cursor was hidden
# (cursor_flag=0) for the whole turn.
if [ "$cursor_mid_flag" != "1" ]; then
	fail "the hardware cursor is hidden while the reply streams (cursor_flag=$cursor_mid_flag)"
fi
case "$cursor_mid_row" in
"❯"*) ;;
*)
	fail "mid-stream the cursor is not on the input's prompt row: '$cursor_mid_row'"
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
	fail "no blank gap row between the live status line and the input box (${status_gap:-status line missing})"
fi
# Phase 8: Esc mid-stream interrupts the turn, codex-style (docs/interrupt.md).
# The hint rides the live status line — Phase 1's pane was captured mid-stream.
expect_has "$pane" -F "esc to interrupt" "the live status line does not show the 'esc to interrupt' hint while streaming"
# ---- drive: Phase 2 ----
# the input box GROWS for a multi-line draft.
# Type two lines separated by Alt+Enter; the box must grow to show both, with the
# prompt on the first line and an indented continuation on the second.
tmux send-keys -t "$S" -l "AAA"
tmux send-keys -t "$S" M-Enter
tmux send-keys -t "$S" -l "BBB"
sleep 0.3
grown="$(tmux capture-pane -t "$S" -p)"
echo "==== captured pane (grown input box) ===="
printf '%s\n' "$grown"

expect_has "$grown" -F "❯ AAA" "first draft line '❯ AAA' not shown in the input box"
expect_has "$grown" -F "  BBB" "input box did not grow — indented continuation '  BBB' missing"
# ---- drive: Phase 3 ----
# Ctrl+O opens the full-screen tool-output view, Ctrl+O returns.
# The dummy interleaves tool calls in its reply (a parallel Bash batch, then a
# lone Read); wait until the Read has FINISHED — its committed inline peek shows
# `fn main(`, and only then is its full output in history — before opening the
# view, which must show each tool's FULL output (the Read tool's later lines are
# hidden in the collapsed inline view but shown here). See docs/parallel-tools.md.
full="$(wait_pane 12 "$S" -S -80 -- -F "fn main(")"
tmux send-keys -t "$S" C-o
sleep 0.4
# The pager opens pinned to the bottom (tail-following the live stream), so the
# user turn near the top has already scrolled off — jump Home, like Phase 29
# does, to check it's actually in the transcript. Poll rather than a fixed
# sleep (this file's own rule): the redraw races the still-streaming reply.
tmux send-keys -t "$S" Home
overlay="$(wait_pane 3 "$S" -F "$USER_MSG")" # up to ~3s
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

# "T R A N S C R I P T" is unique to the overlay's pager title row (it never
# appears in the conversation), so it's a clean marker for "open / closed".
expect_has "$overlay" -F "T R A N S C R I P T" "Ctrl+O did not open the tool-output view"
# Idle with a previous user message, Esc begins the backtrack preview — the
# closing hint must say so instead of promising a quit (docs/backtrack.md).
expect_has "$overlay" -F "esc to edit prev" "the overlay's closing hint does not tell the truth about Esc (expected 'esc to edit prev' while idle with a previous user message)"
expect_has "$overlay" -F "$USER_MSG" "tool-output view did not include the user/AI conversation"
expect_has "$overlay_deep" -F 'if __name__ == "__main__":' "tool-output view did not show the full (expanded) Read output"
if [ "${returned_banner_count:-0}" -gt 1 ]; then
	fail "the Ctrl+O return duplicated the header banner over a long conversation — expected at most 1 'Alter Zero' in screen+scrollback, got ${returned_banner_count:-0}"
fi
# Stamps in the tool view: only the USER message shows one — alone on its own
# right-aligned line, 12-hour hh:mm AM/PM, no seconds, no date.
expect_has "$overlay" -E '^ +(0[1-9]|1[0-2]):[0-5][0-9] (AM|PM)$' "the user message's right-aligned hh:mm AM/PM stamp line is missing from the tool view"
expect_lacks "$overlay" -E '[0-9]{4}-[0-9]{2}-[0-9]{2}|[0-9]{2}:[0-9]{2}:[0-9]{2}' "a dated/seconds timestamp is still shown in the tool view (stamps are hh:mm AM/PM, user messages only)"
expect_lacks "$returned" -F "T R A N S C R I P T" "Ctrl+O did not return to the conversation"
# ---- drive: Phase 4 ----
# the slash-command palette. Typing "/" opens the command list below
# the box, capped at 8 rows (/quit, the registry's last, starts off-window); ↑
# wraps the selection to the last row and the window scrolls /quit in
# (menu_window); running /help posts a system notice listing them.
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

# Typing "/" lists the commands below the box (their descriptions are unique to
# the open palette) — the first 8 only, MENU_MAX_ROWS being the cap; everything
# past that starts off-window and ↓ scrolls it in. The assertions below name
# commands by position-independent facts (the 8th is the window's last row,
# /quit is the registry's last entry) rather than by ordinal.
expect_has "$palette_open" -F "List the available commands" "typing '/' did not open the command palette (/help missing)"
expect_has "$palette_open" -F "Clear the conversation" "the command palette did not list /clear"
expect_has "$palette_open" -F "Toggle fast mode" "the command palette did not list /fast (the 8th command, the window's last row — docs/fast-mode.md)"
expect_lacks "$palette_open" -F "Add or update a provider API key" "the palette shows /login (the 9th command) in its first window — the 8-row cap is gone"
expect_lacks "$palette_open" -F "Exit the app" "the palette shows /quit (the registry's last command) in its first window — the 8-row cap is gone"
expect_has "$palette_scrolled" -F "Exit the app" "↓ to the last command did not scroll /quit into the palette window"
expect_lacks "$palette_scrolled" -F "List the available commands" "the scrolled palette still shows /help — the window did not move"
# The open palette DISPLACES the session footer (codex's popups take its row).
expect_lacks "$palette_open" -F "dummy_model_name" "the session footer is still shown while the palette is open (the band must displace it)"
expect_has "$help_ran" -F "Available commands:" "running /help did not post its system notice"
