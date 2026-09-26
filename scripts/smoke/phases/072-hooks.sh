#!/usr/bin/env bash
# Phase 72 — LIFECYCLE HOOKS

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# LIFECYCLE HOOKS (docs/hooks.md). The user's own commands wedged
# into the tool loop. The dummy's `hooks` scenario plays both halves of the
# contract against the real rendering path: a `PreToolUse` hook refusing a
# destructive command — the refusal text produced by `llm::hooks::block_texts`,
# the very function the live runner calls, so this cell is byte-for-byte the
# one a real hooks.json makes — and a `PostToolUse` hook annotating a call that
# did run, whose dim `⎿` provenance row is the user-visible trace while the
# context itself rides the model-facing text (never the cell). Assert the
# blocked call resolves with no output of its own, the allowed one keeps its
# command output, and the model-facing paragraph never reaches scrollback.
S72="${S}_hooks"
launch "$S72" 100 30
submit "$S72" "show me the hooks demo"
hooks_pane="$(wait_pane 20 "$S72" -S -200 -- -E "$SUMMARY_RE")" # up to ~20s
echo "==== Phase 72: captured pane + scrollback (lifecycle hooks) ===="
printf '%s\n' "$hooks_pane"
expect_has "$hooks_pane" -F "Blocked by hook: no destructive deletes outside ./tmp" "the PreToolUse block never rendered its refusal cell"
# The refusal must be the blocked cell's ONLY row: the line right after its
# header is the `⎿ Blocked by hook…`, never command output. (A plain
# `grep -F` of a two-line literal would match either line on its own, so this
# reads the following line explicitly.)
hooks_after_blocked="$(printf '%s\n' "$hooks_pane" | awk '/● Bash\(rm -rf build\/\)/{getline; print; exit}')"
expect_has "$hooks_after_blocked" -F "Blocked by hook" "the blocked call produced output of its own; it must never have run (saw: $hooks_after_blocked)"
expect_has "$hooks_pane" -F "Context added by hook" "the PostToolUse hook left no dim provenance row on the cell it annotated"
expect_has "$hooks_pane" -F "drwxr-xr-x" "the allowed call lost its own output"
# The long stop-and-wait instruction is what the MODEL reads
# (ToolCall::context_output). It must not be committed to the conversation —
# the one-line cell is the transcript's record.
expect_lacks "$hooks_pane" -F "A configured lifecycle hook blocked this tool call." "the model-facing hook text was committed to scrollback; only the short cell line may show"
# A Stop hook's feedback note is CELL-LESS inline (docs/hooks.md): the demo
# ends with one, and it must not have painted a row in the conversation.
expect_lacks "$hooks_pane" -F "Stop hook feedback" "the hook note leaked into the inline conversation"
tmux send-keys -t "$S72" C-o
sleep 0.6
tmux send-keys -t "$S72" Home
sleep 0.3
hooks_overlay="$(tmux capture-pane -t "$S72" -p)"
echo "==== Phase 72: captured overlay top (the hook block in the transcript) ===="
printf '%s\n' "$hooks_overlay"
expect_has "$hooks_overlay" -F "Blocked by hook" "the Ctrl+O transcript lost the hook's refusal"
tmux send-keys -t "$S72" End
sleep 0.3
hooks_overlay_tail="$(tmux capture-pane -t "$S72" -p)"
echo "==== Phase 72: captured overlay tail (the hook note) ===="
printf '%s\n' "$hooks_overlay_tail"
expect_has "$hooks_overlay_tail" -F "Stop hook feedback" "the Ctrl+O transcript lost the hook note"
tmux send-keys -t "$S72" q
sleep 0.3
tmux kill-session -t "$S72" 2>/dev/null || true
