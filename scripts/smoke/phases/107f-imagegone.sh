#!/usr/bin/env bash
# Phase 107f — a picture whose FILE IS GONE reserves no blank rows

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# A FILE THAT IS GONE (docs/images.md). The `read` tool's fact line keeps a
# picture's size across a resume — `Read image (PNG, 1883x1007, 1.6 MB)` —
# but not its file: a screenshot the agent saved in the session scratchpad
# under /tmp is wiped by a reboot, and resuming the session reserved the
# picture's whole footprint anyway, a block of blank rows under the cell that
# nothing would ever draw into (the reported bug). A picture is now reserved
# only while its file is there, so the cell stands alone, as it does with
# Show images off.
#
# A handcrafted rollout carries two image reads under half-blocks at a 5x10
# cell, launched with `--resume {id}` (the exit hint's own command): one of a
# file that does not exist (its whole scratchpad folder gone, as after the
# reboot), one of a real 100x60 PNG, which must still draw its 6 rows — the
# guard against a run where no picture draws at all. Both are measured inline
# and in the Ctrl+O transcript. Then the real file is deleted while the
# session runs and the terminal is resized: the purge rebuild that follows
# must give its rows back too, instead of reserving a block the purge has
# just dropped the encoded picture for.
S107F="${S}_imagegone"
IMG_DIR="$(mktemp -d "$SMOKE_TMP/imagegone.XXXXXX")"
IMG_SESS="$(mktemp -d "$SMOKE_TMP/imggonesess.XXXXXX")"
img_day="$IMG_SESS/2026/07/23"
mkdir -p "$img_day"
# Phase 107's gradient: 100x60, so exactly 20 columns by 6 rows at 5x10.
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
# The picture the reboot took: never created, its scratchpad folder neither.
GONE="$IMG_DIR/scratchpad/desktop-first.png"
img_file="$img_day/rollout-2026-07-23T10-00-00-107f0000.jsonl"
{
	printf '{"timestamp":"2026-07-23T10:00:00.000Z","type":"session_meta","payload":{"id":"smoke-107f","timestamp":"2026-07-23T10:00:00.000Z","cwd":"%s","model":"dummy_model_name","originator":"alter-zero","version":"0.1.0"}}\n' "$IMG_DIR"
	printf '{"timestamp":"2026-07-23T10:00:01.000Z","type":"message","payload":{"role":"user","text":"look at both pictures","timestamp":"10:00 AM"}}\n'
	printf '{"timestamp":"2026-07-23T10:00:02.000Z","type":"tool","payload":{"name":"Read","args":"%s","ok":true,"output":"Read image (PNG, 100x60, 1 KB)","timestamp":"10:00 AM","shell":false,"truncated":false,"arguments":"{\\"path\\":\\"%s\\"}"}}\n' "$GONE" "$GONE"
	printf '{"timestamp":"2026-07-23T10:00:03.000Z","type":"message","payload":{"role":"assistant","text":"The desktop screenshot.","timestamp":"10:00 AM"}}\n'
	printf '{"timestamp":"2026-07-23T10:00:04.000Z","type":"tool","payload":{"name":"Read","args":"%s/shot.png","ok":true,"output":"Read image (PNG, 100x60, 2 KB)","timestamp":"10:00 AM","shell":false,"truncated":false,"arguments":"{\\"path\\":\\"%s/shot.png\\"}"}}\n' "$IMG_DIR" "$IMG_DIR"
	printf '{"timestamp":"2026-07-23T10:00:05.000Z","type":"message","payload":{"role":"assistant","text":"A colour gradient with a white diagonal.","timestamp":"10:00 AM"}}\n'
} >"$img_file"
IMG_ENV="$CFG_ENV_NOHIST ALTER_ZERO_SESSIONS_DIR=$IMG_SESS ALTER_ZERO_IMAGE_PROTOCOL=halfblocks ALTER_ZERO_IMAGE_CELL_SIZE=5x10 ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS"
# The rows between the line holding $2 (a cell's fact line) and the next line
# holding $3 (the reply under that cell) — blank rows included, which is the
# whole point: the bug's rows were blank. Fixed-string matches (`index`), so
# the fact line's parentheses need no escaping.
gone_between() {
	printf '%s\n' "$1" | awk -v from="$2" -v to="$3" '
		index($0, from) { seen = 1; next }
		seen && index($0, to) { exit }
		seen { print }
	'
}
GONE_FACT="Read image (PNG, 100x60, 1 KB)"
GONE_REPLY="The desktop screenshot."
KEPT_FACT="Read image (PNG, 100x60, 2 KB)"
KEPT_REPLY="A colour gradient"
# The gone cell's rows: its trailing spacer alone. The bug added the picture's
# own blank spacer and its 6 blank rows above it.
gone_rows() { gone_between "$1" "$GONE_FACT" "$GONE_REPLY" | wc -l | tr -d ' '; }
# Every row under the kept cell, and the drawn ones among them.
kept_rows() { gone_between "$1" "$KEPT_FACT" "$KEPT_REPLY" | wc -l | tr -d ' '; }
kept_drawn() { gone_between "$1" "$KEPT_FACT" "$KEPT_REPLY" | grep -cve '^[[:space:]]*$'; }

