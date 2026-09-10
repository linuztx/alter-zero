#!/usr/bin/env bash
# Phase 68 — the AskUserQuestion modal

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the AskUserQuestion modal (docs/ask.md). An "ask … questions"
# prompt makes the dummy raise the three-question demo and BLOCK on the ask
# gate — exactly as the real tool thread does. The modal must show the chip
# strip (the current chip highlighted, answered ones flipping to ☒), the
# numbered options with their descriptions, the multi-select checkboxes with
# their unnumbered Submit row, the side-by-side preview panel with the notes
# line, the review page, and — after Submit answers — the committed green
# "User answered Alter Zero's questions:" cell; the composer draft typed before
# the modal must be stashed and handed back.
S68="${S}_ask"
APP_ASK="env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=800 $BIN"
launch "$S68" 100 44 "$APP_ASK"
submit "$S68" "ask me some questions"
sleep 0.3
# Typed while the request is on its way — this draft must survive the modal.
tmux send-keys -t "$S68" -l "a draft typed while it asks"
ask_prompt=""
for _ in $(seq 1 250); do
	cap="$(tmux capture-pane -t "$S68" -p)"
	if printf '%s' "$cap" | grep -qF "What's your favorite way to drink coffee?"; then
		ask_prompt="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 68: captured pane (the ask modal over the stashed draft) ===="
printf '%s\n' "$ask_prompt"
# The option page is a menu, not a text field — no hardware cursor (the
# permission prompt's rule).
ask_cursor_shown="$(tmux display-message -p -t "$S68" '#{cursor_flag}')"
# A detour to the Submit page with NOTHING answered: it must lead with the
# amber warning and list no review rows (answered questions only), then Tab
# wraps back to the first question.
tmux send-keys -t "$S68" Right Right Right
sleep 0.4
ask_warning="$(tmux capture-pane -t "$S68" -p)"
echo "==== Phase 68: captured pane (the empty review page warns) ===="
printf '%s\n' "$ask_warning"
tmux send-keys -t "$S68" Tab
sleep 0.3
# 2 picks Latte and advances to the multi-select page.
tmux send-keys -t "$S68" -l "2"
sleep 0.4
ask_multi="$(tmux capture-pane -t "$S68" -p)"
echo "==== Phase 68: captured pane (the multi-select page) ===="
printf '%s\n' "$ask_multi"
# 1 toggles the first checkbox on; then walk down to the unnumbered Submit
# row (option rows 1-3, the Other row, then Submit) and confirm.
tmux send-keys -t "$S68" -l "1"
sleep 0.3
ask_checked="$(tmux capture-pane -t "$S68" -p)"
tmux send-keys -t "$S68" Down Down Down Down
sleep 0.2
tmux send-keys -t "$S68" Enter
sleep 0.4
ask_preview="$(tmux capture-pane -t "$S68" -p)"
echo "==== Phase 68: captured pane (the preview page) ===="
printf '%s\n' "$ask_preview"
# n opens the notes field; the typed note survives Esc back to the options.
tmux send-keys -t "$S68" -l "n"
sleep 0.2
tmux send-keys -t "$S68" -l "smoke note"
sleep 0.2
tmux send-keys -t "$S68" Escape
sleep 0.3
ask_notes="$(tmux capture-pane -t "$S68" -p)"
# 1 picks Arrow function — the last question answered, so the review page.
tmux send-keys -t "$S68" -l "1"
sleep 0.4
ask_review="$(tmux capture-pane -t "$S68" -p)"
echo "==== Phase 68: captured pane (the review page) ===="
printf '%s\n' "$ask_review"
tmux send-keys -t "$S68" Enter
ask_done=""
for _ in $(seq 1 250); do
	cap="$(tmux capture-pane -t "$S68" -p)"
	if printf '%s' "$cap" | grep -qF "User answered Alter Zero's questions:" &&
		printf '%s' "$cap" | grep -qF "a draft typed while it asks"; then
		ask_done="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 68: captured pane (submitted; the draft is back) ===="
printf '%s\n' "$ask_done"
tmux kill-session -t "$S68" 2>/dev/null
echo "==== Phase 68: the AskUserQuestion modal — chips, options, checkboxes, preview, notes, review, the committed cell ===="
if [ -z "$ask_prompt" ]; then
	fail "the ask modal never showed"
fi
expect_has "$ask_prompt" -F "☐ Coffee style" "the chip strip is missing the unanswered Coffee style chip"
expect_has "$ask_prompt" -F "✔ Submit" "the chip strip is missing the Submit tab"
expect_has "$ask_prompt" -F "❯ 1. Black" "the first option is not highlighted"
expect_has "$ask_prompt" -F "No milk, no sugar" "the option descriptions are missing"
expect_has "$ask_prompt" -F "Type something." "the free-text Other row is missing"
expect_has "$ask_prompt" -F "Chat about this" "the Chat about this row is missing"
expect_lacks "$ask_prompt" -F "a draft typed while it asks" "the composer draft leaked into the modal"
if [ "$ask_cursor_shown" != "0" ]; then
	fail "the option menu should hide the hardware cursor (got flag $ask_cursor_shown)"
fi
expect_has "$ask_multi" -F "☒ Coffee style" "answering did not flip the chip to ☒"
expect_has "$ask_multi" -F "[ ] Preview panel" "the multi-select checkboxes are missing"
expect_has "$ask_checked" -F "[✔] Preview panel" "the digit toggle did not check the box"
if ! printf '%s' "$ask_preview" | grep -qF "┌" || ! printf '%s' "$ask_preview" | grep -qF "const greet"; then
	fail "the preview panel did not render"
fi
expect_has "$ask_preview" -F "Notes: press n to add notes" "the notes placeholder is missing"
expect_has "$ask_notes" -F "Notes: smoke note" "the typed note did not survive Esc"
expect_has "$ask_review" -F "Review your answers" "the review page did not open"
expect_has "$ask_review" -F "→ Latte" "the review page is missing the recorded answer"
expect_has "$ask_review" -F "❯ 1. Submit answers" "the Submit answers option is missing"
expect_has "$ask_warning" -F "⚠ You have not answered all questions" "the empty review page did not warn"
expect_lacks "$ask_warning" -F "● What's your favorite way" "the empty review page listed an unanswered question"
expect_lacks "$ask_review" -F "⚠" "the fully answered review page still warns"
if [ -z "$ask_done" ]; then
	fail "the answered cell never committed (or the draft never came back)"
# The cell's answer rows word-wrap at this width ("→ Preview / panel"), so
# join the pane's lines and squeeze the gutter indentation before matching.
elif ! printf '%s' "$ask_done" | tr '\n' ' ' | tr -s ' ' | grep -qF "→ Preview panel"; then
	fail "the committed cell is missing the multi-select answer"
fi
