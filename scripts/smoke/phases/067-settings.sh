#!/usr/bin/env bash
# Phase 67 — the `/settings` MENU

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the `/settings` MENU (docs/settings.md). The session's knobs —
# until now environment variables you had to know about before launch — listed
# in the `/model` picker's inline frame, searchable, each cycled with
# Enter/Space. Drive it end to end: open it from the palette, check the frame
# and the value column, search, cycle a value, confirm the toast, and close
# back to the composer. Then a SECOND process against the same config home
# must show the changed value — the file persisted it.
S67="${S}_settings"
SET_CFG="$(mktemp -d)"
APP_SET="env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$SET_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
launch "$S67" 90 30 "$APP_SET"
# The palette lists it (a bare `/set` token filters to it).
tmux send-keys -t "$S67" -l "/set"
sleep 0.4
settings_palette="$(tmux capture-pane -t "$S67" -p)"
echo "==== Phase 67: the palette filtered to /settings ===="
printf '%s\n' "$settings_palette"
expect_has "$settings_palette" -F "Open settings menu" "/settings is missing from the slash-command palette"
tmux send-keys -t "$S67" Enter
sleep 0.6
settings_open="$(tmux capture-pane -t "$S67" -p)"
echo "==== Phase 67: the settings menu open ===="
printf '%s\n' "$settings_open"
# Only rows that are ALWAYS in the opening window: the list is capped at
# SETTINGS_MENU_MAX_ROWS and scrolls with the selection, so once the knob count
# passed the cap the tail rows stopped showing on open. Same lesson the counter
# below already learned — pinning something that grows with each new feature
# fails here instead of in that feature's own phase. A row past the window is
# reached the way a user reaches it: the type-to-search this phase drives next.
for expect in "Hide thinking" "Error retry" "Permission mode" \
	"Type to search · Enter/Space to change · Esc to cancel"; do
	expect_has "$settings_open" -F "$expect" "the settings menu is missing '$expect'"
done
# …and a row below the window is still reachable by search (proving the cap
# hides rows rather than dropping them).
tmux send-keys -t "$S67" -l "max tool"
sleep 0.4
settings_tail="$(tmux capture-pane -t "$S67" -p)"
echo "==== Phase 67: searched for a row past the window ===="
printf '%s\n' "$settings_tail"
expect_has "$settings_tail" -F "Max tool calls" "a setting past the visible window is unreachable by search"
# Clear the query so the search assertions below start from the full list.
for _ in $(seq 1 8); do tmux send-keys -t "$S67" BSpace; done
sleep 0.4
expect_has "$settings_open" -E "→ Hide thinking +false" "the first row is not marked with its value in the value column"
# The counter's SHAPE, not a hard-coded total: this phase is about the menu
# chrome, and pinning the row count made every later feature that adds a knob
# fail here instead of in its own phase (the Hooks row did exactly that).
expect_has "$settings_open" -E "\(1/[0-9]+\)" "the (n/total) counter never showed"
# Type-to-search narrows to one row; Enter cycles it and toasts the new value.
tmux send-keys -t "$S67" -l "retry"
sleep 0.4
settings_search="$(tmux capture-pane -t "$S67" -p)"
echo "==== Phase 67: searched for 'retry' ===="
printf '%s\n' "$settings_search"
expect_has "$settings_search" -F "(1/1)" "the search did not narrow the list to the one match"
expect_lacks "$settings_search" -F "Hide thinking" "the search left non-matching settings listed"
tmux send-keys -t "$S67" Enter
sleep 0.5
settings_cycled="$(tmux capture-pane -t "$S67" -p)"
echo "==== Phase 67: after Enter cycled the value ===="
printf '%s\n' "$settings_cycled"
expect_has "$settings_cycled" -E "Error retry +5" "Enter did not cycle Error retry from 3 to 5"
# Esc clears the query, a second Esc closes back to the composer.
tmux send-keys -t "$S67" Escape
sleep 0.3
tmux send-keys -t "$S67" Escape
sleep 0.5
settings_closed="$(tmux capture-pane -t "$S67" -p)"
echo "==== Phase 67: closed back to the composer ===="
printf '%s\n' "$settings_closed"
expect_lacks "$settings_closed" -F "Enter/Space to change" "Esc did not close the settings menu"
expect_has "$settings_closed" -F "Error retry: 5" "the change was not confirmed with a toast above the box"
expect_has "$settings_closed" -F "❯" "the composer did not come back"
tmux send-keys -t "$S67" -l "/quit"
tmux send-keys -t "$S67" Enter
sleep 0.6
tmux kill-session -t "$S67" 2>/dev/null

# It PERSISTED: the file records only what changed, and a fresh process against
# the same config home opens the menu already showing it.
if [ ! -f "$SET_CFG/settings.json" ]; then
	fail "no settings.json was written to the config home"
elif ! grep -q '"error_retry": *5' "$SET_CFG/settings.json"; then
	echo "==== Phase 67: settings.json ===="
	cat "$SET_CFG/settings.json"
	fail "settings.json does not record the changed value"
elif grep -q 'hide_thinking\|auto_compact' "$SET_CFG/settings.json"; then
	echo "==== Phase 67: settings.json ===="
	cat "$SET_CFG/settings.json"
	fail "settings.json records values the user never changed (defaults must stay off the wire)"
fi
S67B="${S}_settings2"
launch "$S67B" 90 30 "$APP_SET"
tmux send-keys -t "$S67B" -l "/settings"
sleep 0.3
tmux send-keys -t "$S67B" Enter
sleep 0.6
settings_restored="$(tmux capture-pane -t "$S67B" -p)"
echo "==== Phase 67: a fresh process shows the saved value ===="
printf '%s\n' "$settings_restored"
expect_has "$settings_restored" -E "Error retry +5" "the saved setting did not survive the restart"
tmux kill-session -t "$S67B" 2>/dev/null
rm -rf "$SET_CFG"
