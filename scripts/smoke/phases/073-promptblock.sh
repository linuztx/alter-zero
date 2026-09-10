#!/usr/bin/env bash
# Phase 73 — a UserPromptSubmit hook BLOCKS the prompt

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# a UserPromptSubmit hook BLOCKS the prompt (docs/hooks.md). The
# dummy's prompt-block scenario scripts the single PromptBlocked event; the
# loop's arm rolls the submission back out of history AND scrollback (the pop +
# purge repaint — the recorder's shrink-rewrite erases it from the rollout
# too), returns the text to the composer, and commits the red reason-only
# notice. The one remaining "❯ hook demo…" line on screen must be the
# composer's own — exactly one occurrence, not two.
S73="${S}_promptblock"
launch "$S73" 100 30
submit "$S73" "hook demo: block my prompt"
block_pane="$(wait_pane 10 "$S73" -S -200 -- -F "blocked the prompt")" # up to ~10s
echo "==== Phase 73: captured pane (prompt blocked by hook) ===="
printf '%s\n' "$block_pane"
expect_has "$block_pane" -F "UserPromptSubmit hook blocked the prompt" "the block notice never rendered"
expect_has "$block_pane" -F "Reason: no prompts about hooks" "the notice lost the hook's reason"
block_echoes="$(printf '%s\n' "$block_pane" | grep -cF "hook demo: block my prompt" || true)"
if [ "$block_echoes" -ne 1 ]; then
	fail "expected exactly the composer's copy of the blocked text, saw $block_echoes (the sent-message echo must be rolled back, the composer must get the draft back)"
fi
tmux send-keys -t "$S73" C-c C-c
sleep 0.3
tmux kill-session -t "$S73" 2>/dev/null || true
