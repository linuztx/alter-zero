#!/usr/bin/env bash
# Phase 111b — the `/theme` picker

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the `/theme` picker (docs/theme.md). codex's theme picker in the
# family's frame: the colour-theme catalog in the /mcp list's shape (the `❯`
# on the selection, a `↓ N more below` row), each row wearing a swatch of its
# own palette, and the highlighted theme previewed on REAL cells (a user
# bubble, an Edit diff cell, a reply with a code line) under its title and
# subtitle — BESIDE the list at 90 columns, UNDER it once a resize narrows the
# terminal to 64, and left out for a compact three-row list once a resize makes
# the terminal too short for the whole page; ↓ moves the marker, the preview
# and the caption; a filtered
# Enter switches the theme with a toast, a
# {config}/theme.json write and a PURGE REBUILD that recolours what is already
# on screen — the banner's `/login` hint goes from Mocha's sky to Dracula's
# cyan, read off the raw SGR bytes (`capture-pane -e`); and a second process
# against the same config home LAUNCHES in the saved theme.
S111="${S}_theme"
THEME_CFG="$(mktemp -d)"
APP_THEME="env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$THEME_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_SKILLS_DIR=$SMOKE_SKILLS ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
# The accents as SGR truecolor parameters: Catppuccin Mocha's sky (#89DCEB,
# the default) and Dracula's cyan (#8BE9FD).
THEME_MOCHA_SGR="38;2;137;220;235"
THEME_DRACULA_SGR="38;2;139;233;253"
tmux new-session -d -s "$S111" -x 90 -y 34 "$APP_THEME"
sleep 0.8
theme_banner_raw="$(tmux capture-pane -t "$S111" -p -e)"
echo "==== Phase 111: the banner hint wears the default theme's accent ===="
if ! printf '%s\n' "$theme_banner_raw" | grep -F "/login" | grep -qF "$THEME_MOCHA_SGR"; then
	fail "the banner's /login hint is not in Mocha's sky accent at launch"
	printf '%s\n' "$theme_banner_raw" | grep -F "/login" | cat -v >&2
fi
tmux send-keys -t "$S111" -l "/theme"
sleep 0.4
theme_palette="$(tmux capture-pane -t "$S111" -p)"
echo "==== Phase 111: the palette filtered to /theme ===="
printf '%s\n' "$theme_palette"
expect_has "$theme_palette" -F "Choose the colour theme" "/theme is missing from the slash-command palette"
tmux send-keys -t "$S111" Enter
sleep 0.5
theme_open="$(tmux capture-pane -t "$S111" -p)"
echo "==== Phase 111: the picker open (mocha highlighted, the real-cell preview) ===="
printf '%s\n' "$theme_open"
for expect in "❯ mocha" "macchiato" "frappe" "latte" "onedark" "dracula" "nord" "gruvbox" "↓ 1 more below" \
	"❯ Rename the greeting in greet.py" "● Edit(greet.py)" "Updated greet.py (+1 -1)" \
	'-    print("Hello, world")' '+    print(f"Hello, {name}")' "● Done — greet.py greets by name now:" \
	"Catppuccin Mocha" "The darkest flavour, and the default" \
	"↑/↓ to navigate · Type to search · Enter to choose · Esc to cancel"; do
	expect_has "$theme_open" -F -- "$expect" "the open picker is missing '$expect'"
done
# Every listed row wears its five-swatch palette sample.
if [ "$(printf '%s\n' "$theme_open" | grep -cE '^ *(❯ )?[a-z]+ +●●●●●')" -lt 10 ]; then
	fail "the theme rows do not wear their swatches"
fi
# The mocha row is the only one wearing the ✓ (the session's theme).
if [ "$(printf '%s\n' "$theme_open" | grep -c '✓')" -ne 1 ] || ! printf '%s\n' "$theme_open" | grep -F "❯ mocha" | grep -qF "✓"; then
	fail "the ✓ does not sit on the mocha row alone"
fi
# At 90 columns the preview sits BESIDE the list: a theme row and the preview's
# Edit header share a terminal row.
if ! printf '%s\n' "$theme_open" | grep -F "●●●●●" | grep -qF "● Edit(greet.py)"; then
	fail "the preview is not beside the list at 90 columns"
fi
# ↓ to macchiato: the marker and the caption follow the selection.
tmux send-keys -t "$S111" Down
sleep 0.4
theme_down="$(tmux capture-pane -t "$S111" -p)"
echo "==== Phase 111: ↓ previews macchiato ===="
printf '%s\n' "$theme_down"
expect_has "$theme_down" -F "❯ macchiato" "the marker did not follow the selection"
expect_has "$theme_down" -F "Catppuccin Macchiato" "the caption's title did not follow the selection"
expect_has "$theme_down" -F "The medium-dark flavour" "the caption's subtitle did not follow the selection"
# A resize to 64 columns re-lays the open picker: the preview STACKS under the
# list (no row carries both), its caption on rows of their own; back at 90
# it sits beside the list again.
tmux resize-window -t "$S111" -x 64 -y 40
sleep 0.6
theme_narrow="$(tmux capture-pane -t "$S111" -p)"
echo "==== Phase 111: 64 columns stack the preview under the list ===="
printf '%s\n' "$theme_narrow"
expect_has "$theme_narrow" -F "❯ macchiato" "the picker did not survive the resize"
expect_has "$theme_narrow" -F "● Edit(greet.py)" "the stacked page lost its preview"
if printf '%s\n' "$theme_narrow" | grep -F "●●●●●" | grep -qF "● Edit(greet.py)"; then
	fail "the preview stayed beside the list at 64 columns"
