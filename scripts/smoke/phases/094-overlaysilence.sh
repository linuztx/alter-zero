#!/usr/bin/env bash
# Phase 94 — an alternate-screen overlay is SILENT on a page that has not changed, so its t

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# an alternate-screen overlay is SILENT on a page that has not
# changed, so its text can be selected and copied WHILE A TURN RUNS
# (docs/overlay-repaint.md). A terminal drops the user's mouse selection the
# moment the cells under it are rewritten, and the overlay used to
# re-serialize every cell of the screen on every frame — up to 120 a second
# while a turn streamed, floored at ~31 by the status animation's clock chain
# — so text in Ctrl+O / Ctrl+D could not be copied until the turn finished
# (the reported bug). `draw_overlay` now diffs against the frame already on
# the alternate screen and emits nothing at all when nothing moved, and the
# draw tick stops re-arming the clock chain under an overlay (none of what it
# animates is on screen there). Measured, not eyeballed: `tmux pipe-pane`
# captures every byte the app writes to the pane. The turn is a 30s local
# `!sleep`, so it is unambiguously ACTIVE for the whole window and the pane's
# own liveness is asserted after each measurement. The INLINE control must
# write something — otherwise a zero would only prove the meter is broken —
# and each overlay must then write nothing while still coming back to life on
# the next keypress.
S94="${S}_overlaysilence"
op_bytes() { # keys… → bytes written to the pane over 2s of an active turn
	local sess="$1"
	shift
	tmux kill-session -t "$sess" 2>/dev/null
	tmux new-session -d -s "$sess" -x 100 -y 30 "$APP"
	sleep 0.7
	tmux send-keys -t "$sess" -l '!sleep 30'
	sleep 0.3
	tmux send-keys -t "$sess" Enter
	local started=""
	for _ in $(seq 1 80); do
		if tmux capture-pane -t "$sess" -p | grep -qF "Running…"; then
			started=1
			break
		fi
		sleep 0.1
	done
	if [ -z "$started" ]; then
		echo "-1"
		return
	fi
	local k
	for k in "$@"; do
		tmux send-keys -t "$sess" "$k"
		sleep 0.5
	done
	sleep 0.3
	local log="${TMPDIR:-/tmp}/alterzero_smoke_bytes_$$_${sess##*_}"
	: >"$log"
	tmux pipe-pane -t "$sess" -o "cat >> $log"
	sleep 2
	tmux pipe-pane -t "$sess"
	wc -c <"$log"
	rm -f "$log"
}
inline_bytes="$(op_bytes "${S94}_inline")"
# The meter itself: the inline view animates its status line, so it MUST write.
if [ "${inline_bytes:-0}" -le 0 ]; then
	fail "the inline control wrote $inline_bytes bytes during an active turn, so the byte meter proves nothing"
