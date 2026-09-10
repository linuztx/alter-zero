#!/usr/bin/env bash
# Phase 36 — Ctrl+D opens the CONTEXT-DEBUG view

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Ctrl+D opens the CONTEXT-DEBUG view (docs/context.md) — the raw
# LLM context window on the alternate screen: role-tagged entries with the
# conversation verbatim and tool calls in the provider-native form (an assistant
# `→ name(args)` request + a `tool:` result entry, the shape a real backend
# sends), q returns to the conversation.
S36="${S}_ctxdebug"
launch "$S36" 80 24
submit "$S36" "hello there"
wait_for 20 "$S36" -E "^Done for [0-9]+s" # let the turn finish so the tools are in history
tmux send-keys -t "$S36" C-d
sleep 0.4
tmux send-keys -t "$S36" Home # the view opens at the bottom; jump to the top
sleep 0.3
ctxdebug_pane="$(tmux capture-pane -t "$S36" -p)"
echo "==== Phase 36: Ctrl+D context-debug view ===="
printf '%s\n' "$ctxdebug_pane"
tmux send-keys -t "$S36" q # closes the view
sleep 0.4
ctxdebug_returned="$(tmux capture-pane -t "$S36" -p)"
tmux kill-session -t "$S36" 2>/dev/null

# Phase 36: the Ctrl+D context-debug view shows the raw context window.
expect_has "$ctxdebug_pane" -F "C O N T E X T" "Ctrl+D did not open the context-debug view (its slash-tiled title is missing)"
expect_has "$ctxdebug_pane" -E "^user:" "the context view is missing the role-tagged user entry"
expect_has "$ctxdebug_pane" -F "hello there" "the context view is missing the user message's raw text"
expect_has "$ctxdebug_pane" -F 'read({"path":"about.py"})' "the context view is missing the native tool call (→ read(...))"
expect_has "$ctxdebug_pane" -E "^tool:" "the context view is missing the tool-result role entry"
expect_lacks "$ctxdebug_pane" -F "[tool " "the context view still shows the old bracketed tool record"
expect_has "$ctxdebug_pane" -F "q/esc/ctrl+d to quit" "the context view's key-hint row is missing"
expect_lacks "$ctxdebug_returned" -F "C O N T E X T" "q did not close the context-debug view"
expect_has "$ctxdebug_returned" -F "Done for" "the conversation did not repaint after closing the context-debug view"
