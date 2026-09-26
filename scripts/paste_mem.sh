#!/usr/bin/env bash
# Measure what Ctrl+V image pastes cost the running TUI in resident memory
# (docs/memory.md, "Pasting a screenshot"; docs/image-paste.md, "Measuring").
#
# Drives the REAL binary in tmux under a virtual X server, with the
# `clipboard_owner` example serving a screenshot-shaped PNG on the clipboard
# the way a screenshot tool does (image/png, INCR segments), and samples
# VmRSS / VmHWM from /proc after each paste — and, with `send`, after each
# turn that sends the picture to the (dummy) model.
#
#   Xvfb :99 -screen 0 1280x800x24 &
#   cargo build && cargo build --example clipboard_owner
#   scripts/paste_mem.sh [bin] [WxH] [pastes] [protocol] [send]
#
#   bin       target/debug/alter-zero
#   WxH       1920x1080 (the served picture)
#   pastes    3
#   protocol  halfblocks | kitty | sixel | iterm2 (ALTER_ZERO_IMAGE_PROTOCOL)
#   send      0 | 1 — also send each pasted picture in a turn
#
# `PASTE_MEM_DISPLAY` picks the X display (default :99).
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${1:-$ROOT/target/debug/alter-zero}"
SIZE="${2:-1920x1080}"
N="${3:-3}"
PROTO="${4:-halfblocks}"
SEND="${5:-0}"
W="${SIZE%x*}"
H="${SIZE#*x}"
OWNER="$ROOT/target/debug/examples/clipboard_owner"
export DISPLAY="${PASTE_MEM_DISPLAY:-:99}"

for need in "$BIN" "$OWNER"; do
	if [ ! -x "$need" ]; then
		echo "paste_mem: $need not found — cargo build && cargo build --example clipboard_owner" >&2
		exit 2
	fi
done
if ! command -v tmux >/dev/null; then
	echo "paste_mem: tmux is required" >&2
	exit 2
fi

CFG="$(mktemp -d)"
S="paste_mem_$$"
cleanup() {
	tmux kill-session -t "$S" 2>/dev/null
	[ -n "${OWNER_PID:-}" ] && kill "$OWNER_PID" 2>/dev/null
	rm -rf "$CFG"
}
trap cleanup EXIT

"$OWNER" "$W" "$H" >/dev/null 2>&1 &
OWNER_PID=$!
sleep 1.5
ENV="ALTER_ZERO_CONFIG_DIR=$CFG ALTER_ZERO_DUMMY=1 ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_IMAGE_PROTOCOL=$PROTO ALTER_ZERO_IMAGE_CELL_SIZE=10x20 ALTER_ZERO_STARTUP_DELAY_MS=100 DISPLAY=$DISPLAY"
tmux new-session -d -s "$S" -x 140 -y 45 "env $ENV $BIN"
sleep 1.5
PANE_PID="$(tmux list-panes -t "$S" -F '#{pane_pid}')"
APP="$(pgrep -P "$PANE_PID" | head -1)"
APP="${APP:-$PANE_PID}"
rss() {
	awk '/VmRSS/{r=$2} /VmHWM/{h=$2} END{printf "RSS %6.1f MB  peak %6.1f MB", r/1024, h/1024}' /proc/"$APP"/status
}
echo "bin=$BIN picture=${W}x${H} protocol=$PROTO pid=$APP"
echo "startup:   $(rss)"
for i in $(seq 1 "$N"); do
	tmux send-keys -t "$S" C-v
	for _ in $(seq 1 100); do
		if tmux capture-pane -t "$S" -p | grep -q "\[Image #$i\]"; then break; fi
		sleep 0.1
	done
	sleep 0.5
	echo "paste $i:   $(rss)"
	if [ "$SEND" = 1 ]; then
		tmux send-keys -t "$S" -l " describe"
		tmux send-keys -t "$S" Enter
		for _ in $(seq 1 100); do
			# Turn i's summary, whatever its verb (docs/status-indicator.md).
			if [ "$(tmux capture-pane -t "$S" -p -S -400 | grep -cE '^[A-Z][a-z]+ed for [0-9]')" -ge "$i" ]; then break; fi
			sleep 0.1
		done
		sleep 0.5
		echo "  sent $i: $(rss)"
	fi
done
if tmux capture-pane -t "$S" -p -S -200 | grep -q "Failed to paste"; then
	echo "!! a paste FAILED:" >&2
	tmux capture-pane -t "$S" -p -S -200 | grep "Failed to paste" | head -2 >&2
	exit 1
fi
