#!/usr/bin/env bash
# Phase 158 — a labelled link is its label where the terminal opens hyperlinks, `label (url)` elsewhere

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# LABELLED LINKS (docs/links.md *Labelled links*). A reply's `[label](url)`
# used to print `label (url)` everywhere — `Source: FIDE (https://www.fide.com/
# geneva-to-host-…/)` — an address nobody reads, spending a row. Where the
# terminal is known to open an OSC 8 hyperlink the label IS the link now:
# dressed as one, the whole address behind it, the address off the screen.
# Anywhere else the address stays, since hidden there it would simply be
# gone.
#
# This pane is tmux's, so by the app's own rule it is a terminal that keeps
# the address — a multiplexer forwards a hyperlink only to a terminal its
# configuration names. That makes three cases drivable here (the terminal
# the app works out for itself is played on a pty further down, 4 to 7):
#   1. ALTER_ZERO_LINK_TARGETS=hide — the override for a terminal the app
#      cannot name: the tour's `[link](https://github.com/linuztx/alter-zero)`
#      shows `link` alone, and the RAW byte stream (pipe-pane, read before
#      tmux interprets it) opens a hyperlink to the whole address around
#      exactly that word;
#   2. nothing set — the address is spelled out, as it always was;
#   3. hide WITH hyperlinks off — nothing is written for the label to stand
#      in front of, so the address is spelled out and no escape is emitted.
# The bare URL beside it (https://ratatui.rs/) is on screen in all three: it
# has no label to hide behind.
LINK_URL="https://github.com/linuztx/alter-zero"
LINK_PROMPT="stream some markdown to me"
# The tour streams in token-sized pieces at the dummy's chunk pace; a few
# milliseconds keeps this phase about the settled rows, not the stream.
LINK_APP="env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS ALTER_ZERO_CHUNK_DELAY_MS=2"

# run_tour SESSION RAWFILE EXTRA_ENV — play the tour in a 100x60 pane and
# print the settled screen with its scrollback.
run_tour() {
	local sess="$1" raw="$2" extra="$3"
	launch "$sess" 100 60 "$LINK_APP $extra $BIN"
	tmux pipe-pane -t "$sess" -o "cat >> $raw"
	submit "$sess" "$LINK_PROMPT"
	wait_for 20 "$sess" -F "esc to interrupt" || fail "the tour never started in $sess"
	wait_settled 60 "$sess" -S -300 -- -F "$SETTLED_REPLY"
}

# 1. The override: the label alone, the address behind it.
S158="${S}_linklabel"
RAW_HIDE="$SMOKE_TMP/hide.raw"
hide_pane="$(run_tour "$S158" "$RAW_HIDE" "ALTER_ZERO_LINK_TARGETS=hide")"
dump "label only (the link row)" "$(printf '%s\n' "$hide_pane" | grep -F -B1 -A2 "Prose with")"
expect_has "$hide_pane" -F "a link and a bare URL, https://ratatui.rs/" "the label is not followed straight by the prose after the link"
expect_lacks "$hide_pane" -F "$LINK_URL" "the hidden address is on screen"
expect_lacks "$hide_pane" -F "link (" "the label is still followed by a parenthesis"
# The address the screen no longer shows rides the hyperlink: an OSC 8 open
# carrying ALL of it, then nothing but cursor and style sequences before the
# label's own letters.
if ! grep -aqE $'\x1b]8;id=az[0-9]+;https://github\\.com/linuztx/alter-zero\x1b\\\\(\x1b\\[[0-9;:]*[A-Za-z])*link' "$RAW_HIDE"; then
	fail "no hyperlink to the whole address opens around the label in the raw stream"
fi
if ! grep -aqF $'\x1b]8;;\x1b' "$RAW_HIDE"; then
	fail "the hyperlink around the label is never closed"
fi
# The carrier that got the address there never reaches the terminal as the
# underline colour it is stored as (SGR 58 — matched whole, to its `m`: on a
# pane this tall `ESC[58;1H` is a cursor move to row 58).
if grep -aqE $'\x1b\\[58[;:][0-9;:]*m' "$RAW_HIDE"; then
	fail "the link-id carrier leaked as an SGR 58 underline colour"
