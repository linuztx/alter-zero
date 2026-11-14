#!/usr/bin/env bash
# Phase 154 — the banner's mascot moves until the conversation starts, and never after
# smoke: tags=serial

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the banner mascot's motion (docs/mascot-motion.md). From launch until the
# conversation holds its first cell the mascot is alive: 0.7s in it greets (a
# glint, its signature move, a blink), then every few seconds it does
# something else — a move of its body or of the light on it, then a look of
# its eyes, by turns. It is painted IN PLACE, over the banner the launch
# committed — the one thing above the live region a frame may repaint — and
# it must leave no trace of itself anywhere a record is kept:
#
#   - it moves at all, and within the greeting (the art's rows change);
#   - what follows the greeting is a MOVE, not another blink: the frill or
#     the feet change, glyphs or colours, and then the eyes get their turn;
#   - between acts it is SILENT — one frame per pose change, never a chain
#     (measured with pipe-pane, like Phase 94: an idle screen that writes
#     thirty frames a second shimmers the cursor and eats a selection).
#     The windows are timed against the mascot's own schedule, which is why
#     the phase is tagged `serial`;
#   - the first message freezes it AT REST, in the very frame that commits
#     the message — colours included, since the greeting opens on a glint —
#     and it never moves again, on screen or in scrollback;
#   - /clear brings back a fresh banner that greets again;
#   - quitting mid-motion leaves the resting art in the terminal;
#   - the /mascot picker's preview moves too, greeting as a mascot is
#     highlighted;
#   - ALTER_ZERO_MASCOT_ANIMATION=0 (what every other phase runs under) and
#     the /settings row still it entirely.
#
# The suite exports ALTER_ZERO_MASCOT_ANIMATION=0 (lib.sh), so these launches
# turn it back on for themselves.
S154="${S}_motion"
MOTION_ON="env $CFG_ENV_NOHIST ALTER_ZERO_MASCOT_ANIMATION=1 ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
MOTION_OFF="env $CFG_ENV_NOHIST ALTER_ZERO_MASCOT_ANIMATION=0 ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"

# The crest (the default mascot) at rest, as the banner draws its two rows
# that ever move: the frill and the face, each with the gap after it.
REST_FRILL=" ▙▄▙▄▟▄▟  "
REST_FACE="▝▜▄███▄▛▘  "

