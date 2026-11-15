#!/usr/bin/env bash
# Phase 157 — Ctrl+O pins the header of the cell you scroll into, and ←/→ step between cells

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Finding your way in the Ctrl+O transcript (docs/transcript-navigation.md).
# Two `!` commands print more than a screen each. Ctrl+O opens tail-following
# inside the second one's output, its `! seq 100 160` header long scrolled
# off — so the pager pins that header over the body's top row. ← opens the
# entry being read at its own first row (the real header on top, the output
# under it from its first line), ← again the command before it; ↓ from there
# scrolls the output under its pinned header; → steps on to the next
# command. The first hint row names the step.
S157="${S}_transcriptpin"
launch "$S157" 80 24
submit "$S157" "!seq 1 60"
wait_for 10 "$S157" -E '^ +60 *$' ||
	fail "the first command's output never finished"
submit "$S157" "!seq 100 160"
wait_for 10 "$S157" -E '^ +160 *$' ||
	fail "the second command's output never finished"
keys "$S157" C-o
pin_open="$(wait_pane 5 "$S157" -F "T R A N S C R I P T")"
note "Ctrl+O opened at the bottom — the second command's header pinned on top"
printf '%s\n' "$pin_open"
# Row 1 is the title, row 2 the body's first row.
pin_top="$(printf '%s\n' "$pin_open" | sed -n 2p | sed 's/ *$//')"
expect_eq "$pin_top" "! seq 100 160" "the window opened inside the second command's output without its header pinned on top"
expect_has "$pin_open" -E '^ +160 *$' "the tail-following window does not end on the newest output"
expect_has "$pin_open" -F "←/→ prev/next entry" "the first hint row does not name the entry step"

# ← opens the entry being read at its own first row: the real header, the
# output from its first line under it.
keys "$S157" Left
pin_own="$(wait_pane 5 "$S157" -E '^  ⎿  100 *$')"
note "← — the second command at the window's top"
printf '%s\n' "$pin_own"
expect_eq "$(printf '%s\n' "$pin_own" | sed -n 2p | sed 's/ *$//')" "! seq 100 160" "← did not open the second command at its first row"
expect_eq "$(printf '%s\n' "$pin_own" | sed -n 3p | sed 's/ *$//')" "  ⎿  100" "the output does not start under its header"

# ← again: the command before it.
keys "$S157" Left
pin_prev="$(wait_pane 5 "$S157" -E '^  ⎿  1 *$')"
note "← again — the first command at the window's top"
printf '%s\n' "$pin_prev"
expect_eq "$(printf '%s\n' "$pin_prev" | sed -n 2p | sed 's/ *$//')" "! seq 1 60" "the second ← did not open the first command"

# ↓ five rows: the output scrolls under the pinned header — the window
# opens on `5`, the pin covers it, and `6` sits right under the header.
for _ in 1 2 3 4 5; do keys "$S157" Down; done
pin_scrolled="$(wait_pane 5 "$S157" -E '^ +23 *$')"
note "↓×5 — the first command's output scrolled under its pinned header"
printf '%s\n' "$pin_scrolled"
expect_eq "$(printf '%s\n' "$pin_scrolled" | sed -n 2p | sed 's/ *$//')" "! seq 1 60" "the header was not pinned over the scrolled output"
expect_eq "$(printf '%s\n' "$pin_scrolled" | sed -n 3p | sed 's/ *$//')" "     6" "the output under the pin is not the next rows of the command's output"
expect_lacks "$pin_scrolled" -E '^ +⎿ +1 *$' "the output's first row is still on screen after scrolling past it"

# → steps on to the next command.
keys "$S157" Right
pin_next="$(wait_pane 5 "$S157" -E '^  ⎿  100 *$')"
note "→ — on to the second command"
printf '%s\n' "$pin_next"
expect_eq "$(printf '%s\n' "$pin_next" | sed -n 2p | sed 's/ *$//')" "! seq 100 160" "→ did not open the next command"

keys "$S157" q
wait_for 5 "$S157" -F "dummy_model_name" ||
	fail "q did not bring the conversation back"
tmux kill-session -t "$S157" 2>/dev/null
smoke_finish
