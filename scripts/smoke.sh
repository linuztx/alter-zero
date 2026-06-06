#!/usr/bin/env bash
# Drive the TUI inside a real terminal (tmux): type a message, let the dummy AI
# stream, capture the rendered conversation (scrollback + viewport), then quit.
set -u
BIN="${1:-target/debug/inline-tui}"
S="inlinetui_smoke_$$"

tmux kill-session -t "$S" 2>/dev/null
tmux new-session -d -s "$S" -x 80 -y 24 "$BIN"
sleep 0.6
tmux send-keys -t "$S" -l "hello there"
sleep 0.3
tmux send-keys -t "$S" Enter
sleep 3.0                        # let the reply stream into scrollback

echo "==== captured pane (with scrollback) ===="
tmux capture-pane -t "$S" -p -S -60

tmux send-keys -t "$S" Escape    # quit
sleep 0.3
tmux kill-session -t "$S" 2>/dev/null
