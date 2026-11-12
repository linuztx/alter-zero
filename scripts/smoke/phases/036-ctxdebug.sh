#!/usr/bin/env bash
# Phase 36 — Ctrl+D opens the CONTEXT-DEBUG view

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Ctrl+D opens the CONTEXT-DEBUG view (docs/architecture/context.md) — the raw
# LLM context window on the alternate screen: each entry on its role chip
# (` USER `, ` ASSISTANT `, ` TOOL `) over its text hung on the `⎿` gutter,
# the conversation verbatim and tool calls in the provider-native form (an
# assistant `→ name(args)` request closed by its call id + a TOOL result
# entry naming that call, the shape a real backend sends), the bottom rule counting the
# messages, → stepping to the next one, q returning to the conversation. The
# demo's calls are one parallel batch, so they share one assistant entry and
# their TOOL results sit below its arguments (docs/providers/prompt-caching.md) — past
# the first page of a 24-row terminal, which is why the result check pages
# down through the view.
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
# → opens the next message at the top of the body (the row under the title),
# twice: the assistant entry, then the first result.
tmux send-keys -t "$S36" Home
sleep 0.2
tmux send-keys -t "$S36" Right
sleep 0.3
ctxdebug_step1="$(tmux capture-pane -t "$S36" -p | sed -n 2p)"
tmux send-keys -t "$S36" Right
sleep 0.3
ctxdebug_step2="$(tmux capture-pane -t "$S36" -p | sed -n 2p)"
tmux send-keys -t "$S36" q # closes the view
sleep 0.4
ctxdebug_returned="$(tmux capture-pane -t "$S36" -p)"
tmux kill-session -t "$S36" 2>/dev/null

# Phase 36: the Ctrl+D context-debug view shows the raw context window.
expect_has "$ctxdebug_pane" -F "C O N T E X T" "Ctrl+D did not open the context-debug view (its slash-tiled title is missing)"
expect_has "$ctxdebug_pane" -E "^ USER  #1 · " "the context view is missing the user entry's role chip"
expect_has "$ctxdebug_pane" -E "^  ⎿  hello there" "the user message is not hung on the ⎿ gutter"
expect_has "$ctxdebug_pane" -F "hello there" "the context view is missing the user message's raw text"
expect_has "$ctxdebug_pane" -F 'read({"path":"about.py"})' "the context view is missing the native tool call (→ read(...))"
expect_has "$ctxdebug_pages" -E "^ TOOL  #3 · read · call_0 · " "the TOOL result entry is missing, or does not name the call it answers"
expect_has "$ctxdebug_pane" -E 'read\(\{"path":"about\.py"\}\)  call_0$' "the read call does not close on the id its result names"
expect_has "$ctxdebug_pane" -E "^─ [0-9]+ messages · ≈[0-9.k]+ tokens ─" "the bottom rule does not count the messages"
expect_has "$ctxdebug_pane" -F "←/→ prev/next message" "the key hints do not teach the message step"
expect_has "$ctxdebug_step1" -E "^ ASSISTANT  #2 · " "→ did not open the next message at the top (got: $ctxdebug_step1)"
expect_has "$ctxdebug_step2" -E "^ TOOL  #3 · read · " "a second → did not open the message after it (got: $ctxdebug_step2)"
expect_lacks "$ctxdebug_pages" -F "[tool " "the context view still shows the old bracketed tool record"
expect_has "$ctxdebug_pane" -F "q/esc/ctrl+d to quit" "the context view's key-hint row is missing"
expect_lacks "$ctxdebug_returned" -F "C O N T E X T" "q did not close the context-debug view"
expect_has "$ctxdebug_returned" -E "$SUMMARY_RE" "the conversation did not repaint after closing the context-debug view"
