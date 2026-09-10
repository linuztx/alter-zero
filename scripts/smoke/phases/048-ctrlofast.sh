#!/usr/bin/env bash
# Phase 48 — Ctrl+O on a resumed CODE-HEAVY session opens WARM and ATOMIC

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Ctrl+O on a resumed CODE-HEAVY session opens WARM and ATOMIC
# (docs/tool-view-performance.md). A handcrafted rollout carries three Write
# tools of ~1000 numbered HTML lines each (~110 KB — the grammar-highlight
# heavy shape that made the old open re-render everything on a blank alt
# screen for hundreds of ms, kitty's cursor-trail streaking up it). After
# /resume, the loop-bottom warm pre-renders the transcript, and enter_overlay
# only QUEUES the switch so the first overlay frame lands in the same flush:
# Ctrl+O must show the transcript promptly, and NO capture taken while it
# opens may ever be a blank screen (the old flushed-blank window). The return
# must land back on the intact composer.
S48="${S}_ctrlofast"
CTRLO_DIR="$(mktemp -d "$SMOKE_TMP/ctrlo.XXXXXX")"
ctrlo_day="$CTRLO_DIR/2026/07/23"
mkdir -p "$ctrlo_day"
ctrlo_file="$ctrlo_day/rollout-2026-07-23T10-00-00-48484848.jsonl"
{
	printf '{"timestamp":"2026-07-23T10:00:00.000Z","type":"session_meta","payload":{"id":"smoke-48","timestamp":"2026-07-23T10:00:00.000Z","cwd":"%s","model":"dummy_model_name","originator":"alter-zero","version":"0.1.0"}}\n' "$PWD"
	printf '{"timestamp":"2026-07-23T10:00:01.000Z","type":"message","payload":{"role":"user","text":"resumed ctrlo html app","timestamp":"10:00 AM"}}\n'
	for t in 1 2 3; do
		body="Created /tmp/buddies${t}.html (1000 lines)"
		n=1
		while [ "$n" -le 1000 ]; do
			# Right-aligned 4-wide gutter — the numbered-file-cell shape the
			# overlay syntax-highlights by the .html extension. No quotes or
			# backslashes in the content, so the line is JSON-safe verbatim.
			body="$body\\n$(printf '%4d' "$n") <div class=b${n}>buddy ${n} of file ${t}</div>"
			n=$((n + 1))
		done
		printf '{"timestamp":"2026-07-23T10:00:02.000Z","type":"tool","payload":{"name":"Write","args":"/tmp/buddies%s.html","ok":true,"output":"%s","timestamp":"10:00 AM","shell":false,"truncated":false}}\n' "$t" "$body"
	done
	printf '{"timestamp":"2026-07-23T10:00:03.000Z","type":"message","payload":{"role":"assistant","text":"All three buddy files saved and animated.","timestamp":"10:00 AM"}}\n'
} >"$ctrlo_file"
launch "$S48" 100 30 "env $CFG_ENV_NOHIST ALTER_ZERO_SESSIONS_DIR=$CTRLO_DIR ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux send-keys -t "$S48" -l "/resume"
sleep 0.3
tmux send-keys -t "$S48" Enter # open the picker
ctrlo_listed=0
for _ in $(seq 1 40); do # the seeded session's preview row (same cwd → Cwd filter keeps it)
	if tmux capture-pane -t "$S48" -p | grep -qF "resumed ctrlo html app"; then
		ctrlo_listed=1
		break
	fi
	sleep 0.1
done
tmux send-keys -t "$S48" Enter # load it
ctrlo_loaded=0
for _ in $(seq 1 60); do # the loaded conversation repaints inline (collapsed Write cell)
	if tmux capture-pane -t "$S48" -p -S -60 | grep -qF "buddies3.html"; then
		ctrlo_loaded=1
		break
	fi
	sleep 0.1
done
sleep 1.0 # let the loop-bottom transcript warm finish after the load
ctrlo_t0="$(date +%s.%N)"
tmux send-keys -t "$S48" C-o
ctrlo_blank=0
ctrlo_open_ms=""
for _ in $(seq 1 200); do # sample tightly: no capture may be a blank screen
	ctrlo_pane="$(tmux capture-pane -t "$S48" -p)"
	if [ "$(printf '%s' "$ctrlo_pane" | grep -cve '^[[:space:]]*$')" -eq 0 ]; then
		ctrlo_blank=1
	fi
	if printf '%s' "$ctrlo_pane" | grep -q "T R A N S C R I P T"; then
		ctrlo_open_ms="$(awk "BEGIN{printf \"%.0f\", ($(date +%s.%N) - $ctrlo_t0)*1000}")"
		break
	fi
	sleep 0.01
done
# The overlay opens tail-following: the resumed conversation's final reply is
# in view — proof the transcript content itself rendered, not just the chrome.
ctrlo_tail=0
if tmux capture-pane -t "$S48" -p | grep -qF "All three buddy files saved"; then
	ctrlo_tail=1
fi
tmux send-keys -t "$S48" C-o # return to the inline view
ctrlo_back=0
for _ in $(seq 1 40); do
	if tmux capture-pane -t "$S48" -p | grep -qF "dummy_model_name"; then
		ctrlo_back=1
		break
	fi
	sleep 0.1
done
echo "==== Phase 48: resumed code-heavy Ctrl+O — listed=$ctrlo_listed loaded=$ctrlo_loaded open_ms=${ctrlo_open_ms:-none} blank_capture=$ctrlo_blank tail=$ctrlo_tail back=$ctrlo_back ===="
tmux capture-pane -t "$S48" -p | grep -v '^$' | tail -4
tmux kill-session -t "$S48" 2>/dev/null
if [ "$ctrlo_listed" != 1 ] || [ "$ctrlo_loaded" != 1 ]; then
	fail "precondition — the seeded code-heavy session did not list/load (listed=$ctrlo_listed loaded=$ctrlo_loaded)"
fi
if [ -z "$ctrlo_open_ms" ]; then
	fail "Ctrl+O never showed the transcript overlay"
elif [ "$ctrlo_open_ms" -gt 2000 ]; then
	fail "Ctrl+O took ${ctrlo_open_ms}ms on the resumed session (warm open must not rebuild the transcript)"
fi
if [ "$ctrlo_blank" != 0 ]; then
	fail "a capture during the Ctrl+O switch was a BLANK screen (the switch must land with the frame in one flush)"
fi
if [ "$ctrlo_tail" != 1 ]; then
	fail "the overlay did not show the resumed conversation's tail content"
fi
if [ "$ctrlo_back" != 1 ]; then
	fail "the return from Ctrl+O did not restore the inline view"
fi
