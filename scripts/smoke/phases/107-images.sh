#!/usr/bin/env bash
# Phase 107 — INLINE IMAGES

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# INLINE IMAGES (docs/images.md). A handcrafted rollout carries
# an image `read` of a real PNG in the cwd; resuming it must draw the picture
# in the terminal — flush at the left margin, one blank row under the cell —
# and Ctrl+O must show the same picture in the transcript. The protocol is
# pinned to half-blocks (real coloured cells, so `capture-pane` can see them)
# and the cell size to 5x10, so the footprint is deterministic: a 100x60 PNG
# is exactly 20 columns by 6 rows. Then `/settings` **Show images** is cycled
# off, which must purge-rebuild the conversation WITHOUT the picture.
S107="${S}_images"
IMG_DIR="$(mktemp -d "$SMOKE_TMP/images.XXXXXX")"
IMG_SESS="$(mktemp -d "$SMOKE_TMP/imgsess.XXXXXX")"
img_day="$IMG_SESS/2026/07/23"
mkdir -p "$img_day"
base64 -d >"$IMG_DIR/shot.png" <<'PNG64'
iVBORw0KGgoAAAANSUhEUgAAAGQAAAA8CAIAAAAfXYiZAAADYUlEQVR4Ae3AA6AkWZbG8f937o3I
zKdyS2Oubdu2bdu2bdu2bWmMnpZKr54yMyLu+Xa3anqmhztr1a/aLvqjQguy0IIstCALLchCK7Qg
Cy3IQguy0IIstEILstCCLLQgCy3IQguy0AotyEILstCCLLQgC63Qgiy0IAstyEILstCCLLRCC7LQ
giy0IAstyEIrtCALLchCC7LQgiy0IAut0IIstCALLchCC7KQBQIKBBQIKBBQIKBAgYACAQUCCgQU
qOr+JCpX/QsAqFSu+pcBUKk80P7+6x3f/lWuei4AVDoeaPvkb5SOq54bAJXKVf8yACqVF+Kuu976
5ht+gqsAqFReiBtu+elSuQoAKh1X/csAqFRedE984ns85lHfw/9DAFQqL7pHvdj3ReX/IwAqlav+
ZQBUOv7N/vzPP/QVX/7r+f8AgErl3+zlX/kbo/L/AgCVylX/MgAqlf8ov/3bn/h6r/1F/J8EQKXj
P8prv8GXlo7/mwCoVK76lwFQqfwn+bmf+7y3fotP5f8GACqV/yRv8TafUSr/RwBQ6bjqXwZApfJf
4wd/8Kvf/V0/kv+lAKhU/mu863t+dFT+twKgUrnqXwZApeO/xbd8y3d8yAe9D/9bAFCp/Lf4oA97
v6j8rwFApXLVvwyASuV/gq/4ih/5xI97e/7HAqDS8T/Bx33yO5WO/7kAqFSu+pcBUKn8D/TZn/2L
n/fZb8T/HABUKv8Dffbnv2mp/A8CQKXjqn8ZAJXK/3wf93G/91Vf8Wr8NwKgUvmf7yu+5jWi8t8J
gErlqn8ZAJWO/3U+8AP/+tu/9SX5rwRApfK/zrd+50tH5b8UAJXKVf8yACqV/+3e9V2f8iM/+BD+
UwFQ6fjf7gd/7OGl4z8XAJXKVf8yACqV/2Pe4i3u/cWfO81/LAAqlf9jfu6Xri2V/2AAVDqu+pcB
UKn83/bar330u789598JgErl/7bf/v2NqPx7AVCpXPUvA6DS8f/Ky72c/+ovxL8WAJXK/yt/8TeK
yr8aAJXKVf8yACqV/88e9Sg/5YniXwRApeP/syc+TaXjXwZApXLVvwyASuWqZ7nhBt97l3heAFQq
Vz3LXfepVJ4PACodV/3LAKhUrnpBtrd9uC8AACqVq16Q/aWiAgBApXLVvwyAfwRqc6dvjMnyQgAA
AABJRU5ErkJggg==
PNG64
img_file="$img_day/rollout-2026-07-23T10-00-00-10710710.jsonl"
{
	printf '{"timestamp":"2026-07-23T10:00:00.000Z","type":"session_meta","payload":{"id":"smoke-107","timestamp":"2026-07-23T10:00:00.000Z","cwd":"%s","model":"dummy_model_name","originator":"alter-zero","version":"0.1.0"}}\n' "$IMG_DIR"
	printf '{"timestamp":"2026-07-23T10:00:01.000Z","type":"message","payload":{"role":"user","text":"look at shot.png","timestamp":"10:00 AM"}}\n'
	printf '{"timestamp":"2026-07-23T10:00:02.000Z","type":"tool","payload":{"name":"Read","args":"%s/shot.png","ok":true,"output":"Read image (PNG, 100x60, 2 KB)","timestamp":"10:00 AM","shell":false,"truncated":false,"arguments":"{\\"path\\":\\"%s/shot.png\\"}"}}\n' "$IMG_DIR" "$IMG_DIR"
	printf '{"timestamp":"2026-07-23T10:00:03.000Z","type":"message","payload":{"role":"assistant","text":"A colour gradient with a white diagonal.","timestamp":"10:00 AM"}}\n'
} >"$img_file"
IMG_ENV="$CFG_ENV_NOHIST ALTER_ZERO_SESSIONS_DIR=$IMG_SESS ALTER_ZERO_IMAGE_PROTOCOL=halfblocks ALTER_ZERO_IMAGE_CELL_SIZE=5x10 ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS"
launch -c "$IMG_DIR" "$S107" 100 30 "env $IMG_ENV $BIN_ABS"
tmux send-keys -t "$S107" -l "/resume"
sleep 0.3
tmux send-keys -t "$S107" Enter
img_listed=0
for _ in $(seq 1 40); do
	if tmux capture-pane -t "$S107" -p | grep -qF "look at shot.png"; then
		img_listed=1
		break
	fi
	sleep 0.1