# at_rest CONTENT — are the banner's art rows the resting crest's?
at_rest() {
	has "$1" -F -- "$REST_FRILL" && has "$1" -F -- "$REST_FACE"
}
# moved_within SECONDS SESSION — poll (tightly: a pose holds for as little as
# 45ms, the greeting's art moves for ~400ms in all) until the banner's art is
# NOT at rest, printing the capture that caught it. Returns 1 on timeout.
moved_within() {
	local secs="$1" sess="$2" last="" deadline
	deadline=$(($(date +%s%N) + secs * 1000000000))
	while [ "$(date +%s%N)" -lt "$deadline" ]; do
		last="$(pane "$sess")"
		if ! at_rest "$last"; then
			printf '%s\n' "$last"
			return 0
		fi
		sleep 0.02
	done
	printf '%s\n' "$last"
	return 1
}
# still_for SECONDS SESSION [capture opts] — poll for the whole span; returns
# 1 (printing the capture) the moment the banner's art is not at rest.
still_for() {
	local secs="$1" sess="$2" last="" deadline
	shift 2
	deadline=$(($(date +%s%N) + secs * 1000000000))
	while [ "$(date +%s%N)" -lt "$deadline" ]; do
		last="$(pane "$sess" "$@")"
		if ! at_rest "$last"; then
			printf '%s\n' "$last"
			return 1
		fi
		sleep 0.03
	done
	return 0
}
# pane_log SESSION SECONDS — record every byte the app writes to the pane
# over that long (Phase 94's meter) into $PANE_LOG.
pane_log() {
	PANE_LOG="$SMOKE_TMP/bytes_$1"
	: >"$PANE_LOG"
	tmux pipe-pane -t "$1" -o "cat >> $PANE_LOG"
	sleep "$2"
	tmux pipe-pane -t "$1"
}
# pane_bytes SESSION SECONDS — how many bytes that was.
pane_bytes() {
	pane_log "$1" "$2"
	wc -c <"$PANE_LOG" | tr -d ' '
}
# pane_frames SESSION SECONDS — how many FRAMES that was: every frame opens
# a synchronized update, so its marker counts them whatever each one paints
# (a glint frame recolours a dozen cells, a blink two).
pane_frames() {
	pane_log "$1" "$2"
	grep -a -o $'\x1b\\[?2026h' "$PANE_LOG" | wc -l | tr -d ' '
}
# sleep_until NANOS — sleep until that reading of `date +%s%N`.
sleep_until() {
	local left=$(($1 - $(date +%s%N)))
	[ "$left" -gt 0 ] && sleep "$(awk -v n="$left" 'BEGIN { printf "%.3f", n / 1000000000 }')"
	return 0
}
# banner_cells SESSION [capture opts] — the banner's three rows WITH their
# colours: what "at rest" means once a glint is involved.
banner_cells() {
	local sess="$1"
	shift
	pane "$sess" -e "$@" | head -3
}
# body_of CELLS — the frill and the feet of a banner_cells capture: the two
# art rows no look ever touches. (The eyes are on the row between.)
body_of() {
	printf '%s\n' "$1" | sed -n '1p;3p'
}
# face_of CELLS — the row the eyes are on.
face_of() {
	printf '%s\n' "$1" | sed -n '2p'
}
# art_of CELLS — a banner_cells capture as the art alone, its colours
# stripped: what a dump can show. A move of the light changes no glyph, so
# its art is the resting banner's.
art_of() {
	printf '%s\n' "$1" | sed $'s/\x1b\\[[0-9;]*m//g'
}
# changes_until NANOS SESSION PART — poll until that reading of `date +%s%N`
# for a capture whose PART (body_of / face_of) is not the resting banner's,
# printing the capture that caught it. Returns 1 if none did.
changes_until() {
	local deadline="$1" sess="$2" part="$3" last="" rest
	rest="$("$part" "$REST_CELLS")"
	while [ "$(date +%s%N)" -lt "$deadline" ]; do
		last="$(banner_cells "$sess")"
		if [ "$("$part" "$last")" != "$rest" ]; then
			printf '%s\n' "$last"
			return 0
		fi
		sleep 0.02
	done
	printf '%s\n' "$last"
	return 1
}

# ---- the reference: a stilled launch -----------------------------------
note "ALTER_ZERO_MASCOT_ANIMATION=0: the banner holds still and writes nothing"
launch "${S154}_off" 80 24 "$MOTION_OFF"
sleep 0.4
off_boot="$(pane "${S154}_off")"
dump "the stilled banner" "$off_boot"
at_rest "$off_boot" || fail "the stilled banner is not the resting crest"
REST_CELLS="$(banner_cells "${S154}_off")"
if ! off_moved="$(still_for 3 "${S154}_off")"; then
	dump "it moved" "$off_moved"
	fail "the banner moved under ALTER_ZERO_MASCOT_ANIMATION=0"
fi
off_bytes="$(pane_bytes "${S154}_off" 2)"
expect_eq "$off_bytes" "0" "a stilled, idle session wrote to the terminal"
tmux kill-session -t "${S154}_off" 2>/dev/null

# ---- it moves, and rests in silence -------------------------------------
note "the mascot greets: its art moves within the first seconds"
launch "$S154" 80 24 "$MOTION_ON"
launched="$(date +%s%N)"
if greeting="$(moved_within 4 "$S154")"; then
	dump "caught mid-move" "$(printf '%s\n' "$greeting" | head -4)"
else
	dump "never moved" "$greeting"
	fail "the mascot never moved in the first 4s of a fresh session"
fi
# The metadata beside the art never moves with it.
expect_has "$greeting" -F "Alter Zero (v" "the banner's title left its row mid-move"
expect_has "$greeting" -F "/ commands  @ files  ? shortcuts" "the banner's hint left its row mid-move"

