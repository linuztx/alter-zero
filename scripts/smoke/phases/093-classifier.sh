#!/usr/bin/env bash
# Phase 93 — the Ctrl+D view's CLASSIFIER PAGE

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the Ctrl+D view's CLASSIFIER PAGE (docs/permissions.md). Auto
# mode's reviewer reads a bounded task context — the recent user requests plus
# one line per action — and that context is the one input to a verdict the
# user cannot otherwise see: the request is silent and the cell shows only the
# outcome. It is Tab away inside the Ctrl+D view, sharing its chrome. Four
# claims: Ctrl+D opens on the LLM window, TAB flips to the classifier page
# (its own title + the mode note, plus a hint pointing back), Tab flips back,
# and the whole round trip is a NO-OP on the terminal — screen and scrollback
# byte-identical, the overlay-return invariant every alternate-screen view
# owes (invariant 4). The dummy keeps no classifier log (its offline auto demo
# answers from the pure `auto_verdict` heuristic), so the body is the
# placeholder here; the live block is covered by the classifier unit tests and
# tests/live_openrouter.rs.
S93="${S}_classifier"
launch "$S93" 90 24
submit "$S93" "hello"
cg_done=""
for _ in $(seq 1 120); do # up to ~12s
	if tmux capture-pane -t "$S93" -p | grep -qE "Done for [0-9]+"; then
		cg_done=1
		break
	fi
	sleep 0.1
done
if [ -z "$cg_done" ]; then
	fail "the reply never finished, so the round trip proves nothing"
fi
sleep 0.3
cg_before="$(tmux capture-pane -t "$S93" -p)"
cg_scroll_before="$(tmux capture-pane -t "$S93" -p -S -60)"
tmux send-keys -t "$S93" C-d
sleep 0.6
cg_llm="$(tmux capture-pane -t "$S93" -p)"
expect_has "$cg_llm" -F "C O N T E X T" "Ctrl+D did not open on the LLM context page"
expect_has "$cg_llm" -F "tab for classifier context" "the LLM page never advertises its other half"
tmux send-keys -t "$S93" Tab
sleep 0.5
cg_view="$(tmux capture-pane -t "$S93" -p)"
echo "==== Phase 93: captured pane (Ctrl+D, Tab — the classifier page) ===="
printf '%s\n' "$cg_view"
expect_has "$cg_view" -F "C L A S S I F I E R" "Tab did not flip to the classifier page"
expect_has "$cg_view" -F "tab for llm context" "the classifier page never points back"
# The mode note always shows, so the page can never read as "the classifier is
# deciding this" when it is not — here the suite's default manual mode, whose
# note says the log is recorded but consulted only in auto.
expect_has "$cg_view" -E "auto mode|disabled" "no mode note above the block"
tmux send-keys -t "$S93" Tab
sleep 0.5
if ! tmux capture-pane -t "$S93" -p | grep -qF "C O N T E X T"; then
	fail "Tab did not flip back to the LLM context page"
fi
tmux send-keys -t "$S93" C-d
sleep 0.6
cg_after="$(tmux capture-pane -t "$S93" -p)"
cg_scroll_after="$(tmux capture-pane -t "$S93" -p -S -60)"
if [ "$cg_before" != "$cg_after" ]; then
	fail "the Ctrl+D round trip changed the screen"
	printf 'before:\n%s\nafter:\n%s\n' "$cg_before" "$cg_after" >&2
fi
if [ "$cg_scroll_before" != "$cg_scroll_after" ]; then
	fail "the Ctrl+D round trip lost or doubled scrollback rows"
fi
# …and the page persists across opens: Ctrl+D comes back to the classifier
# page when that is where you left it, and q closes from there.
tmux send-keys -t "$S93" C-d
sleep 0.4
tmux send-keys -t "$S93" Tab
sleep 0.4
tmux send-keys -t "$S93" C-d
sleep 0.4
tmux send-keys -t "$S93" C-d
sleep 0.5
if ! tmux capture-pane -t "$S93" -p | grep -qF "C L A S S I F I E R"; then
	fail "the view did not reopen on the page it was left on"
fi
tmux send-keys -t "$S93" -l "q"
sleep 0.5
if tmux capture-pane -t "$S93" -p | grep -qF "C L A S S I F I E R"; then
	fail "q did not close the view from the classifier page"
fi
tmux kill-session -t "$S93" 2>/dev/null
