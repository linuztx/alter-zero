#!/usr/bin/env bash
# Phase 122 — the `/export` page

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the `/export` page (docs/export.md). `/copy`'s sibling over the WHOLE
# conversation: a read-only two-row page — `❯ 1. Copy to clipboard` over
# `2. Save to file`, the highlighted row's description under the list, a
# key hint — that closes on the pick. Launched in a throwaway cwd so the
# file lands somewhere the phase can read. First an empty conversation:
# `/export` is a `Nothing to export` toast, never an empty file. Then a
# reply settles; `/export` opens the page; ↓ moves the `❯` to the file row
# and its description names the file's shape; Enter writes
# `conversation-YYYY-MM-DD-HHMMSS.txt` into the cwd — the toast names it,
# the file opens with the startup banner the terminal did and holds the
# user's message and the reply, no row carries trailing whitespace — and
# the composer comes back. A second
# `/export` picks the clipboard row: headless here (the suite's own unset
# of the display variables) arboard has no clipboard server, so the OSC 52
# fallback lands the same text in tmux's paste buffer (Phase 28's trick).
S122="${S}_export"
EXDIR="$(mktemp -d "$SMOKE_TMP/export.XXXXXX")"
launch -c "$EXDIR" "$S122" 90 40 "$APP_ABS"
# tmux must capture OSC 52 from the app into its own buffer (Phase 28).
tmux set-option -g set-clipboard on
tmux send-keys -t "$S122" -l "/export"
sleep 0.4
export_palette="$(tmux capture-pane -t "$S122" -p)"
echo "==== Phase 122: the palette filtered to /export ===="
printf '%s\n' "$export_palette"
expect_has "$export_palette" -F "Copy or save the conversation as plain text" "/export is missing from the slash-command palette"
# An empty conversation: a toast, no page, no file.
tmux send-keys -t "$S122" Enter
export_empty="$(wait_pane 3 "$S122" -F "Nothing to export")" # up to ~3s
echo "==== Phase 122: /export with nothing to export ===="
printf '%s\n' "$export_empty"
expect_has "$export_empty" -F "Nothing to export" "/export on an empty conversation did not toast 'Nothing to export'"
expect_lacks "$export_empty" -F "Export conversation" "/export opened its page over an empty conversation"
if ls "$EXDIR"/conversation-*.txt >/dev/null 2>&1; then
	fail "/export wrote a file for an empty conversation"
fi
# A reply, settled (its closing hand-off committed and the screen still).
submit "$S122" "hello there"
export_settled="$(wait_settled 15 "$S122" -F "$SETTLED_REPLY")"
echo "==== Phase 122: the reply settled ===="
printf '%s\n' "$export_settled"
expect_has "$export_settled" -F "$SETTLED_REPLY" "the dummy reply never settled"
# /export opens the page: the title, the blurb, both rows, the clipboard
# row's description, the hint; the composer's footer is off screen under it.
tmux send-keys -t "$S122" -l "/export"
sleep 0.3
tmux send-keys -t "$S122" Enter
export_open="$(wait_pane 3 "$S122" -F "Export conversation")" # up to ~3s
echo "==== Phase 122: the page open (clipboard highlighted) ===="
printf '%s\n' "$export_open"
for expect in "Export conversation" "as plain text" \
	"❯ 1. Copy to clipboard" "  2. Save to file" \
	"Copies the transcript to the system clipboard." \
	"↑↓ navigate  enter select  esc close"; do
	expect_has "$export_open" -F "$expect" "the open page is missing '$expect'"
done
expect_lacks "$export_open" -F "dummy_model_name ·" "the footer is still on screen under the page"
# A row is its label and nothing more (read as the whole trimmed row).
for label in "❯ 1. Copy to clipboard" "2. Save to file"; do
	if ! printf '%s\n' "$export_open" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//' | grep -qxF "$label"; then
		fail "the row '$label' carries something after the label"
	fi
done
# ↓ moves the ❯ to the file row; the description now names the file's
# shape and the directory it lands in.
tmux send-keys -t "$S122" Down
sleep 0.4
export_file_row="$(tmux capture-pane -t "$S122" -p)"
echo "==== Phase 122: ↓ highlights the file row ===="
printf '%s\n' "$export_file_row"
expect_has "$export_file_row" -F "❯ 2. Save to file" "↓ did not move the ❯ to the file row"
expect_lacks "$export_file_row" -F "❯ 1. Copy" "the clipboard row still wears the ❯ after ↓"
expect_has "$export_file_row" -F "Writes conversation-YYYY-MM-DD-HHMMSS.txt into" "the file row's description does not name the file's shape"
expect_lacks "$export_file_row" -F "Copies the transcript" "the clipboard row's description is still showing under the file row"
# Enter saves the file: the toast names it, the page closes and the
# composer comes back.
tmux send-keys -t "$S122" Enter
export_saved="$(wait_pane 3 "$S122" -E "Saved the conversation to conversation-[0-9]{4}-[0-9]{2}-[0-9]{2}-[0-9]{6}\.txt")" # up to ~3s
echo "==== Phase 122: after Enter on the file row — the toast ===="
printf '%s\n' "$export_saved"
expect_has "$export_saved" -E "Saved the conversation to conversation-[0-9]{4}-[0-9]{2}-[0-9]{2}-[0-9]{6}\.txt" "Enter on the file row did not confirm with a toast naming the file"
expect_lacks "$export_saved" -F "Export conversation" "the pick did not close the page"
expect_has "$export_saved" -F "dummy_model_name ·" "the composer and footer did not come back after the pick"
# The file is in the cwd, named as the toast said, opens with the startup
# banner exactly as the terminal did (its title row first, its hint row
# under it), and holds the conversation — the user's message and the
# reply's hand-off; no row carries trailing whitespace (a user bubble and
# the banner's rows are padded to the full width on screen).
export_file="$(ls "$EXDIR"/conversation-*.txt 2>/dev/null | head -1)"
echo "==== Phase 122: the exported file ($export_file) ===="
if [ -z "$export_file" ]; then
	fail "no conversation-*.txt landed in the cwd"
