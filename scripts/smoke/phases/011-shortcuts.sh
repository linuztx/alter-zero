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
# The band starts two columns in, where the footer row it displaces starts,
# and on an 80-column pane each group of bindings is a column of its own —
# the composer's sigils, the session keys, the editing keys — each column as
# wide as its widest entry plus a three-column gutter, six rows in all.
expect_has "$band_open" -E '^  / for commands {13}esc to quit {17}shift\+enter for newline$' "the shortcuts band does not give each group its own column at 80 columns"
expect_has "$band_open" -E '^  @ for file paths {11}ctrl\+c to quit {14}ctrl\+v for image paste$' "the shortcuts band does not list '@ for file paths' under '/ for commands'"
expect_has "$band_open" -E '^  ctrl\+r to search history {3}ctrl\+d for llm context$' "the shortcuts band's last row is not the first two groups' last entries"
expect_lacks "$band_open" -E '^(/ for|\$ for|esc to)' "a shortcuts row still starts flush against the edge"
expect_has "$band_open" -F "ctrl+c to quit" "the shortcuts band is missing its quit entry"
expect_has "$band_open" -F "alt+↑ to edit queue" "the shortcuts band is missing the alt+↑ queue-edit entry"
expect_lacks "$band_closed" -F "for commands" "a second '?' did not close the shortcuts band"
# The shortcuts band displaces the session footer too; dismissing it brings the
# footer back (same slot, codex's shortcut-overlay behaviour).
expect_lacks "$band_open" -F "dummy_model_name" "the session footer is still shown while the shortcuts band is open"
expect_has "$band_closed" -F "dummy_model_name ·" "the session footer did not return after the shortcuts band closed"
expect_has "$band_typed" -F "❯ really?" "'?' inside a draft was not typed as a literal character"
expect_lacks "$band_typed" -F "for commands" "typing a draft ending in '?' re-opened the shortcuts band"

# A pane too narrow for the groups side by side (under 80) flows the
# bindings down two columns in reading order, the first filled before the
# second; one too narrow for two (under 54) lists one entry per row, every
# entry whole, where a second column used to be cut mid-word at the edge
# (`shift+tab for perm`). Tall enough for all seventeen.
S8M="${S}_shortcuts_medium"
launch "$S8M" 60 30
tmux send-keys -t "$S8M" -l "?"
band_medium="$(wait_pane 3 "$S8M" -F "for commands")"
dump "the shortcuts band on a 60-column pane" "$band_medium"
tmux kill-session -t "$S8M" 2>/dev/null
expect_has "$band_medium" -E '^  / for commands {14}ctrl\+t to cycle thinking$' "the 60-column band does not flow into two columns"
expect_has "$band_medium" -E '^  shift\+tab for permissions$' "the 60-column band's first column does not end on the shift+tab entry"

S8N="${S}_shortcuts_narrow"
launch "$S8N" 50 30
tmux send-keys -t "$S8N" -l "?"
band_narrow="$(wait_pane 3 "$S8N" -F "edit queue")"
dump "the shortcuts band on a 50-column pane" "$band_narrow"
tmux kill-session -t "$S8N" 2>/dev/null
for entry in '/ for commands' '! for shell command' 'shift\+tab for permissions' 'ctrl\+w/u/k to kill text' 'alt\+↑ to edit queue'; do
	expect_has "$band_narrow" -E "^  ${entry}\$" "the narrow band does not list '$entry' whole on a row of its own"
done
expect_lacks "$band_narrow" -E 'for commands +[^ ]' "the narrow band kept two columns and cut the second at the edge"

# A wide pane spends its width before its rows: each group splits across two
# columns, never sharing one, and the band takes three rows instead of six.
S8W="${S}_shortcuts_wide"
launch "$S8W" 160 24
tmux send-keys -t "$S8W" -l "?"
band_wide="$(wait_pane 3 "$S8W" -F "for commands")"
dump "the shortcuts band on a 160-column pane" "$band_wide"
tmux kill-session -t "$S8W" 2>/dev/null
expect_has "$band_wide" -E '^  / for commands {5}! for shell command {8}esc to quit {17}ctrl\+t to cycle thinking {3}shift\+enter for newline {3}tab to queue next turn$' "the 160-column band does not split each group across two columns"
expect_has "$band_wide" -E '^  \$ for skills {7}ctrl\+r to search history {3}shift\+tab for permissions {3}ctrl\+d for llm context {5}ctrl\+w/u/k to kill text$' "the 160-column band is not three rows deep"
