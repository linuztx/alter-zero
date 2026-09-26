#!/usr/bin/env bash
# Phase 16 — /clear MID-TURN kills the generation (codex instead *disables* /new//clear dur

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# /clear MID-TURN kills the generation (codex instead *disables*
# /new//clear during a task — the kill is our spec). Run /clear while the reply
# streams: the screen must blank with no trace of the turn (no echo, no reply
# text, no live status, no interrupt notice), and the backend must be cancelled
# + reaped + its channel drained — so nothing recommits while the rest of the
# turn's schedule would still have been streaming (the pre-fix bug: the screen
# cleared but chunks kept flowing in). The loop must survive the kill: a fresh
# message streams and finishes normally.
S13="${S}_clearkill"
launch "$S13" 80 24
submit "$S13" "hello there"
wait_for 4 "$S13" -F "Happy" # up to ~4s: wait until the reply is visibly streaming
submit "$S13" "/clear"
sleep 0.5
# /clear now clears the visible screen AND purges the terminal's own scrollback
# (codex's clear_scrollback_and_visible_screen_ansi: ED2 to clear the screen +
# the ED3 scrollback purge, emitted as one ANSI write). So both must be blank —
# a bare clear_region(All)/ED2 used to leave the whole conversation sitting one
# scroll up. Capture the VISIBLE screen first, then the scrollback (-S).
cleared_now="$(tmux capture-pane -t "$S13" -p)"
echo "==== captured visible screen (right after /clear mid-stream) ===="
printf '%s\n' "$cleared_now"
cleared_scrollback="$(tmux capture-pane -t "$S13" -p -S -200)"
echo "==== captured scrollback (-S -200) right after /clear — must not hold the old turn ===="
printf '%s\n' "$cleared_scrollback"
# The dummy turn would keep streaming (text, thinking, tools) for several more
# seconds; if the backend survived the /clear its output would recommit into
# the blank screen. Let that window pass, then look again.
sleep 2.5
cleared_later="$(tmux capture-pane -t "$S13" -p)"
echo "==== captured visible screen (2.5s after /clear — must still be blank) ===="
printf '%s\n' "$cleared_later"
# The loop survives the kill: a fresh turn streams and finishes
# ($SUMMARY_TURN2 — turn 2's summary, as in the Esc-interrupt phase).
submit "$S13" "again please"
after_clear="$(wait_pane 20.1 "$S13" -S -30 -- -F "$SUMMARY_TURN2")" # up to ~20s: wait for the fresh turn's summary
echo "==== captured pane (fresh turn after the /clear kill) ===="
printf '%s\n' "$after_clear"
tmux kill-session -t "$S13" 2>/dev/null

# Phase 16: /clear mid-turn killed the generation. Right after the clear the
# screen holds no trace of the turn …
expect_lacks "$cleared_now" -F "❯ hello there" "the old conversation ('❯ hello there') survived a mid-turn /clear"
# … and the SCROLLBACK is purged too (codex's ED3), not just the visible screen:
# scrolling up after /clear must show nothing of the old turn.
expect_lacks "$cleared_scrollback" -F "❯ hello there" "/clear did not purge scrollback — the old conversation ('❯ hello there') is still one scroll up (the ED3 purge is missing)"
expect_lacks "$cleared_now" -F "esc to interrupt" "the live status line is still up after a mid-turn /clear"
expect_lacks "$cleared_now" -F "Conversation interrupted" "/clear recorded the interrupt notice — it must wipe, not interrupt"
expect_has "$cleared_now" -F "dummy_model_name ·" "the idle input box + footer did not reseat after a mid-turn /clear"
# … and the backend is dead: nothing recommitted while the remainder of the
# turn's schedule played out (reply text, tool peeks, a summary).
for leak in "Happy" "⎿" "esc to interrupt"; do
	expect_lacks "$cleared_later" -F "$leak" "the backend kept streaming after a mid-turn /clear ('$leak' appeared on the cleared screen)"
done
expect_lacks "$cleared_later" -E "$SUMMARY_ANY_RE" "the backend kept streaming after a mid-turn /clear (a turn summary appeared on the cleared screen)"
# … and the loop survived the kill: the fresh turn streamed to completion.
expect_has "$after_clear" -F "❯ again please" "the message sent after a mid-turn /clear was not echoed"
expect_has "$after_clear" -F "$SUMMARY_TURN2" "the turn after a mid-turn /clear did not finish (no '$SUMMARY_TURN2 …' summary)"
