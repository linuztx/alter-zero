#!/usr/bin/env bash
# Phase 153 — a thought settling ahead of the call it led to never lifts the box

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The STRIP HOLD (docs/strip-hold.md). A reasoning model's round is a thought,
# then the call it decided on — and the call is only announced once the model
# has finished GENERATING it, its arguments streaming meanwhile. The thought
# settles the moment the first argument piece arrives: its four-row live
# block collapses into the one-line `Thought for …` cell, and for the whole
# time the call is still being written there was nothing left in the strip
# to show. The region is content-anchored, so the box rose by the rows the
# block gave back — and dropped again when the call's cell went up. A fast
# model makes that a hop for a frame or two (the reported flicker: a
# thinking model loading a skill); the dummy's default turn generates its
# three calls for ~0.8s, so here it is a hop that holds still long enough to
# sample. The thought's rows are now held, blank, until the call's cell lands
# on them — and nothing else is held: a call settling at the end of its round
# leaves the status line one spacer under it while the next request is out,
# where an earlier, broader hold left three blank rows there (the second
# report).
#
# Watched by sampling the footer's row through the whole turn (Phase 121's
# hop check): the region is content-anchored, so commits push the footer
# down until it reaches the bottom, where it stays, and nothing may pull it
# back up while the status line is on screen. A drop seen once is confirmed
# against a second capture a beat later — a capture can read a frame
# half-parsed, a real hop holds until the next cell. On a tall terminal,
# where the region has room to rise into, and on a short one, where it sits
# at the bottom and a rise strands blank rows under the box. Then once more
# through the REAL backend against a paced stub provider: a three-line
# thought, a `bash` call written over about a second, a command whose
# tailing cell is taller than the peek it settles into, and a three-second
# wait before the second round.
S153="${S}_striphold"
S153S="${S}_stripholdshort"
S153R="${S}_stripholdreal"

# watch_turn SESSION — sample the footer row until the turn's summary
# commits; sets HOPS (confirmed rises while a status line was up) and
# SAMPLES, noting the first two hops' screens.
watch_turn() {
	local sess="$1" vis again prev=0 now n
	HOPS=0
	SAMPLES=0
	for ((n = 0; n < 600; n++)); do # up to ~40s
		vis="$(tmux capture-pane -t "$sess" -p)"
		SAMPLES=$((SAMPLES + 1))
		now="$(printf '%s\n' "$vis" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)"
		now="${now:-0}"
		if [ "$now" -gt 0 ] && [ "$prev" -gt 0 ] && [ "$now" -lt "$prev" ] &&
			printf '%s' "$vis" | grep -qF "esc to interrupt"; then
			sleep 0.03
			again="$(tmux capture-pane -t "$sess" -p)"
			now="$(printf '%s\n' "$again" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)"
			now="${now:-0}"
			if [ "$now" -gt 0 ] && [ "$now" -lt "$prev" ] &&
				printf '%s' "$again" | grep -qF "esc to interrupt"; then
				HOPS=$((HOPS + 1))
				if [ "$HOPS" -le 2 ]; then
					note "$sess: the box rose from row $prev to row $now mid-turn (sample $SAMPLES):"
					printf '%s\n' "$again"
				fi
			fi
		fi
		[ "$now" -gt 0 ] && prev="$now"
		if printf '%s' "$vis" | grep -qE "^$SUMMARY_RE"; then
			return 0
		fi
		sleep 0.05
	done
	fail "$sess: the turn did not finish within the sampling budget"
}

# ---- 1. a tall terminal: the region has room to rise into -------------------
launch "$S153" 100 40
submit "$S153" "$USER_MSG"
watch_turn "$S153"
tall_hops="$HOPS"
tall_samples="$SAMPLES"
tall="$(pane "$S153" -S -60)"
dump "the settled tall session" "$tall"
note "tall: sampled $tall_samples times, $tall_hops hop(s)"
# The turn really passed through the shape the hold is for: a settled thought,
# then the batch it led to.
expect_has "$tall" -E "^Thought for [0-9]+s" "the turn's thought never settled into its line"
for call in "● Read(about.py)" "● Edit(about.py)" "● Bash(python3 about.py)"; do
	expect_has "$tall" -F "$call" "the batch's $call cell never committed"
done
expect_eq "$tall_hops" "0" "the box rose mid-turn on the tall terminal: the settled thought gave back rows it never committed"

# ---- 2. a short terminal: the region sits on the bottom row -----------------
launch "$S153S" 80 16
submit "$S153S" "$USER_MSG"
watch_turn "$S153S"
short_hops="$HOPS"
short="$(pane "$S153S")"
dump "the settled short session" "$short"
note "short: sampled $SAMPLES times, $short_hops hop(s)"
expect_eq "$short_hops" "0" "the box rose off the bottom row mid-turn on the short terminal"
# Settled, the box is flush at the bottom: the held rows went with the turn.
footer_row="$(printf '%s\n' "$short" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)"
expect_eq "${footer_row:-0}" "16" "the settled box is not on the bottom row: held rows outlived the turn"
expect_eq "$(count_footers "$short")" "1" "the settled short session shows the footer more than once"

