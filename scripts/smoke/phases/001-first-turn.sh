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
# The session-context footer ("{model} · {cwd}", docs/ui/footer.md) sits under the
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
# Phase 8: Esc mid-stream interrupts the turn, codex-style (docs/input/interrupt.md).
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
# hidden in the collapsed inline view but shown here). See docs/tools/parallel-tools.md.
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
# commits beneath it and never rebuilds — docs/ui/header.md; Phase 81 pins the
# stronger property, that nothing an overlay-covered turn produced is lost).
returned_full="$(tmux capture-pane -t "$S" -p -S -80)"
returned_banner_count=$(printf '%s\n' "$returned_full" | grep -cF "Alter Zero")

# "T R A N S C R I P T" is unique to the overlay's pager title row (it never
# appears in the conversation), so it's a clean marker for "open / closed".
expect_has "$overlay" -F "T R A N S C R I P T" "Ctrl+O did not open the tool-output view"
# Idle with a previous user message, Esc begins the backtrack preview — the
# closing hint must say so instead of promising a quit (docs/input/backtrack.md).
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
# the box, five commands over a `(n/total)` counter (/quit, the registry's
# last, starts off-window); ↑ wraps the selection to the last row and the
# window scrolls /quit in; three more ↑ walk the highlight back to the band's
# middle row, where it stays while the list scrolls under it
# (centered_window_rows, the /model list's rule), so /quit scrolls back out
# below; Enter on the first row runs /clear.
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
# feature's new command to break (docs/state/settings.md's /settings, then
# docs/tools/hooks-menu.md's /hooks, then docs/tools/skills.md's /skills each broke the
# old counted walk; the over-press-↓-and-clamp idiom that replaced it died
# with the clamp — 40 ↓ now land at 40 % n). The five-command window follows
# the selection, so the top rows leave and /quit scrolls in — on the bottom
# row, the list having run out below it.
tmux send-keys -t "$S" Up
sleep 0.3
palette_scrolled="$(tmux capture-pane -t "$S" -p)"
echo "==== captured pane (slash palette scrolled to /quit) ===="
printf '%s\n' "$palette_scrolled"

# Three more ↑ walk the highlight up to the band's middle row (two to reach
# it, one to scroll), and from there the window scrolls with it, keeping the
# commands below it in view — so the tail (/quit) leaves at the bottom. The edge-pinned window this replaced kept
# the highlight on the bottom row the whole way, scrolling the list up with it
# and showing nothing below it — /quit leaves under that window too, so only
# the highlight's row tells the two apart. The highlighted row is the one the
# `→ ` marker opens (the selection also lights up cyan and bolds its name; the
# other rows open on the marker's two-space inset), so a plain capture shows
# it.
for _ in 1 2 3; do
	tmux send-keys -t "$S" Up
done
sleep 0.3
palette_recentered="$(tmux capture-pane -t "$S" -p)"
echo "==== captured pane (slash palette recentered, three up from /quit) ===="
printf '%s\n' "$palette_recentered"

# Back to the top the same way — three ↓ return to /quit and one more wraps
# to row 0 (the window follows the selection up again) so Enter runs /clear,
# not /quit.
for _ in 1 2 3 4; do
	tmux send-keys -t "$S" Down
done
sleep 0.3
palette_top="$(tmux capture-pane -t "$S" -p)"
echo "==== captured pane (slash palette wrapped back to the top) ===="
printf '%s\n' "$palette_top"
# The whole history, before the wipe: the first message is more than twenty
# rows into the scrollback by now, past what a `-S -20` capture reaches (the
# check that read that much passed whether or not anything was wiped), so the
# wipe check below reads all of it and first proves the message is there.
clear_before="$(tmux capture-pane -t "$S" -p -S -)"

# Run /clear (highlighted first) → wipes the conversation, scrollback and all.
tmux send-keys -t "$S" Enter
sleep 0.3
clear_ran="$(tmux capture-pane -t "$S" -p -S -)"
echo "==== captured pane (after running /clear, the whole history) ===="
printf '%s\n' "$clear_ran"

# Quit with Ctrl+C (empty composer) — the way out whatever the history holds:
# with user messages in it, idle Esc arms the Esc-Esc backtrack instead of
# quitting (docs/input/backtrack.md).
tmux send-keys -t "$S" C-c
sleep 0.2