done
tmux send-keys -t "$S107" Enter # load it
img_loaded=0
for _ in $(seq 1 60); do
	if tmux capture-pane -t "$S107" -p -S -60 | grep -qF "Read image (PNG, 100x60"; then
		img_loaded=1
		break
	fi
	sleep 0.1
done
sleep 0.6
img_pane="$(tmux capture-pane -t "$S107" -p -S -60)"
# The picture's rows are the non-blank ones between the `Read image` cell and
# the reply that follows it — counted structurally rather than by matching the
# block glyphs, which `grep`'s byte-oriented classes can't size in the C
# locale. A 100x60 PNG at a 5x10 cell is exactly 6 rows.
img_between() {
	printf '%s\n' "$1" | awk '
		/Read image \(PNG, 100x60/ { seen = 1; next }
		seen && /A colour gradient/ { exit }
		seen { print }
	'
}
img_rows="$(img_between "$img_pane" | grep -cve '^[[:space:]]*$')"
# …and they start at column 0: not indented into the cell's `⎿` gutter.
img_flush=0
if [ -n "$(img_between "$img_pane" | grep -ve '^[[:space:]]*$' | grep -ve '^[[:space:]]')" ]; then
	img_flush=1
fi
# …directly under the cell, one blank row between (the picture is not indented
# into the `⎿` gutter, and it is not glued to it either).
img_gap=0
if printf '%s\n' "$img_pane" | grep -A 2 -F "Read image (PNG, 100x60" | sed -n '2p' | grep -qE '^[[:space:]]*$'; then
	img_gap=1
fi
echo "==== Phase 107: inline image — listed=$img_listed loaded=$img_loaded rows=$img_rows flush=$img_flush gap=$img_gap ===="
printf '%s\n' "$img_pane" | grep -v '^$' | tail -12
# Ctrl+O shows the same picture in the full-screen transcript.
tmux send-keys -t "$S107" C-o
img_overlay=0
for _ in $(seq 1 40); do
	if [ "$(img_between "$(tmux capture-pane -t "$S107" -p)" | grep -cve '^[[:space:]]*$')" -ge 6 ]; then
		img_overlay=1
		break
	fi
	sleep 0.1
done
tmux send-keys -t "$S107" C-o
sleep 0.5
# /settings → Show images → off: the rebuild must drop the picture.
tmux send-keys -t "$S107" -l "/settings"
sleep 0.3
tmux send-keys -t "$S107" Enter
sleep 0.5
tmux send-keys -t "$S107" Down
sleep 0.2
img_row_seen=0
if tmux capture-pane -t "$S107" -p | grep -qE 'Show images +true'; then
	img_row_seen=1
fi
tmux send-keys -t "$S107" Enter
sleep 1.0
tmux send-keys -t "$S107" Escape
sleep 0.8
img_after="$(tmux capture-pane -t "$S107" -p -S -60)"
img_gone=1
if [ "$(img_between "$img_after" | grep -cve '^[[:space:]]*$')" -ne 0 ]; then
	img_gone=0
fi
img_cell_kept=0
if printf '%s' "$img_after" | grep -qF "Read image (PNG, 100x60"; then
	img_cell_kept=1
fi
echo "==== Phase 107: overlay=$img_overlay row_seen=$img_row_seen gone_after_toggle=$img_gone cell_kept=$img_cell_kept ===="
# …and back ON: the rebuild must redraw the picture — and the setting must
# not stay off, because it persists to settings.json in the config home the
# whole suite shares, where it would blank every picture the phases after
# this one (107b, 107c) expect to see.
tmux send-keys -t "$S107" -l "/settings"
sleep 0.3
tmux send-keys -t "$S107" Enter
sleep 0.5
tmux send-keys -t "$S107" Down
sleep 0.2
img_row_off=0
if tmux capture-pane -t "$S107" -p | grep -qE 'Show images +false'; then
	img_row_off=1
fi
tmux send-keys -t "$S107" Enter
sleep 1.0
tmux send-keys -t "$S107" Escape
sleep 0.8
img_back_rows="$(img_between "$(tmux capture-pane -t "$S107" -p -S -60)" | grep -cve '^[[:space:]]*$')"
echo "==== Phase 107: Show images back on — row_off_seen=$img_row_off rows=$img_back_rows ===="
tmux kill-session -t "$S107" 2>/dev/null
rm -rf "$IMG_DIR" "$IMG_SESS"
if [ "$img_listed" != 1 ] || [ "$img_loaded" != 1 ]; then
	fail "precondition — the seeded image session did not list/load (listed=$img_listed loaded=$img_loaded)"
fi
if [ "$img_rows" -ne 6 ]; then
	fail "expected 6 picture rows for a 100x60 image at a 5x10 cell, found $img_rows"
fi
if [ "$img_flush" != 1 ]; then
	fail "the picture is indented; it must start at column 0"
fi
if [ "$img_gap" != 1 ]; then
	fail "no blank row between the tool cell and the picture"
fi
if [ "$img_overlay" != 1 ]; then
	fail "the Ctrl+O transcript did not draw the picture"
fi
if [ "$img_row_seen" != 1 ]; then
	fail "/settings has no live `Show images` row"
fi
if [ "$img_gone" != 1 ]; then
	fail "turning Show images off left the picture on screen"
fi
if [ "$img_cell_kept" != 1 ]; then
	fail "the rebuild lost the tool cell along with the picture"
fi
if [ "$img_row_off" != 1 ]; then
	fail "/settings did not show Show images as false after the toggle"
fi
if [ "$img_back_rows" -ne 6 ]; then
	fail "turning Show images back on did not redraw the picture (rows=$img_back_rows)"
fi
