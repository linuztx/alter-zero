#!/usr/bin/env bash
# Phase 117 — the /fast SERVICE TIER command

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the /fast command (docs/fast-mode.md). The speed lane a request runs
# in, ported from codex's service tiers: a `/fast` palette row right after
# `/model`, a `fast` marker in the footer while the lane is on, and — on a
# model that publishes no lanes, which the offline dummy is — an explanatory
# toast rather than a key that does nothing. The dummy has no catalog, so what
# is drivable offline is exactly that: the row is discoverable, and running it
# explains instead of going silent.
S117="${S}_fast"
launch "$S117" 80 24

# The palette lists it, with its description, under /model.
tmux send-keys -t "$S117" -l "/fast"
sleep 0.4
fast_palette="$(tmux capture-pane -t "$S117" -p)"
echo "==== Phase 117: captured pane (the /fast palette row) ===="
printf '%s\n' "$fast_palette"
expect_has "$fast_palette" -F "fast" "the palette never offered a /fast row"
expect_has "$fast_palette" -F "Toggle the model's fast service tier" "the /fast row carries no description"

# Running it on a model with no lanes explains — the Ctrl+T-on-a-non-reasoner
# rule. A silent no-op here is the bug this asserts against.
tmux send-keys -t "$S117" Enter
fast_toast="$(wait_pane 8 "$S117" -- -F "does not offer a fast service tier")"
echo "==== Phase 117: captured pane (the explanatory toast) ===="
printf '%s\n' "$fast_toast"
expect_has "$fast_toast" -F "does not offer a fast service tier" "/fast went silent on a model with no service tiers"
# …and it must NOT claim a lane it never switched on: no footer marker, and
# nothing committed to scrollback (a toast is UI, never conversation).
fast_after="$(tmux capture-pane -t "$S117" -p -S -80)"
expect_lacks "$fast_after" -E "dummy_model_name fast" "the footer marked a fast lane the model does not offer"
expect_lacks "$fast_after" -F "Fast mode: on" "/fast reported switching a lane the model does not offer"

tmux kill-session -t "$S117" 2>/dev/null
