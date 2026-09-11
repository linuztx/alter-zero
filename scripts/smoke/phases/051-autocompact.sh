#!/usr/bin/env bash
# Phase 51 — AUTO-compact + the footer context gauge

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# AUTO-compact + the footer context gauge (docs/compact.md).
# `ALTER_ZERO_CONTEXT_WINDOW=100` forces a tiny window onto the dummy: the
# footer shows the `{used}/{window} ({pct}%)` gauge, and one turn's estimate
# blows past codex's 90% threshold — the loop then starts the summarization
# turn ON ITS OWN (no /compact typed): the marker cell commits with the
# `· {before} → {after} tokens[ · {elapsed}] · auto` clause (the duration
# appears when the summarization turn took ≥1s) and the transcript is
# untouched.
S51="${S}_autocompact"
tmux new-session -d -s "$S51" -x 100 -y 24 "env ALTER_ZERO_CONTEXT_WINDOW=100 $APP"
sleep 0.4
gauge_idle="$(tmux capture-pane -t "$S51" -p)"
echo "==== captured pane (idle footer gauge under a forced 100-token window) ===="
printf '%s\n' "$gauge_idle"
submit "$S51" "hello there"
# The turn ends, the estimate crosses the threshold, and the loop auto-runs
# the compact turn — poll straight for the auto-tagged marker cell.
auto_pane=""
# The budget covers the WHOLE sequence — the demo turn (which now streams a
# thinking phase too, docs/thinking-stream.md), the auto-compact turn's own
# startup pause, and its summary stream — so it is generous on purpose: at ~10s
# the turn alone could eat it and the phase failed on speed, not behaviour.
auto_pane="$(wait_pane 25 "$S51" -S -80 -- -E "tokens( · [0-9]+[hms][0-9ms ]*)? · auto")" # up to ~25s
echo "==== captured pane (auto-compact after one turn) ===="
printf '%s\n' "$auto_pane"
tmux kill-session -t "$S51" 2>/dev/null
echo "==== Phase 51: footer gauge + threshold-triggered auto-compact ===="
expect_has "$gauge_idle" -E '/100 \([0-9]+\.[0-9]%\)' "the footer does not show the context gauge under ALTER_ZERO_CONTEXT_WINDOW"
expect_has "$auto_pane" -F "Context compacted" "no auto-compaction happened past the 90% threshold"
expect_has "$auto_pane" -E "tokens( · [0-9]+[hms][0-9ms ]*)? · auto" "the marker cell is missing the '· {before} → {after} tokens[ · {elapsed}] · auto' clause"
expect_has "$auto_pane" -F "hello there" "the transcript above the auto marker was not preserved"
