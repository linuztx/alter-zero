#!/usr/bin/env bash
# Phase 72 — LIFECYCLE HOOKS

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# LIFECYCLE HOOKS (docs/tools/hooks.md). The user's own commands wedged
# into the tool loop. The dummy's `hooks` scenario plays both halves of the
# contract against the real rendering path: a `PreToolUse` hook refusing a
# destructive command — the refusal text produced by `llm::hooks::block_texts`,
# the very function the live runner calls, so this cell is byte-for-byte the
# one a real hooks.json makes — and a `PostToolUse` hook annotating a call that
# did run, whose dim `⎿` provenance row is the user-visible trace while the
# context itself rides the model-facing text (never the cell). Assert the
# blocked call resolves with no output of its own, its cell names the event,
# the hook and the hooks.json it is configured in (docs/tools/hooks.md *What the
# user sees*), the allowed one keeps its command output under a row naming
# its event, the model-only instruction never reaches scrollback, and the
# closing Stop hook's block is a `● Ran 1 Stop hook` cell inline that Ctrl+O
# expands into the hook that ran (docs/tools/hooks.md *The hook-run cell*).
S72="${S}_hooks"
launch "$S72" 100 30
submit "$S72" "show me the hooks demo"
hooks_pane="$(wait_pane 20 "$S72" -S -200 -- -E "$SUMMARY_RE")" # up to ~20s
echo "==== Phase 72: captured pane + scrollback (lifecycle hooks) ===="
printf '%s\n' "$hooks_pane"
expect_has "$hooks_pane" -F "Blocked by PreToolUse hook: no destructive deletes outside ./tmp" "the PreToolUse block never rendered its refusal cell, or the cell no longer names its event"
# The refusal's output is its own three rows: the line right after the header
# is the `⎿ Blocked by PreToolUse hook…`, never command output, and the two
# under it name the hook and its file. (A plain `grep -F` of a multi-line
# literal would match each line on its own, so this reads the following lines
# explicitly.)
hooks_after_blocked="$(printf '%s\n' "$hooks_pane" | awk '/● Bash\(rm -rf build\/\)/{for (i = 0; i < 3 && (getline line) > 0; i++) print line; exit}')"
expect_has "$(printf '%s\n' "$hooks_after_blocked" | sed -n 1p)" -F "Blocked by PreToolUse hook" "the blocked call produced output of its own; it must never have run (saw: $hooks_after_blocked)"
expect_has "$(printf '%s\n' "$hooks_after_blocked" | sed -n 2p)" -F "Hook: ~/.alter-zero/hooks/no-deletes.sh" "the refusal cell did not name the hook that refused (saw: $hooks_after_blocked)"
expect_has "$(printf '%s\n' "$hooks_after_blocked" | sed -n 3p)" -F "Config: ~/.alter-zero/hooks.json" "the refusal cell did not name the hooks.json its hook is configured in (saw: $hooks_after_blocked)"
expect_has "$hooks_pane" -F "Context added by PostToolUse hook" "the PostToolUse hook left no dim provenance row naming its event on the cell it annotated"
expect_has "$hooks_pane" -F "drwxr-xr-x" "the allowed call lost its own output"
# The model reads one thing the cell does not show — the line on what to do
# next (ToolCall::context_output). It may not be committed to the
# conversation: the cell is the transcript's record.
expect_lacks "$hooks_pane" -F "Do not retry the same call" "the model-facing instruction was committed to scrollback"
# The demo's Stop hook blocks as the turn would finish (docs/tools/hooks.md *The
# hook-run cell*): its `● Ran 1 Stop hook` cell is committed under the reply
# with the feedback the agent continues on right under it, and the round it
# bought follows. The hook's own command is Ctrl+O's, never inline.
expect_has "$hooks_pane" -F "● Ran 1 Stop hook (ctrl+o to expand)" "the Stop hook left no cell inline — its block was invisible again"
hooks_stop_row="$(printf '%s\n' "$hooks_pane" | awk '/● Ran 1 Stop hook \(ctrl\+o to expand\)/{if ((getline line) > 0) print line; exit}')"
expect_has "$hooks_stop_row" -F "⎿  Stop hook feedback: Before you finish, explain what a Stop hook does." "the Stop hook's cell lost the feedback row under its header (saw: $hooks_stop_row)"
expect_has "$hooks_pane" -F "so this paragraph is the round it" "the Stop hook's block bought no continuation round"
expect_lacks "$hooks_pane" -F "hooks/stop.sh" "the Stop hook's command reached the inline cell — it belongs to Ctrl+O"
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
echo "==== Phase 72: captured overlay tail (the Stop hook's expanded cell) ===="
printf '%s\n' "$hooks_overlay_tail"
# Expanded, the cell lists the hook that ran — its command and how long it
# ran, then the hooks.json it is configured in — over what it said. The
# refusal cell names the same hooks.json, so a bare grep for the line could
# pass on the cell while this one had lost its own: read the three lines
# right under the expanded header.
hooks_expanded="$(printf '%s\n' "$hooks_overlay_tail" | awk '/● Ran 1 Stop hook *$/{for (i = 0; i < 3 && (getline line) > 0; i++) print line; exit}')"
expect_has "$(printf '%s\n' "$hooks_expanded" | sed -n 1p)" -E "⎿  ~/\.alter-zero/hooks/stop\.sh \([0-9.]+m?s\)" "the Ctrl+O cell lost the hook that ran, or how long it ran (saw: $hooks_expanded)"
expect_has "$(printf '%s\n' "$hooks_expanded" | sed -n 2p)" -F "Config: ~/.alter-zero/hooks.json" "the Ctrl+O cell lost the hooks.json its hook is configured in (saw: $hooks_expanded)"
expect_has "$(printf '%s\n' "$hooks_expanded" | sed -n 3p)" -F "⎿  Stop hook feedback: Before you finish" "the Ctrl+O cell lost what the hook said (saw: $hooks_expanded)"
tmux send-keys -t "$S72" q
sleep 0.3
tmux kill-session -t "$S72" 2>/dev/null || true
