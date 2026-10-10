#!/usr/bin/env bash
# Phase 107e — a terminal the environment cannot name is asked whether it draws kitty graphics

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# A terminal the environment cannot name is ASKED (docs/images.md,
# *Detecting the terminal*). herdr's panes say `TERM=xterm-256color` and
# `TERM_PROGRAM=herdr` — nothing that names a graphics protocol — while its
# emulator speaks kitty's, so detection by environment alone drew every
# picture in half-blocks there. Where the environment leaves that fallback,
# `InlineViewport::init` now asks: the kitty graphics query and XTVERSION
# ride ahead of the cursor query in the one startup read (invariant 1), and
# a terminal answers in order, so the cursor report still ends the read.
#
# tmux cannot play that terminal — it answers no kitty query, and a
# multiplexer is never asked anyway — so this phase plays it itself: a pty
# whose master end answers the way the terminal under test would and records
# every byte the app writes. Each run resumes a rollout holding an image read
# and counts kitty transmits (`ESC _ G … a=T`) against half-block cells in
# the raw stream:
#
# 1. herdr 0.9.3's own answer (recorded from a real pane: `OK`, then
#    `libghostty`, then the cursor) → the picture goes out as kitty graphics,
#    and the query sits inside a pushed and popped title (tmux files an
#    unknown APC string as the pane's title);
# 2. a terminal that says nothing → half-blocks, as before;
# 3. WezTerm, which answers `OK` but draws no unicode placeholders, naming
#    itself → half-blocks;
# 4. a session under tmux → never asked at all;
# 5. keys typed while the read waits → replayed into the composer;
# 6. no cursor report at all → the old two-second failure, the tty restored.
EQ_TERM="$SMOKE_TMP/terminal.py"
cat >"$EQ_TERM" <<'PY'
"""Play the app's terminal on a pty: answer its startup questions, record its bytes."""
import argparse
import fcntl
import os
import select
import struct
import subprocess
import termios
import time

ap = argparse.ArgumentParser()
ap.add_argument("--kitty", choices=["ok", "silent"], default="silent")
ap.add_argument("--version")
ap.add_argument("--typeahead")
ap.add_argument("--no-cursor", action="store_true")
ap.add_argument("--seconds", type=float, default=3.0)
ap.add_argument("--log", required=True)
ap.add_argument("--report", required=True)
ap.add_argument("cmd", nargs=argparse.REMAINDER)
a = ap.parse_args()
cmd = a.cmd[1:] if a.cmd[:1] == ["--"] else a.cmd

master, slave = os.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 1000, 600))


def controlling_tty():
    os.setsid()
    fcntl.ioctl(0, termios.TIOCSCTTY, 0)


proc = subprocess.Popen(cmd, stdin=slave, stdout=slave, stderr=slave,
                        preexec_fn=controlling_tty, close_fds=True)
start = time.time()
out, pos, asked, typed = b"", 0, [], False


def answer():
    """Answer each complete question past `pos`, in the order it was asked."""
    global pos, typed
    while True:
        hits = [(out.find(p, pos), p) for p in (b"\x1b[6n", b"\x1b_G", b"\x1b[>q")]
        hits = [hit for hit in hits if hit[0] >= 0]
        if not hits:
            return
        at, pat = min(hits)
        if pat == b"\x1b_G":
            end = out.find(b"\x1b\\", at)
            if end < 0:
                return
            pos = end + 2
            if b"a=q" not in out[at:end]:
                continue  # a picture going out, not a question
            asked.append("kitty")
            reply = b"\x1b_Gi=31;OK\x1b\\" if a.kitty == "ok" else b""
        elif pat == b"\x1b[>q":
            pos = at + len(pat)
            asked.append("version")
            reply = b"\x1bP>|" + a.version.encode() + b"\x1b\\" if a.version else b""
        else:
            pos = at + len(pat)
            asked.append("cursor")
            reply = b"" if a.no_cursor else b"\x1b[1;1R"
        if a.typeahead and not typed:
            # Typed while the app waits on its answer: raw mode is on (the
            # question came after it), so nothing echoes these back.
            typed = True
            os.write(master, a.typeahead.encode())
            time.sleep(0.2)
        os.write(master, reply)


exited_after = None
with open(a.log, "wb") as log:
    while time.time() - start < a.seconds:
        if exited_after is None and proc.poll() is not None:
            exited_after = time.time() - start
        ready, _, _ = select.select([master], [], [], 0.05)
        if ready:
            data = os.read(master, 65536)
            out += data
            log.write(data)
            answer()
        elif exited_after is not None:
            break
if proc.poll() is None:
    proc.terminate()
    proc.wait()
attrs = termios.tcgetattr(slave)
with open(a.report, "w") as report:
    report.write("asked=%s\n" % ",".join(asked))
    report.write("exit=%s\n" % (proc.returncode if exited_after is not None else "running"))
    report.write("exited_after=%s\n" % ("%.1f" % exited_after if exited_after is not None else "-"))
    report.write("canonical=%d\n" % int(bool(attrs[3] & termios.ICANON) and bool(attrs[3] & termios.ECHO)))
PY