# The greeting is over 2.5s after the first frame and the next act starts no
# sooner than 5.7s after it. From 3.0s to 4.5s after the launch: nothing. A
# frame chain (the status line's 32ms one) would draw ~45 frames there.
sleep_until $((launched + 3000000000))
rest_frames="$(pane_frames "$S154" 1.5)"
note "frames drawn over 1.5s between two acts: $rest_frames"
if [ "$rest_frames" -gt 2 ]; then
	fail "the idle banner keeps a frame chain running between its acts ($rest_frames frames in 1.5s)"
fi
resting="$(pane "$S154")"
at_rest "$resting" || {
	dump "between acts" "$resting"
	fail "the mascot is not at rest between its acts"
}
# …and the schedule goes on: one act a bar (4.8s), a move and a look by
# turns. The bar after the greeting plays a MOVE — the body or the light on
# it, never just the eyes — somewhere between 5.7s and 8.7s after the first
# frame, on any seed: the frill or the feet change, in glyph or in colour.
note "what follows the greeting is a move of the body, not another blink"
if first_move="$(changes_until $((launched + 8800000000)) "$S154" body_of)"; then
	dump "the first move, caught (a move of the light shows as the resting art)" "$(art_of "$first_move")"
else
	dump "the banner at the end of the bar" "$(art_of "$first_move")"
	fail "the bar after the greeting moved nothing but the eyes (or nothing at all)"
fi
# …and the bar after that is the eyes' turn: a look, 10.5–13.5s in. (The
# move is over 8.7s in at the latest, and a hop moves the face row too, so
# the wait for the eyes starts once it must be.)
note "then the eyes get their turn"
sleep_until $((launched + 8900000000))
if look="$(changes_until $((launched + 13900000000)) "$S154" face_of)"; then
	dump "the first look, caught" "$(art_of "$look")"
else
	fail "the second bar after the greeting never moved the eyes"
fi
# Over a longer idle that is an act every 3.0–6.6s, a couple of dozen frames
# at most — where a chain draws ~190 in 6s.
idle_frames="$(pane_frames "$S154" 6)"
note "frames drawn over 6s of idling: $idle_frames"
if [ "$idle_frames" -gt 60 ]; then
	fail "an idle banner drew $idle_frames frames in 6s — that is a chain, not an act"
fi
tmux kill-session -t "$S154" 2>/dev/null

# ---- the first cell freezes it at rest ----------------------------------
note "the first message settles the banner at rest, colours and all"
launch "$S154" 80 30 "$MOTION_ON"
type_text "$S154" "hello"
# Enter lands inside the greeting's glint (0.7–1.2s after the first frame).
sleep 0.7
lit="$(banner_cells "$S154")"
keys "$S154" Enter
sleep 0.15
after_enter="$(banner_cells "$S154" -S -200)"
if [ "$lit" = "$REST_CELLS" ]; then
	note "(the Enter missed the glint this run — the check below still holds)"
else
	note "the Enter landed mid-glint"
fi
if [ "$after_enter" != "$REST_CELLS" ]; then
	echo "==== Phase 154: the banner after the first message ===="
	printf '%s\n' "$after_enter" | cat -v
	echo "==== Phase 154: the banner at rest ===="
	printf '%s\n' "$REST_CELLS" | cat -v
	fail "the first message did not leave the banner at rest (art or colours differ)"
fi
settled="$(wait_summaries 30 "$S154" 1 -S -200)" || fail "the first turn never settled"
# For longer than a bar and its act: not once, on screen or in scrollback.
if ! woke="$(still_for 7 "$S154" -S -200)"; then
	dump "it moved after the first cell" "$woke"
	fail "the banner moved after the conversation had started"