launch -c "$IMG_DIR" "$S107F" 100 40 "env $IMG_ENV $BIN_ABS --resume 107f0000"
wait_for 10 "$S107F" -S -80 -- -F "$KEPT_REPLY" || fail "precondition — the seeded session did not load"
sleep 0.6
inline="$(pane "$S107F" -S -80)"
dump "inline, resumed" "$(printf '%s\n' "$inline" | grep -v '^$' | tail -20)"
inline_gone="$(gone_rows "$inline")"
inline_kept="$(kept_drawn "$inline")"
note "inline: gone cell rows=$inline_gone (1 = the spacer alone) kept picture rows=$inline_kept"
expect_eq "$inline_gone" 1 "a read whose file is gone left a blank block under its cell"
expect_eq "$inline_kept" 6 "precondition — the picture whose file is there did not draw its 6 rows"

# Ctrl+O draws the same transcript: no block for the gone picture there
# either, and the kept one still drawn.
keys "$S107F" C-o
wait_for 5 "$S107F" -F "T R A N S C R I P T" || fail "Ctrl+O did not open the transcript"
overlay=""
poll_overlay() {
	overlay="$(pane "$S107F")"
	[ "$(kept_drawn "$overlay")" -ge 6 ]
}
poll 5 poll_overlay || true
dump "Ctrl+O" "$(printf '%s\n' "$overlay" | grep -v '^$' | head -30)"
overlay_gone="$(gone_rows "$overlay")"
overlay_kept="$(kept_drawn "$overlay")"
note "Ctrl+O: gone cell rows=$overlay_gone kept picture rows=$overlay_kept"
expect_eq "$overlay_gone" 1 "the transcript left a blank block under the read whose file is gone"
expect_eq "$overlay_kept" 6 "precondition — the transcript did not draw the picture whose file is there"
keys "$S107F" C-o
wait_for 5 "$S107F" -E '^❯' || fail "Ctrl+O did not close"

# The kept file goes while the session runs, and a resize rebuilds the
# conversation from history: the rebuild must not reserve its block.
rm -f "$IMG_DIR/shot.png"
tmux resize-window -t "$S107F" -x 90 -y 40
after=""
poll_rebuilt() {
	after="$(pane "$S107F")"
	[ "$(kept_rows "$after")" = 1 ]
}
poll 5 poll_rebuilt || true
dump "after deleting shot.png and resizing to 90 columns" "$(printf '%s\n' "$after" | grep -v '^$' | tail -20)"
after_kept="$(kept_rows "$after")"
after_gone="$(gone_rows "$after")"
note "after the resize: deleted picture rows=$after_kept (1 = the spacer alone) gone cell rows=$after_gone"
expect_eq "$after_kept" 1 "a picture deleted mid-session still reserved its block after the resize rebuilt the conversation"
expect_eq "$after_gone" 1 "the resize brought a blank block back under the read whose file is gone"
expect_has "$after" -F "$KEPT_FACT" "the rebuild lost the cell along with its picture"

tmux kill-session -t "$S107F" 2>/dev/null
rm -rf "$IMG_DIR" "$IMG_SESS"
