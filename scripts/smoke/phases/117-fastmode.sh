#!/usr/bin/env bash
# Phase 117 — /fast: the palette row, the unsupported-model toast, and nothing claimed

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The /fast command (docs/fast-mode.md): codex's fast mode, a static palette
# row right after /model. The offline dummy lists no speed tier, so what is
# drivable offline is exactly the shape a model without one meets: the row is
# discoverable with its cost in the description, running it explains instead
# of going silent (Ctrl+T's rule for a non-reasoner), and nothing claims a
# tier that was never switched on — no footer word, no toast saying it was,
# and the command is run rather than sent as a message.
S117="${S}_fast"
launch "$S117" 80 24

tmux send-keys -t "$S117" -l "/fast"
sleep 0.4
fast_palette="$(tmux capture-pane -t "$S117" -p)"
echo "==== Phase 117: captured pane (the /fast palette row) ===="
printf '%s\n' "$fast_palette"
expect_has "$fast_palette" -F "Toggle fast mode (faster replies, more usage)" "the palette did not list /fast with its description"

tmux send-keys -t "$S117" Enter
fast_toast="$(wait_pane 8 "$S117" -- -F "dummy_model_name does not support fast mode")"
echo "==== Phase 117: captured pane (the explanatory toast) ===="
printf '%s\n' "$fast_toast"
expect_has "$fast_toast" -F "dummy_model_name does not support fast mode" "/fast went silent on a model that lists no speed tier"
expect_has "$fast_toast" -F "dummy_model_name ·" "the footer is gone after /fast"
expect_lacks "$fast_toast" -F "dummy_model_name fast" "the footer wears a tier the model does not list"
expect_lacks "$fast_toast" -F "Speed:" "/fast reported switching a tier the model does not list"
expect_lacks "$fast_toast" -F "❯ /fast" "/fast was sent as a message instead of run"

tmux kill-session -t "$S117" 2>/dev/null
