#!/usr/bin/env bash
# Phase 11 — `?` from an empty composer toggles the shortcuts band below the box (codex's f

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# `?` from an empty composer toggles the shortcuts band below the
# box (codex's footer shortcut overlay — docs/shortcuts.md); a second `?` hides
# it; and with a draft in the box `?` is just a character (no band).
S8="${S}_shortcuts"
launch "$S8" 80 24
tmux send-keys -t "$S8" -l "?"
sleep 0.3
band_open="$(tmux capture-pane -t "$S8" -p)"
echo "==== captured pane (shortcuts band open) ===="
printf '%s\n' "$band_open"
tmux send-keys -t "$S8" -l "?"
sleep 0.3
band_closed="$(tmux capture-pane -t "$S8" -p)"
tmux send-keys -t "$S8" -l "really?"
sleep 0.3
band_typed="$(tmux capture-pane -t "$S8" -p)"
echo "==== captured pane (after typing a draft containing '?') ===="
printf '%s\n' "$band_typed"
tmux kill-session -t "$S8" 2>/dev/null

# Phase 11: the `?` shortcuts band (docs/shortcuts.md). "for commands" only
# ever appears in the band, so it's a clean open/closed marker.
expect_has "$band_open" -F "for commands" "'?' with an empty composer did not open the shortcuts band"
expect_has "$band_open" -F "ctrl+c to quit" "the shortcuts band is missing its quit entry"
expect_has "$band_open" -F "alt+↑ to edit queue" "the shortcuts band is missing the alt+↑ queue-edit entry"
expect_lacks "$band_closed" -F "for commands" "a second '?' did not close the shortcuts band"
# The shortcuts band displaces the session footer too; dismissing it brings the
# footer back (same slot, codex's shortcut-overlay behaviour).
expect_lacks "$band_open" -F "dummy_model_name" "the session footer is still shown while the shortcuts band is open"
expect_has "$band_closed" -F "dummy_model_name ·" "the session footer did not return after the shortcuts band closed"
expect_has "$band_typed" -F "❯ really?" "'?' inside a draft was not typed as a literal character"
expect_lacks "$band_typed" -F "for commands" "typing a draft ending in '?' re-opened the shortcuts band"
