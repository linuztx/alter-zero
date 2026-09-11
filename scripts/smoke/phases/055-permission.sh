#!/usr/bin/env bash
# Phase 55 — tool permission requests

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# tool permission requests (docs/permissions.md). A "permission"
# prompt makes the dummy raise a scripted `write` request and BLOCK on the gate
# — exactly as a real backend's tool thread does. The composer draft typed
# before it arrives must be stashed and handed back; the prompt must show the
# framed body, the numbered contents, the question, the three options with the
# cyan `❯` on the first, and the hint row; Tab must swap the options for the
# amend field; `2` must approve and remember, so the cell commits and the draft
# returns.
S55="${S}_permission"
# A long pre-stream pause so the draft below is unambiguously typed BEFORE the
# request lands — once the prompt is up it owns every key (`a` would answer it).
APP_PERM="env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=2500 $BIN"
launch "$S55" 100 44 "$APP_PERM"
submit "$S55" "permission demo please"
sleep 0.4
# Typed WHILE the request is on its way — this draft must survive the prompt.
tmux send-keys -t "$S55" -l "a draft I was typing"
perm_prompt=""
for _ in $(seq 1 250); do
	cap="$(tmux capture-pane -t "$S55" -p)"
	if printf '%s' "$cap" | grep -qF "Do you want to create hello.py?"; then
		perm_prompt="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 55: captured pane (the permission prompt over the stashed draft) ===="
printf '%s\n' "$perm_prompt"
# The hardware cursor: the option list is a menu, not a text field, so the
# frame shows **no cursor at all** — the terminal cursor is the one thing on
# screen that moves by itself, and a kitty cursor-trail animation draws every
# jump it makes. `#{cursor_flag}` is tmux's report of DECTCEM (1 shown, 0
# hidden). Where it *rests* still follows the highlight, straight down the
# options, so its return when the prompt closes starts somewhere sensible
# (docs/permissions.md).
perm_cursor_shown="$(tmux display-message -p -t "$S55" '#{cursor_flag}')"
perm_cursor_1="$(tmux display-message -p -t "$S55" '#{cursor_x} #{cursor_y}')"
perm_marker_row=$(printf '%s\n' "$perm_prompt" | grep -n '❯ 1\.' | head -1 | cut -d: -f1)
tmux send-keys -t "$S55" Down
sleep 0.4
perm_cursor_2="$(tmux display-message -p -t "$S55" '#{cursor_x} #{cursor_y}')"
perm_marker_row_2=$(tmux capture-pane -t "$S55" -p | grep -n '❯ 2\.' | head -1 | cut -d: -f1)
tmux send-keys -t "$S55" Up
sleep 0.4
echo "==== Phase 55: cursor shown=$perm_cursor_shown · seat on option 1 = ($perm_cursor_1) row ${perm_marker_row:-none} · on option 2 = ($perm_cursor_2) row ${perm_marker_row_2:-none} ===="
tmux send-keys -t "$S55" Tab
sleep 0.3
tmux send-keys -t "$S55" -l "use pathlib"
sleep 0.3
# Tab's amend field IS typed into, so the caret comes back with it.
perm_cursor_shown_amend="$(tmux display-message -p -t "$S55" '#{cursor_flag}')"
perm_amend="$(tmux capture-pane -t "$S55" -p)"
echo "==== Phase 55: captured pane (Tab's amend field) ===="
printf '%s\n' "$perm_amend"
tmux send-keys -t "$S55" Escape
sleep 0.3
tmux send-keys -t "$S55" -l "2"
perm_done=""
for _ in $(seq 1 250); do
	cap="$(tmux capture-pane -t "$S55" -p)"
	if printf '%s' "$cap" | grep -qF "Wrote 8 lines to hello.py" &&
		printf '%s' "$cap" | grep -qF "a draft I was typing"; then
		perm_done="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 55: captured pane (approved; the draft is back) ===="
printf '%s\n' "$perm_done"
# …and the composer's caret comes back with the composer.
perm_cursor_shown_after="$(tmux display-message -p -t "$S55" '#{cursor_flag}')"
# The remembered scope: a second request never asks — the turn runs straight
# through to its summary with no prompt.
tmux send-keys -t "$S55" C-c
sleep 0.2
submit "$S55" "permission demo again"
perm_silent=""
for _ in $(seq 1 250); do
	cap="$(tmux capture-pane -t "$S55" -p)"
	if printf '%s' "$cap" | grep -qF "the file is written" &&
		! printf '%s' "$cap" | grep -qF "Do you want to create"; then
		perm_silent="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 55: captured pane (allow-listed — no second prompt) ===="
