#!/usr/bin/env bash
# Phase 27 — Ctrl+V image paste

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Ctrl+V image paste (docs/image-paste.md). The happy path needs a
# real clipboard server (covered by the attach_image unit tests, which call it
# with a path directly — codex tests it the same way). Here — headless, with no
# clipboard, which the suite's `unset DISPLAY WAYLAND_DISPLAY XAUTHORITY` makes
# true on a desktop too — Ctrl+V must fail GRACEFULLY: a red "Failed to paste
# image" notice commits to scrollback, and the composer stays responsive
# afterwards (no hang, no crash). This exercises the key binding + the boundary
# error path. It is also every container and SSH session (docs/docker.md), so
# the notice has to say WHY — no display is named — and what works instead.
S24="${S}_imagepaste"
launch "$S24" 80 24
tmux send-keys -t "$S24" C-v
imgpaste="$(wait_pane 4 "$S24" -S -20 -- -F "read its path")" # refused before any clipboard is probed; the wait is for the notice's LAST words, so the capture holds it whole
echo "==== captured pane (Ctrl+V with no clipboard → graceful notice) ===="
printf '%s\n' "$imgpaste"
# Still responsive after the failed paste: typing into the composer still works.
tmux send-keys -t "$S24" -l "still alive"
sleep 0.3
imgalive="$(tmux capture-pane -t "$S24" -p)"
echo "==== captured pane (composer responsive after the failed paste) ===="
printf '%s\n' "$imgalive"
tmux kill-session -t "$S24" 2>/dev/null

# Phase 27: Ctrl+V with no clipboard fails gracefully — a red "Failed to paste
# image" notice — and the composer stays responsive afterwards (docs/image-paste.md).
expect_has "$imgpaste" -F "Failed to paste image" "Ctrl+V with no clipboard did not show a 'Failed to paste image' notice"
# The notice wraps at 80 columns, so each assertion is a phrase short enough
# to sit on one row wherever the wrap falls.
expect_has "$imgpaste" -F "no desktop clipboard" "a paste with no display must say there is no desktop clipboard, not report a failed X11 probe"
expect_has "$imgpaste" -F "WAYLAND_DISPLAY" "the refusal must name the variables, since forwarding one is the fix (docs/docker.md)"
expect_has "$imgpaste" -F "read its path" "the refusal must say what works instead: read the image by path"
expect_has "$imgalive" -F "still alive" "the composer was unresponsive after a failed image paste (Ctrl+V hung or crashed the app)"
