#!/usr/bin/env bash
# Phase 11 — `?` from an empty composer toggles the shortcuts band below the box, laid out to fit the terminal

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# `?` from an empty composer toggles the shortcuts band below the
# box (codex's footer shortcut overlay — docs/ui/shortcuts.md); a second `?` hides
# it; and with a draft in the box `?` is just a character (no band).
S8="${S}_shortcuts"
launch "$S8" 80 24
tmux send-keys -t "$S8" -l "?"
band_open="$(wait_pane 3 "$S8" -F "ctrl+c to quit")"
dump "the shortcuts band open on an 80-column pane" "$band_open"
tmux send-keys -t "$S8" -l "?"
sleep 0.3
band_closed="$(tmux capture-pane -t "$S8" -p)"
tmux send-keys -t "$S8" -l "really?"
sleep 0.3
band_typed="$(tmux capture-pane -t "$S8" -p)"
dump "after typing a draft containing '?'" "$band_typed"
tmux kill-session -t "$S8" 2>/dev/null

# "for commands" only ever appears in the band, so it's a clean open/closed
# marker.
expect_has "$band_open" -F "for commands" "'?' with an empty composer did not open the shortcuts band"
# At 80 columns the band is three columns of six rows, read down each column
# (the sigils and history, editing and the running turn, the views, modes and
# the way out), inset two columns like the footer row it displaces, the
# columns three apart — which ends the widest row at column 80.
expect_has "$band_open" -E '^  / for commands {13}shift\+enter for newline {3}ctrl\+o for transcript$' "the 80-column band's first row is not three columns, inset two, three apart"
expect_has "$band_open" -E '^  ↑ for input history {8}alt\+↑ to edit queue {7}esc to quit$' "the 80-column band does not read down its columns"
expect_has "$band_open" -E '^  ctrl\+r to search history {3}ctrl\+b to background {6}ctrl\+c to quit$' "the 80-column band's sixth row is not its last three entries"
expect_has "$band_open" -E '^  @ for file paths ' "the band does not list the @ file picker"
expect_lacks "$band_open" -E '^(/ for|\$ for|esc to|ctrl\+)' "a shortcuts row still starts flush against the edge"
expect_lacks "$band_closed" -F "for commands" "a second '?' did not close the shortcuts band"
# The shortcuts band displaces the session footer too; dismissing it brings the
# footer back (same slot, codex's shortcut-overlay behaviour).
expect_lacks "$band_open" -F "dummy_model_name" "the session footer is still shown while the shortcuts band is open"
expect_has "$band_closed" -F "dummy_model_name ·" "the session footer did not return after the shortcuts band closed"
expect_has "$band_typed" -F "❯ really?" "'?' inside a draft was not typed as a literal character"
expect_lacks "$band_typed" -F "for commands" "typing a draft ending in '?' re-opened the shortcuts band"

# A wider terminal spends its width on columns rather than rows: at 120
# columns four columns of five rows (5, 5, 4, 4 — the gaps at the bottom
# right), the columns spread six apart.
S8W="${S}_shortcuts_wide"
launch "$S8W" 120 20
tmux send-keys -t "$S8W" -l "?"
band_wide="$(wait_pane 3 "$S8W" -F "ctrl+c to quit")"
dump "the shortcuts band on a 120-column pane" "$band_wide"
tmux kill-session -t "$S8W" 2>/dev/null
expect_has "$band_wide" -E '^  / for commands {11}ctrl\+r to search history {6}alt\+↑ to edit queue {9}ctrl\+t to cycle thinking$' "the 120-column band's first row is not four columns six apart"
expect_has "$band_wide" -E '^  \$ for skills {13}ctrl\+w/u/k to kill text {7}ctrl\+d for llm context {6}ctrl\+c to quit$' "the 120-column band's fourth row does not end on the way out"
expect_has "$band_wide" -E '^  ↑ for input history {6}tab to queue next turn$' "the 120-column band's last row is not the first two columns' fifth entries"

# Under 53 columns the full wording cannot make two columns, so the band
# drops each label's `for`/`to` and keeps two columns of nine rows — where
# eighteen rows of one column crowded a streaming reply out of the strip
# (Phase 99).
S8T="${S}_shortcuts_terse"
launch "$S8T" 50 24
tmux send-keys -t "$S8T" -l "?"
band_terse="$(wait_pane 3 "$S8T" -F "ctrl+c quit")"
dump "the shortcuts band on a 50-column pane" "$band_terse"
tmux kill-session -t "$S8T" 2>/dev/null
expect_has "$band_terse" -E '^  / commands {17}tab queue next turn$' "the 50-column band's first row is not two terse columns"
expect_has "$band_terse" -E '^  ctrl\+w/u/k kill text {7}ctrl\+c quit$' "the 50-column band's ninth row is not its last two entries"
expect_lacks "$band_terse" -E '^  [/!@$] for ' "the 50-column band kept the full wording"

# Under 46 even the terse columns do not fit, and the band lists one entry
# per row in the full wording, every entry whole, where a fixed second
# column used to be cut mid-word at the edge (`shift+tab for perm`). Tall
# enough for all eighteen.
S8N="${S}_shortcuts_narrow"
launch "$S8N" 40 30
tmux send-keys -t "$S8N" -l "?"
band_narrow="$(wait_pane 3 "$S8N" -F "ctrl+c to quit")"
dump "the shortcuts band on a 40-column pane" "$band_narrow"
tmux kill-session -t "$S8N" 2>/dev/null
for entry in '/ for commands' '! for shell command' '@ for file paths' 'shift\+tab for permissions' 'ctrl\+w/u/k to kill text' 'ctrl\+b to background' 'ctrl\+c to quit'; do
	expect_has "$band_narrow" -E "^  ${entry}\$" "the narrow band does not list '$entry' whole on a row of its own"
done
expect_lacks "$band_narrow" -E 'for commands +[!s]' "the narrow band kept a second column and cut it at the edge"

# …and on one too short for the whole band as well, the band shows the rows
# the region could give it — the composer keeps its own — and ends on a count
# of the entries that did not fit, instead of losing the bottom of the list
# (the way out among it) to the clip without a word.
S8S="${S}_shortcuts_short"
launch "$S8S" 40 10
tmux send-keys -t "$S8S" -l "?"
band_short="$(wait_pane 3 "$S8S" -F "more")"
dump "the shortcuts band on a 40x10 pane" "$band_short"
tmux kill-session -t "$S8S" 2>/dev/null
expect_has "$band_short" -E '^❯' "the short band squeezed the composer off the screen"
expect_has "$band_short" -E '^  / for commands$' "the short band lost its first entry"
expect_has "$band_short" -E '^  … \+12 more$' "the short band does not end on a count of the twelve entries it left out"
