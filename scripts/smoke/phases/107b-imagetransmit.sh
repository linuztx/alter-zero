#!/usr/bin/env bash
# Phase 107b — the Ctrl+O overlay must TRANSMIT the picture, not just place it

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the Ctrl+O overlay must TRANSMIT the picture, not just place
# it (docs/images.md). A graphics placement belongs to the screen it was made
# on, and the kitty protocol transmits its image — creating that placement —
# exactly once per encoded protocol object. Carrying one across the switch to
# the alternate screen therefore painted the transcript with unicode
# placeholders pointing at a placement that only existed on the primary
# screen: reserved rows, and nothing in them. `capture-pane` cannot see that
# (the placeholders ARE ordinary cells), so this phase reads the raw byte
# stream with `pipe-pane` and asserts a kitty transmit — `ESC _ G … a=T` —
# lands on *each* side of the switch.
S107B="${S}_imagetransmit"
IMG_DIR2="$(mktemp -d "$SMOKE_TMP/imagetx.XXXXXX")"
IMG_SESS2="$(mktemp -d "$SMOKE_TMP/imgtxsess.XXXXXX")"
IMG_RAW="$(mktemp -d "$SMOKE_TMP/imgraw.XXXXXX")/raw.bin"
mkdir -p "$IMG_SESS2/2026/07/23"
base64 -d >"$IMG_DIR2/shot.png" <<'PNG64'
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
img_file2="$IMG_SESS2/2026/07/23/rollout-2026-07-23T10-00-00-10710711.jsonl"
{
	printf '{"timestamp":"2026-07-23T10:00:00.000Z","type":"session_meta","payload":{"id":"smoke-107b","timestamp":"2026-07-23T10:00:00.000Z","cwd":"%s","model":"dummy_model_name","originator":"alter-zero","version":"0.1.0"}}\n' "$IMG_DIR2"
	printf '{"timestamp":"2026-07-23T10:00:01.000Z","type":"message","payload":{"role":"user","text":"look at shot.png","timestamp":"10:00 AM"}}\n'
	printf '{"timestamp":"2026-07-23T10:00:02.000Z","type":"tool","payload":{"name":"Read","args":"%s/shot.png","ok":true,"output":"Read image (PNG, 100x60, 2 KB)","timestamp":"10:00 AM","shell":false,"truncated":false,"arguments":"{\\"path\\":\\"%s/shot.png\\"}"}}\n' "$IMG_DIR2" "$IMG_DIR2"
	printf '{"timestamp":"2026-07-23T10:00:03.000Z","type":"message","payload":{"role":"assistant","text":"A colour gradient with a white diagonal.","timestamp":"10:00 AM"}}\n'
} >"$img_file2"
IMG_ENV2="$CFG_ENV_NOHIST ALTER_ZERO_SESSIONS_DIR=$IMG_SESS2 ALTER_ZERO_IMAGE_PROTOCOL=kitty ALTER_ZERO_IMAGE_CELL_SIZE=5x10 ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS"
launch -c "$IMG_DIR2" "$S107B" 100 30 "env $IMG_ENV2 $BIN_ABS"
tmux pipe-pane -t "$S107B" -o "cat >> $IMG_RAW"
sleep 0.3
tmux send-keys -t "$S107B" -l "/resume"
sleep 0.3
tmux send-keys -t "$S107B" Enter
sleep 1.2
tmux send-keys -t "$S107B" Enter # load the seeded session
img_tx_inline=0
for _ in $(seq 1 60); do
	if tmux capture-pane -t "$S107B" -p -S -60 | grep -qF "Read image (PNG, 100x60"; then
		img_tx_inline=1
		break
	fi
	sleep 0.1
done
sleep 0.8
img_split="$(stat -c%s "$IMG_RAW" 2>/dev/null || echo 0)"
tmux send-keys -t "$S107B" C-o
sleep 1.5
# A kitty transmit is `ESC _ G <controls incl. a=T> ; <base64> ESC \` — count
# them on each side of the byte offset the switch happened at.
img_kitty_tx() { grep -ao "$(printf '\033')_G[^;]*a=T" 2>/dev/null | wc -l; }
img_tx_before="$(head -c "$img_split" "$IMG_RAW" | img_kitty_tx)"
img_tx_after="$(tail -c +$((img_split + 1)) "$IMG_RAW" | img_kitty_tx)"
# …and it is uploaded ONCE per screen, not once per keypress. A picture is
# megabytes on the wire (raw RGBA — 4.5 MB of base64 for a 120x35-cell one),
# so re-uploading it on the way out, or on every reopen, is the flicker the
# per-screen cache exists to avoid. Coming back costs nothing because the
# overlay never touched the primary screen's store; reopening costs nothing
# because a virtually-placed image survives the 1049 switch.
img_split_back="$(stat -c%s "$IMG_RAW" 2>/dev/null || echo 0)"
tmux send-keys -t "$S107B" C-o
sleep 1.5
img_tx_back="$(tail -c +$((img_split_back + 1)) "$IMG_RAW" | img_kitty_tx)"
img_split_again="$(stat -c%s "$IMG_RAW" 2>/dev/null || echo 0)"
tmux send-keys -t "$S107B" C-o
sleep 1.5
img_tx_again="$(tail -c +$((img_split_again + 1)) "$IMG_RAW" | img_kitty_tx)"
echo "==== Phase 107b: kitty transmits — inline=$img_tx_before overlay=$img_tx_after back=$img_tx_back reopen=$img_tx_again (cell seen=$img_tx_inline) ===="
tmux kill-session -t "$S107B" 2>/dev/null
rm -rf "$IMG_DIR2" "$IMG_SESS2" "$(dirname "$IMG_RAW")"
if [ "$img_tx_inline" != 1 ]; then
	fail "precondition — the seeded image session never loaded"
fi
if [ "$img_tx_before" -lt 1 ]; then
	fail "the inline commit never transmitted the picture to the terminal"
fi
if [ "$img_tx_after" -lt 1 ]; then
	fail "the Ctrl+O overlay placed the picture without transmitting it for the alternate screen; it draws nothing there"
fi
if [ "$img_tx_back" -ne 0 ]; then
	fail "returning from the overlay re-uploaded the picture ($img_tx_back transmits); the primary screen's encoding must survive the round trip"
fi
if [ "$img_tx_again" -ne 0 ]; then
	fail "reopening the overlay re-uploaded the picture ($img_tx_again transmits); a virtually-placed image survives the 1049 switch, so each screen uploads once"
fi
