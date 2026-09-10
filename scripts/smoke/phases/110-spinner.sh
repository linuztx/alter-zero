#!/usr/bin/env bash
# Phase 110 — the `/spinner` picker

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the `/spinner` picker (docs/spinner.md). The `/mascot`
# picker's frame over the status line's spinner styles, LIVE: every row wears
# its own spinner and the highlighted style previews as a whole status line,
# both ticking with NO turn running (the open picker re-arms the animation
# chain); Enter persists to {config}/spinner.json and confirms with a toast;
# the very next turn's status line opens with the new style; and a second
# process against the same config home LAUNCHES with it. The startup delay is
# long so the status line can be captured mid pre-stream pause (Phase 20's
# trick).
S110="${S}_spinner"
SPIN_CFG="$(mktemp -d)"
APP_SPINNER="env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$SPIN_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_SKILLS_DIR=$SMOKE_SKILLS ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=2000 $BIN"
tmux new-session -d -s "$S110" -x 90 -y 30 "$APP_SPINNER"
sleep 0.8
tmux send-keys -t "$S110" -l "/spinner"
sleep 0.4
spin_palette="$(tmux capture-pane -t "$S110" -p)"
echo "==== Phase 110: the palette filtered to /spinner ===="
printf '%s\n' "$spin_palette"
expect_has "$spin_palette" -F "Choose the status spinner style" "/spinner is missing from the slash-command palette"
tmux send-keys -t "$S110" Enter
sleep 0.5
spin_open="$(tmux capture-pane -t "$S110" -p)"
echo "==== Phase 110: the picker open (comet highlighted, every row live) ===="
printf '%s\n' "$spin_open"
for expect in "→ comet" "gravity" "wave" "sparkle" "dots" "blocks" "pulse" "bars" "line" "(1/9)" \
	"Working…" "esc to interrupt" "A comet sweeping between two dim walls" \
	"Type to search · Enter to choose · Esc to cancel"; do
	expect_has "$spin_open" -F "$expect" "the open picker is missing '$expect'"
done
# The comet's frame — a `(` wall, the `●` head, a `)` wall — shows twice: on
# its own row and in the preview line.
if [ "$(printf '%s\n' "$spin_open" | grep -c '(.*●.*)')" -lt 2 ]; then
	fail "expected the comet on its row and in the preview"
fi
# The two braille tracks (docs/spinner.md). Their rows are LIVE — the ball
# has moved on from its opening frame by the time the pane is captured — so
# the checks are structural: eight braille cells each (U+2800–U+28FF, the
# bytes E2 A0..A3 80..BF — counted bytewise under LC_ALL=C, since a
# multibyte range in a bracket expression is not portable across locales),
# and the gravity track's bare floor `⣀` under at least six of them (the
# 2×2-dot ball covers at most two cells).
braille_cells() {
	printf '%s' "$1" | LC_ALL=C grep -oE $'\xe2[\xa0-\xa3][\x80-\xbf]' | wc -l
}
spin_gravity_row="$(printf '%s\n' "$spin_open" | grep -E "^ *gravity ")"
spin_wave_row="$(printf '%s\n' "$spin_open" | grep -E "^ *wave ")"
if [ "$(braille_cells "$spin_gravity_row")" -ne 8 ]; then
	fail "the gravity row is not an eight-cell braille track: '$spin_gravity_row'"
fi
if [ "$(printf '%s' "$spin_gravity_row" | grep -oF "⣀" | wc -l)" -lt 6 ]; then
	fail "the gravity track's floor is missing under the ball: '$spin_gravity_row'"
fi
if [ "$(braille_cells "$spin_wave_row")" -ne 8 ]; then
	fail "the wave row is not an eight-cell braille track: '$spin_wave_row'"
fi
# The page is LIVE with no turn running: the preview line must have moved
# between two captures 0.3 s apart (the comet steps a frame every 80 ms).
spin_preview_a="$(printf '%s\n' "$spin_open" | grep -F "Working…")"
sleep 0.3
spin_preview_b="$(tmux capture-pane -t "$S110" -p | grep -F "Working…")"
echo "==== Phase 110: the preview line 0.3 s apart ===="
printf '%s\n%s\n' "$spin_preview_a" "$spin_preview_b"
if [ "$spin_preview_a" = "$spin_preview_b" ]; then
	fail "the preview did not animate with no turn running"
