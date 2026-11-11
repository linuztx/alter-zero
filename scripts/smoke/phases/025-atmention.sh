#!/usr/bin/env bash
# Phase 25 — `@` file-path mentions

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# `@` file-path mentions (docs/file-search.md). Typing `@query`
# opens a file picker BELOW the box listing workspace files that fuzzy-match the
# query (fetched asynchronously by a background walk+rank worker) in columned
# `→ name  parent/  File|Dir` rows, five at a time over a `(n/total)` counter
# that counts every match, the highlight riding the middle row; Enter inserts
# the highlighted path into the composer, replacing the `@token`; and the walk
# is per-query, so a file created after startup shows up too. Launch in a temp
# dir with known files so the match set is deterministic.
S22="${S}_atmention"
ATDIR="$(mktemp -d)"
: >"$ATDIR/alpha_smoke.txt"
: >"$ATDIR/readme_notes.md"
mkdir -p "$ATDIR/subdir"
: >"$ATDIR/subdir/beta_smoke.txt"
# Twelve files `@pick_` matches and nothing else does — more than the eight
# the worker used to send back — ranked pick_01 … pick_12 (one score, one
# length, so the name decides).
for n in 01 02 03 04 05 06 07 08 09 10 11 12; do
	: >"$ATDIR/pick_$n.txt"
done
# The session starts in $ATDIR, so the binary needs an ABSOLUTE path ($APP's is
# relative to the project dir); the app then walks $ATDIR for the @ picker.
APP_ABS="env $CFG_ENV ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $(realpath "$BIN")"
launch -c "$ATDIR" "$S22" 80 24 "$APP_ABS"
tmux send-keys -t "$S22" -l "see @alpha"
at_open="$(wait_pane 3 "$S22" -F "alpha_smoke.txt")" # up to ~3s: the worker walks + ranks, the picker shows
echo "==== captured pane (@ file picker open) ===="
printf '%s\n' "$at_open"
# Enter accepts the highlighted file: the `@alpha` token becomes the path + a space.
tmux send-keys -t "$S22" Enter
sleep 0.3
at_inserted="$(tmux capture-pane -t "$S22" -p)"
echo "==== captured pane (file path inserted into the composer) ===="
printf '%s\n' "$at_inserted"
# A file created AFTER startup (what the agent does when asked to create one)
# must appear in a later `@` search: the worker walks the cwd afresh per query
# instead of serving a startup-cached list (docs/file-search.md).
: >"$ATDIR/gamma_new.txt"
tmux send-keys -t "$S22" -l " @gamma"
at_fresh="$(wait_pane 3 "$S22" -F "gamma_new.txt")" # up to ~3s for the fresh walk to list the new file
echo "==== captured pane (@ picker lists a file created after startup) ===="
printf '%s\n' "$at_fresh"
# Every match, five at a time: the band lists five of the twelve over a
# `(n/total)` counter that counts all twelve, ↓ walks the highlight to the
# middle row and the list scrolls under it (the `/` palette's window), and ↑
# from the first wraps to the twelfth — reachable because the worker now sends
# back every match rather than the best eight.
tmux send-keys -t "$S22" -l " @pick_"
at_many="$(wait_pane 3 "$S22" -E '^  \(1/12\)$')"
echo "==== captured pane (@ picker over twelve matches) ===="
printf '%s\n' "$at_many"
tmux send-keys -t "$S22" Down Down Down
at_centered="$(wait_pane 3 "$S22" -E '^  \(4/12\)$')"
echo "==== captured pane (@ picker with its highlight centered) ===="
printf '%s\n' "$at_centered"
tmux send-keys -t "$S22" Up Up Up Up
at_wrapped="$(wait_pane 3 "$S22" -E '^  \(12/12\)$')"
echo "==== captured pane (@ picker wrapped to its last match) ===="
printf '%s\n' "$at_wrapped"
tmux kill-session -t "$S22" 2>/dev/null
rm -rf "$ATDIR"

# Phase 25: the `@` file picker (docs/file-search.md). Typing "@alpha" lists the
# matching workspace file below the box; Enter inserts its path into the composer.
expect_has "$at_open" -F "alpha_smoke.txt" "typing '@alpha' did not list the matching file in the picker below the box"
# The picker displaces the session footer (codex's popups take its row), like the palette.
expect_lacks "$at_open" -F "dummy_model_name" "the session footer is still shown while the @ file picker is open (the band must displace it)"
expect_has "$at_inserted" -F "❯ see alpha_smoke.txt" "Enter did not insert the highlighted file path into the composer (expected '❯ see alpha_smoke.txt')"
# The picker closed on accept: the session footer returns to its row.
expect_has "$at_inserted" -F "dummy_model_name ·" "the session footer did not return after the @ file picker closed on accept"
# The columned row layout (docs/file-search.md): the selected row carries the
# `→` marker, then the name, the `./` parent column, and the `File` kind label.
expect_has "$at_open" -E "→ alpha_smoke\.txt +\./ +File" "the @ picker row is not the columned '→ name  ./  File' layout"
# The fresh-walk fix: a file created after startup appears in a later search.
expect_has "$at_fresh" -F "gamma_new.txt" "a file created after startup never appeared in the @ picker (stale startup index)"
# Every match, five at a time, over the counter.
pick_rows() { printf '%s\n' "$1" | grep -E '^(→ |  )pick_[0-9]+\.txt +\./ +File' || true; }
expect_eq "$(pick_rows "$at_many" | wc -l | tr -d ' ')" "5" "the @ picker does not show five of its twelve matches"
expect_has "$at_many" -E '^→ pick_01\.txt ' "the @ picker does not open on its best match"
expect_lacks "$at_many" -F "pick_06.txt" "the @ picker shows a sixth match in its five-row window"
expect_has "$at_many" -E '^  \(1/12\)$' "the @ picker's counter does not count all twelve matches"
# Centered: three steps down put pick_04 on the middle row, pick_01 scrolled out.
expect_has "$(pick_rows "$at_centered" | sed -n 3p)" -E '^→ pick_04\.txt ' "the @ picker's highlight is not on its middle row"
expect_has "$(pick_rows "$at_centered" | sed -n 1p)" -E '^  pick_02\.txt ' "the @ picker's window did not scroll to keep its highlight centered"
expect_lacks "$at_centered" -F "pick_01.txt" "the @ picker kept its first match in view with the highlight past the middle"
# Wrapped: the twelfth match is reachable, flush with the window's bottom.
expect_has "$(pick_rows "$at_wrapped" | sed -n 5p)" -E '^→ pick_12\.txt ' "↑ from the first match did not wrap to the twelfth"
expect_lacks "$at_wrapped" -F "pick_07.txt" "the @ picker's window did not follow the highlight to the end of the list"
