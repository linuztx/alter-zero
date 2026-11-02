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
# blocked call resolves with no output of its own, its cell names the event,
# the hook and the hooks.json, the allowed one keeps its command output, and
# the instruction meant for the model never reaches scrollback.
S72="${S}_hooks"
launch "$S72" 100 30
submit "$S72" "show me the hooks demo"
hooks_pane="$(wait_pane 20 "$S72" -S -200 -- -E "$SUMMARY_RE")" # up to ~20s
echo "==== Phase 72: captured pane + scrollback (lifecycle hooks) ===="
printf '%s\n' "$hooks_pane"
expect_has "$hooks_pane" -F "Blocked by PreToolUse hook: no destructive deletes outside ./tmp" "the PreToolUse block never rendered its refusal cell, or its cell did not name the event"
# The refusal is the blocked cell's whole body: the line right after its
# header is the `⎿ Blocked by PreToolUse hook…`, never command output, and the
# two under it name the hook and the hooks.json it is configured in. (A plain
# `grep -F` of a multi-line literal would match any one line on its own, so
# this reads the three lines under the header explicitly.)
hooks_blocked_cell="$(printf '%s\n' "$hooks_pane" | awk '/● Bash\(rm -rf build\/\)/{for (i = 0; i < 3 && (getline line) > 0; i++) print line; exit}')"
hooks_after_blocked="$(printf '%s\n' "$hooks_blocked_cell" | sed -n 1p)"
expect_has "$hooks_after_blocked" -F "Blocked by PreToolUse hook" "the blocked call produced output of its own; it must never have run (saw: $hooks_after_blocked)"
expect_has "$(printf '%s\n' "$hooks_blocked_cell" | sed -n 2p)" -F "Hook: ~/.alter-zero/hooks/no-deletes.sh" "the refusal cell did not name the hook that refused (saw: $hooks_blocked_cell)"
expect_has "$(printf '%s\n' "$hooks_blocked_cell" | sed -n 3p)" -F "Config: ~/.alter-zero/hooks.json" "the refusal cell did not name the hooks.json its hook is configured in (saw: $hooks_blocked_cell)"
expect_has "$hooks_pane" -F "Context added by hook" "the PostToolUse hook left no dim provenance row on the cell it annotated"
expect_has "$hooks_pane" -F "drwxr-xr-x" "the allowed call lost its own output"
# The model reads one line more than the cell shows — what to do
# (ToolCall::context_output). That line is addressed to the model and may
# never be committed to the conversation.
expect_lacks "$hooks_pane" -F "Do not retry the same call" "the model-facing instruction was committed to scrollback"
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
expect_has "$hooks_overlay" -F "Blocked by PreToolUse hook" "the Ctrl+O transcript lost the hook's refusal"
tmux send-keys -t "$S72" End
sleep 0.3
hooks_overlay_tail="$(tmux capture-pane -t "$S72" -p)"
echo "==== Phase 72: captured overlay tail (the hook note) ===="
printf '%s\n' "$hooks_overlay_tail"
expect_has "$hooks_overlay_tail" -F "Stop hook feedback" "the Ctrl+O transcript lost the hook note"
expect_has "$hooks_overlay_tail" -F "Hook: ~/.alter-zero/hooks/stop.sh" "the Ctrl+O transcript lost the hook that gave the note"
# The refusal cell names the same hooks.json, so the note's own Config line is
# the one right under its Hook line.
hooks_note_config="$(printf '%s\n' "$hooks_overlay_tail" | awk '/Hook: ~\/\.alter-zero\/hooks\/stop\.sh/{getline; print; exit}')"
expect_has "$hooks_note_config" -F "Config: ~/.alter-zero/hooks.json" "the Ctrl+O transcript lost the hooks.json the note's hook is configured in (saw: $hooks_note_config)"
tmux send-keys -t "$S72" q
sleep 0.3
tmux kill-session -t "$S72" 2>/dev/null || true