fi
for probe in "C-d:the Ctrl+D context view" "C-o:the Ctrl+O transcript"; do
	key="${probe%%:*}"
	what="${probe#*:}"
	got="$(op_bytes "${S94}_${key//-/}" "$key")"
	echo "==== Phase 94: $what wrote $got bytes over 2s of an active turn (inline control: $inline_bytes) ===="
	if [ "${got:-1}" -lt 0 ]; then
		fail "the shell turn never started under $what"
	elif [ "${got:-1}" -ne 0 ]; then
		fail "$what rewrote the terminal ($got bytes) over an unchanged page: a mouse selection there is dropped, so its text cannot be copied mid-turn"
	fi
done
# …and silence is not death: the page still repaints the moment its content
# changes. Open the transcript mid-turn, let the reply stream under it, and
# assert the view actually moved — the diff baseline going stale would show
# here as a frozen page.
tmux kill-session -t "$S94" 2>/dev/null
tmux new-session -d -s "$S94" -x 100 -y 30 "$APP"
sleep 0.7
tmux send-keys -t "$S94" -l "$USER_MSG"
sleep 0.3
tmux send-keys -t "$S94" Enter
sleep 0.4
tmux send-keys -t "$S94" C-o # open before the dummy's pre-stream pause ends
sleep 0.5
live_a="$(tmux capture-pane -t "$S94" -p)"
wait_for 12 "$S94" -F "$EXPECT_REPLY" # …and let the reply stream under it
sleep 1.2
live_b="$(tmux capture-pane -t "$S94" -p)"
echo "==== Phase 94: the transcript after the reply streamed under it ===="
printf '%s\n' "$live_b" | sed -n '1,14p'
expect_has "$live_b" -F "T R A N S C R I P T" "the transcript is not up"
if [ "$live_a" = "$live_b" ]; then
	fail "the overlay froze: the reply streamed underneath and the page never repainted (a stale diff baseline)"
fi
expect_has "$live_b" -F "$EXPECT_REPLY" "the streamed reply never reached the open transcript"
# …and the OTHER half, which the byte count cannot see: that the incremental
# paints leave the *right* cells on screen. Let the whole turn settle under the
# overlay (hundreds of diff frames), scroll away from the tail and back (more
# diffs), then resize away and straight back — a burst whose net size is the one
# the baseline records, which is why `resized` drops that baseline outright
# instead of trusting the area check (whether the two events actually coalesce
# into one frame is up to the scheduler, so this is a best-effort reproduction
# and a permanent guard on the post-resize repaint path either way). Finally
# close and REOPEN: `enter_overlay` clears the alternate screen, so that frame
# is a pure full repaint of the same content at the same scroll seat (End
# re-engaged tail-follow, and an open re-arms it). The two screens must match
# exactly — any mismatch is a cell the diff path left stale.
wait_for 18 "$S94" -F "$SETTLED_REPLY" # let the turn finish under the overlay
sleep 1.0 # StreamDone + the Done-for summary land under it
for _ in 1 2 3; do
	tmux send-keys -t "$S94" PageUp
	sleep 0.15
done
tmux send-keys -t "$S94" End
sleep 0.5
tmux resize-window -t "$S94" -x 88 -y 26 2>/dev/null
tmux resize-window -t "$S94" -x 100 -y 30 2>/dev/null
sleep 0.8
overlay_incremental="$(tmux capture-pane -t "$S94" -p)"
tmux send-keys -t "$S94" -l "q" # close (q, not Esc: idle Esc arms the backtrack)
sleep 0.7
tmux send-keys -t "$S94" C-o # …and reopen: a clear + full repaint
sleep 0.9
overlay_full="$(tmux capture-pane -t "$S94" -p)"
if ! printf '%s' "$overlay_full" | grep -qF "T R A N S C R I P T"; then
	fail "the transcript did not reopen, so the stale-cell check proves nothing"
elif [ "$overlay_incremental" != "$overlay_full" ]; then
	fail "the incrementally-painted overlay differs from a full repaint of the same content: the diff left a stale cell on the alternate screen"
	echo "---- incrementally painted ----" >&2
	printf '%s\n' "$overlay_incremental" >&2
	echo "---- full repaint ----" >&2
	printf '%s\n' "$overlay_full" >&2
fi
# …and the clock chain the draw tick stopped re-arming must come BACK on the
# return: it seeds from the Submit keypress and would otherwise stay broken for
# the rest of the turn, freezing the inline timer and the spinner's sweep. Round
# trip through the overlay mid-turn, then assert the inline view is writing
# again and its elapsed counter is advancing.
tmux kill-session -t "$S94" 2>/dev/null
tmux new-session -d -s "$S94" -x 100 -y 30 "$APP"
sleep 0.7
tmux send-keys -t "$S94" -l '!sleep 30'
sleep 0.3
tmux send-keys -t "$S94" Enter
reseed_go=""
for _ in $(seq 1 80); do
	if tmux capture-pane -t "$S94" -p | grep -qF "Running…"; then
		reseed_go=1
		break
	fi
	sleep 0.1
done
if [ -z "$reseed_go" ]; then
	fail "the shell turn never started for the re-seed check"
fi
tmux send-keys -t "$S94" C-o # into the overlay (the chain stops)…
sleep 0.8
tmux send-keys -t "$S94" C-o # …and back out (it must re-seed)
sleep 0.8
reseed_log="${TMPDIR:-/tmp}/alterzero_smoke_reseed_$$"
: >"$reseed_log"
tmux pipe-pane -t "$S94" -o "cat >> $reseed_log"
sleep 2.5
tmux pipe-pane -t "$S94"
reseed_bytes="$(wc -c <"$reseed_log")"
rm -f "$reseed_log"
echo "==== Phase 94: inline wrote $reseed_bytes bytes over 2.5s after an overlay round trip mid-turn ===="
if [ "${reseed_bytes:-0}" -le 0 ]; then
	fail "the animation chain never re-seeded after the overlay return: the inline status line is frozen for the rest of the turn"
fi
if ! tmux capture-pane -t "$S94" -p | grep -qE "Running… \([0-9]+s\)"; then
	fail "the live region's elapsed counter is not advancing after the overlay return"
fi
tmux kill-session -t "$S94" 2>/dev/null
echo "==== Phase 94: the overlays are silent on an unchanged page and repaint the moment one changes ===="
