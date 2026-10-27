#!/usr/bin/env bash
# Phase 107f — a picture whose file is gone holds no blank rows, and draws again once it is back

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# A PICTURE WHOSE FILE IS GONE (docs/images.md). The reported bug: the agent
# reads a screenshot it saved under `/tmp`, the machine reboots and `/tmp`
# goes with it, and resuming the session drew a screenful of blank rows
# between the `Read image` cell and the reply. The cell's own fact line still
# names the picture's size, so its rows were reserved, and with no file left
# there was nothing to draw into them. The boundary now looks again for every
# picture a conversation shows whenever it lays the conversation out from
# history — a `--resume`/`--continue` load and every purge rebuild — and a
# picture it cannot find reserves nothing.
#
# A handcrafted rollout carries an image `read` of `shot.png`, a 100x60 PNG
# that is NOT on disk, and a later message with a Ctrl+V paste whose file IS
# there — the same look is what gives a resumed paste its size back, since a
# paste has no fact line to carry one. Half-blocks at a 5x10 cell, so either
# picture, whenever it draws, is exactly 6 rows (Phase 107's footprint):
#
# 1. `--resume {id}` commits the transcript at startup with no rebuild: one
#    blank row between the read and its reply, as if there were no picture,
#    and the paste drawn under its bubble;
# 2. the `/resume` picker loads it through the purge rebuild: the same, and
#    so does the Ctrl+O transcript built over it;
# 3. the file is written back and a resize rebuilds: the picture draws again;
# 4. the file is deleted mid-session and a resize rebuilds: its rows go with
#    it, rather than staying a blank block the purge left nothing to fill.
S107F="${S}_imagegone"
IMG_DIR="$(mktemp -d "$SMOKE_TMP/imagegone.XXXXXX")"
IMG_SESS="$(mktemp -d "$SMOKE_TMP/imggonesess.XXXXXX")"
img_day="$IMG_SESS/2026/07/23"
mkdir -p "$img_day"
# Phase 107's fixture: a 100x60 colour gradient with a white diagonal, whose
# half-block rows are visible glyphs to `capture-pane` (a solid colour would
# paint as coloured spaces, which a plain capture cannot tell from blanks).
IMG_PNG64='iVBORw0KGgoAAAANSUhEUgAAAGQAAAA8CAIAAAAfXYiZAAADYUlEQVR4Ae3AA6AkWZbG8f937o3IzKdyS2Oubdu2bdu2bdu2bWmMnpZKr54yMyLu+Xa3anqmhztr1a/aLvqjQguy0IIstCALLchCK7QgCy3IQguy0IIstEILstCCLLQgCy3IQguy0AotyEILstCCLLQgC63Qgiy0IAstyEILstCCLLRCC7LQgiy0IAstyEIrtCALLchCC7LQgiy0IAut0IIstCALLchCC7KQBQIKBBQIKBBQIKBAgYACAQUCCgQUqOr+JCpX/QsAqFSu+pcBUKk80P7+6x3f/lWuei4AVDoeaPvkb5SOq54bAJXKVf8yACqVF+Kuu9765ht+gqsAqFReiBtu+elSuQoAKh1X/csAqFRedE984ns85lHfw/9DAFQqL7pHvdj3ReX/IwAqlav+ZQBUOv7N/vzPP/QVX/7r+f8AgErl3+zlX/kbo/L/AgCVylX/MgAqlf8ov/3bn/h6r/1F/J8EQKXjP8prv8GXlo7/mwCoVK76lwFQqfwn+bmf+7y3fotP5f8GACqV/yRv8TafUSr/RwBQ6bjqXwZApfJf4wd/8Kvf/V0/kv+lAKhU/mu863t+dFT+twKgUrnqXwZApeO/xbd8y3d8yAe9D/9bAFCp/Lf4oA97v6j8rwFApXLVvwyASuV/gq/4ih/5xI97e/7HAqDS8T/Bx33yO5WO/7kAqFSu+pcBUKn8D/TZn/2Ln/fZb8T/HABUKv8Dffbnv2mp/A8CQKXjqn8ZAJXK/3wf93G/91Vf8Wr8NwKgUvmf7yu+5jWi8t8JgErlqn8ZAJWO/3U+8AP/+tu/9SX5rwRApfK/zrd+50tH5b8UAJXKVf8yACqV/+3e9V2f8iM/+BD+UwFQ6fjf7gd/7OGl4z8XAJXKVf8yACqV/2Pe4i3u/cWfO81/LAAqlf9jfu6Xri2V/2AAVDqu+pcBUKn83/bar330u789598JgErl/7bf/v2NqPx7AVCpXPUvA6DS8f/Ky72c/+ovxL8WAJXK/yt/8TeKyr8aAJXKVf8yACqV/88e9Sg/5YniXwRApeP/syc+TaXjXwZApXLVvwyASuWqZ7nhBt97l3heAFQqVz3LXfepVJ4PACodV/3LAKhUrnpBtrd9uC8AACqVq16Q/aWiAgBApXLVvwyAfwRqc6dvjMnyQgAAAABJRU5ErkJggg=='
printf '%s' "$IMG_PNG64" | base64 -d >"$IMG_DIR/pasted.png"
img_file="$img_day/rollout-2026-07-23T10-00-00-107f0000.jsonl"
{
	printf '{"timestamp":"2026-07-23T10:00:00.000Z","type":"session_meta","payload":{"id":"smoke-107f","timestamp":"2026-07-23T10:00:00.000Z","cwd":"%s","model":"dummy_model_name","originator":"alter-zero","version":"0.1.0"}}\n' "$IMG_DIR"
	printf '{"timestamp":"2026-07-23T10:00:01.000Z","type":"message","payload":{"role":"user","text":"look at shot.png","timestamp":"10:00 AM"}}\n'
	printf '{"timestamp":"2026-07-23T10:00:02.000Z","type":"tool","payload":{"name":"Read","args":"%s/shot.png","ok":true,"output":"Read image (PNG, 100x60, 922 B)","timestamp":"10:00 AM","shell":false,"truncated":false,"arguments":"{\\"path\\":\\"%s/shot.png\\"}"}}\n' "$IMG_DIR" "$IMG_DIR"
	printf '{"timestamp":"2026-07-23T10:00:03.000Z","type":"message","payload":{"role":"assistant","text":"That is the desktop.","timestamp":"10:00 AM"}}\n'
	printf '{"timestamp":"2026-07-23T10:00:04.000Z","type":"message","payload":{"role":"user","text":"[Image #1] and this one","timestamp":"10:01 AM","images":["%s/pasted.png"]}}\n' "$IMG_DIR"
	printf '{"timestamp":"2026-07-23T10:00:05.000Z","type":"message","payload":{"role":"assistant","text":"A gradient.","timestamp":"10:01 AM"}}\n'
} >"$img_file"
IMG_ENV="$CFG_ENV_NOHIST ALTER_ZERO_SESSIONS_DIR=$IMG_SESS ALTER_ZERO_IMAGE_PROTOCOL=halfblocks ALTER_ZERO_IMAGE_CELL_SIZE=5x10 ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS"

