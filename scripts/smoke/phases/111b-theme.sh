#!/usr/bin/env bash
# Phase 111b — the `/theme` picker

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the `/theme` picker (docs/theme.md). The `/spinner` picker's
# frame over the colour-theme catalog, each row wearing a swatch of its own
# palette and the highlighted theme previewed on REAL cells (a user bubble, an
# Edit diff cell, a reply with a code line); ↓ moves the counter, the preview
# and the description; a filtered Enter switches the theme with a toast, a
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
for expect in "→ mocha" "macchiato" "frappe" "latte" "onedark" "dracula" "nord" "gruvbox" "(1/11)" \
	"❯ Rename the greeting in greet.py" "● Edit(greet.py)" "Updated greet.py (+1 -1)" \
	'-    print("Hello, world")' '+    print(f"Hello, {name}")' "● Done — greet.py greets by name now:" \
	"Catppuccin Mocha — the darkest flavour, and the default" \
	"Type to search · Enter to choose · Esc to cancel"; do
	expect_has "$theme_open" -F -- "$expect" "the open picker is missing '$expect'"
done
# Every listed row wears its five-swatch palette sample.
if [ "$(printf '%s\n' "$theme_open" | grep -cE '^ *(→ )?[a-z]+ +●●●●●')" -lt 10 ]; then
	fail "the theme rows do not wear their swatches"
fi
# The mocha row is the only one wearing the ✓ (the session's theme).
if [ "$(printf '%s\n' "$theme_open" | grep -c '✓')" -ne 1 ] || ! printf '%s\n' "$theme_open" | grep -F "→ mocha" | grep -qF "✓"; then
	fail "the ✓ does not sit on the mocha row alone"
fi
# ↓ to macchiato: the counter and the description follow the selection.
tmux send-keys -t "$S111" Down
sleep 0.4
theme_down="$(tmux capture-pane -t "$S111" -p)"
echo "==== Phase 111: ↓ previews macchiato ===="
printf '%s\n' "$theme_down"
expect_has "$theme_down" -F "(2/11)" "the counter did not follow the selection"
expect_has "$theme_down" -F "Catppuccin Macchiato — the medium-dark flavour" "the description did not follow the selection"
# Type-to-search narrows to dracula; Enter switches the theme.
tmux send-keys -t "$S111" -l "drac"
sleep 0.3
theme_filtered="$(tmux capture-pane -t "$S111" -p)"
expect_has "$theme_filtered" -F "(1/1)" "'drac' did not narrow the list to dracula"
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
expect_has "$theme_relaunch" -F "(6/11)" "the saved theme did not seat the highlight on dracula after a relaunch"
if ! printf '%s\n' "$theme_relaunch" | grep -F "→ dracula" | grep -qF "✓"; then
	fail "the saved theme does not wear the ✓ after a relaunch"
fi
tmux kill-session -t "$S111" 2>/dev/null
rm -rf "$THEME_CFG"
