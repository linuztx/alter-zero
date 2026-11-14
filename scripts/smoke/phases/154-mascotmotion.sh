#!/usr/bin/env bash
# Phase 154 — the banner mascot animates over an empty conversation, and only there

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
# good — sent mid-move, and again when the app quits mid-move — `/clear`
# brings the empty conversation, and the motion, back; and a mascot just
# chosen in `/mascot` greets.
#
# The suite pins the animation off (lib.sh); these launches turn it on. The
# default mascot is the crest: rest ` ▙▄▙▄▟▄▟` over `▝▜▄███▄▛▘`, its frill
# flared ` ▙▙▙▄▟▟▟`, an eyelid `▆`.
S154="${S}_mascotmotion"
MM_APP="env $CFG_ENV_NOHIST ALTER_ZERO_MASCOT_ANIMATION=1 ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
MM_REST_TOP=' ▙▄▙▄▟▄▟   Alter Zero'
MM_REST_EYES='▝▜▄███▄▛▘'
MM_FLARE=' ▙▙▙▄▟▟▟   Alter Zero'
MM_LOG="$SMOKE_TMP/mascot.stream"

# mm_record SESSION — start recording every byte the app writes to the pane.
mm_record() {
	: >"$MM_LOG"
	tmux pipe-pane -t "$1" -o "cat >> $MM_LOG"
}
# mm_banners SESSION — banners on the screen and in its scrollback.
mm_banners() { pane "$1" -S -80 | grep -c 'Alter Zero (v'; }

# --- the greeting, on the wire -------------------------------------------
tmux new-session -d -s "$S154" -x 80 -y 24 "$MM_APP"
mm_record "$S154"
if ! wait_for 10 "$S154" -E '^❯'; then
	fail "the app never came up with the animation on"
fi
# A blink is the last of the greeting's three moves (the eyelid, about two and
# a half seconds in): once its cell is on the wire the whole greeting is.
if ! wait_file 10 "$MM_LOG" -aF '▆'; then
	fail "the greeting never blinked (no eyelid cell was written)"
fi
# Every cell is written under its own colour, so the stream is read frame by
# frame with the escapes taken out: which frame flared the frill, which one
# first dropped an eyelid.
mm_greeting="$(
	python3 - "$MM_LOG" <<'PY'
import re, sys
data = open(sys.argv[1], "rb").read().decode("utf-8", "replace")
frames = re.findall(r"\x1b\[\?2026h(.*?)\x1b\[\?2026l", data, flags=re.S)
text = [re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", frame) for frame in frames]
find = lambda glyphs: next((i for i, t in enumerate(text) if glyphs in t), -1)
print(find("▙▙▙"), find("▆"))
PY
)"
mm_flare="${mm_greeting%% *}"
mm_blink="${mm_greeting##* }"
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
mm_greeted="$(wait_pane 6 "$S154" -F "$MM_REST_EYES")"
dump "after the greeting" "$mm_greeted"
expect_has "$mm_greeted" -F "$MM_REST_TOP" "the banner did not come back to its rest art after the greeting"
expect_eq "$(mm_banners "$S154")" "1" "the animation left more than one banner behind"
expect_eq "$(count_bare_prompts "$mm_greeted")" "1" "the animation disturbed the composer"
tmux pipe-pane -t "$S154"
tmux kill-session -t "$S154" 2>/dev/null

# --- the row off: an idle session writes nothing ---------------------------
launch "$S154" 80 24
sleep 0.6 # the first frame and its cursor seat
mm_record "$S154"
sleep 4
tmux pipe-pane -t "$S154"
mm_off="$(wc -c <"$MM_LOG")"
note "bytes written over 4s of an idle session with the animation off: $mm_off"
expect_eq "$mm_off" "0" "an idle session wrote to the terminal with ALTER_ZERO_MASCOT_ANIMATION=0"
tmux kill-session -t "$S154" 2>/dev/null