# The rows between the `Read image` cell and the reply under it, blank ones
# included: 1 with no picture (the item's spacer), 8 with one (a spacer, the
# 6 rows of the picture, the item's spacer) — and 8 blank ones was the bug.
gone_between() { # $1 = a captured pane
	printf '%s\n' "$1" | awk '
		/Read image \(PNG, 100x60/ { seen = 1; next }
		seen && /That is the desktop/ { exit }
		seen { n++ }
		END { print n + 0 }
	'
}
# …and only the non-blank ones: the picture's own rows.
gone_picture_rows() { # $1 = a captured pane
	printf '%s\n' "$1" | awk '
		/Read image \(PNG, 100x60/ { seen = 1; next }
		seen && /That is the desktop/ { exit }
		seen && NF { n++ }
		END { print n + 0 }
	'
}
# The pasted picture's rows: the non-blank ones under its bubble.
gone_paste_rows() { # $1 = a captured pane
	printf '%s\n' "$1" | awk '
		/\[Image #1\] and this one/ { seen = 1; next }
		seen && /A gradient/ { exit }
		seen && NF { n++ }
		END { print n + 0 }
	'
}

# 1. `--resume {id}`: the startup load commits the transcript under the
#    banner, no rebuild involved.
launch -c "$IMG_DIR" "$S107F" 100 30 "env $IMG_ENV $BIN_ABS --resume 107f0000"
gone_cli_loaded=0
if wait_for 6 "$S107F" -S -60 -- -F "A gradient"; then
	gone_cli_loaded=1
fi
gone_cli="$(pane "$S107F" -S -60)"
gone_cli_between="$(gone_between "$gone_cli")"
gone_cli_paste="$(gone_paste_rows "$gone_cli")"
dump "--resume with the read's file gone — rows between: $gone_cli_between, paste rows: $gone_cli_paste" "$gone_cli"
tmux kill-session -t "$S107F" 2>/dev/null

# 2. The `/resume` picker: the load is a purge rebuild.
launch -c "$IMG_DIR" "$S107F" 100 30 "env $IMG_ENV $BIN_ABS"
type_text "$S107F" "/resume"
sleep 0.3
keys "$S107F" Enter
gone_listed=0
if wait_for 4 "$S107F" -F "look at shot.png"; then
	gone_listed=1
fi
keys "$S107F" Enter # load it
gone_picker_loaded=0
if wait_for 6 "$S107F" -S -60 -- -F "A gradient"; then
	gone_picker_loaded=1
fi
gone_picker="$(pane "$S107F" -S -60)"
gone_picker_between="$(gone_between "$gone_picker")"
gone_picker_paste="$(gone_paste_rows "$gone_picker")"
dump "/resume with the read's file gone — rows between: $gone_picker_between, paste rows: $gone_picker_paste" "$gone_picker"
# …and the Ctrl+O transcript, built through its own cache, agrees: read from
# its top (Home), since it opens pinned to the bottom.
keys "$S107F" C-o
gone_overlay_open=0
if wait_for 4 "$S107F" -F "T R A N S C R I P T"; then
	gone_overlay_open=1
fi
keys "$S107F" Home
gone_overlay=""
gone_overlay_closed() {
	gone_overlay="$(pane "$S107F")"
	[ "$(gone_between "$gone_overlay")" -eq 1 ]
}
poll 3 gone_overlay_closed
gone_overlay_between="$(gone_between "$gone_overlay")"
dump "Ctrl+O over the resumed session — rows between: $gone_overlay_between" "$gone_overlay"
keys "$S107F" C-o
wait_for 4 "$S107F" -F "dummy_model_name ·" || fail "Ctrl+O did not close"

# 3. The file comes back, and the rebuild a resize runs finds it.
printf '%s' "$IMG_PNG64" | base64 -d >"$IMG_DIR/shot.png"
tmux resize-window -t "$S107F" -x 96 -y 30
gone_back=""
gone_drawn() {
	gone_back="$(pane "$S107F" -S -60)"
	[ "$(gone_picture_rows "$gone_back")" -eq 6 ]
}
poll 4 gone_drawn
gone_back_rows="$(gone_picture_rows "$gone_back")"
dump "the file written back, a resize rebuilt — picture rows: $gone_back_rows" "$gone_back"

# 4. The file goes again mid-session: the next rebuild drops the encoded
#    picture with the scrollback, so its rows must go too.
rm -f "$IMG_DIR/shot.png"
tmux resize-window -t "$S107F" -x 100 -y 30
gone_again=""
gone_closed() {
	gone_again="$(pane "$S107F" -S -60)"
	[ "$(gone_between "$gone_again")" -eq 1 ]
}
poll 4 gone_closed
gone_again_between="$(gone_between "$gone_again")"
dump "the file deleted, a resize rebuilt — rows between: $gone_again_between" "$gone_again"

if [ "$gone_cli_loaded" != 1 ] || [ "$gone_listed" != 1 ] || [ "$gone_picker_loaded" != 1 ]; then
	fail "precondition — the seeded session did not load (--resume=$gone_cli_loaded listed=$gone_listed picker=$gone_picker_loaded)"
fi
expect_eq "$gone_cli_between" 1 "--resume held blank rows for a picture whose file is gone"
expect_eq "$gone_picker_between" 1 "the /resume picker held blank rows for a picture whose file is gone"
expect_eq "$gone_cli_paste" 6 "--resume did not draw the pasted picture whose file is there"
expect_eq "$gone_picker_paste" 6 "the /resume picker did not draw the pasted picture whose file is there"
expect_eq "$gone_overlay_open" 1 "precondition — Ctrl+O did not open the transcript"
expect_eq "$gone_overlay_between" 1 "the Ctrl+O transcript held blank rows for a picture whose file is gone"
expect_eq "$gone_back_rows" 6 "the picture did not draw again once its file was back"
expect_eq "$gone_again_between" 1 "a rebuild after the file was deleted kept a blank block where the picture was"
