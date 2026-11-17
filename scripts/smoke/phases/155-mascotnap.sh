#!/usr/bin/env bash
# Phase 155 — the banner mascot naps when it is left alone, and a keystroke wakes it

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the banner mascot naps when it is left alone, and a keystroke wakes it
# (docs/mascot-animation.md *The nap*). Nobody at the keyboard for three
# quarters of a minute and the mascot over an empty conversation nods off:
# its eyes close — each a thin lid line where the eye was, never a blank —
# its head goes down, once, and it sleeps there, ALL of it on screen and
# STILL: the one thing of it that moves is its snore, a `z` in the top
# corner of its block that grows to a `Z`. The first key wakes it with a
# start: a `!` where the snore was, and it is wide awake — and then, since
# something is being typed, its eyes go to the text, to its right, like any
# other time. What this pins: it does not doze the moment the app opens;
# its lids are drawn in reverse (cut out of the body's colour); asleep the
# whole mascot is on screen and none of it is repainted but the snore's one
# cell, a frame per stage of a breath and nothing between (no clock chain
# turning under a sleeper, and no body bobbing up and down);
# the start is on the wire and no snore follows it; the banner the woken
# mascot's message commits is the rest art — nothing of the sleeper, the
# start or the watch left in scrollback — and scrollback then stays still.
# (That quitting, resizing or sending while it is ASLEEP leaves the banner
# clean is the anchored block's own contract, pinned by Phase 154 mid-move;
# a second nap here would cost this phase another minute.)
#
# The suite pins the animation off (lib.sh); this launch turns it on. The
# default mascot is the crest: rest ` ▙▄▙▄▟▄▟` over `▝▜▄███▄▛▘` over
# `  ▘▘ ▝▝`; its eyes close where it stands — a lid through the middle of
# each eye cell, `▝▜━███━▛▘` — and it sleeps half a cell lower: the frill's
# tips ` ▖ ▖ ▗ ▗` over `▗█▁███▁█▖` (the lids along the bottom of their
# cells) over ` ▝▛▛▀▜▜▘`, with the snore in the ninth column of the top
# row.
S155="${S}_mascotnap"
MN_APP="env $CFG_ENV_NOHIST ALTER_ZERO_MASCOT_ANIMATION=1 ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
MN_REST_TOP=' ▙▄▙▄▟▄▟   Alter Zero'
MN_REST_EYES='▝▜▄███▄▛▘'
MN_WATCH_RIGHT='▝▜▙███▙▛▘'
# The sleeper's eye row — each eye a lid — and its eye row as its eyes
# closed, still on its feet; and the same rows with the eyes filled in
# solid, which is the blank face a sleeper must not show. (Spelled out
# glyph by glyph: this grep reads a bracket of block glyphs byte by byte.)
MN_ASLEEP_EYES='▗█▁███▁█▖'
MN_LIDS="▝▜━███━▛▘|$MN_ASLEEP_EYES"
MN_BLANK='▝▜█████▛▘|▗███████▖'
# The snore beside the sleeper's head: a `z`, then a `Z`, in the corner of
# its block — the cell after the frill's tips, on the title's row.
MN_SNORE='^ ▖ ▖ ▗ ▗[zZ] +Alter Zero'
MN_LOG="$SMOKE_TMP/mascotnap.stream"
MN_NOD_LOG="$SMOKE_TMP/mascotnap.nod.stream"