# The fixture: Phase 107's PNG, and a rollout whose image read draws it.
EQ_DIR="$(mktemp -d "$SMOKE_TMP/eq.XXXXXX")"
base64 -d >"$EQ_DIR/shot.png" <<'PNG64'
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
eq_day="$SMOKE_SESSIONS/2026/07/23"
mkdir -p "$eq_day"
{
	printf '{"timestamp":"2026-07-23T10:00:00.000Z","type":"session_meta","payload":{"id":"smoke-107e","timestamp":"2026-07-23T10:00:00.000Z","cwd":"%s","model":"dummy_model_name","originator":"alter-zero","version":"0.1.0"}}\n' "$EQ_DIR"
	printf '{"timestamp":"2026-07-23T10:00:01.000Z","type":"message","payload":{"role":"user","text":"look at shot.png","timestamp":"10:00 AM"}}\n'
	printf '{"timestamp":"2026-07-23T10:00:02.000Z","type":"tool","payload":{"name":"Read","args":"%s/shot.png","ok":true,"output":"Read image (PNG, 100x60, 2 KB)","timestamp":"10:00 AM","shell":false,"truncated":false,"arguments":"{\\"path\\":\\"%s/shot.png\\"}"}}\n' "$EQ_DIR" "$EQ_DIR"
	printf '{"timestamp":"2026-07-23T10:00:03.000Z","type":"message","payload":{"role":"assistant","text":"A colour gradient with a white diagonal.","timestamp":"10:00 AM"}}\n'
} >"$eq_day/rollout-2026-07-23T10-00-00-10710715.jsonl"

# eq_run NAME [terminal options…] [-- VAR=VALUE…] — one session under the
# played terminal, in an environment that names nothing: no inherited
# terminal identity at all (`env -i`), herdr's two variables, the suite's
# hermetic config. Its raw bytes land in NAME.raw, the verdict in NAME.report.
eq_run() {
	local name="$1"
	shift
	local args=() extra=()
	while [ $# -gt 0 ] && [ "$1" != "--" ]; do
		args+=("$1")
		shift
	done
	[ $# -gt 0 ] && shift
	extra=("$@")
	# shellcheck disable=SC2086 # CFG_ENV_NOHIST is a list of assignments
	(cd "$EQ_DIR" && env -i HOME="$HOME" PATH="$PATH" TERM=xterm-256color TERM_PROGRAM=herdr \
		$CFG_ENV_NOHIST ALTER_ZERO_IMAGE_CELL_SIZE=5x10 ${extra[@]+"${extra[@]}"} \
		python3 -I "$EQ_TERM" --log "$SMOKE_TMP/$name.raw" --report "$SMOKE_TMP/$name.report" \
		${args[@]+"${args[@]}"} -- "$BIN_ABS" --resume 10710715)
}
eq_run herdr --kitty ok --version libghostty &
eq_run silent &
eq_run wezterm --kitty ok --version "WezTerm 20240203-110809-5046fc22" &
eq_run tmux --kitty ok --version libghostty -- TMUX=/tmp/tmux-0/default,1,0 &
eq_run typed --kitty ok --version libghostty --typeahead "hello from before" &
eq_run mute --kitty ok --version libghostty --no-cursor --seconds 4 &
wait

eq_field() { sed -n "s/^$2=//p" "$SMOKE_TMP/$1.report" 2>/dev/null; }
eq_count() { grep -ao -- "$2" "$SMOKE_TMP/$1.raw" 2>/dev/null | wc -l; }
eq_tx() { eq_count "$1" "$(printf '\033')_G[^;]*a=T"; }
# Whole glyphs: under the C locale a bracket expression would match the bytes
# of every UTF-8 box-drawing character too.
eq_blocks() { eq_count "$1" '▀\|▄'; }
EQ_QUERY="$(printf '\033[22;0t\033_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\033\\\033[23;0t\033[>q\033[6n')"

# 1. herdr: asked, believed, drawn with kitty graphics.
expect_eq "$(eq_field herdr asked)" "kitty,version,cursor" "the herdr-like terminal was not asked in order"
expect_eq "$(grep -acF -- "$EQ_QUERY" "$SMOKE_TMP/herdr.raw")" "1" \
	"the startup query is not the titled kitty query, XTVERSION, then the cursor"
[ "$(eq_tx herdr)" -ge 1 ] || fail "herdr's OK did not make the picture kitty graphics (no ESC _ G … a=T)"
# 2. silence: half-blocks, as before — the picture's cells are what the
#    herdr run did NOT draw (both share the banner mascot's).
expect_eq "$(eq_field silent asked)" "kitty,version,cursor" "the silent terminal was not asked"
expect_eq "$(eq_tx silent)" "0" "a terminal that said nothing got kitty graphics"
[ "$(eq_blocks silent)" -gt "$(eq_blocks herdr)" ] ||
	fail "the silent terminal's picture is not half-blocks ($(eq_blocks silent) vs herdr's $(eq_blocks herdr))"
# 3. WezTerm's OK, with its own name on it, is not believed.
expect_eq "$(eq_tx wezterm)" "0" "WezTerm's OK was believed (it draws no unicode placeholders)"
# 4. Under tmux nothing is asked: the cursor query alone.
expect_eq "$(eq_field tmux asked)" "cursor" "a session under tmux asked about graphics"
expect_eq "$(eq_tx tmux)" "0" "a session under tmux drew kitty graphics"
# 5. Keys typed while the read waited reach the composer.
[ "$(eq_count typed 'hello from before')" -ge 1 ] ||
	fail "keys typed during the startup read never reached the composer"
[ "$(eq_tx typed)" -ge 1 ] || fail "type-ahead between the answers broke the kitty answer"
# 6. No cursor report: the old two-second failure, raw mode undone.
expect_eq "$(eq_field mute exit)" "1" "a terminal that never reports the cursor did not fail startup"
case "$(eq_field mute exited_after)" in
1.9 | 2.* | 3.0) ;;
*) fail "startup gave up after $(eq_field mute exited_after)s, not ~2s" ;;
esac
expect_eq "$(eq_field mute canonical)" "1" "a failed startup left the terminal out of canonical mode"
[ "$(eq_count mute 'could not be read')" -ge 1 ] ||
	fail "a failed startup did not say the cursor position could not be read"
