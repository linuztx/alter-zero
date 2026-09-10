#!/usr/bin/env bash
# Phase 33 — transient TOASTS above the box

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# transient TOASTS above the box (docs/toast.md). A one-line status
# message that self-clears after a few seconds instead of committing a scrollback
# bullet. /copy's confirmation is a toast — it APPEARS then VANISHES (a committed
# message would persist). /help and /resume run mid-turn surface a toast rejection
# rather than a bulletpoint; /model now OPENS its inline picker mid-turn (it only
# swaps the composer, never the running turn). A 2s startup delay widens the
# pre-stream pause so the turn is reliably active when the mid-turn keys land.
S33="${S}_toast"
launch "$S33" 80 24 "env $CFG_ENV ALTER_ZERO_STARTUP_DELAY_MS=2000 $BIN"
# Get a finished reply into history so /copy has something to confirm.
submit "$S33" "hello there"
toast_prev=""
for _ in $(seq 1 90); do # up to ~18s: reply committed AND the screen settled
	toast_cur="$(tmux capture-pane -t "$S33" -p)"
	if printf '%s' "$toast_cur" | grep -qF "$SETTLED_REPLY" && [ "$toast_cur" = "$toast_prev" ]; then
		break
	fi
	toast_prev="$toast_cur"
	sleep 0.2
done
# /copy → the confirmation toast appears above the box.
tmux send-keys -t "$S33" -l "/copy"
sleep 0.3
tmux send-keys -t "$S33" Enter
toast_shown="$(wait_pane 2 "$S33" -F "Copied last message to clipboard")" # up to ~2s
echo "==== Phase 33: pane with the /copy toast shown ===="
printf '%s\n' "$toast_shown"
# Wait past the ~4s TTL: the toast self-clears (a committed bullet would remain).
sleep 5
toast_cleared="$(tmux capture-pane -t "$S33" -p)"
echo "==== Phase 33: pane ~5s later (the toast has cleared) ===="
printf '%s\n' "$toast_cleared"
# Mid-turn: start a turn, then during its 2s pre-stream pause run /help — its
# command list would interleave with the reply, so it's rejected with a toast.
submit "$S33" "second question"
sleep 0.3 # inside the 2s pause: the turn is active
tmux send-keys -t "$S33" -l "/help"
sleep 0.3
tmux send-keys -t "$S33" Enter
toast_help="$(wait_pane 1.5 "$S33" -F "/help is disabled while a task is in progress")" # up to ~1.5s (still inside the pause)
echo "==== Phase 33: pane with the mid-turn /help toast ===="
printf '%s\n' "$toast_help"
# Still mid-turn (same active turn): /model OPENS its inline picker rather than
# being blocked. No provider key is configured in the smoke env, so the picker
# opens on its needs-login hint — proof it opened rather than being rejected.
# And it replaces the COMPOSER ONLY: the spinner status line stays above it
# (the reported bug was the picker taking the whole region and hiding the
# running turn — docs/llm.md, the ↓ manager band's rule).
tmux send-keys -t "$S33" -l "/model"
sleep 0.3
tmux send-keys -t "$S33" Enter
toast_model="$(wait_pane 1.5 "$S33" -F "run /login to add one")" # up to ~1.5s
echo "==== Phase 33: pane with /model opened mid-turn ===="
printf '%s\n' "$toast_model"
# Esc closes the picker; /login opens the same way over the same live turn.
tmux send-keys -t "$S33" Escape
sleep 0.3
tmux send-keys -t "$S33" -l "/login"
sleep 0.3
tmux send-keys -t "$S33" Enter
toast_login=""
for _ in $(seq 1 15); do # up to ~1.5s (still inside the 2s pause)
	toast_login="$(tmux capture-pane -t "$S33" -p)"
	# The flow's ROOT, not the provider list: `/login` asks how you sign in
	# first (Phase 103), so the provider step's `Keys are saved to` hint is a
	# step further in and never appears on the page this phase opens.
	if printf '%s' "$toast_login" | grep -qF "Use a subscription"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 33: pane with /login opened mid-turn ===="
printf '%s\n' "$toast_login"
tmux kill-session -t "$S33" 2>/dev/null

# Phase 33: the /copy confirmation shows as a transient toast that self-clears.
expect_has "$toast_shown" -F "Copied last message to clipboard" "/copy did not show the 'Copied last message to clipboard' toast"
expect_lacks "$toast_cleared" -F "Copied last message to clipboard" "the toast did NOT self-clear after its TTL (a transient toast must vanish, not persist like a scrollback bullet)"
# Phase 33: /help run mid-turn is rejected with a toast, not committed.
expect_has "$toast_help" -F "/help is disabled while a task is in progress" "mid-turn /help did not show the disabled toast"
# Phase 33: /model OPENS its inline picker mid-turn (no rejection).
expect_has "$toast_model" -F "run /login to add one" "mid-turn /model did not open the inline picker (it should no longer be blocked while a task runs)"
# Phase 33: …and it replaces the COMPOSER only — the streaming strip's status
# line stays above it, so the picker never hides the turn it was opened beside
# (the reported bug; the ↓ manager band's rule — docs/llm.md).
expect_has "$toast_model" -F "esc to interrupt" "the mid-turn /model picker hid the status indicator — it must replace the composer only, keeping the streaming strip above it"
# Phase 33: /login opens mid-turn under the same live status line — on its
# method root (the sign-in fork, Phase 103), which is what a mid-turn `/login`
# now shows.
expect_has "$toast_login" -F "Use a subscription" "mid-turn /login did not open the inline onboarding flow"
expect_has "$toast_login" -F "esc to interrupt" "the mid-turn /login flow hid the status indicator — it must replace the composer only"