# mn_record SESSION [LOG] — record the pane's raw stream from here on.
mn_record() {
	local log="${2:-$MN_LOG}"
	: >"$log"
	tmux pipe-pane -t "$1" -o "cat >> $log"
}
# mn_frames — the recorded frames, escapes taken out, one per line (a frame's
# own newlines folded to spaces).
mn_frames() {
	python3 - "$MN_LOG" <<'PY'
import re, sys
data = open(sys.argv[1], "rb").read().decode("utf-8", "replace")
for frame in re.findall(r"\x1b\[\?2026h(.*?)\x1b\[\?2026l", data, flags=re.S):
    print(re.sub(r"\s+", " ", re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", frame)))
PY
}

# --- left alone, it nods off -----------------------------------------------
launch "$S155" 80 24 "$MN_APP"
mn_began="$(date +%s.%N)"
# Recorded from the start: its eyes close on the way down, and once it is
# asleep nothing writes them again.
mn_record "$S155" "$MN_NOD_LOG"
if ! wait_for 80 "$S155" -E "$MN_SNORE"; then
	dump "never asleep" "$(pane "$S155")"
	fail "the mascot never fell asleep: no snore beside the banner in 80s of an untouched session"
fi
tmux pipe-pane -t "$S155"
mn_took="$(python3 -c "print(round($(date +%s.%N) - $mn_began, 1))")"
note "the mascot was snoring ${mn_took}s after the app opened"
# Three quarters of a minute up, a couple of seconds to nod off and settle,
# and the first snore: never sooner.
if python3 -c "import sys; sys.exit(0 if $mn_took >= 44 else 1)"; then :; else
	fail "the mascot was asleep ${mn_took}s after the app opened (it stays up three quarters of a minute)"
fi
mn_asleep="$(pane "$S155")"
dump "asleep" "$mn_asleep"
# Asleep, the whole of it on screen and settled: the tips of its frill on
# the title's row, its eyes closed on the row under it — a lid where each
# was, not a blank — and its feet on the third.
expect_has "$mn_asleep" -E '^ ▖ ▖ ▗ ▗[zZ ] +Alter Zero' "the sleeping mascot's head is not down on its block's top row"
expect_has "$mn_asleep" -E "^$MN_ASLEEP_EYES " "the sleeping mascot's closed eyes show no lid"
expect_lacks "$mn_asleep" -E "$MN_BLANK" "the sleeping mascot's eyes are filled in: a blank face"
expect_lacks "$mn_asleep" -F "$MN_REST_EYES" "the sleeping mascot's eyes are open"
expect_has "$mn_asleep" -E '^ ▝▛▛▀▜▜▘ ' "the sleeping mascot's feet are not on screen"
expect_eq "$(printf '%s\n' "$mn_asleep" | grep -c 'Alter Zero (v')" "1" "the nap left more than one banner behind"
expect_eq "$(count_bare_prompts "$mn_asleep")" "1" "the nap disturbed the composer"

# --- asleep, it holds still: only its snore is written ---------------------
mn_record "$S155"
sleep 7
tmux pipe-pane -t "$S155"
mn_breathing="$(mn_frames | wc -l)"
note "frames in 7s asleep: $mn_breathing"
# Two breaths of three stages each — quiet, a `z`, a `Z`. A 32ms chain
# turning under the sleeper would have written two hundred.
if [ "$mn_breathing" -lt 4 ] || [ "$mn_breathing" -gt 12 ]; then
	fail "a sleeping mascot wrote $mn_breathing frames in 7s (expected about six: one per stage of a breath)"
fi
# (Captured first and matched with `has`: piped into `grep -q` the writer
# can die of the closed pipe and, under pipefail, turn a match into a miss.)
mn_slept="$(mn_frames)"
if ! has "$mn_slept" 'z'; then
	fail "seven seconds asleep and not one snore was written"
fi
# …and nothing else: a frame of its sleep writes the snore's one cell — a
# `z`, a `Z`, or the blank that takes it away — and no cell of its body. A
# sleeper that rose and sank with each breath rewrote its whole block.
mn_moved="$(grep -vE '^ *[zZ]? *$' <<<"$mn_slept" || true)"
if [ -n "$mn_moved" ]; then
	dump "frames that moved the sleeper" "$mn_moved"
	fail "the sleeping mascot moved: a frame of its sleep wrote more than its snore"
fi
# A lid is cut out of the ink: every one written as its eyes closed — on
# its feet, and again as its head went down — is written in reverse video,
# the body's colour filling the cell and the line left in the ground's.
mn_lids="$(python3 - "$MN_NOD_LOG" <<'PY'
import re, sys
data = open(sys.argv[1], "rb").read().decode("utf-8", "replace")
reverse, lids, cut = False, 0, 0
for found in re.finditer(r"\x1b\[([0-9;]*)m|([▁━])", data):
    if found.group(2):
        lids += 1
        cut += reverse
        continue
    params = found.group(1).split(";")
    i = 0
    while i < len(params):
        if params[i] in ("38", "48", "58"):
            # an extended colour: skip its operands, any of which may be 7
            i += 5 if params[i + 1 : i + 2] == ["2"] else 3
            continue
        if params[i] == "7":
            reverse = True
        elif params[i] in ("", "0", "27"):
            reverse = False
        i += 1
print(lids, cut)
PY
)"
note "lids written as it fell asleep, and how many of them in reverse: $mn_lids"
if [ "${mn_lids% *}" -lt 4 ] || [ "${mn_lids% *}" != "${mn_lids#* }" ]; then
	fail "a sleeper's lids were not all drawn in reverse: wrote ${mn_lids% *}, ${mn_lids#* } in reverse"
fi

# --- a keystroke wakes it with a start -------------------------------------
mn_record "$S155"
type_text "$S155" "hello"
# Its eyes on the text, to its right: the start is over and it is watching.
if ! wait_for 4 "$S155" -F "$MN_WATCH_RIGHT"; then
	fail "the woken mascot did not turn its eyes to what was being typed"
fi
sleep 1.5
tmux pipe-pane -t "$S155"
mn_woke="$(mn_frames)"
mn_bang="$(grep -n -m1 '!' <<<"$mn_woke" | cut -d: -f1)"
note "the start was written in frame ${mn_bang:-none} after the key"
if [ -z "$mn_bang" ]; then
	fail "the keystroke woke no start: no \`!\` was written beside the mascot"
fi
if [ -n "$mn_bang" ] && has "$(tail -n "+$((mn_bang + 1))" <<<"$mn_woke")" '[zZ]'; then
	fail "the mascot went on snoring after it was woken"
fi
mn_woken="$(pane "$S155")"
dump "woken, watching" "$mn_woken"
expect_has "$mn_woken" -F "$MN_REST_TOP" "the woken mascot is not back on its feet"
expect_lacks "$mn_woken" -E "$MN_SNORE" "a snore was left beside the woken mascot"

# --- the message it woke for: the banner goes to scrollback at rest --------
keys "$S155" Enter
mn_turn="$(wait_summaries 60 "$S155" 1 -S -80)"
dump "after the first turn" "$mn_turn"
expect_has "$mn_turn" -F "$MN_REST_TOP" "the banner over the conversation is not at its rest art"
expect_has "$mn_turn" -F "$MN_REST_EYES" "the banner over the conversation has its eyes off rest"
expect_lacks "$mn_turn" -F "$MN_WATCH_RIGHT" "the eyes that watched the draft were left turned in the scrollback"
expect_lacks "$mn_turn" -E "[▟▗][zZ!] +Alter Zero|^ ▖ ▖ ▗ ▗|$MN_LIDS|$MN_BLANK" "a snore, a start or a sleeper was left in the scrollback above the conversation"
expect_eq "$(printf '%s\n' "$mn_turn" | grep -c 'Alter Zero (v')" "1" "the first cell doubled the banner"
sleep 0.5
mn_record "$S155"
sleep 4
tmux pipe-pane -t "$S155"
mn_after="$(wc -c <"$MN_LOG")"
note "bytes written over 4s of idle after the first turn: $mn_after"
expect_eq "$mn_after" "0" "the banner (or something) kept repainting after the conversation started"
tmux kill-session -t "$S155" 2>/dev/null
