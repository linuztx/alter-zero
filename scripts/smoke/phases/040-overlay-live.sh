#!/usr/bin/env bash
# Phase 40 — the Ctrl+O tool-output overlay shows a running bash tool's output LIVE

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the Ctrl+O tool-output overlay shows a running bash tool's output
# LIVE (docs/tool-streaming.md) — unlike Claude Code, whose transcript only shows
# tool output once the tool finishes. Open the overlay during a fresh parallel
# turn and poll for the fix-demonstrating state: a Bash(ping) cell RUNNING with
# streamed output (an `icmp_seq` line) while BOTH batch siblings still show
# `⎿ Waiting…`. When two calls are still Waiting, the first is the only active one
# and nothing has committed, so an `icmp_seq` line can ONLY come from the running
# call's live stream — impossible if the overlay were static (it would show a
# frozen `⎿ Running…`). The overlay tail-follows, so the frontier stays in view.
S_OVL="${S}_ovl"
launch "$S_OVL" 100 44
submit "$S_OVL" "run three pings in parallel"
sleep 0.7 # let the reply text start, then open the overlay before the tools stream
tmux send-keys -t "$S_OVL" C-o
overlay_stream=""
for _ in $(seq 1 250); do # ~15s cap — catches the running call's live-output window
	cap="$(tmux capture-pane -t "$S_OVL" -p)"
	waiting=$(printf '%s\n' "$cap" | grep -c 'Waiting…')
	if [ "$waiting" -ge 2 ] && printf '%s' "$cap" | grep -qE 'icmp_seq'; then
		overlay_stream="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 40: captured pane (Ctrl+O overlay — a running Bash(ping) cell streaming live over ⎿ Waiting… siblings) ===="
printf '%s\n' "$overlay_stream"
tmux kill-session -t "$S_OVL" 2>/dev/null

# Phase 40: the Ctrl+O overlay shows a running bash tool's output LIVE — a running
# Bash(ping) cell streamed an `icmp_seq` line while both siblings were still
# `⎿ Waiting…` (only the live-updating overlay can show that; docs/tool-streaming.md).
overlay_waiting=$(printf '%s\n' "$overlay_stream" | grep -c 'Waiting…')
if [ "${overlay_waiting:-0}" -lt 2 ] || ! printf '%s' "$overlay_stream" | grep -qE 'icmp_seq'; then
	fail "the Ctrl+O overlay did not show a running bash tool's live output (no streamed 'icmp_seq' line while two siblings were still ⎿ Waiting…) — the overlay stayed static during the stream"
fi
