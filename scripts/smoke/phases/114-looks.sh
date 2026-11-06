#!/usr/bin/env bash
# Phase 114 — PER-DIRECTORY LOOKS

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# PER-DIRECTORY LOOKS (docs/per-directory-state.md). /mascot and
# /spinner remember the directory they were chosen in — one config home, three
# working directories, config.json's rule: a choice made in a directory is that
# directory's own AND the last made anywhere (the file's top level, the shape a
# pre-directory mascot.json/spinner.json has); a directory launched in for the
# first time takes the last and PINS it as its own; a later choice elsewhere
# never moves a pinned directory; a third directory takes the new last.
S114="${S}_looks"
LK_CFG="$(mktemp -d)"
LK_A="$(mktemp -d "$SMOKE_TMP/looks-a.XXXXXX")"
LK_B="$(mktemp -d "$SMOKE_TMP/looks-b.XXXXXX")"
LK_C="$(mktemp -d "$SMOKE_TMP/looks-c.XXXXXX")"
# The paths as the app keys them — the kernel's cwd, symlinks resolved.
LK_A_KEY="$(cd "$LK_A" && pwd -P)"
LK_B_KEY="$(cd "$LK_B" && pwd -P)"
LK_C_KEY="$(cd "$LK_C" && pwd -P)"
APP_LK="env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$LK_CFG ALTER_ZERO_SESSIONS_DIR=$LK_CFG/sessions ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_SKILLS_DIR=$SMOKE_SKILLS ALTER_ZERO_AGENTS_DIR=$SMOKE_AGENTS ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"
# Choose a look through its picker: the palette entry, type-to-search, Enter.
lk_pick() {
	tmux send-keys -t "$1" -l "$2"
	sleep 0.3
	tmux send-keys -t "$1" Enter
	sleep 0.5
	tmux send-keys -t "$1" -l "$3"
	sleep 0.3
	tmux send-keys -t "$1" Enter
	sleep 0.8
}
lk_quit() {
	submit "$1" "/quit"
	sleep 0.6
	tmux kill-session -t "$1" 2>/dev/null
}
# The look one of the two files records for a directory key: the value on the
# line after `"<dir>": {` (pretty JSON, one field per line).
lk_entry() {
	grep -A1 -F "\"$2\": {" "$1" 2>/dev/null | tail -1 | sed -E 's/.*: *"([a-z]+)".*/\1/'
}
# The file's top-level value — the last choice made anywhere (its line 2).
lk_last() {
	sed -n -E '2s/.*"(mascot|spinner)": *"([a-z]+)".*/\2/p' "$1" 2>/dev/null
}
lk_expect() { # file, what, expected, actual
	if [ "$4" != "$3" ]; then
		echo "==== Phase 114: $(basename "$1") ===="
		cat "$1" 2>/dev/null
		fail "$2: expected '$3', got '$4'"
	fi
}

# (a) The first directory chooses bloom + dots: the files record them as its
# own AND as the last made anywhere.
tmux new-session -d -s "$S114" -x 90 -y 30 -c "$LK_A" "$APP_LK"
sleep 0.8
lk_pick "$S114" "/mascot" "blo"
lk_a_after="$(tmux capture-pane -t "$S114" -p -S -40)"
echo "==== Phase 114: bloom chosen in the first directory ===="
printf '%s\n' "$lk_a_after"
expect_has "$lk_a_after" -F "▀█▄███▄█▀" "the banner did not redraw with bloom"
lk_pick "$S114" "/spinner" "braille"
lk_quit "$S114"
echo "==== Phase 114: mascot.json + spinner.json after the first directory ===="
cat "$LK_CFG/mascot.json" "$LK_CFG/spinner.json" 2>/dev/null
lk_expect "$LK_CFG/mascot.json" "the first directory's mascot entry" bloom "$(lk_entry "$LK_CFG/mascot.json" "$LK_A_KEY")"
lk_expect "$LK_CFG/mascot.json" "the last mascot" bloom "$(lk_last "$LK_CFG/mascot.json")"
lk_expect "$LK_CFG/spinner.json" "the first directory's spinner entry" dots "$(lk_entry "$LK_CFG/spinner.json" "$LK_A_KEY")"
lk_expect "$LK_CFG/spinner.json" "the last spinner" dots "$(lk_last "$LK_CFG/spinner.json")"

