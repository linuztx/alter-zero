#!/usr/bin/env bash
# Phase 159 — the banner mascot animates over an empty conversation, and only there

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the banner mascot animates over an empty conversation, and only there
# (docs/mascot-animation.md). While no cell has been committed under it the
# banner stays repaintable where it sits above the composer: a light sweeps
# the mascot as the app opens, it shows its own move, it blinks, and from
# there it blinks and glances on a rhythm drawn by lot. The repaint is IN
# PLACE — the cells that changed, inside the frame the live region paints in
# — so this phase reads the raw byte stream (`tmux pipe-pane`), the only
# place a 90ms blink and a colour-only sweep can be seen at all. What it
# pins: the greeting's frames reach the terminal and the banner is neither
# re-committed nor left off its rest pose; frames are written only when the
# pose changes (no clock chain turning at idle); with the row off an idle
# session writes NOTHING; the first cell puts the banner back to rest for
# good — sent mid-move, and again when the app quits mid-move — the two
# things that open a fresh banner greet from it again: `/clear`, and a
# mascot just chosen in `/mascot`; the picker's preview greets as the
# highlight lands on a mascot; and while the first message is typed the
# mascot watches it — its eyes turned toward the text, to its right, never
# to the screen's left edge — looking away a few seconds after the last
# keystroke, and at once when the draft is cleared.
#
# The suite pins the animation off (lib.sh); these launches turn it on. The
# default mascot is the crest: rest ` ▙▄▙▄▟▄▟` over `▝▜▄███▄▛▘`, its frill
# flared ` ▙▙▙▄▟▟▟`, an eyelid `▆`.
S159="${S}_mascotmotion"
MM_APP="env $CFG_ENV_NOHIST ALTER_ZERO_MASCOT_ANIMATION=1 ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
MM_REST_TOP=' ▙▄▙▄▟▄▟   Alter Zero'
MM_REST_EYES='▝▜▄███▄▛▘'
MM_FLARE=' ▙▙▙▄▟▟▟   Alter Zero'
# The crest watching the composer: its eyes turned to the right, toward the
# text — and turned to the left, at the edge of the screen, which a watch
# must never show.
MM_WATCH_RIGHT='▝▜▙███▙▛▘'
MM_WATCH_LEFT='▝▜▟███▟▛▘'
MM_LOG="$SMOKE_TMP/mascot.stream"