# Typing "/" lists the commands below the box (their descriptions are unique to
# the open palette) — the first five only, MENU_MAX_COMMANDS being the cap;
# everything past that starts off-window and ↓ scrolls it in. The assertions
# below name commands by position-independent facts (the registry's first two
# entries, /quit is its last) rather than by ordinal — and the CAP itself is
# asserted by COUNTING the rows, not by naming whichever command happens to
# sit last in the window: this line once pinned "/login, the 8th command", and
# `/fast` (docs/providers/fast-mode.md) landing before it broke a check about the cap
# for a reason that had nothing to do with the cap. A palette row opens on
# the `→ ` marker or its two-space inset, then its slash, its name and two or
# more spaces before its description ($PALETTE_ENTRY) — a shape nothing else
# on screen has, where the bare opening also matches the reply's wrapped prose.
expect_has "$palette_open" -F "Clear the conversation" "typing '/' did not open the command palette (/clear missing)"
expect_has "$palette_open" -E '^→ /clear +Clear the conversation' "the highlighted /clear row does not open on the → marker"
expect_has "$palette_open" -F "Copy the last response to the clipboard" "the command palette did not list /copy"
palette_rows="$(printf '%s\n' "$palette_open" | grep -cE "$PALETTE_ENTRY" || true)"
if [ "$palette_rows" -ne 5 ]; then
	fail "the command palette shows $palette_rows commands, not the 5 MENU_MAX_COMMANDS caps it at"
fi
expect_lacks "$palette_open" -F "Exit the app" "the palette shows /quit (the registry's last command) in its first window — the five-command cap is gone"
# Under the commands, a dim `(n/total)` counter in the names' column: which
# match the highlight is on, of how many. Read back as "n total" so each
# capture can be checked against where its highlight is, whatever the
# registry's length: the first of them all on opening, the last after the
# wrap, three short of the last after walking back up.
palette_counter() { printf '%s\n' "$1" | sed -nE 's/^  \(([0-9]+)\/([0-9]+)\)$/\1 \2/p' | tail -1; }
read -r open_n open_total <<<"$(palette_counter "$palette_open")"
read -r scrolled_n scrolled_total <<<"$(palette_counter "$palette_scrolled")"
read -r recentered_n recentered_total <<<"$(palette_counter "$palette_recentered")"
if [ "${open_n:-}" != 1 ] || [ "${open_total:-0}" -le 5 ]; then
	fail "the open palette's counter reads (${open_n:-?}/${open_total:-?}), not (1/total) for a registry longer than its window"
fi
if [ "${scrolled_n:-}" != "${open_total:-}" ] || [ "${scrolled_total:-}" != "${open_total:-}" ]; then
	fail "the palette wrapped to /quit reads (${scrolled_n:-?}/${scrolled_total:-?}), not (${open_total:-?}/${open_total:-?})"
fi
if [ -z "${open_total:-}" ] || [ "${recentered_n:-}" != $((open_total - 3)) ]; then
	fail "three ↑ from /quit left the counter at (${recentered_n:-?}/${recentered_total:-?}), not ($((${open_total:-3} - 3))/${open_total:-?})"
fi
expect_has "$palette_scrolled" -F "Exit the app" "↓ to the last command did not scroll /quit into the palette window"
expect_lacks "$palette_scrolled" -F "Clear the conversation" "the scrolled palette still shows /clear — the window did not move"
# Walking back up from /quit recenters: the window still holds its five
# commands, the highlight rides the middle (3rd) one with commands below it,
# and /quit — three rows under the highlight — has scrolled out.
recentered_rows="$(printf '%s\n' "$palette_recentered" | grep -cE "$PALETTE_ENTRY" || true)"
if [ "$recentered_rows" -ne 5 ]; then
	fail "the recentered command palette shows $recentered_rows commands, not 5"
fi
highlight_row="$(printf '%s\n' "$palette_recentered" | grep -E "$PALETTE_ENTRY" | grep -n '^→ ' | cut -d: -f1)"
if [ "$highlight_row" != 3 ]; then
	fail "three ↑ from /quit left the palette highlight on row ${highlight_row:-none} of 5, not the middle (3rd) row with commands below it"
fi
expect_lacks "$palette_recentered" -F "Exit the app" "three ↑ from /quit left it in view — the window did not scroll with the highlight once it reached the middle row"
# The open palette DISPLACES the session footer (codex's popups take its row).
expect_lacks "$palette_open" -F "dummy_model_name" "the session footer is still shown while the palette is open (the band must displace it)"
# ↓ past /quit wrapped the highlight back to /clear, the counter with it — so
# the Enter below runs /clear and nothing else.
expect_has "$palette_top" -E '^→ /clear +Clear the conversation' "↓ past /quit did not wrap the palette highlight back to /clear"
read -r top_n top_total <<<"$(palette_counter "$palette_top")"
if [ "${top_n:-}" != 1 ] || [ "${top_total:-}" != "${open_total:-}" ]; then
	fail "the palette wrapped back to /clear reads (${top_n:-?}/${top_total:-?}), not (1/${open_total:-?})"
fi
expect_has "$clear_before" -F "❯ $USER_MSG" "the first message is not in the pane's history before /clear runs, so the wipe check below would prove nothing"
expect_lacks "$clear_ran" -F "❯ $USER_MSG" "Enter on the highlighted /clear did not wipe the conversation"
expect_has "$clear_ran" -F "dummy_model_name ·" "running /clear did not close the palette (the footer it displaces is not back)"