# ---- 3. the real backend: a slow call, then a slow next round -------------
SR_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$SR_PORT" <<'PY' &
import json, sys, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        data = b'{"object":"list","data":[{"id":"stub-model"}]}'
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def emit(self, delta, pause=0.0, finish=None):
        choice = {"delta": delta}
        if finish:
            choice["finish_reason"] = finish
        self.wfile.write(("data: %s\n\n" % json.dumps({"choices": [choice]})).encode())
        self.wfile.flush()
        time.sleep(pause)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length") or 0)))
        last = body.get("messages", [{}])[-1]
        # HTTP/1.0 with no length: the stream ends when the connection does,
        # so each frame reaches the client as it is written.
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        if last.get("role") == "user":
            for line in ["Let me run the command first.\n", "It prints thirty lines.\n",
                         "Then I can answer.\n"]:
                self.emit({"reasoning_content": line}, 0.15)
            command = "for i in $(seq 1 30); do echo line-$i; sleep 0.05; done"
            args = json.dumps({"command": command, "description": "Print thirty lines"})
            pieces = [args[i:i + 12] for i in range(0, len(args), 12)]
            # The thought settles on the first piece; the call is announced
            # only once the last one is in, about a second later.
            self.emit({"tool_calls": [{"index": 0, "id": "call_153", "type": "function",
                                       "function": {"name": "bash", "arguments": pieces[0]}}]}, 0.12)
            for piece in pieces[1:]:
                self.emit({"tool_calls": [{"index": 0, "function": {"arguments": piece}}]}, 0.12)
            self.emit({}, finish="tool_calls")
        else:
            # The next round's wait — what the reported screen was showing.
            time.sleep(3)
            self.emit({"content": "Done."})
            self.emit({}, finish="stop")
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

    def log_message(self, *_args):
        pass

ThreadingHTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
SR_STUB=$!
smoke_on_exit kill "$SR_STUB"
for _ in $(seq 1 30); do # wait for the stub to listen
	if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $SR_PORT)) == 0 else 1)" 2>/dev/null; then
		break
	fi
	sleep 0.1
done
SR_PROVIDERS="$SMOKE_TMP/providers153.toml"
printf '[providers.stub]\nname = "Stub"\n\n[providers.stub.kwargs]\napi_base = "http://127.0.0.1:%s/v1"\n' "$SR_PORT" >"$SR_PROVIDERS"
SR_WORK="$(work_dir real)"
# Master mode: the call runs unasked, so nothing but the strip is on screen.
SR_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_PERMISSION_MODE=master ALTER_ZERO_PROVIDERS_FILE=$SR_PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"
launch -c "$SR_WORK" "$S153R" 100 30 "$SR_APP"
submit "$S153R" "run the command"
# One watch over the whole turn, from its first frame: the footer's row for
# a rise BEFORE the call's cell went up — the thought's hand-off; the round's
# end may lift the box, since its cell settles shorter than it ran and
# nothing is held there — and, in the stub's three-second wait, the round
# boundary: the command's cell settled into its peek, the second round not
# answered yet. The newest such screen is the one measured, by then well
# inside the wait.
SR_BOUNDARY=""
SR_EARLY=0
SR_HOPS=0
sr_prev=0
for ((n = 0; n < 400; n++)); do # up to ~30s
	sr_vis="$(tmux capture-pane -t "$S153R" -p)"
	sr_now="$(printf '%s\n' "$sr_vis" | grep -nF 'stub-model ·' | tail -1 | cut -d: -f1)"
	sr_now="${sr_now:-0}"
	if [ "$sr_now" -gt 0 ] && [ "$sr_prev" -gt 0 ] && [ "$sr_now" -lt "$sr_prev" ] &&
		printf '%s' "$sr_vis" | grep -qF "esc to interrupt"; then
		sleep 0.03 # a torn capture lasts microseconds, a real rise until the next cell
		sr_again="$(tmux capture-pane -t "$S153R" -p)"
		sr_now="$(printf '%s\n' "$sr_again" | grep -nF 'stub-model ·' | tail -1 | cut -d: -f1)"
		sr_now="${sr_now:-0}"
		if [ "$sr_now" -gt 0 ] && [ "$sr_now" -lt "$sr_prev" ] &&
			printf '%s' "$sr_again" | grep -qF "esc to interrupt"; then
			SR_HOPS=$((SR_HOPS + 1))
			if ! printf '%s' "$sr_again" | grep -qF "● Bash("; then
				SR_EARLY=$((SR_EARLY + 1))
				note "the box rose from row $sr_prev to row $sr_now before the call's cell:"
				printf '%s\n' "$sr_again"
			fi
		fi
	fi
	[ "$sr_now" -gt 0 ] && sr_prev="$sr_now"
	if printf '%s' "$sr_vis" | grep -qE '… \+[0-9]+ lines \(ctrl\+o to expand\)' &&
		printf '%s' "$sr_vis" | grep -qF "esc to interrupt" &&
		! printf '%s' "$sr_vis" | grep -qF "Done."; then
		SR_BOUNDARY="$sr_vis"
	fi
	if printf '%s' "$sr_vis" | grep -qE "^$SUMMARY_RE"; then
		break
	fi
	sleep 0.05
done
real="$(pane "$S153R" -S -60)"
dump "the round boundary, the next request out" "$SR_BOUNDARY"
dump "the settled real-backend session" "$real"
note "real backend: $SR_HOPS hop(s), $SR_EARLY before the call's cell"
expect_has "$real" -E "^Thought for [0-9]+s" "the stub's thought never settled into its line"
expect_has "$real" -F "● Bash(for i in" "the stub's call never got its cell"
expect_has "$real" -F "Done." "the second round never answered"
expect_eq "$SR_EARLY" "0" "the box rose while the call was still being written: the settled thought gave back rows it never committed"
# Exactly one blank row — the cell's spacer — between the peek's hint and the
# status line while the next request is out.
sr_gap="$(printf '%s\n' "$SR_BOUNDARY" | awk '
	/… \+[0-9]+ lines \(ctrl\+o to expand\)/ { hint = NR; blank = 0; next }
	hint && /esc to interrupt/ { print blank; exit }
	hint && /^[[:space:]]*$/ { blank++ }')"
expect_eq "${sr_gap:-none}" "1" "the status line is not one spacer under the settled call: rows were held across the round's end"