printf '%s\n' "$perm_silent"
tmux kill-session -t "$S55" 2>/dev/null
# Option 2 on a write prompt is the switch to edit mode, and it PERSISTS per
# project (docs/permissions.md): the file must record this cwd with mode
# "edit" before the cleanup below.
if ! grep -qF '"mode": "edit"' "$SMOKE_CFG/permissions.json" 2>/dev/null; then
	fail "option 2 did not persist the edit mode to permissions.json"
fi
# Phase 55's `2` persisted its standing approval (mode=edit for this project)
# into $SMOKE_CFG/permissions.json — drop it so the later permission phases
# (56, 58, 60, 62, 63) still get their prompts.
rm -f "$SMOKE_CFG/permissions.json"
echo "==== Phase 55: permission prompt — draft stash/restore, options, amend, remember ===="
if [ -z "$perm_prompt" ]; then
	fail "the permission prompt never showed"
fi
expect_has "$perm_prompt" -F "Create file" "the prompt lacks its coloured title"
# The call that raised it stays on screen ABOVE the modal — the prompt is a
# question about something visible, not a box out of nowhere — over the same
# dim ⎿ Waiting… a batch sibling shows (Claude Code's look; the approve seam
# runs before ToolStart, so the call genuinely is waiting).
expect_has "$perm_prompt" -F "● Write(hello.py)" "the pending call is hidden behind the prompt"
expect_has "$perm_prompt" -F "⎿  Waiting…" "the call under the prompt lacks its ⎿ Waiting… row"
expect_has "$perm_prompt" -F "#!/usr/bin/env python3" "the prompt lacks the numbered file body"
expect_has "$perm_prompt" -F "❯ 1. Yes" "the ❯ selection is missing from the first option"
expect_has "$perm_prompt" -F "2. Yes, allow all edits during this session (shift+tab)" "the remember option is missing (or still says the old (a) shortcut)"
expect_has "$perm_prompt" -F "Esc to cancel · Tab to amend" "the hint row is missing"
expect_lacks "$perm_prompt" -F "a draft I was typing" "the stashed draft is still on screen under the prompt"
# The cursor is not shown over the options — an options list has nothing for
# one to point at, and every move it makes is drawn by a terminal cursor-trail
# animation. It returns for Tab's amend field and for the composer after.
if [ "$perm_cursor_shown" != "0" ]; then
	fail "the terminal still shows a cursor over the prompt's options (cursor_flag=$perm_cursor_shown)"
fi
if [ "$perm_cursor_shown_amend" != "1" ]; then
	fail "Tab's amend field is typed into but shows no cursor (cursor_flag=$perm_cursor_shown_amend)"
fi
if [ "$perm_cursor_shown_after" != "1" ]; then
	fail "the cursor never came back after the prompt closed (cursor_flag=$perm_cursor_shown_after)"
fi
# Its resting seat still follows the highlight: `#{cursor_y}` is 0-based and
# grep -n 1-based, so the row it rests on is the marker row minus one, at the
# one-space inset plus the two-column `❯ `.
if [ "$perm_cursor_1" != "3 $((${perm_marker_row:-0} - 1))" ]; then
	fail "the cursor rests at ($perm_cursor_1), not on the '❯ 1. Yes' row (${perm_marker_row:-none}) at column 3"
fi
if [ "$perm_cursor_2" != "3 $((${perm_marker_row_2:-0} - 1))" ]; then
	fail "↓ moved the selection to row ${perm_marker_row_2:-none} but left the cursor resting at ($perm_cursor_2)"
fi
if [ -z "$perm_amend" ] || ! printf '%s' "$perm_amend" | grep -qF "❯ use pathlib"; then
	fail "Tab did not open the amend field"
fi
expect_has "$perm_amend" -F "Enter to reject with this feedback" "the amend field's hint row is missing"
if [ -z "$perm_done" ]; then
	fail "approving did not commit the cell with the draft restored"
fi
# The footer's right-edge mode flipped with the option-2 mode switch.
expect_has "$perm_done" -E "dummy_model_name · .*edit$" "the footer's right-edge mode did not flip to edit after option 2"
if [ -z "$perm_silent" ]; then
	fail "the allow-listed second request asked again"
fi
