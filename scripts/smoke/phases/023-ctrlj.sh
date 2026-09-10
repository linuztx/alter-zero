#!/usr/bin/env bash
# Phase 23 — Ctrl+J is the UNIVERSAL newline key

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Ctrl+J is the UNIVERSAL newline key (docs/shift-enter.md). Unlike
# Shift+Enter (which needs keyboard enhancement to even be reported), Ctrl+J grows
# the input box on every terminal — in raw mode the byte 0x0A parses to
# Char('j')+CONTROL. Type two lines separated by Ctrl+J: the box must grow to show
# the prompt on the first line and an indented continuation on the second (same
# shape as Phase 2's Alt+Enter, via a different key). A plain Enter then submits
# the whole multi-line draft.
S20="${S}_ctrlj"
launch "$S20" 80 24
tmux send-keys -t "$S20" -l "CCC"
tmux send-keys -t "$S20" C-j
tmux send-keys -t "$S20" -l "DDD"
sleep 0.3
ctrlj_grown="$(tmux capture-pane -t "$S20" -p)"
echo "==== captured pane (Ctrl+J grew the input box) ===="
printf '%s\n' "$ctrlj_grown"
tmux send-keys -t "$S20" Enter # a plain Enter submits the multi-line draft
ctrlj_sent=""
for _ in $(seq 1 40); do # up to ~6s: the two-line message commits to scrollback
	ctrlj_sent="$(tmux capture-pane -t "$S20" -p -S -40)"
	if printf '%s' "$ctrlj_sent" | grep -qF "❯ CCC" &&
		printf '%s' "$ctrlj_sent" | grep -qF "DDD"; then
		break
	fi
	sleep 0.15
done
echo "==== captured pane (multi-line draft submitted with Enter) ===="
printf '%s\n' "$ctrlj_sent"
tmux kill-session -t "$S20" 2>/dev/null

# Phase 23: Ctrl+J grows the box (the universal newline fallback, docs/shift-enter.md).
expect_has "$ctrlj_grown" -F "❯ CCC" "first draft line '❯ CCC' not shown in the input box after Ctrl+J"
expect_has "$ctrlj_grown" -F "  DDD" "Ctrl+J did not insert a newline — indented continuation '  DDD' missing (the box did not grow)"
# A plain Enter then submits the whole multi-line draft to scrollback.
expect_has "$ctrlj_sent" -F "❯ CCC" "a plain Enter did not submit the Ctrl+J multi-line draft ('❯ CCC' not committed)"
