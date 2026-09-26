#!/usr/bin/env bash
# Phase 45 — the startup header banner

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Phase 45: the startup header banner (docs/header.md). A fresh session shows the
# gradient mascot + title + cwd + hint at the top of scrollback. It is chrome
# (never in `history`), re-emitted on every full repaint — so it survives a
# resize (a width change purges scrollback and rebuilds from history, which the
# header is NOT part of, so it must be re-emitted) and re-shows after /clear (a
# fresh-start banner). The tier-independent title word is the marker (both the
# mascot tier and the narrow badge carry the literal "Alter Zero"); the
# borderless design adds no `─` rule / bare prompt / footer, so Phases 16/17
# stay green.
S_HEADER="${S}_header"
launch "$S_HEADER" 80 24
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
submit "$S_HEADER" "/clear"
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
submit "$S_HEADER" "hello there"
wait_for 20.1 "$S_HEADER" -F "$SUMMARY_TURN1" # up to ~20s — the full dummy turn, tools included
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
expect_has "$header_start" -F "$HEADER_MARK" "the startup header banner did not show ('$HEADER_MARK' missing)"
expect_has "$header_narrow" -F "$HEADER_MARK" "the header did not survive a width shrink to 50 (not re-emitted on the Purge rebuild)"
expect_has "$header_regrown" -F "$HEADER_MARK" "the header did not survive a resize round-trip"
expect_has "$header_cleared" -F "$HEADER_MARK" "the header did not re-show after /clear"
expect_has "$header_overlay" -F "$HEADER_MARK" "the Ctrl+O transcript does not open with the banner"
expect_has "$header_returned" -F "$HEADER_MARK" "the header vanished on the Ctrl+O return (the InPlace repaint dropped the banner)"
expect_has "$header_overlay_conv" -F "$HEADER_MARK" "the Ctrl+O transcript of a real conversation is missing the banner at its top"
if [ "${header_roundtrip_count:-0}" != "1" ]; then
	fail "after a Ctrl+O round trip over a real conversation the banner should appear exactly once (saw ${header_roundtrip_count:-0})"
fi
expect_has "$header_roundtrip" -F "Happy to help" "the conversation itself did not survive the Ctrl+O round trip beneath the banner"
