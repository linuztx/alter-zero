#!/usr/bin/env bash
# Phase 107d — a picture REWRITTEN IN PLACE draws its new bytes, not the cached ones

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# A FILE THAT CHANGED UNDERNEATH (docs/images.md). The agent downloads a
# picture, shows it, converts it in place — same path, same pixel size — and
# reads it again: the new cell must show the NEW picture. An encoded picture
# is cached per placement, and a placement interns on (path, cols, rows), so
# a file rewritten at the same size hit the stale entry and painted the old
# bytes (the reported "still shows the colour cat after it went black and
# white" bug). The store now remembers the file state — size and mtime — an
# encoding was made from, and re-encodes when the file's differs.
#
# Driven the way Phase 107 is: a handcrafted rollout carrying an image `read`
# of a solid RED 100x60 PNG is resumed under half-blocks at a 5x10 cell, so
# the picture is real coloured cells `capture-pane -e` can read. Then the file
# is overwritten with a solid BLUE PNG of the same size — the same byte length
# too, so only the mtime tells the two apart — and Ctrl+O opens the
# transcript: the one fresh paint of the same placement this suite can drive
# without a purge (a purge already drops the cache). Half-blocks share one
# cache entry across both screens, so the overlay meets exactly the cache hit
# a re-read's new cell meets inline. It must draw blue, not the red it
# encoded first.
S107D="${S}_imagerefresh"
IMG_DIR="$(mktemp -d "$SMOKE_TMP/imagerefresh.XXXXXX")"
IMG_SESS="$(mktemp -d "$SMOKE_TMP/imgrefsess.XXXXXX")"
img_day="$IMG_SESS/2026/07/23"
mkdir -p "$img_day"
# Two solid 100x60 PNGs, 154 bytes each.
RED_PNG64='iVBORw0KGgoAAAANSUhEUgAAAGQAAAA8CAIAAAAfXYiZAAAAYUlEQVR42u3QQQ0AAAgAoetfWiv4dmwkoCmOFMiSJUuWLBTIkiVLliwUyJIlS5YsFMiSJUuWLBTIkiVLliwUyJIlS5YsFMiSJUuWLBTIkiVLliwUyJIlS5YsFMiSJUvWUwvLB1nqu2gyFwAAAABJRU5ErkJggg=='
BLUE_PNG64='iVBORw0KGgoAAAANSUhEUgAAAGQAAAA8CAIAAAAfXYiZAAAAYUlEQVR42u3QQQ0AAAgAoetfWiv4dmwkoBrOFMiSJUuWLBTIkiVLliwUyJIlS5YsFMiSJUuWLBTIkiVLliwUyJIlS5YsFMiSJUuWLBTIkiVLliwUyJIlS5YsFMiSJUvWSwsXNVnq7UnP2gAAAABJRU5ErkJggg=='
printf '%s' "$RED_PNG64" | base64 -d >"$IMG_DIR/shot.png"
# A mtime decades back, so the rewrite below is a different state whatever
# the clock's granularity and however fast the launch went.
touch -d '2001-01-01 00:00:00' "$IMG_DIR/shot.png"
img_file="$img_day/rollout-2026-07-23T10-00-00-107d0000.jsonl"
{
	printf '{"timestamp":"2026-07-23T10:00:00.000Z","type":"session_meta","payload":{"id":"smoke-107d","timestamp":"2026-07-23T10:00:00.000Z","cwd":"%s","model":"dummy_model_name","originator":"alter-zero","version":"0.1.0"}}\n' "$IMG_DIR"
	printf '{"timestamp":"2026-07-23T10:00:01.000Z","type":"message","payload":{"role":"user","text":"look at shot.png","timestamp":"10:00 AM"}}\n'
	printf '{"timestamp":"2026-07-23T10:00:02.000Z","type":"tool","payload":{"name":"Read","args":"%s/shot.png","ok":true,"output":"Read image (PNG, 100x60, 154 B)","timestamp":"10:00 AM","shell":false,"truncated":false,"arguments":"{\\"path\\":\\"%s/shot.png\\"}"}}\n' "$IMG_DIR" "$IMG_DIR"
	printf '{"timestamp":"2026-07-23T10:00:03.000Z","type":"message","payload":{"role":"assistant","text":"A solid square.","timestamp":"10:00 AM"}}\n'
} >"$img_file"
IMG_ENV="$CFG_ENV_NOHIST ALTER_ZERO_SESSIONS_DIR=$IMG_SESS ALTER_ZERO_IMAGE_PROTOCOL=halfblocks ALTER_ZERO_IMAGE_CELL_SIZE=5x10 ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS"
launch -c "$IMG_DIR" "$S107D" 100 30 "env $IMG_ENV $BIN_ABS"
tmux send-keys -t "$S107D" -l "/resume"
sleep 0.3
tmux send-keys -t "$S107D" Enter
img_listed=0
for _ in $(seq 1 40); do
	if tmux capture-pane -t "$S107D" -p | grep -qF "look at shot.png"; then
		img_listed=1
		break
	fi
	sleep 0.1
