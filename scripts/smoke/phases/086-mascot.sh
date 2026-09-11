#!/usr/bin/env bash
# Phase 86 — the `/mascot` picker

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the `/mascot` picker (docs/mascot.md). The `/settings`
# family's frame over the mascot catalog with a LIVE banner preview: the page
# previews the highlighted mascot through the header's own builder, Enter
# switches the startup banner in place (the selection purge-rebuilds, so the
# chrome at the top of scrollback redraws at once), the switch is confirmed
# with a toast and persisted to {config}/mascot.json — and a second process
# against the same config home must LAUNCH with the switched mascot.
S86="${S}_mascot"
MASC_CFG="$(mktemp -d)"
APP_MASCOT="env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$MASC_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_SKILLS_DIR=$SMOKE_SKILLS ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux new-session -d -s "$S86" -x 80 -y 30 "$APP_MASCOT"
sleep 0.8
mascot_boot="$(tmux capture-pane -t "$S86" -p)"
echo "==== Phase 86: the startup banner (default crest) ===="
printf '%s\n' "$mascot_boot"
# The default banner: crest's crown row beside the bold title row.
expect_has "$mascot_boot" -F "▙▄▙▄▟▄▟" "the default crest mascot is missing from the startup banner"
expect_has "$mascot_boot" -F "Alter Zero (v" "the banner's title row is missing"
tmux send-keys -t "$S86" -l "/mascot"
sleep 0.4
mascot_palette="$(tmux capture-pane -t "$S86" -p)"
echo "==== Phase 86: the palette filtered to /mascot ===="
printf '%s\n' "$mascot_palette"
expect_has "$mascot_palette" -F "Choose the banner mascot" "/mascot is missing from the slash-command palette"
tmux send-keys -t "$S86" Enter
sleep 0.5
mascot_open="$(tmux capture-pane -t "$S86" -p)"
echo "==== Phase 86: the picker open (crest highlighted + previewed) ===="
printf '%s\n' "$mascot_open"
for expect in "→ crest ✓" "sprout" "gem" "(1/6)" \
	"A crested hatchling flaring its frill" \
	"Type to search · Enter to choose · Esc to cancel"; do
	expect_has "$mascot_open" -F "$expect" "the open picker is missing '$expect'"
done
# ↓ to bloom: the preview follows the selection live (bloom's petal row shows,
# crest's crown row leaves the preview slot — the startup banner above keeps
# its own crest, so the check is scoped to the picker's preview rows).
tmux send-keys -t "$S86" Down
sleep 0.4
mascot_bloom="$(tmux capture-pane -t "$S86" -p)"
echo "==== Phase 86: ↓ previews bloom ===="
printf '%s\n' "$mascot_bloom"
expect_has "$mascot_bloom" -F "▀█▄███▄█▀" "moving the selection did not preview bloom's art"
expect_has "$mascot_bloom" -F "(2/6)" "the counter did not follow the selection"
# Type-to-search narrows to sprout; Enter switches the banner.
tmux send-keys -t "$S86" -l "spr"
sleep 0.3
tmux send-keys -t "$S86" Enter
sleep 0.8
mascot_after="$(tmux capture-pane -t "$S86" -p -S -40)"
echo "==== Phase 86: after Enter — the banner redrawn + the toast ===="
printf '%s\n' "$mascot_after"
expect_has "$mascot_after" -F "▝▛▛▀▜▜▘" "the banner did not redraw with sprout after Enter"
expect_lacks "$mascot_after" -F "▙▄▙▄▟▄▟" "the old crest banner survived the switch's purge rebuild"
expect_has "$mascot_after" -F "Mascot: sprout" "the switch was not confirmed with a toast"
if ! grep -qF '"mascot": "sprout"' "$MASC_CFG/mascot.json" 2>/dev/null; then
	fail "mascot.json was not written (or holds the wrong mascot)"
fi
submit "$S86" "/quit"
sleep 0.6
tmux kill-session -t "$S86" 2>/dev/null
# The persistence half: a fresh process against the same config home boots
# with sprout in the banner (the bootstrap seed, docs/mascot.md).
tmux new-session -d -s "$S86" -x 80 -y 30 "$APP_MASCOT"
sleep 0.8
mascot_relaunch="$(tmux capture-pane -t "$S86" -p)"
echo "==== Phase 86: a fresh launch keeps the saved mascot ===="
printf '%s\n' "$mascot_relaunch"
expect_has "$mascot_relaunch" -F "▝▛▛▀▜▜▘" "the saved mascot did not survive a relaunch"
tmux kill-session -t "$S86" 2>/dev/null
rm -rf "$MASC_CFG"
