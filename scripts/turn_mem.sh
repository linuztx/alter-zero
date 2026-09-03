#!/usr/bin/env bash
# Measure what a conversation that carries a pasted picture costs the running
# TUI per turn (docs/memory.md, "Every turn re-sent the picture").
#
# `paste_mem.sh` measures the paste itself; this is its sibling for the turns
# after it. It drives the REAL binary in tmux under a virtual X server, with
# the `clipboard_owner` example serving a screenshot-shaped PNG, pastes it
# once, sends it, then sends N plain-text follow-ups while the picture stays
# in the context — every one of which re-sends it — and samples VmRSS / VmHWM
# plus the resident `[heap]` / anonymous-mapping totals from /proc after each
# step. A per-turn residue shows as a climbing RSS column.
#
#   Xvfb :99 -screen 0 1280x800x24 &
#   cargo build && cargo build --example clipboard_owner
#   scripts/turn_mem.sh [bin] [WxH] [followups] [protocol]
#
#   bin        target/debug/alter-zero
#   WxH        1920x1200 (the served picture)
#   followups  5
#   protocol   halfblocks | kitty | sixel | iterm2 (ALTER_ZERO_IMAGE_PROTOCOL)
#
# The dummy backend answers by default (it never encodes the picture, so it
# is the control). `LIVE=1` drives a real provider instead: set
# `ALTER_ZERO_PROVIDER`, `ALTER_ZERO_MODEL` (a vision-capable one) and the
# provider's key variable in the environment — they are passed through.
# `TURN_MEM_DISPLAY` picks the X display (default :99).
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${1:-$ROOT/target/debug/alter-zero}"
SIZE="${2:-1920x1200}"
N="${3:-5}"
PROTO="${4:-halfblocks}"
W="${SIZE%x*}"
H="${SIZE#*x}"
OWNER="$ROOT/target/debug/examples/clipboard_owner"
export DISPLAY="${TURN_MEM_DISPLAY:-:99}"

for need in "$BIN" "$OWNER"; do
	if [ ! -x "$need" ]; then
		echo "turn_mem: $need not found — cargo build && cargo build --example clipboard_owner" >&2
		exit 2
	fi
done
if ! command -v tmux >/dev/null; then
	echo "turn_mem: tmux is required" >&2
	exit 2
fi
if [ "${LIVE:-0}" = 1 ] && { [ -z "${ALTER_ZERO_PROVIDER:-}" ] || [ -z "${ALTER_ZERO_MODEL:-}" ]; }; then
	echo "turn_mem: LIVE=1 needs ALTER_ZERO_PROVIDER, ALTER_ZERO_MODEL and the provider's key in the environment" >&2
	exit 2
fi

CFG="$(mktemp -d)"
S="turn_mem_$$"
cleanup() {
	tmux kill-session -t "$S" 2>/dev/null
	[ -n "${OWNER_PID:-}" ] && kill "$OWNER_PID" 2>/dev/null
	rm -rf "$CFG"
}
trap cleanup EXIT

"$OWNER" "$W" "$H" >/dev/null 2>&1 &
OWNER_PID=$!
sleep 1.5
ENV="ALTER_ZERO_CONFIG_DIR=$CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_IMAGE_PROTOCOL=$PROTO ALTER_ZERO_IMAGE_CELL_SIZE=10x20 ALTER_ZERO_STARTUP_DELAY_MS=100 DISPLAY=$DISPLAY"
if [ "${LIVE:-0}" != 1 ]; then
	ENV="$ENV ALTER_ZERO_DUMMY=1"
fi
tmux new-session -d -s "$S" -x 140 -y 45 "env $ENV $BIN"
sleep 2
PANE_PID="$(tmux list-panes -t "$S" -F '#{pane_pid}')"
APP="$(pgrep -P "$PANE_PID" | head -1)"
APP="${APP:-$PANE_PID}"
rss() {
	awk '/VmRSS/{r=$2} /VmHWM/{h=$2} END{printf "RSS %6.1f MB  peak %6.1f MB", r/1024, h/1024}' /proc/"$APP"/status
}
maps() {
	# Resident bytes of the brk heap, of anonymous mappings (thread arenas and
	# large blocks), and of everything else (the binary, libraries, files).
	awk '/^[0-9a-f]+-[0-9a-f]+ /{name=$6; if(name=="")name="[anon]"} /^Rss:/{ if(name=="[heap]")h+=$2; else if(name=="[anon]")a+=$2; else o+=$2 } END{printf "heap %6.1f MB  anon %6.1f MB  other %6.1f MB", h/1024, a/1024, o/1024}' /proc/"$APP"/smaps
}
# The turn-end summary is `{verb} for {n}s[ · …]`, the verb picked per turn.
turns_done() {
	tmux capture-pane -t "$S" -p -S -2000 | grep -cE "^[A-Z][a-z]+( [a-z]+)? for [0-9]+s( ·.*)?$"
}
wait_turn() {
	local want="$1"
	for _ in $(seq 1 900); do
		if [ "$(turns_done)" -ge "$want" ]; then return 0; fi
		sleep 0.2
	done
	echo "!! timed out waiting for turn $want" >&2
	tmux capture-pane -t "$S" -p -S -60 >&2
	return 1
}
echo "bin=$BIN picture=${W}x${H} protocol=$PROTO live=${LIVE:-0} pid=$APP"
echo "startup:      $(rss)   $(maps)"
tmux send-keys -t "$S" C-v
for _ in $(seq 1 100); do
	if tmux capture-pane -t "$S" -p | grep -q "\[Image #1\]"; then break; fi
	sleep 0.1
done
sleep 0.5
echo "paste:        $(rss)   $(maps)   file $(du -b "$CFG"/image-cache/*/1.png 2>/dev/null | cut -f1) bytes"
tmux send-keys -t "$S" -l " Reply with the single word OK."
tmux send-keys -t "$S" Enter
wait_turn 1 || exit 1
sleep 1
echo "send image:   $(rss)   $(maps)"
for i in $(seq 1 "$N"); do
	tmux send-keys -t "$S" -l "Reply with the single word OK ($i)."
	tmux send-keys -t "$S" Enter
	wait_turn $((i + 1)) || exit 1
	sleep 1
	echo "followup $i:   $(rss)   $(maps)"
done
if tmux capture-pane -t "$S" -p -S -400 | grep -q "Failed to paste"; then
	echo "!! a paste FAILED:" >&2
	tmux capture-pane -t "$S" -p -S -400 | grep "Failed to paste" | head -2 >&2
	exit 1
fi
