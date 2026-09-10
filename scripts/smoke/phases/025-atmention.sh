#!/usr/bin/env bash
# Phase 25 — `@` file-path mentions

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# `@` file-path mentions (docs/file-search.md). Typing `@query`
# opens a file picker BELOW the box listing workspace files that fuzzy-match the
# query (fetched asynchronously by a background walk+rank worker) in columned
# `→ name  parent/  File|Dir` rows; Enter inserts the highlighted path into the
# composer, replacing the `@token`; and the walk is per-query, so a file created
# after startup shows up too. Launch in a temp dir with known files so the match
# set is deterministic.
S22="${S}_atmention"
ATDIR="$(mktemp -d)"
: >"$ATDIR/alpha_smoke.txt"
: >"$ATDIR/readme_notes.md"
mkdir -p "$ATDIR/subdir"
: >"$ATDIR/subdir/beta_smoke.txt"
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