done
tmux send-keys -t "$S107D" Enter # load it
img_loaded=0
for _ in $(seq 1 60); do
	if tmux capture-pane -t "$S107D" -p -S -60 | grep -qF "Read image (PNG, 100x60"; then
		img_loaded=1
		break
	fi
	sleep 0.1
done
sleep 0.6
# The picture's rows are the ones between the `Read image` cell and the reply
# under it (Phase 107's structural cut), located on the text with the SGR
# sequences stripped and then read off the `-e` capture, where a run of
# same-coloured half-block cells opens with `ESC[38;2;R;G;Bm` — one run per
# row of a solid picture, since tmux coalesces equal cells into one escape.
img_block_fg() { # $1 = a `capture-pane -e` dump → the fg colour of each run on the picture rows
	local raw="$1" plain start end
	plain="$(printf '%s\n' "$raw" | sed -E $'s/\x1b\\[[0-9;?]*[A-Za-z]//g')"
	start="$(printf '%s\n' "$plain" | grep -n -F 'Read image (PNG, 100x60' | head -1 | cut -d: -f1)"
	end="$(printf '%s\n' "$plain" | grep -n -F 'A solid square' | head -1 | cut -d: -f1)"
	if [ -z "$start" ] || [ -z "$end" ] || [ "$end" -le "$start" ]; then
		return 0
	fi
	printf '%s\n' "$raw" | sed -n "$((start + 1)),$((end - 1))p" | grep -oE '38;2;[0-9]+;[0-9]+;[0-9]+m'
}
img_inline_fg="$(img_block_fg "$(tmux capture-pane -t "$S107D" -p -e -S -60)")"
img_inline_red_rows="$(printf '%s\n' "$img_inline_fg" | grep -c '^38;2;255;0;0m$')"
img_inline_blue_rows="$(printf '%s\n' "$img_inline_fg" | grep -c '^38;2;0;0;255m$')"
echo "==== Phase 107d: inline (red fixture) — listed=$img_listed loaded=$img_loaded red_rows=$img_inline_red_rows blue_rows=$img_inline_blue_rows ===="
# The agent's conversion: the same file, the same size, new bytes.
printf '%s' "$BLUE_PNG64" | base64 -d >"$IMG_DIR/shot.png"
sleep 0.3
# Ctrl+O paints the same placement afresh — from the file as it is now.
tmux send-keys -t "$S107D" C-o
img_overlay_fg=""
for _ in $(seq 1 40); do
	img_overlay_fg="$(img_block_fg "$(tmux capture-pane -t "$S107D" -p -e)")"
	if [ "$(printf '%s\n' "$img_overlay_fg" | grep -c '38;2;')" -ge 6 ]; then
		break
	fi
	sleep 0.1
done
img_overlay_red_rows="$(printf '%s\n' "$img_overlay_fg" | grep -c '^38;2;255;0;0m$')"
img_overlay_blue_rows="$(printf '%s\n' "$img_overlay_fg" | grep -c '^38;2;0;0;255m$')"
echo "==== Phase 107d: Ctrl+O after the rewrite — red_rows=$img_overlay_red_rows blue_rows=$img_overlay_blue_rows ===="
tmux capture-pane -t "$S107D" -p | grep -v '^$' | head -12
tmux send-keys -t "$S107D" C-o
sleep 0.5
tmux kill-session -t "$S107D" 2>/dev/null
rm -rf "$IMG_DIR" "$IMG_SESS"
if [ "$img_listed" != 1 ] || [ "$img_loaded" != 1 ]; then
	fail "precondition — the seeded image session did not list/load (listed=$img_listed loaded=$img_loaded)"
fi
if [ "$img_inline_red_rows" -lt 6 ] || [ "$img_inline_blue_rows" -ne 0 ]; then
	fail "precondition — the red fixture did not paint red half-block rows inline (red=$img_inline_red_rows blue=$img_inline_blue_rows)"
fi
if [ "$img_overlay_blue_rows" -lt 6 ]; then
	fail "the picture was rewritten in place but the transcript drew $img_overlay_blue_rows blue rows — the stale encoding was served instead of the file as it is now"
fi
if [ "$img_overlay_red_rows" -ne 0 ]; then
	fail "the transcript still shows $img_overlay_red_rows red rows of the picture that was overwritten"
fi