fi
# The label wears the link dress: underlined on screen. A styled capture
# writes a cell's attributes as an SGR of their own — single-digit codes,
# `4` the underline — ahead of its colour (and, from tmux 3.4, its
# hyperlink), so the label must follow one that holds a 4.
hide_styled="$(pane "$S158" -e -S -300 | grep -F "Prose with")"
if ! printf '%s\n' "$hide_styled" | grep -qE $'\x1b\\[([0-9];)*4(;[0-9])*m(\x1b\\[[0-9;:]*m|\x1b]8;[^\x1b]*\x1b\\\\)*link'; then
	fail "the label is not underlined: $(printf '%s' "$hide_styled" | cat -v)"
fi
tmux kill-session -t "$S158" 2>/dev/null

# 2. A terminal the app cannot vouch for keeps the address on screen.
S158B="${S}_linkshown"
RAW_SHOWN="$SMOKE_TMP/shown.raw"
shown_pane="$(run_tour "$S158B" "$RAW_SHOWN" "")"
dump "address shown (the link row)" "$(printf '%s\n' "$shown_pane" | grep -F -B1 -A2 "Prose with")"
expect_has "$shown_pane" -F "($LINK_URL)" "the address is not spelled out after the label"
if ! grep -aqE $']8;id=az[0-9]+;https://github\\.com/linuztx/alter-zero\x1b' "$RAW_SHOWN"; then
	fail "the spelled-out address is not a hyperlink"
fi
tmux kill-session -t "$S158B" 2>/dev/null

# 3. With hyperlinks off nothing may hide behind one.
S158C="${S}_linkgate"
RAW_GATE="$SMOKE_TMP/gate.raw"
gate_pane="$(run_tour "$S158C" "$RAW_GATE" "ALTER_ZERO_LINK_TARGETS=hide ALTER_ZERO_HYPERLINKS=0")"
dump "hide + hyperlinks off (the link row)" "$(printf '%s\n' "$gate_pane" | grep -F -B1 -A2 "Prose with")"
expect_has "$gate_pane" -F "($LINK_URL)" "the address was hidden with no hyperlink to reach it by"
if grep -aqF "]8;" "$RAW_GATE"; then
	fail "ALTER_ZERO_HYPERLINKS=0 still emitted OSC 8"
fi
tmux kill-session -t "$S158C" 2>/dev/null

# 4–7. The terminal the app works out for ITSELF. tmux can only ever be the
# multiplexer, so — Phase 107e's way — the terminal is played on a pty: its
# master end answers the startup read the way the terminal under test would
# and records every byte the app writes. Each run resumes a rollout whose
# reply holds the reported link, in an environment that names nothing but
# what the leg adds:
#   4. a terminal that ANSWERS with its name (XTVERSION) — what a WezTerm
#      reads like over ssh, where its variables stay behind and `TERM` is
#      plain: the label alone, the address behind it;
#   5. a terminal that says nothing: the address spelled out;
#   6. a terminal the ENVIRONMENT names (`TERM=xterm-kitty`), which is not
#      asked at all: the label alone;
#   7. an editor's own terminal, whatever answers: the address spelled out.
LT_URL="https://www.fide.com/geneva-to-host-fide-world-championship-match-2026/"
LT_TERM="$SMOKE_TMP/terminal.py"
cat >"$LT_TERM" <<'PY'
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
ap.add_argument("--version")
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
out, pos, asked = b"", 0, []


def answer():
    """Answer each complete question past `pos`, in the order it was asked."""
    global pos
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
            asked.append("kitty")  # no graphics here: this one goes unanswered
            continue
        pos = at + len(pat)
        if pat == b"\x1b[>q":
            asked.append("version")
            if a.version:
                os.write(master, b"\x1bP>|" + a.version.encode() + b"\x1b\\")
        else:
            asked.append("cursor")
            os.write(master, b"\x1b[1;1R")


with open(a.log, "wb") as log:
    while time.time() - start < a.seconds and proc.poll() is None:
        ready, _, _ = select.select([master], [], [], 0.05)
        if ready:
            data = os.read(master, 65536)
            out += data
            log.write(data)
            answer()
if proc.poll() is None:
    proc.terminate()
    proc.wait()
