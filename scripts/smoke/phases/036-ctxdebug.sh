#!/usr/bin/env bash
# Phase 36 — Ctrl+D opens the CONTEXT-DEBUG view

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Ctrl+D opens the CONTEXT-DEBUG view (docs/context.md) — the raw
# LLM context window on the alternate screen: role-tagged entries with the
# conversation verbatim and tool calls in the provider-native form (an assistant
# `→ name(args)` request + a `tool:` result entry, the shape a real backend
# sends), q returns to the conversation. The demo's calls are one parallel
# batch, so they share one assistant entry and their `tool:` results sit
# below its arguments (docs/prompt-caching.md) — past the first page of a
# 24-row terminal, which is why the result check pages down through the view.
S36="${S}_ctxdebug"
launch "$S36" 80 24
submit "$S36" "hello there"
wait_for 20 "$S36" -E "^$SUMMARY_RE" # let the turn finish so the tools are in history
tmux send-keys -t "$S36" C-d
sleep 0.4
tmux send-keys -t "$S36" Home # the view opens at the bottom; jump to the top
sleep 0.3
ctxdebug_pane="$(tmux capture-pane -t "$S36" -p)"
echo "==== Phase 36: Ctrl+D context-debug view ===="
printf '%s\n' "$ctxdebug_pane"
# Every page of the view, top to bottom: the batch's results come after the
# whole assistant entry, whose `edit` arguments alone fill a page.
ctxdebug_pages="$ctxdebug_pane"
for _ in 1 2 3 4 5 6; do
  tmux send-keys -t "$S36" NPage
  sleep 0.2
  ctxdebug_pages="$ctxdebug_pages"$'\n'"$(tmux capture-pane -t "$S36" -p)"
done
tmux send-keys -t "$S36" q # closes the view
sleep 0.4
ctxdebug_returned="$(tmux capture-pane -t "$S36" -p)"
tmux kill-session -t "$S36" 2>/dev/null

# Phase 36: the Ctrl+D context-debug view shows the raw context window.
expect_has "$ctxdebug_pane" -F "C O N T E X T" "Ctrl+D did not open the context-debug view (its slash-tiled title is missing)"
expect_has "$ctxdebug_pane" -E "^user:" "the context view is missing the role-tagged user entry"
expect_has "$ctxdebug_pane" -F "hello there" "the context view is missing the user message's raw text"
expect_has "$ctxdebug_pane" -F 'read({"path":"about.py"})' "the context view is missing the native tool call (→ read(...))"
expect_has "$ctxdebug_pages" -E "^tool:" "the context view is missing the tool-result role entry"
expect_lacks "$ctxdebug_pages" -F "[tool " "the context view still shows the old bracketed tool record"
expect_has "$ctxdebug_pane" -F "q/esc/ctrl+d to quit" "the context view's key-hint row is missing"
expect_lacks "$ctxdebug_returned" -F "C O N T E X T" "q did not close the context-debug view"
expect_has "$ctxdebug_returned" -E "$SUMMARY_RE" "the conversation did not repaint after closing the context-debug view"