fi
expect_has "$theme_narrow" -E '^  Catppuccin Macchiato *$' "the stacked caption's title is not on a row of its own"
# A terminal too short for the whole page gets the compact one: the search
# line, three theme rows and the hint stay on screen while the preview yields —
# where bottom-anchoring the whole page kept only the preview and flowed the
# theme names into scrollback.
tmux resize-window -t "$S111" -x 90 -y 14
sleep 0.6
theme_short="$(tmux capture-pane -t "$S111" -p)"
echo "==== Phase 111: a 14-row terminal keeps the theme choices on screen ===="
printf '%s\n' "$theme_short"
for expect in "❯ macchiato" "mocha" "frappe" "↓ 8 more below" "↑/↓ to navigate · Type to search"; do
	expect_has "$theme_short" -F -- "$expect" "the short page is missing '$expect'"
done
expect_has "$theme_short" -E '^  ❯ *$' "the short page lost its search line"
expect_lacks "$theme_short" -F "● Edit(greet.py)" "the short page still shows the preview"
if [ "$(printf '%s\n' "$theme_short" | grep -c '●●●●●')" -ne 3 ]; then
	fail "the short page does not show exactly three theme rows"
fi
tmux resize-window -t "$S111" -x 90 -y 34
sleep 0.6
theme_wide_again="$(tmux capture-pane -t "$S111" -p)"
if ! printf '%s\n' "$theme_wide_again" | grep -F "●●●●●" | grep -qF "● Edit(greet.py)"; then
	fail "the preview did not return beside the list back at 90 columns"
	printf '%s\n' "$theme_wide_again" >&2
fi
# Type-to-search narrows to dracula; Enter switches the theme.
tmux send-keys -t "$S111" -l "drac"
sleep 0.3
theme_filtered="$(tmux capture-pane -t "$S111" -p)"
expect_has "$theme_filtered" -F "❯ dracula" "'drac' did not narrow the list to dracula"
expect_lacks "$theme_filtered" -E '^ *(❯ )?macchiato +●' "'drac' left other themes in the list"
tmux send-keys -t "$S111" Enter
sleep 0.8
theme_after="$(tmux capture-pane -t "$S111" -p)"
theme_after_raw="$(tmux capture-pane -t "$S111" -p -e)"
echo "==== Phase 111: after Enter — the toast, the recoloured banner ===="
printf '%s\n' "$theme_after"
expect_has "$theme_after" -F "Theme: dracula" "the switch was not confirmed with a toast"
if ! grep -qF '"theme": "dracula"' "$THEME_CFG/theme.json" 2>/dev/null; then
	fail "theme.json was not written (or holds the wrong theme)"
fi
# The purge rebuild recoloured the banner already on screen: its /login hint
# now carries Dracula's cyan and no longer Mocha's sky.
if ! printf '%s\n' "$theme_after_raw" | grep -F "/login" | grep -qF "$THEME_DRACULA_SGR"; then
	fail "the banner hint was not recoloured into Dracula's accent"
	printf '%s\n' "$theme_after_raw" | grep -F "/login" | cat -v >&2
fi
if printf '%s\n' "$theme_after_raw" | grep -F "/login" | grep -qF "$THEME_MOCHA_SGR"; then
	fail "the banner hint still carries Mocha's accent after the switch"
fi
expect_has "$theme_after" -F "dummy_model_name ·" "the composer and footer did not come back after the switch"
submit "$S111" "/quit"
sleep 0.6
tmux kill-session -t "$S111" 2>/dev/null
# The persistence half: a fresh process against the same config home LAUNCHES
# in Dracula (the banner hint is cyan before anything is pressed) and opens
# the picker seated on dracula, wearing the ✓ (the bootstrap seed).
tmux new-session -d -s "$S111" -x 90 -y 34 "$APP_THEME"
sleep 0.8
theme_relaunch_raw="$(tmux capture-pane -t "$S111" -p -e)"
if ! printf '%s\n' "$theme_relaunch_raw" | grep -F "/login" | grep -qF "$THEME_DRACULA_SGR"; then
	fail "a fresh launch did not paint the banner in the saved theme"
	printf '%s\n' "$theme_relaunch_raw" | grep -F "/login" | cat -v >&2
fi
tmux send-keys -t "$S111" -l "/theme"
sleep 0.3
tmux send-keys -t "$S111" Enter
sleep 0.5
theme_relaunch="$(tmux capture-pane -t "$S111" -p)"
echo "==== Phase 111: a fresh launch keeps the saved theme ===="
printf '%s\n' "$theme_relaunch"
expect_has "$theme_relaunch" -F "❯ dracula" "the saved theme did not seat the highlight on dracula after a relaunch"
if ! printf '%s\n' "$theme_relaunch" | grep -F "❯ dracula" | grep -qF "✓"; then
	fail "the saved theme does not wear the ✓ after a relaunch"
fi
tmux kill-session -t "$S111" 2>/dev/null
rm -rf "$THEME_CFG"
