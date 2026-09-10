#!/usr/bin/env bash
# Phase 50 — /compact runs codex's summarization turn against the dummy and lands the marke

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# /compact runs codex's summarization turn against the dummy and
# lands the marker (docs/compact.md). An empty conversation is rejected with the
# 'Nothing to compact' toast; after a real turn, /compact streams the dummy's
# canned summary into the compact buffer (NEVER rendered — the pane must not
# show the summary text), commits the '● Context compacted' cell with the
# transcript above it untouched, and the Ctrl+D context-debug view then derives
# the COMPACTED context: the SUMMARY_PREFIX bridge in place of the old reply
# (the old assistant text gone from the derivation, the user text retained).
S50="${S}_compact"
launch "$S50" 80 24
# Empty conversation: nothing to compact → the transient toast.
tmux send-keys -t "$S50" -l "/compact"
sleep 0.3
tmux send-keys -t "$S50" Enter
compact_empty="$(wait_pane 3 "$S50" -S -20 -- -F "Nothing to compact")" # up to ~3s
echo "==== captured pane (/compact with nothing to compact) ===="
printf '%s\n' "$compact_empty"
# A real turn first (every dummy reply ends on the hand-off paragraph),
# settled like Phase 28: the tail committed AND the screen stable.
submit "$S50" "hello there"
compact_prev=""
for _ in $(seq 1 60); do # up to ~12s
	compact_cur="$(tmux capture-pane -t "$S50" -p)"
	if printf '%s' "$compact_cur" | grep -qF "$SETTLED_REPLY" && [ "$compact_cur" = "$compact_prev" ]; then
		break
	fi
	compact_prev="$compact_cur"
	sleep 0.2
done
tmux send-keys -t "$S50" -l "/compact"
sleep 0.3
tmux send-keys -t "$S50" Enter
compact_pane="$(wait_pane 20 "$S50" -S -80 -- -F "Context compacted")" # up to ~20s (the dummy pause + the summary stream)
echo "==== captured pane (after /compact — marker cell, summary never rendered) ===="
printf '%s\n' "$compact_pane"
# The derived context is now the compacted shape: Ctrl+D shows the bridge.
tmux send-keys -t "$S50" C-d
sleep 0.6
compact_ctx="$(tmux capture-pane -t "$S50" -p)"
echo "==== captured pane (Ctrl+D context-debug after /compact) ===="
printf '%s\n' "$compact_ctx"
tmux send-keys -t "$S50" q
sleep 0.4
tmux kill-session -t "$S50" 2>/dev/null
echo "==== Phase 50: /compact — empty-reject, marker cell, hidden summary, compacted Ctrl+D ===="
expect_has "$compact_empty" -F "Nothing to compact" "/compact on an empty conversation did not toast 'Nothing to compact'"
expect_has "$compact_pane" -F "Context compacted" "the '● Context compacted' cell never committed"
expect_has "$compact_pane" -F "hello there" "the transcript above the marker was not preserved (append-only compaction)"
expect_lacks "$compact_pane" -F "canned handoff summary" "the streamed summary text rendered into the conversation (it must stay hidden)"
expect_has "$compact_ctx" -F "Another language model" "Ctrl+D does not show the SUMMARY_PREFIX bridge (the derivation did not compact)"
expect_lacks "$compact_ctx" -F "Happy to help" "the old assistant reply is still in the derived context (it must compact away)"
expect_has "$compact_ctx" -F "hello there" "the recent user message dropped from the compacted context (the budget walk must keep it)"