fi
# ↓ to gravity: the counter, the preview and the description follow the
# selection — the preview line now opens with the braille track, not the
# comet's wall.
tmux send-keys -t "$S110" Down
sleep 0.4
spin_gravity="$(tmux capture-pane -t "$S110" -p)"
echo "==== Phase 110: ↓ previews gravity ===="
printf '%s\n' "$spin_gravity"
expect_has "$spin_gravity" -F "(2/9)" "the counter did not follow the selection"
expect_has "$spin_gravity" -F "A ball hopping along the track" "the description did not follow the selection"
if printf '%s\n' "$spin_gravity" | grep -F "Working…" | grep -q '(.*●.*) Working'; then
	fail "the preview still wears the comet after ↓"
fi
spin_gravity_preview="$(printf '%s\n' "$spin_gravity" | grep -F "Working…")"
if [ "$(braille_cells "${spin_gravity_preview%% Working*}")" -ne 8 ]; then
	fail "the preview does not open with the eight-cell braille track: '$spin_gravity_preview'"
fi
# Type-to-search matches descriptions too: `braille` narrows to dots; Enter
# switches the style.
tmux send-keys -t "$S110" -l "braille"
sleep 0.3
spin_filtered="$(tmux capture-pane -t "$S110" -p)"
expect_has "$spin_filtered" -F "(1/1)" "'braille' did not narrow the list to the dots style"
tmux send-keys -t "$S110" Enter
sleep 0.6
spin_after="$(tmux capture-pane -t "$S110" -p)"
echo "==== Phase 110: after Enter — the toast ===="
printf '%s\n' "$spin_after"
expect_has "$spin_after" -F "Spinner: dots" "the switch was not confirmed with a toast"
if ! grep -qF '"spinner": "dots"' "$SPIN_CFG/spinner.json" 2>/dev/null; then
	fail "spinner.json was not written (or holds the wrong style)"
fi
# The very next turn's status line opens with the new style: capture mid
# pre-stream pause (the 2 s startup delay is still running), where the line
# shows a braille glyph before the verb and no comet wall.
submit "$S110" "$USER_MSG"
sleep 0.9
spin_status="$(tmux capture-pane -t "$S110" -p | grep -F "esc to interrupt")"
echo "==== Phase 110: the status line mid-pause wears dots ===="
printf '%s\n' "$spin_status"
expect_has "$spin_status" -E '⠋|⠙|⠹|⠸|⠼|⠴|⠦|⠧|⠇|⠏' "the status line did not open with a braille glyph"
expect_lacks "$spin_status" '(.*●.*)' "the status line still opens with the comet"
wait_for 15 "$S110" -S -60 -- -F "$SETTLED_REPLY" # let the turn settle before quitting
submit "$S110" "/quit"
sleep 0.6
tmux kill-session -t "$S110" 2>/dev/null
# The persistence half: a fresh process against the same config home opens
# the picker seated on dots, wearing the ✓ (the bootstrap seed).
tmux new-session -d -s "$S110" -x 90 -y 30 "$APP_SPINNER"
sleep 0.8
tmux send-keys -t "$S110" -l "/spinner"
sleep 0.3
tmux send-keys -t "$S110" Enter
sleep 0.5
spin_relaunch="$(tmux capture-pane -t "$S110" -p)"
echo "==== Phase 110: a fresh launch keeps the saved style ===="
printf '%s\n' "$spin_relaunch"
expect_has "$spin_relaunch" -F "(5/9)" "the saved style did not seat the highlight on dots after a relaunch"
if ! printf '%s\n' "$spin_relaunch" | grep -F "→ dots" | grep -qF "✓"; then
	fail "the saved style does not wear the ✓ after a relaunch"
fi
tmux kill-session -t "$S110" 2>/dev/null
rm -rf "$SPIN_CFG"
