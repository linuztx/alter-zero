#!/usr/bin/env bash
# Phase 125 — the spinner tip

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the spinner tip (docs/tips.md). A turn's first seconds show the status line
# alone; once it has run three, a dim `⎿  Tip: …` row hangs directly under it
# with the status slot's gap below, and it goes with the turn — never a row
# of scrollback. Each turn opens on the next tip in the catalog, and the
# walk's position lives in the per-user tips.json, so a relaunch carries on
# where the last session left off. `/settings` → Show tips turns the row off
# and persists that to tips.json (never settings.json); ALTER_ZERO_TIPS
# seeds the row for a run either way and is never saved.
#
# The suite exports ALTER_ZERO_TIPS=0 (lib.sh), so these launches build their
# own string that UNSETS it and let tips.json decide. A long pre-stream pause
# (6 s) holds each turn still past the delay, so the tip reads off a quiet
# strip — and an Esc then undoes the turn (nothing streamed yet,
# docs/interrupt.md) instead of leaving a reply to wait out. One turn runs to
# its summary, for the commit path.
S125="${S}_tips"
TIPS_BASE="env -u ALTER_ZERO_TIPS ALTER_ZERO_CONFIG_DIR=$SMOKE_CFG ALTER_ZERO_SESSIONS_DIR=$SMOKE_SESSIONS ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_SKILLS_DIR=$SMOKE_SKILLS ALTER_ZERO_AGENTS_DIR=$SMOKE_AGENTS ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_UPDATE_CHECK=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=6000"
APP_TIPS="$TIPS_BASE $BIN"
TIPS_JSON="$SMOKE_CFG/tips.json"
TIP_ROW="⎿  Tip: "

# The catalog in walk order, one tip per line, read from the source so a
# reordered or edited catalog moves this phase with it (`tips::TIPS`).
mapfile -t TIPS_CATALOG < <(
	sed -n '/^pub const TIPS: &\[&str\] = &\[/,/^\];/p' "$SMOKE_ROOT/src/tips.rs" |
		sed -n 's/^    "\(.*\)",$/\1/p' | sed 's/\\"/"/g'
)
if [ "${#TIPS_CATALOG[@]}" -lt 4 ]; then
	fail "could not read the tip catalog out of src/tips.rs (${#TIPS_CATALOG[@]} entries)"
fi

# rows_after CONTENT PATTERN N — the N rows after the first row containing
# PATTERN (fixed string).
rows_after() {
	printf '%s\n' "$1" | awk -v pat="$2" -v n="$3" 'found && n-- > 0 { print } !found && index($0, pat) { found = 1 }'
}
# tip_shown SESSION INDEX LABEL — wait for the tip row and check it is the
# INDEXth catalog tip, hanging directly under the status line over the
# slot's blank gap and the box's rule. Prints the frame.
tip_shown() {
	local sess="$1" index="$2" label="$3" frame after
	frame="$(wait_pane 10 "$sess" -F "$TIP_ROW")"
	dump "$label" "$frame"
	after="$(rows_after "$frame" "esc to interrupt" 3)"
	expect_eq "$(printf '%s\n' "$after" | sed -n 1p)" "  $TIP_ROW${TIPS_CATALOG[$index]}" \
		"$label: the row under the status line is not tip #$index"
	expect_eq "$(printf '%s\n' "$after" | sed -n 2p)" "" "$label: no blank gap under the tip"
	if ! printf '%s\n' "$after" | sed -n 3p | grep -qE '^(─)+$'; then
		fail "$label: the box's rule does not follow the tip's gap"
	fi
	# The tip waited out the delay: the clock beside it reads at least 3s.
	local secs
	secs="$(printf '%s\n' "$frame" | grep -F "esc to interrupt" | sed -n 's/.*… (\([0-9]*\)s · .*/\1/p' | head -1)"
	if [ -z "$secs" ] || [ "$secs" -lt 3 ]; then
		fail "$label: the tip showed before the turn had run three seconds (clock '${secs:-?}s')"
	fi
}
# no_tip_past_delay SESSION LABEL — wait until the turn's clock reads 4s or
# more and check no tip row hangs anywhere.
no_tip_past_delay() {
	local sess="$1" label="$2" frame
	frame="$(wait_pane 10 "$sess" -E '… \([4-9]s · ')"
	dump "$label" "$frame"
	expect_has "$frame" -E '… \([4-9]s · ' "$label: the turn never reached four seconds"
	expect_lacks "$frame" -F "$TIP_ROW" "$label: a tip showed"
}
# status_gone SESSION — no status line left in the pane.
status_gone() { ! pane_has "$1" -F "esc to interrupt"; }
# undo_turn SESSION — Esc the still-pausing turn (the interrupt's undo puts
# the message back in the composer), wait for the strip to go, and Ctrl+C the
# restored draft away.
undo_turn() {
	keys "$1" Escape
	if ! poll 5 status_gone "$1"; then
		fail "Esc did not end the turn in $1"
	fi
	keys "$1" C-c
	sleep 0.2
}
quit_app() {
	submit "$1" "/quit"
	wait_gone 5 "$1" || fail "the app in $1 did not quit"
}