with open(a.report, "w") as report:
    report.write("asked=%s\n" % ",".join(asked))
PY

LT_DIR="$(mktemp -d "$SMOKE_TMP/lt.XXXXXX")"
lt_day="$SMOKE_SESSIONS/2026/07/23"
mkdir -p "$lt_day"
{
	printf '{"timestamp":"2026-07-23T10:00:00.000Z","type":"session_meta","payload":{"id":"smoke-158","timestamp":"2026-07-23T10:00:00.000Z","cwd":"%s","model":"dummy_model_name","originator":"alter-zero","version":"0.1.0"}}\n' "$LT_DIR"
	printf '{"timestamp":"2026-07-23T10:00:01.000Z","type":"message","payload":{"role":"user","text":"where is the match","timestamp":"10:00 AM"}}\n'
	printf '{"timestamp":"2026-07-23T10:00:02.000Z","type":"message","payload":{"role":"assistant","text":"Source: [FIDE](%s)","timestamp":"10:00 AM"}}\n' "$LT_URL"
} >"$lt_day/rollout-2026-07-23T10-00-00-15815815.jsonl"

# lt_run NAME [terminal options…] [-- VAR=VALUE…] — one session under the
# played terminal, in an environment that names nothing: no inherited
# terminal identity at all (`env -i`), a plain `TERM`, the suite's hermetic
# config. Its raw bytes land in NAME.raw, what it was asked in NAME.report.
lt_run() {
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
	(cd "$LT_DIR" && env -i HOME="$HOME" PATH="$PATH" TERM=xterm-256color \
		$CFG_ENV_NOHIST ${extra[@]+"${extra[@]}"} \
		python3 -I "$LT_TERM" --log "$SMOKE_TMP/$name.raw" --report "$SMOKE_TMP/$name.report" \
		${args[@]+"${args[@]}"} -- "$BIN_ABS" --resume 15815815)
}
lt_run named --version "WezTerm 20240203-110809-5046fc22" &
lt_run silent &
lt_run envkitty -- TERM=xterm-kitty &
lt_run editor --version "kitty(0.36.4)" -- NVIM=/run/user/1000/nvim.4242.0 &
wait

lt_asked() { sed -n "s/^asked=//p" "$SMOKE_TMP/$1.report" 2>/dev/null; }
# What the played terminal would SHOW: the raw stream with its hyperlink
# escapes and its cursor and style sequences taken out.
lt_text() {
	sed -e $'s/\x1b]8;[^\x1b]*\x1b\\\\//g' -e $'s/\x1b\\[[0-9;:?<=>]*[A-Za-z]//g' "$SMOKE_TMP/$1.raw"
}
# The label standing alone: the address nowhere in what is shown, and a
# hyperlink to all of it opening right before the label's letters.
lt_expect_label() {
	local text
	text="$(lt_text "$1")"
	expect_has "$text" -F "Source: FIDE" "$1: the label is not on screen"
	expect_lacks "$text" -F "www.fide.com" "$1: the address is still spelled out"
	if ! grep -aqE $'\x1b]8;id=az[0-9]+;https://www\\.fide\\.com/geneva-to-host-fide-world-championship-match-2026/\x1b\\\\(\x1b\\[[0-9;:]*[A-Za-z])*FIDE' "$SMOKE_TMP/$1.raw"; then
		fail "$1: no hyperlink to the whole address opens around the label"
	fi
}
lt_expect_address() {
	expect_has "$(lt_text "$1")" -F "Source: FIDE ($LT_URL)" "$1: the address is not spelled out after the label"
}

# 4. The terminal named itself in its answer: asked, believed.
expect_eq "$(lt_asked named)" "kitty,version,cursor" "the unnamed terminal was not asked who it is"
lt_expect_label named
# 5. Asked, and no wiser: the address stays.
expect_eq "$(lt_asked silent)" "kitty,version,cursor" "the silent terminal was not asked who it is"
lt_expect_address silent
# 6. The environment already names kitty: nothing is asked, the label stands.
expect_eq "$(lt_asked envkitty)" "cursor" "a terminal the environment names was asked anyway"
lt_expect_label envkitty
# 7. An editor's terminal keeps the address whatever answered.
lt_expect_address editor