# --- the first cell stills it, mid-move ------------------------------------
launch "$S154" 80 24 "$MM_APP"
type_text "$S154" "hi"
# The greeting's flare holds for most of half a second: send the message
# while the frill is up. (If the poll misses it the message is sent at rest,
# and every check below still has to hold.)
wait_for 6 "$S154" -F "$MM_FLARE" || note "the flare was not caught on screen; submitting at rest"
keys "$S154" Enter
mm_turn="$(wait_summaries 60 "$S154" 1 -S -80)"
dump "after the first turn" "$mm_turn"
expect_has "$mm_turn" -F "$MM_REST_TOP" "the banner over the conversation is not at its rest art"
expect_has "$mm_turn" -F "$MM_REST_EYES" "the banner over the conversation has its eyes off rest"
expect_lacks "$mm_turn" -F "$MM_FLARE" "a pose was left in the scrollback above the conversation"
expect_eq "$(printf '%s\n' "$mm_turn" | grep -c 'Alter Zero (v')" "1" "the first cell doubled the banner"
# Scrollback does not move: with a conversation under it the banner is
# never written again.
sleep 0.5
mm_record "$S154"
sleep 4
tmux pipe-pane -t "$S154"
mm_after="$(wc -c <"$MM_LOG")"
note "bytes written over 4s of idle after the first turn: $mm_after"
expect_eq "$mm_after" "0" "the banner (or something) kept repainting after the conversation started"

# --- /clear: the empty conversation, and the motion, come back -------------
submit "$S154" "/clear"
mm_cleared="$(wait_settled 6 "$S154" -E '^❯ *$')"
expect_lacks "$mm_cleared" -F "❯ hi" "/clear did not empty the conversation"
mm_record "$S154"
# The mascot is never still for more than about eight seconds, so a move is
# due inside this wait however the lots fall: an eyelid, a glance, the frill.
if ! wait_file 14 "$MM_LOG" -aE '▆|▟|▙'; then
	fail "the banner did not start moving again after /clear"
fi
tmux pipe-pane -t "$S154"
tmux kill-session -t "$S154" 2>/dev/null

# --- quitting mid-move leaves the banner at rest ---------------------------
# The pane outlives the app (the `echo`), so the screen it left can be read.
launch "$S154" 80 24 "sh -c '$MM_APP; echo MM-APP-EXITED; sleep 30'"
wait_for 6 "$S154" -F "$MM_FLARE" || note "the flare was not caught on screen; quitting at rest"
keys "$S154" C-c
if ! wait_for 6 "$S154" -F "MM-APP-EXITED"; then
	fail "Ctrl+C on the empty composer did not quit"
fi
mm_dead="$(pane "$S154")"
dump "the screen the app left behind" "$mm_dead"
expect_has "$mm_dead" -F "$MM_REST_TOP" "the app quit with its mascot off its rest art"
expect_lacks "$mm_dead" -F "$MM_FLARE" "the app quit mid-move and left the pose on the dead screen"
tmux kill-session -t "$S154" 2>/dev/null

# --- a mascot just chosen greets -------------------------------------------
# (Last, since the choice is saved for this directory and every launch after
# it would open on the twin.)
# Choosing a mascot restarts the animation's clock, so the banner that
# confirms the choice opens on the sweep, the mascot's own move and a blink.
# Let the launch's greeting pass first (its blink is the last of it), so the
# frames counted below are the new mascot's: about thirty in two and a half
# seconds, where a mascot merely carrying on would write a blink's four.
launch "$S154" 80 24 "$MM_APP"
mm_record "$S154"
wait_file 10 "$MM_LOG" -aF '▆' || fail "the launch's greeting never blinked"
tmux pipe-pane -t "$S154"
submit "$S154" "/mascot"
if ! wait_for 5 "$S154" -F "Enter to choose"; then
	fail "the /mascot picker did not open"
fi
type_text "$S154" "twin"
sleep "$SMOKE_TYPE_SETTLE"
keys "$S154" Enter
if ! wait_for 5 "$S154" -F " ██▄▄▄██   Alter Zero"; then
	fail "the banner was not redrawn with the mascot just chosen"
fi
mm_record "$S154"
sleep 2.5
tmux pipe-pane -t "$S154"
mm_greet="$(grep -ao $'\x1b\\[?2026h' "$MM_LOG" | wc -l)"
note "frames in the 2.5s after choosing a mascot: $mm_greet"
if [ "$mm_greet" -lt 12 ]; then
	fail "the mascot just chosen did not greet ($mm_greet frames in 2.5s, expected about thirty)"
fi
tmux kill-session -t "$S154" 2>/dev/null
