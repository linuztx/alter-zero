#!/usr/bin/env bash
# Phase 138 — the pickers on a short terminal

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The composer-replacing pickers on a terminal too short for their whole page
# (docs/view-flow.md *A page that sheds rows before it flows*). Bottom-
# anchoring such a page kept its tail — a description, a preview, the hint —
# and flowed its search line and its highlighted row into scrollback, so the
# row Enter would act on was off screen. Each picker now sheds what its
# choices don't need and keeps the search line, three rows between the ↑/↓
# overflow slots, and the hint. Drive each one on a 12-row pane. Counts are
# matched loosely: a catalog that grows with each new feature must not fail
# this phase instead of that feature's own.
S138="${S}_shortpages"
launch "$S138" 90 12
# /settings: three settings between the slots, the highlighted one on screen,
# no counter and no description.
submit "$S138" "/settings"
short_settings="$(wait_pane 5 "$S138" -F "Enter/Space to change")"
echo "==== Phase 138: /settings on 12 rows ===="
printf '%s\n' "$short_settings"
expect_has "$short_settings" -E '^  ❯ *$' "/settings lost its search line on 12 rows"
expect_has "$short_settings" -E '^→ Hide thinking +' "/settings lost its highlighted row on 12 rows"
expect_has "$short_settings" -E '^  ↓ [0-9]+ more below$' "/settings does not name the rows under its window"
expect_lacks "$short_settings" -E '\([0-9]+/[0-9]+\)' "/settings still shows its counter on 12 rows"
expect_lacks "$short_settings" -F "chain-of-thought" "/settings still shows its description on 12 rows"
# ↓ walks the window down the list: it rides the selection, and both slots
# name what they hide.
tmux send-keys -t "$S138" Down Down Down Down Down Down
sleep 0.4
short_settings_walk="$(pane "$S138")"
echo "==== Phase 138: /settings after six ↓ ===="
printf '%s\n' "$short_settings_walk"
expect_has "$short_settings_walk" -E '^→ Permission mode +' "the /settings window did not follow the selection"
expect_has "$short_settings_walk" -E '^  ↑ [0-9]+ more above$' "/settings does not name the rows over its window"
expect_has "$short_settings_walk" -E '^  ↓ [0-9]+ more below$' "/settings lost the row naming what is under its window"
tmux send-keys -t "$S138" Escape
sleep 0.4
expect_has "$(pane "$S138")" -F "dummy_model_name" "Esc did not close /settings back to the composer"
tmux kill-session -t "$S138" 2>/dev/null
