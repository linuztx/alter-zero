#!/usr/bin/env bash
# Phase 26 — a large BRACKETED PASTE collapses to a compact "[Pasted Content N chars]" plac

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# a large BRACKETED PASTE collapses to a compact
# "[Pasted Content N chars]" placeholder in the composer instead of dumping the
# raw text (docs/paste.md). term::init enables bracketed paste, so tmux's
# `paste-buffer -p` (which wraps the buffer in the ESC[200~ … ESC[201~ control
# codes) is delivered as one Event::Paste — unlike Phase 6's `send-keys -l`
# burst, which is real keystrokes typed verbatim and stays unaffected.
S23="${S}_paste"
launch "$S23" 80 24
PASTE_CONTENT="$(printf 'P%.0s' $(seq 1 1500))" # 1500 chars, over the 1000 threshold
tmux set-buffer -- "$PASTE_CONTENT"
tmux paste-buffer -p -t "$S23"
paste_pane="$(wait_pane 2 "$S23" -F "[Pasted Content 1500 chars]")" # up to ~2s for the placeholder to render
echo "==== captured pane (large bracketed paste → placeholder) ===="
printf '%s\n' "$paste_pane"
# A single Backspace removes the WHOLE placeholder atomically (docs/paste.md) —
# not one of its ~27 characters. After one keystroke the composer is empty again.
tmux send-keys -t "$S23" BSpace
paste_backspaced=""
for _ in $(seq 1 20); do # up to ~2s for the redraw
	paste_backspaced="$(tmux capture-pane -t "$S23" -p)"
	if ! printf '%s' "$paste_backspaced" | grep -qF "[Pasted Content"; then
		break
	fi
	sleep 0.1
done
echo "==== captured pane (after one Backspace — placeholder gone) ===="
printf '%s\n' "$paste_backspaced"

# A bracketed paste into the /model picker's type-to-search FILTERS the list
# (docs/llm.md): a model id is copied far more often than typed, and the paste
# used to be swallowed outright. The dummy backend has no provider key, so the
# picker shows its /login hint — the assertion is that the pasted text lands in
# the search field (it echoes on the `❯` filter row), not in the composer
# draft underneath.
tmux send-keys -t "$S23" -l "/model"
sleep 0.3
tmux send-keys -t "$S23" Enter
sleep 0.6
tmux set-buffer -- "openai/gpt-4o-mini"
tmux paste-buffer -p -t "$S23"
model_paste="$(wait_pane 2.5 "$S23" -F "openai/gpt-4o-mini")" # up to ~2.5s for the filter row to redraw
echo "==== captured pane (bracketed paste into the /model search) ===="
printf '%s\n' "$model_paste"
tmux kill-session -t "$S23" 2>/dev/null

# Phase 26: a large bracketed paste collapses to the "[Pasted Content N chars]"
# placeholder in the composer rather than dumping the raw text (docs/paste.md).
expect_has "$paste_pane" -F "[Pasted Content 1500 chars]" "a large bracketed paste did not collapse to the '[Pasted Content 1500 chars]' placeholder in the composer"
expect_lacks "$paste_pane" -E 'P{20,}' "the raw pasted text was dumped into the composer instead of the placeholder"
# … and a single Backspace removes the whole placeholder atomically (one
# keystroke, not one of its characters — docs/paste.md).
expect_lacks "$paste_backspaced" -F "[Pasted Content" "one Backspace did not remove the whole '[Pasted Content …]' placeholder (atomic delete regressed)"
# … and a bracketed paste into the /model picker's search reaches the FILTER
# (docs/llm.md) — it used to be swallowed, so nothing happened at all.
expect_has "$model_paste" -F "openai/gpt-4o-mini" "a bracketed paste into the /model search never reached the filter (swallowed paste regressed)"
expect_has "$model_paste" -F "No API key yet" "the /model picker was not open when the paste landed (the phase tested nothing)"