# ---------------------------------------------------------------------------
note "ALTER_ZERO_TIPS=0 keeps a turn tipless and tips.json untouched"
launch "$S125" 100 40 "$TIPS_BASE ALTER_ZERO_TIPS=0 $BIN"
submit "$S125" "hello there"
no_tip_past_delay "$S125" "a turn past the delay with ALTER_ZERO_TIPS=0"
undo_turn "$S125"
quit_app "$S125"
if [ -e "$TIPS_JSON" ]; then
	fail "tips.json was written by a session that showed no tip: $(cat "$TIPS_JSON")"
fi

# ---------------------------------------------------------------------------
note "a fresh config home: no tip at first, the first tip after three seconds"
launch "$S125" 100 40 "$APP_TIPS"
submit "$S125" "hello there"
early="$(wait_pane 5 "$S125" -F "esc to interrupt")"
dump "the turn's first moments" "$early"
expect_has "$early" -F "esc to interrupt" "the turn's status line never showed"
expect_lacks "$early" -F "$TIP_ROW" "a tip showed in the turn's first moments"
tip_shown "$S125" 0 "the first turn, past the delay"
# The first turn runs to its summary: the tip leaves with the strip and no
# row of it is committed.
settled="$(wait_summaries 40 "$S125" 1 -S -200)"
dump "the first turn settled (with scrollback)" "$settled"
expect_has "$settled" -E "^$SUMMARY_RE" "the first turn never settled"
expect_lacks "$settled" -F "$TIP_ROW" "a tip row reached scrollback or outlived the turn"
if ! wait_file 3 "$TIPS_JSON" -F '"next": 1'; then
	fail "tips.json does not record the walk's next position: $(cat "$TIPS_JSON" 2>/dev/null)"
fi

note "the next turn opens on the next tip"
submit "$S125" "hello again"
tip_shown "$S125" 1 "the second turn"
undo_turn "$S125"
wait_file 3 "$TIPS_JSON" -F '"next": 2' || fail "the second tip did not move the walk on"
quit_app "$S125"

# ---------------------------------------------------------------------------
note "a relaunch carries the walk on"
launch "$S125" 100 40 "$APP_TIPS"
submit "$S125" "one more"
tip_shown "$S125" 2 "the relaunch's first turn"
undo_turn "$S125"

note "/settings → Show tips turns the row off, per user"
type_text "$S125" "/settings"
sleep 0.3
keys "$S125" Enter
wait_for 3 "$S125" -F "Type to search" || fail "/settings did not open"
type_text "$S125" "tips"
menu="$(wait_pane 3 "$S125" -E "Show tips +true")"
dump "/settings filtered to the Show tips row" "$menu"
expect_has "$menu" -E "Show tips +true" "the Show tips row does not read true on a fresh config"
keys "$S125" Space
toggled="$(wait_pane 3 "$S125" -F "Show tips: false")"
dump "after Space on the Show tips row" "$toggled"
expect_has "$toggled" -F "Show tips: false" "no 'Show tips: false' toast after Space"
# Esc clears the typed query, a second one closes the menu.
keys "$S125" Escape
sleep 0.2
keys "$S125" Escape
wait_for 3 "$S125" -F "dummy_model_name ·" || fail "the menu did not close back to the composer"
wait_file 3 "$TIPS_JSON" -F '"enabled": false' || fail "tips.json does not record the switch: $(cat "$TIPS_JSON" 2>/dev/null)"
expect_file_has "$TIPS_JSON" -F '"next": 3' "the switch's write lost the walk's position"
if [ -f "$SMOKE_CFG/settings.json" ] && grep -q "tips" "$SMOKE_CFG/settings.json"; then
	fail "the per-user switch leaked into settings.json: $(cat "$SMOKE_CFG/settings.json")"
fi
submit "$S125" "and now?"
no_tip_past_delay "$S125" "a turn with Show tips off"
undo_turn "$S125"
quit_app "$S125"

# ---------------------------------------------------------------------------
note "the switch survives a relaunch"
launch "$S125" 100 40 "$APP_TIPS"
submit "$S125" "still off?"
no_tip_past_delay "$S125" "the relaunch with tips.json off"
undo_turn "$S125"
quit_app "$S125"

note "ALTER_ZERO_TIPS=1 seeds the row on for a run, never saved"
launch "$S125" 100 40 "$TIPS_BASE ALTER_ZERO_TIPS=1 $BIN"
submit "$S125" "on for this run"
tip_shown "$S125" 3 "a turn under ALTER_ZERO_TIPS=1"
undo_turn "$S125"
quit_app "$S125"
expect_file_has "$TIPS_JSON" -F '"enabled": false' "the environment's seed was written to tips.json"
expect_file_has "$TIPS_JSON" -F '"next": 4' "the run under the override did not move the walk on"
