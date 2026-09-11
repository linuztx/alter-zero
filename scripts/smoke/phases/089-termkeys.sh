#!/usr/bin/env bash
# Phase 89 — the COMPOSER'S TERMINAL SHORTCUTS

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the COMPOSER'S TERMINAL SHORTCUTS (docs/textarea.md). Cursor
# positions are made visible by typing after each motion: Ctrl+A/E jump the
# line ends, idle Ctrl+B steps left (nothing backgroundable is running),
# Alt+B walks a word back, Ctrl+W rubs out a unix word, Ctrl+U kills the
# line, Ctrl+H backspaces. All in the draft — nothing is ever submitted.
S89="${S}_termkeys"
launch "$S89" 100 24
tmux send-keys -t "$S89" -l "alpha beta"
sleep 0.2
tmux send-keys -t "$S89" C-a
tmux send-keys -t "$S89" -l "zero "
sleep 0.2
tk_home="$(tmux capture-pane -t "$S89" -p)"
tmux send-keys -t "$S89" C-e
tmux send-keys -t "$S89" -l " delta"
sleep 0.2
tk_end="$(tmux capture-pane -t "$S89" -p)"
tmux send-keys -t "$S89" M-b
tmux send-keys -t "$S89" -l "X"
sleep 0.2
tk_word="$(tmux capture-pane -t "$S89" -p)"
tmux send-keys -t "$S89" C-e
tmux send-keys -t "$S89" C-w
sleep 0.2
tk_killw="$(tmux capture-pane -t "$S89" -p)"
tmux send-keys -t "$S89" C-u
sleep 0.2
tk_killu="$(tmux capture-pane -t "$S89" -p)"
tmux send-keys -t "$S89" -l "ab"
tmux send-keys -t "$S89" C-b
tmux send-keys -t "$S89" -l "Y"
sleep 0.2
tk_left="$(tmux capture-pane -t "$S89" -p)"
tmux send-keys -t "$S89" C-e
tmux send-keys -t "$S89" C-h
sleep 0.2
tk_bsp="$(tmux capture-pane -t "$S89" -p)"
tmux kill-session -t "$S89" 2>/dev/null
echo "==== Phase 89: terminal editing shortcuts in the composer ===="
expect_has "$tk_home" -F "zero alpha beta" "Ctrl+A did not move to the line start (typed text not at the front)"
expect_has "$tk_end" -F "zero alpha beta delta" "Ctrl+E did not move to the line end"
expect_has "$tk_word" -F "zero alpha beta Xdelta" "Alt+B did not step back one word"
if ! printf '%s' "$tk_killw" | grep -qF "zero alpha beta" ||
	printf '%s' "$tk_killw" | grep -qF "Xdelta"; then
	fail "Ctrl+W did not rub out the last word"
fi
expect_lacks "$tk_killu" -F "zero alpha" "Ctrl+U did not kill the line"
expect_has "$tk_left" -F "aYb" "idle Ctrl+B did not step the cursor left"
if ! printf '%s' "$tk_bsp" | grep -qF "aY" || printf '%s' "$tk_bsp" | grep -qF "aYb"; then
	fail "Ctrl+H did not backspace"
fi