# mm_record SESSION — start recording every byte the app writes to the pane.
mm_record() {
	: >"$MM_LOG"
	tmux pipe-pane -t "$1" -o "cat >> $MM_LOG"
}
# mm_banners SESSION — banners on the screen and in its scrollback.
mm_banners() { pane "$1" -S -80 | grep -c 'Alter Zero (v'; }
# mm_frame GLYPHS — the first recorded frame (0-based) whose cells hold
# GLYPHS, -1 when none does. Every cell is written under its own colour, so
# the stream is read frame by frame with the escapes taken out.
mm_frame() {
	python3 - "$MM_LOG" "$1" <<'PY'
import re, sys
data = open(sys.argv[1], "rb").read().decode("utf-8", "replace")
frames = re.findall(r"\x1b\[\?2026h(.*?)\x1b\[\?2026l", data, flags=re.S)
text = [re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", frame) for frame in frames]
print(next((i for i, t in enumerate(text) if sys.argv[2] in t), -1))
PY
}
# mm_frames — how many frames were recorded.
mm_frames() { grep -ao $'\x1b\\[?2026h' "$MM_LOG" | wc -l; }

# --- the greeting, on the wire -------------------------------------------
tmux new-session -d -s "$S159" -x 80 -y 24 "$MM_APP"
mm_record "$S159"
if ! wait_for 10 "$S159" -E '^❯'; then
	fail "the app never came up with the animation on"
fi
# A blink is the last of the greeting's three moves (the eyelid, about two and
# a half seconds in): once its cell is on the wire the whole greeting is.
if ! wait_file 10 "$MM_LOG" -aF '▆'; then
	fail "the greeting never blinked (no eyelid cell was written)"
fi
# Which frame flared the frill, and which one first dropped an eyelid.
mm_flare="$(mm_frame '▙▙▙')"
mm_blink="$(mm_frame '▆')"
note "the frill flared in frame $mm_flare, the first eyelid dropped in frame $mm_blink"
if [ "${mm_flare:--1}" -lt 0 ]; then
	fail "the greeting never flared the crest's frill"
fi
# The sweep of light is colour only, so it shows as FRAMES: the ones written
# between the first paint and the flare — about twenty; with no sweep the
# flare would be the third frame of the session. (The bar is low on purpose:
# a loaded machine draws fewer of them.)
if [ "${mm_flare:-0}" -lt 5 ]; then
	fail "no sweep of light crossed the mascot before its move ($mm_flare frames ahead of the flare, expected about twenty)"
fi
# …and frames are written when the pose changes, never on a clock: a 32ms
# chain turning under the banner would have written eighty by now.
if [ "${mm_blink:-0}" -gt 45 ]; then
	fail "the idle banner wrote $mm_blink frames before its first blink — a frame chain is running instead of one frame per pose"
fi
# Painted in place: back at rest it is the one banner, over the one composer.
mm_greeted="$(wait_pane 6 "$S159" -F "$MM_REST_EYES")"
dump "after the greeting" "$mm_greeted"
expect_has "$mm_greeted" -F "$MM_REST_TOP" "the banner did not come back to its rest art after the greeting"
expect_eq "$(mm_banners "$S159")" "1" "the animation left more than one banner behind"
expect_eq "$(count_bare_prompts "$mm_greeted")" "1" "the animation disturbed the composer"
tmux pipe-pane -t "$S159"
tmux kill-session -t "$S159" 2>/dev/null

# --- the row off: an idle session writes nothing ---------------------------
launch "$S159" 80 24
sleep 0.6 # the first frame and its cursor seat
mm_record "$S159"
sleep 4
tmux pipe-pane -t "$S159"
mm_off="$(wc -c <"$MM_LOG")"
note "bytes written over 4s of an idle session with the animation off: $mm_off"
expect_eq "$mm_off" "0" "an idle session wrote to the terminal with ALTER_ZERO_MASCOT_ANIMATION=0"
tmux kill-session -t "$S159" 2>/dev/null

# --- the first cell stills it, mid-move ------------------------------------
launch "$S159" 80 24 "$MM_APP"
type_text "$S159" "hello"
# The mascot watches the draft — its eyes turned to the text, a pose off
# rest: send the message while they are. (If the poll misses it the message
# is sent at rest, and every check below still has to hold.)
wait_for 6 "$S159" -F "$MM_WATCH_RIGHT" || note "the watching eyes were not caught on screen; submitting at rest"
keys "$S159" Enter
mm_turn="$(wait_summaries 60 "$S159" 1 -S -80)"
dump "after the first turn" "$mm_turn"
expect_has "$mm_turn" -F "$MM_REST_TOP" "the banner over the conversation is not at its rest art"
expect_has "$mm_turn" -F "$MM_REST_EYES" "the banner over the conversation has its eyes off rest"
expect_lacks "$mm_turn" -F "$MM_FLARE" "a pose was left in the scrollback above the conversation"
expect_lacks "$mm_turn" -F "$MM_WATCH_RIGHT" "the eyes that watched the draft were left turned in the scrollback"
expect_eq "$(printf '%s\n' "$mm_turn" | grep -c 'Alter Zero (v')" "1" "the first cell doubled the banner"
# Scrollback does not move: with a conversation under it the banner is
# never written again.
sleep 0.5
mm_record "$S159"
sleep 4
tmux pipe-pane -t "$S159"
mm_after="$(wc -c <"$MM_LOG")"
note "bytes written over 4s of idle after the first turn: $mm_after"
expect_eq "$mm_after" "0" "the banner (or something) kept repainting after the conversation started"

# --- /clear: the empty conversation comes back, and the mascot greets it ---
# A cleared screen opens on a fresh banner, so the animation's clock starts
# over: the sweep, the frill, a blink — about thirty frames in the three
# seconds after the clear, where a mascot merely carrying on its rhythm would
# write one clip's handful, or nothing.
submit "$S159" "/clear"
mm_is_cleared() { ! pane_has "$S159" -F "❯ hi"; }
if ! poll 6 mm_is_cleared; then
	fail "/clear did not empty the conversation"
fi
mm_record "$S159"
sleep 3
tmux pipe-pane -t "$S159"
mm_regreet="$(mm_frames)"
mm_reflare="$(mm_frame '▙▙▙')"
note "frames in the 3s after /clear: $mm_regreet (the frill flared in frame $mm_reflare)"
if [ "$mm_regreet" -lt 12 ]; then
	fail "the mascot did not greet the cleared screen ($mm_regreet frames in 3s, expected about thirty)"
fi
if [ "${mm_reflare:--1}" -lt 0 ]; then
	fail "the greeting after /clear never flared the crest's frill"
fi
mm_fresh="$(wait_pane 6 "$S159" -F "$MM_REST_EYES")"
dump "after /clear's greeting" "$mm_fresh"
expect_has "$mm_fresh" -F "$MM_REST_TOP" "the banner did not come back to its rest art after /clear's greeting"
expect_eq "$(mm_banners "$S159")" "1" "/clear left more than one banner behind"
expect_eq "$(count_bare_prompts "$mm_fresh")" "1" "/clear's greeting disturbed the composer"
tmux kill-session -t "$S159" 2>/dev/null

# --- watching the user type ------------------------------------------------
# While the first message is typed the mascot turns to it. It stands at the
# screen's left edge, over the composer's first columns, so the text runs
# off to its RIGHT: its eyes go right as soon as the caret is past it — a
# line of three characters is — and straight ahead while the caret is still
# under it. They never go left: left of the mascot is the edge of the
# screen, and it used to stare at it through every short message. A few
# seconds after the last keystroke (MASCOT_WATCH_LINGER, 4s) it looks away
# and goes back to its own moves. With the caret to its right a watch shows
# no rest eyes (a blink shows a lid), so the first rest eyes after the
# typing stops are the look away.
launch "$S159" 80 24 "$MM_APP"
mm_record "$S159"
wait_file 10 "$MM_LOG" -aF '▆' || fail "the launch's greeting never blinked"
tmux pipe-pane -t "$S159"
sleep 1
type_text "$S159" "hello"
if ! wait_for 3 "$S159" -F "$MM_WATCH_RIGHT"; then
	fail "the mascot did not turn its eyes toward a short line of text"
fi
expect_lacks "$(pane "$S159")" -F "$MM_WATCH_LEFT" "the mascot looked at the screen's left edge while a short line was typed"
type_text "$S159" " the quick brown fox jumps over the lazy dog and runs on"
sleep 0.4
mm_long="$(pane "$S159")"
expect_has "$mm_long" -F "$MM_WATCH_RIGHT" "the mascot did not keep its eyes on a long line of text"
expect_lacks "$mm_long" -F "$MM_WATCH_LEFT" "the mascot looked at the screen's left edge while a long line was typed"
# The caret brought back to the start of the line is under the mascot
# again: it looks straight ahead — at once, not when the watch runs out —
# and right again when the caret goes back to the end.
keys "$S159" Home
if ! wait_for 1.5 "$S159" -F "$MM_REST_EYES"; then
	fail "the mascot did not look straight ahead with the caret back under it"
fi
expect_lacks "$(pane "$S159")" -F "$MM_WATCH_LEFT" "the mascot looked left at a caret at the start of the line"
keys "$S159" End
if ! wait_for 1.5 "$S159" -F "$MM_WATCH_RIGHT"; then
	fail "the mascot did not look right again when the caret went back to the end of the line"
fi
mm_typed_at="$(date +%s.%N)"
if ! wait_for 9 "$S159" -F "$MM_REST_EYES"; then
	fail "the mascot never looked away from a draft left alone"
fi
mm_looked_away="$(python3 -c "print(round($(date +%s.%N) - $mm_typed_at, 1))")"
note "the mascot looked away ${mm_looked_away}s after the last keystroke"
if python3 -c "import sys; sys.exit(0 if 3.0 <= $mm_looked_away <= 7.5 else 1)"; then :; else
	fail "the mascot looked away ${mm_looked_away}s after the last keystroke (expected about 4s)"
fi
# A cleared draft leaves nothing to read: the mascot looks away at once.
type_text "$S159" "x"
if ! wait_for 3 "$S159" -F "$MM_WATCH_RIGHT"; then
	fail "the mascot did not watch the draft again after a keystroke"
fi
keys "$S159" C-u
if ! wait_for 1.5 "$S159" -F "$MM_REST_EYES"; then
	fail "the mascot kept watching an emptied composer"
fi
mm_watched="$(pane "$S159")"
dump "after the watch" "$mm_watched"
expect_eq "$(mm_banners "$S159")" "1" "watching the draft left more than one banner behind"
expect_eq "$(count_bare_prompts "$mm_watched")" "1" "watching the draft disturbed the composer"
tmux kill-session -t "$S159" 2>/dev/null

# --- quitting mid-move leaves the banner at rest ---------------------------
# The pane outlives the app (the `echo`), so the screen it left can be read.
launch "$S159" 80 24 "sh -c '$MM_APP; echo MM-APP-EXITED; sleep 30'"
wait_for 6 "$S159" -F "$MM_FLARE" || note "the flare was not caught on screen; quitting at rest"
keys "$S159" C-c
if ! wait_for 6 "$S159" -F "MM-APP-EXITED"; then
	fail "Ctrl+C on the empty composer did not quit"
fi
mm_dead="$(pane "$S159")"
dump "the screen the app left behind" "$mm_dead"
expect_has "$mm_dead" -F "$MM_REST_TOP" "the app quit with its mascot off its rest art"
expect_lacks "$mm_dead" -F "$MM_FLARE" "the app quit mid-move and left the pose on the dead screen"
tmux kill-session -t "$S159" 2>/dev/null

# --- the picker's preview greets, and so does the mascot just chosen -------
# (Last, since the choice is saved for this directory and every launch after
# it would open on the twin.)
# Let the launch's greeting pass first (its blink is the last of it), so the
# frames counted below are not the banner's own.
launch "$S159" 80 24 "$MM_APP"
mm_record "$S159"
wait_file 10 "$MM_LOG" -aF '▆' || fail "the launch's greeting never blinked"
tmux pipe-pane -t "$S159"
submit "$S159" "/mascot"
if ! wait_for 5 "$S159" -F "Enter to choose"; then
	fail "the /mascot picker did not open"
fi
# The preview keeps a clock of its own, started by the frame that first
# shows the mascot under the highlight. Searching onto the twin plays its
# greeting there — the sweep, an ear, a blink: about thirty frames, where a
# preview merely carrying on would write a clip's handful beside the four
# the typing costs.
mm_record "$S159"
type_text "$S159" "twin"
sleep 2.5
tmux pipe-pane -t "$S159"
mm_preview="$(mm_frames)"
note "frames in the 2.5s after the highlight landed on the twin: $mm_preview"
if [ "$mm_preview" -lt 16 ]; then
	fail "the /mascot preview did not greet as the highlight landed on a mascot ($mm_preview frames in 2.5s, expected about thirty)"
fi
# Choosing a mascot restarts the animation's clock, so the banner that
# confirms the choice opens on the sweep, the mascot's own move and a blink:
# about thirty frames again, where a mascot merely carrying on would write a
# blink's four.
keys "$S159" Enter
if ! wait_for 5 "$S159" -F " ██▄▄▄██   Alter Zero"; then
	fail "the banner was not redrawn with the mascot just chosen"
fi
mm_record "$S159"
sleep 2.5
tmux pipe-pane -t "$S159"
mm_greet="$(mm_frames)"
note "frames in the 2.5s after choosing a mascot: $mm_greet"
if [ "$mm_greet" -lt 12 ]; then
	fail "the mascot just chosen did not greet ($mm_greet frames in 2.5s, expected about thirty)"
fi
tmux kill-session -t "$S159" 2>/dev/null
