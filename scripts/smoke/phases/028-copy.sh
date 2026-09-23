#!/usr/bin/env bash
# Phase 28 — /copy copies the last assistant response to the clipboard

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# /copy copies the last assistant response to the clipboard
# (docs/copy.md). Headless here — the display variables are unset for the whole
# suite, so this holds on a developer's desktop as well as on CI — so arboard
# has no clipboard server and the OSC 52 fallback fires, which tmux
# (set-clipboard on, on the suite's own server) captures into its paste buffer,
# so `show-buffer` reads it back. With a display reaching the app the native
# path wins instead: nothing lands in tmux's buffer and the copy goes to the
# user's real clipboard. First an empty conversation: /copy
# reports "No agent response to copy". Then after a reply finishes, /copy writes
# the reply's tail to the clipboard and confirms "Copied last message to
# clipboard".
S25="${S}_copy"
tmux new-session -d -s "$S25" -x 80 -y 24 "$APP"
# tmux must capture OSC 52 from the app into its own buffer (default is
# `external`: forward-only, not stored) — `on` stores it so show-buffer sees it.
tmux set-option -g set-clipboard on
sleep 0.4
# Empty conversation: nothing to copy → "No agent response to copy", an info
# toast like /export's "Nothing to export" (nothing failed — docs/toast.md).
tmux send-keys -t "$S25" -l "/copy"
sleep 0.3
tmux send-keys -t "$S25" Enter
copy_empty="$(wait_pane 3 "$S25" -S -20 -- -F "No agent response to copy")" # up to ~3s
copy_empty_styled="$(pane "$S25" -e)"
echo "==== captured pane (/copy with nothing to copy) ===="
printf '%s\n' "$copy_empty"
# Now send a message and let the dummy reply finish (its closing hand-off
# committed AND the screen settled, so the final segment is in history).
submit "$S25" "hello there"
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
copy_ok="$(wait_pane 3 "$S25" -S -20 -- -F "Copied last message to clipboard")" # up to ~3s
copy_clip="$(tmux show-buffer 2>/dev/null)"
echo "==== captured pane (after /copy) ===="
printf '%s\n' "$copy_ok"
echo "==== tmux clipboard buffer (the OSC 52 fallback landed here) ===="
printf '%s\n' "$copy_clip"
tmux kill-session -t "$S25" 2>/dev/null

# Phase 28: /copy reports the empty case, confirms the copy, and the OSC 52
# fallback actually reached the clipboard — tmux's buffer holds the reply tail
# (docs/copy.md).
expect_has "$copy_empty" -F "No agent response to copy" "/copy with no assistant message did not show 'No agent response to copy'"
# Mocha is the fixture's default theme: an info toast wears its dim (#7f849c),
# the footer's colour — Phase 120's check for /diff's info toast.
if ! printf '%s\n' "$copy_empty_styled" | grep -F 'No agent response to copy' | grep -qF '38;2;127;132;156'; then
	fail "/copy with nothing to copy is not styled as a dim info toast"
fi
expect_has "$copy_ok" -F "Copied last message to clipboard" "/copy did not confirm with 'Copied last message to clipboard'"
expect_has "$copy_clip" -F "$SETTLED_REPLY" "/copy's OSC 52 fallback did not put the reply on the clipboard (tmux buffer missing the reply tail)"