else
	cat "$export_file"
	export_name="$(basename "$export_file")"
	expect_has "$export_saved" -F "$export_name" "the toast named a different file than the one written"
	if ! printf '%s' "$export_name" | grep -qE '^conversation-[0-9]{4}-[0-9]{2}-[0-9]{2}-[0-9]{6}\.txt$'; then
		fail "the file name is not conversation-YYYY-MM-DD-HHMMSS.txt: $export_name"
	fi
	expect_file_has "$export_file" -F "❯ hello there" "the file does not hold the user's message"
	expect_file_has "$export_file" -F "$SETTLED_REPLY" "the file does not hold the reply"
	if [ -z "$(head -1 "$export_file" | tr -d '[:space:]')" ]; then
		fail "the file does not open on the banner's first row"
	fi
	# The title shares one of the mascot's art rows — which one depends on
	# how many metadata rows the banner has — so it is "within the opening
	# block", not "line 1".
	if ! head -3 "$export_file" | grep -qF "$HEADER_MARK (v"; then
		fail "the file does not open with the startup banner's title row"
	fi
	expect_file_has "$export_file" -F "/login   /model   /resume" "the file does not carry the banner's hint row"
	if [ "$(grep -nF "/login   /model   /resume" "$export_file" | head -1 | cut -d: -f1)" -ge "$(grep -nF "❯ hello there" "$export_file" | head -1 | cut -d: -f1)" ]; then
		fail "the banner does not sit above the conversation in the file"
	fi
	if grep -qE '[[:space:]]$' "$export_file"; then
		fail "a row of the file carries trailing whitespace"
	fi
	if [ "$(tail -c1 "$export_file" | od -An -c | tr -d ' ')" != '\n' ]; then
		fail "the file does not end in a newline"
	fi
fi
# A second /export, Enter on the clipboard row: the toast, the page closed,
# and the OSC 52 fallback lands the same text in tmux's buffer.
while tmux delete-buffer 2>/dev/null; do :; done
tmux send-keys -t "$S122" -l "/export"
sleep 0.3
tmux send-keys -t "$S122" Enter
wait_pane 3 "$S122" -F "Export conversation" >/dev/null || fail "the second /export did not open the page"
tmux send-keys -t "$S122" Enter
export_copied="$(wait_pane 3 "$S122" -F "Copied the conversation to clipboard")" # up to ~3s
echo "==== Phase 122: after Enter on the clipboard row — the toast ===="
printf '%s\n' "$export_copied"
expect_has "$export_copied" -F "Copied the conversation to clipboard" "Enter on the clipboard row did not confirm the copy"
expect_lacks "$export_copied" -F "Export conversation" "the clipboard pick did not close the page"
export_clip="$(tmux show-buffer 2>/dev/null)"
echo "==== Phase 122: tmux clipboard buffer (the OSC 52 fallback landed here) ===="
printf '%s\n' "$export_clip"
expect_has "$export_clip" -F "❯ hello there" "the clipboard does not hold the user's message"
expect_has "$export_clip" -F "$SETTLED_REPLY" "the clipboard does not hold the reply"
expect_has "$export_clip" -F "$HEADER_MARK (v" "the clipboard does not open with the startup banner"
if [ -n "$export_file" ] && [ "$export_clip" != "$(cat "$export_file")" ]; then
	fail "the clipboard and the file disagree about the conversation"
fi
# Esc closes an open page without picking anything: no third file.
tmux send-keys -t "$S122" -l "/export"
sleep 0.3
tmux send-keys -t "$S122" Enter
wait_pane 3 "$S122" -F "Export conversation" >/dev/null || fail "the third /export did not open the page"
tmux send-keys -t "$S122" Escape
sleep 0.4
export_closed="$(tmux capture-pane -t "$S122" -p)"
echo "==== Phase 122: after Esc ===="
printf '%s\n' "$export_closed"
expect_lacks "$export_closed" -F "Export conversation" "the page is still on screen after Esc"
expect_has "$export_closed" -F "dummy_model_name ·" "the composer and footer did not come back after Esc"
export_files="$(ls "$EXDIR"/conversation-*.txt 2>/dev/null | wc -l | tr -d ' ')"
expect_eq "$export_files" "1" "the cwd holds a different number of export files than the one pick wrote"
tmux kill-session -t "$S122" 2>/dev/null