fi
final="$(banner_cells "$S154" -S -200)"
[ "$final" = "$REST_CELLS" ] || fail "the committed banner is not the resting one after the turn"
expect_eq "$(pane "$S154" -S -200 | grep -cF "Alter Zero (v")" "1" "the banner is not on screen+scrollback exactly once"

# ---- /clear greets again --------------------------------------------------
note "/clear brings back a banner that greets"
submit "$S154" "/clear"
if cleared="$(moved_within 4 "$S154")"; then
	dump "the fresh banner mid-move" "$(printf '%s\n' "$cleared" | head -4)"
else
	dump "never moved" "$cleared"
	fail "the banner did not greet again after /clear"
fi
tmux kill-session -t "$S154" 2>/dev/null

# ---- quitting mid-motion leaves the resting art ---------------------------
note "a quit mid-motion leaves the resting banner in the terminal"
# The pane outlives the app so what it left behind can be read.
launch "${S154}_quit" 80 24 "$MOTION_ON; echo; echo left-the-app; sleep 60"
sleep 0.75
before_quit="$(banner_cells "${S154}_quit")"
keys "${S154}_quit" C-c
if ! wait_for 10 "${S154}_quit" -F "left-the-app"; then
	fail "Ctrl+C on an empty composer did not quit"
fi
after_quit="$(banner_cells "${S154}_quit")"
if [ "$before_quit" = "$REST_CELLS" ]; then
	note "(the quit missed the glint this run — the check below still holds)"
else
	note "the quit landed mid-glint"
fi
if [ "$after_quit" != "$REST_CELLS" ]; then
	echo "==== Phase 154: the banner the app left behind ===="
	printf '%s\n' "$after_quit" | cat -v
	fail "quitting left the banner mid-motion in the terminal"
fi
tmux kill-session -t "${S154}_quit" 2>/dev/null

# ---- the /mascot picker's preview ----------------------------------------
note "the /mascot picker's preview greets as a mascot is highlighted"
launch "$S154" 80 30 "$MOTION_ON"
# Let the banner's own greeting finish, so what moves below is the preview.
sleep 2.8
type_text "$S154" "/mascot"
sleep 0.3
keys "$S154" Enter
sleep 0.2
keys "$S154" Down Down Down Down
picker_open="$(wait_pane 5 "$S154" -F "→ skiter")" || fail "the picker did not open on skiter"
dump "the picker on skiter" "$picker_open"
# Skiter's legs trade places (` ▝▝▘ ▝▘▘`) in its signature, and its eyes
# shut (`▗▟█████▙▖`) in the blink that follows.
preview_moved=""
deadline=$(($(date +%s%N) + 4 * 1000000000))
while [ "$(date +%s%N)" -lt "$deadline" ]; do
	page="$(pane "$S154")"
	if has "$page" -F -- " ▝▝▘ ▝▘▘" || has "$page" -F -- "▗▟█████▙▖"; then
		preview_moved="$page"
		break
	fi
	sleep 0.02
done
if [ -n "$preview_moved" ]; then
	dump "the preview mid-move" "$preview_moved"
	expect_has "$preview_moved" -F "→ skiter" "the page lost its selection mid-move"
	expect_has "$preview_moved" -F "↑/↓ to navigate" "the page lost its hint mid-move"
else
	fail "the highlighted mascot never moved in the picker's preview (4s)"
fi
keys "$S154" Escape
sleep 0.3

# ---- the /settings row ------------------------------------------------------
note "the /settings row stills it, and says so"
type_text "$S154" "/settings"
sleep 0.3
keys "$S154" Enter
sleep 0.3
type_text "$S154" "mascot"
sleep 0.3
row_on="$(pane "$S154")"
dump "searched for the row" "$row_on"
expect_has "$row_on" -E "→ Mascot animation +true" "the Mascot animation row is missing, or not on under the run's seed"
keys "$S154" Enter
sleep 0.3
keys "$S154" Escape
sleep 0.2
keys "$S154" Escape
row_off="$(wait_pane 5 "$S154" -F "Mascot animation: false")" || fail "turning the row off raised no toast"
dump "after turning it off" "$row_off"
if ! woke="$(still_for 7 "$S154")"; then
	dump "it moved with the row off" "$woke"
	fail "the banner moved with Mascot animation off"
fi
[ "$(banner_cells "$S154")" = "$REST_CELLS" ] || fail "turning the row off did not leave the banner at rest"
if ! grep -q '"mascot_animation": *false' "$SMOKE_CFG/settings.json" 2>/dev/null; then
	echo "==== Phase 154: settings.json ===="
	cat "$SMOKE_CFG/settings.json" 2>/dev/null
	fail "settings.json does not record the stilled mascot for this directory"
fi
tmux kill-session -t "$S154" 2>/dev/null