# (b) A second directory, launched in for the first time: it wears the last
# choices (bloom in the banner, dots ✓ in the picker) and PINS them as its own.
tmux new-session -d -s "$S114" -x 90 -y 30 -c "$LK_B" "$APP_LK"
sleep 0.8
lk_b_boot="$(tmux capture-pane -t "$S114" -p)"
echo "==== Phase 114: the second directory's first launch ===="
printf '%s\n' "$lk_b_boot"
expect_has "$lk_b_boot" -F "▀█▄███▄█▀" "a new directory did not start from the last mascot (bloom)"
lk_expect "$LK_CFG/mascot.json" "the second directory's pinned mascot" bloom "$(lk_entry "$LK_CFG/mascot.json" "$LK_B_KEY")"
lk_expect "$LK_CFG/spinner.json" "the second directory's pinned spinner" dots "$(lk_entry "$LK_CFG/spinner.json" "$LK_B_KEY")"
tmux send-keys -t "$S114" -l "/spinner"
sleep 0.3
tmux send-keys -t "$S114" Enter
sleep 0.5
lk_b_spin="$(tmux capture-pane -t "$S114" -p)"
echo "==== Phase 114: the second directory's /spinner picker (dots ✓) ===="
printf '%s\n' "$lk_b_spin"
if ! printf '%s\n' "$lk_b_spin" | grep -F "→ dots" | grep -qF "✓"; then
	fail "a new directory did not start from the last spinner (dots)"
fi
tmux send-keys -t "$S114" Escape
sleep 0.4
# …then chooses its own, gem + line. The first directory's entries must not move.
lk_pick "$S114" "/mascot" "gem"
lk_pick "$S114" "/spinner" "line"
lk_quit "$S114"
echo "==== Phase 114: the files after the second directory chose gem + line ===="
cat "$LK_CFG/mascot.json" "$LK_CFG/spinner.json" 2>/dev/null
lk_expect "$LK_CFG/mascot.json" "the second directory's mascot entry" gem "$(lk_entry "$LK_CFG/mascot.json" "$LK_B_KEY")"
lk_expect "$LK_CFG/mascot.json" "the first directory's mascot entry after a choice elsewhere" bloom "$(lk_entry "$LK_CFG/mascot.json" "$LK_A_KEY")"
lk_expect "$LK_CFG/mascot.json" "the last mascot" gem "$(lk_last "$LK_CFG/mascot.json")"
lk_expect "$LK_CFG/spinner.json" "the second directory's spinner entry" line "$(lk_entry "$LK_CFG/spinner.json" "$LK_B_KEY")"
lk_expect "$LK_CFG/spinner.json" "the first directory's spinner entry after a choice elsewhere" dots "$(lk_entry "$LK_CFG/spinner.json" "$LK_A_KEY")"
lk_expect "$LK_CFG/spinner.json" "the last spinner" line "$(lk_last "$LK_CFG/spinner.json")"

# (c) The first directory relaunched: still bloom + dots — a pinned directory
# never follows the last choice.
tmux new-session -d -s "$S114" -x 90 -y 30 -c "$LK_A" "$APP_LK"
sleep 0.8
lk_a_again="$(tmux capture-pane -t "$S114" -p)"
echo "==== Phase 114: the first directory relaunched ===="
printf '%s\n' "$lk_a_again"
expect_has "$lk_a_again" -F "▀█▄███▄█▀" "the first directory lost its own mascot (bloom)"
expect_lacks "$lk_a_again" -F "▝▀██▄██▀▘" "the first directory followed the second's mascot (gem)"
tmux send-keys -t "$S114" -l "/spinner"
sleep 0.3
tmux send-keys -t "$S114" Enter
sleep 0.5
lk_a_spin="$(tmux capture-pane -t "$S114" -p)"
echo "==== Phase 114: the first directory's /spinner picker (dots ✓, not line) ===="
printf '%s\n' "$lk_a_spin"
if ! printf '%s\n' "$lk_a_spin" | grep -F "→ dots" | grep -qF "✓"; then
	fail "the first directory lost its own spinner (dots)"
fi
tmux send-keys -t "$S114" Escape
sleep 0.3
lk_quit "$S114"

# (d) A third directory takes the NEW last (gem + line) and pins it.
tmux new-session -d -s "$S114" -x 90 -y 30 -c "$LK_C" "$APP_LK"
sleep 0.8
lk_c_boot="$(tmux capture-pane -t "$S114" -p)"
echo "==== Phase 114: a third directory's first launch (gem) ===="
printf '%s\n' "$lk_c_boot"
expect_has "$lk_c_boot" -F "▝▀██▄██▀▘" "a third directory did not start from the new last mascot (gem)"
lk_quit "$S114"
lk_expect "$LK_CFG/mascot.json" "the third directory's pinned mascot" gem "$(lk_entry "$LK_CFG/mascot.json" "$LK_C_KEY")"
lk_expect "$LK_CFG/spinner.json" "the third directory's pinned spinner" line "$(lk_entry "$LK_CFG/spinner.json" "$LK_C_KEY")"
rm -rf "$LK_CFG" "$LK_A" "$LK_B" "$LK_C"
