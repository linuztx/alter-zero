#!/usr/bin/env bash
# Phase 153 — a settled thought holds the box still until the next cell lands, and leaves no blank rows

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# THE THOUGHT HAND-OFF (docs/thought-handoff.md). The backend settles a
# thinking phase at the round's first tool-call piece, but the call has a cell
# only once its batch is announced. Collapsing the `● Thinking…` block at once
# lifted the box by the block's rows for as long as the call took to generate,
# then dropped it back when the cell landed — the flicker reported between a
# quick think and a `skill` call. The block now settles IN PLACE: its header
# becomes the `Thought for …` line, the thought stays under it, and the
# batch's cells take those rows the frame they land.
#
# The first fix held the rows blank instead, and that is the second half of
# this phase: blank rows held between rounds — after a command's tall live
# tail settled into its peek — sat as three empty lines over the status line
# for as long as the next request took (the reported "it should be 1 new
# line"). Mid-turn the status line has exactly ONE blank row above it.
#
# Each turn is sampled ~20 times a second: the footer never moves up while
# the hand-off is under way, the settled line is seen with the thought still
# under it, and no sample shows two blank rows directly above the status line
# — each candidate confirmed against a second capture a beat later, Phase
# 121's rule (a torn frame lasts microseconds, a hop or a held gap as long as
# the call takes to generate). Four turns:
#   1. the default dummy turn — a paragraph, a two-line thought, then a
#      Read+Edit+Bash batch generated over about a second — on a terminal with
#      room below the box, its byte stream checked for purge rebuilds and its
#      thought's line and cells for landing exactly once;
#   2. the same turn on a short terminal, where the box sits on the bottom row
#      and a rise strands blank rows under it;
#   3–4. the REAL backend against a paced stub provider (ported from
#      fix/box-flicker-after-thinking's Phase 153): a call written over about
#      a second and a three-second round boundary; and a thought whose round
#      records only a task call, then a retried request.
S153="${S}_thoughthandoff"
S153S="${S}_thoughthandoffshort"
RAW153="$SMOKE_TMP/phase153.raw"
THOUGHT153="Let me read the file first."

# The blank rows directly above the status line in a capture, or nothing when
# no status line is on screen.
gap_above_status() {
	printf '%s\n' "$1" | awk '
		/esc to interrupt/ { print blank; exit }
		/^[[:space:]]*$/ { blank++; next }
		{ blank = 0 }'
}

# watch_turn SESSION FOOTER THOUGHT [SETTLED_CELL] — sample a turn until its
# status line goes; sets W_SAMPLES, W_HOPS (confirmed rises of the FOOTER
# row while the status line was up — none counted once a sample shows
# SETTLED_CELL, whose round's end may lift the box), W_GAPS (confirmed
# samples with more than one blank row above the status line) and W_HELD
# (a sample showed the settled line with THOUGHT still under it — only the
# strip ever shows the thought, which never commits).
watch_turn() {
	local sess="$1" footer="$2" thought="$3" settled="${4:-}" vis again now prev=0 seen=0 gap
	W_SAMPLES=0
	W_HOPS=0
	W_GAPS=0
	W_HELD=0
	for _ in $(seq 1 600); do # up to ~30s
		vis="$(tmux capture-pane -t "$sess" -p)"
		W_SAMPLES=$((W_SAMPLES + 1))
		if printf '%s' "$vis" | grep -qF "esc to interrupt"; then
			seen=1
		elif [ "$seen" -eq 1 ]; then
			# The turn is over: its end collapses the strip into the summary,
			# which is a settle rather than a hop.
			break
		fi
		if printf '%s' "$vis" | grep -qE '^Thought for [0-9]+s' &&
			printf '%s' "$vis" | grep -qF "$thought"; then
			W_HELD=1
		fi
		now=$(printf '%s\n' "$vis" | grep -nF "$footer" | tail -1 | cut -d: -f1)
		if [ "${now:-0}" -gt 0 ] && [ "$now" -lt "$prev" ]; then
			sleep 0.03
			again="$(tmux capture-pane -t "$sess" -p)"
			local now_again
			now_again=$(printf '%s\n' "$again" | grep -nF "$footer" | tail -1 | cut -d: -f1)
			if printf '%s' "$again" | grep -qF "esc to interrupt" &&
				{ [ -z "$settled" ] || ! printf '%s' "$again" | grep -qF "$settled"; } &&
				[ "${now_again:-0}" -gt 0 ] && [ "$now_again" -lt "$prev" ]; then
				W_HOPS=$((W_HOPS + 1))
				if [ "$W_HOPS" -le 2 ]; then
					dump "$sess: the box hopped up from row $prev to row $now_again (sample $W_SAMPLES)" "$again"
				fi
			else
				now="${now_again:-$now}"
			fi
		fi
		prev="${now:-$prev}"
		gap="$(gap_above_status "$vis")"
		if [ "${gap:-1}" -gt 1 ]; then
			sleep 0.03
			again="$(tmux capture-pane -t "$sess" -p)"
			gap="$(gap_above_status "$again")"
			if [ "${gap:-1}" -gt 1 ]; then
				W_GAPS=$((W_GAPS + 1))
				if [ "$W_GAPS" -le 2 ]; then
					dump "$sess: $gap blank rows above the status line (sample $W_SAMPLES)" "$again"
				fi
			fi
		fi
		sleep 0.05
	done
	[ "$seen" -eq 1 ] || fail "$sess: the turn never showed its status line"
	if [ "$W_SAMPLES" -lt 20 ]; then
		fail "$sess: only $W_SAMPLES samples of the turn: the watch saw too little to mean anything"
	fi
}

# expect_watch WHAT — the verdicts on the last watch_turn.
expect_watch() {
	if [ "$W_HELD" -ne 1 ]; then
		fail "$1: the settled thought never held its rows: no sample showed its line with the thought still under it"
	fi
	if [ "$W_HOPS" -ne 0 ]; then
		fail "$1: the box hopped up $W_HOPS time(s) mid-turn: the thought gave its rows up before the next cell could take them (see the frames above)"
	fi
	if [ "$W_GAPS" -ne 0 ]; then
		fail "$1: $W_GAPS sample(s) held more than one blank row above the status line: rows were held blank instead of collapsing to one spacer (see the frames above)"
	fi
}

# ---- 1. the dummy turn, with room below the box ------------------------------
launch "$S153" 80 24
tmux pipe-pane -t "$S153" -o "cat > $RAW153"
submit "$S153" "$USER_MSG"
watch_turn "$S153" 'dummy_model_name ·' "$THOUGHT153"
note "tall: sampled $W_SAMPLES times; box hops=$W_HOPS; held gaps=$W_GAPS; held thought seen=$W_HELD"
expect_watch "tall"
wait_summaries 20 "$S153" 1 >/dev/null || fail "the default turn never finished"
tmux pipe-pane -t "$S153" # close the recording before quitting

final="$(pane "$S153" -S -200)"
dump "captured pane + scrollback (the settled turn)" "$final"
# Liveness: the turn reached the hand-off, so a quiet watch is a real pass.
expect_eq "$(grep -cE '^Thought for [0-9]+s' <<<"$final")" "1" \
	"the thought did not settle into exactly one 'Thought for' line"
expect_lacks "$final" -F "$THOUGHT153" "the thought's own text reached the conversation: only its line may commit"
expect_eq "$(grep -cF '● Read(about.py)' <<<"$final")" "1" "the read's cell is not there exactly once"
expect_eq "$(grep -cF '● Edit(about.py)' <<<"$final")" "1" "the edit's cell is not there exactly once"
expect_eq "$(grep -cF '● Bash(python3 about.py)' <<<"$final")" "1" "the run's cell is not there exactly once"
purges=$(grep -c $'\x1b\\[3J' "$RAW153" 2>/dev/null || true)
expect_eq "${purges:-0}" "0" "the turn purge-rebuilt the screen"
keys "$S153" C-c
wait_gone 10 "$S153" || fail "the app did not quit on Ctrl+C"

# ---- 2. the dummy turn on a short terminal: the box on the bottom row --------
launch "$S153S" 80 16
submit "$S153S" "$USER_MSG"
watch_turn "$S153S" 'dummy_model_name ·' "$THOUGHT153"
note "short: sampled $W_SAMPLES times; box hops=$W_HOPS; held gaps=$W_GAPS; held thought seen=$W_HELD"
expect_watch "short"
short="$(wait_summaries 20 "$S153S" 1)" || fail "the short session's turn never finished"
# Settled, the box is flush at the bottom: nothing held outlived the turn.
footer_row="$(printf '%s\n' "$short" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)"
expect_eq "${footer_row:-0}" "16" "the settled box is not on the bottom row: a held row outlived the turn"
expect_eq "$(count_footers "$short")" "1" "the settled short session shows the footer more than once"
keys "$S153S" C-c
wait_gone 10 "$S153S" || fail "the short session did not quit on Ctrl+C"

# ---- the real backend against a paced stub provider ------------------------
# The dummy scripts its events; these two turns go through `LlmBackend` and
# `stream_round` the way a provider's stream does. The stub picks its script
# by the user's message:
#   "run the command" — a three-line thought, then a `bash` call whose
#     arguments stream over about a second (the hold's whole window), a
#     command whose tailing cell stands taller than its settled peek, and a
#     three-second wait before the second round (the round boundary the
#     reported blank rows sat in).
#   "plan the work" — a thought, then only a `taskcreate`, which records no
#     cell, and a next request that fails with a 500 once before it answers.
#     A held line used to be dropped by the retry as though the failed
#     attempt had produced it, so the thought's line never reached scrollback.
SR_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$SR_PORT" <<'PY' &
import json, sys, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])
failed = set()  # the scripts whose second round has failed once already

def said(messages):
    """Every user-role text of the request, joined."""
    out = []
    for message in messages:
        if message.get("role") != "user":
            continue
        content = message.get("content")
        if isinstance(content, str):
            out.append(content)
        elif isinstance(content, list):
            out.extend(part.get("text", "") for part in content if isinstance(part, dict))
    return "\n".join(out)

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

    def think(self, lines):
        for line in lines:
            self.emit({"reasoning_content": line}, 0.15)

    def call(self, name, args, pause):
        # The thought settles on the first piece; the call is announced only
        # once the last one is in.
        pieces = [args[i:i + 12] for i in range(0, len(args), 12)]
        self.emit({"tool_calls": [{"index": 0, "id": "call_153", "type": "function",
                                   "function": {"name": name, "arguments": pieces[0]}}]}, pause)
        for piece in pieces[1:]:
            self.emit({"tool_calls": [{"index": 0, "function": {"arguments": piece}}]}, pause)
        self.emit({}, finish="tool_calls")

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length") or 0)))
        messages = body.get("messages", [])
        script = "plan" if "plan the work" in said(messages) else "run"
        first_round = messages[-1].get("role") == "user"
        if script == "plan" and not first_round and script not in failed:
            failed.add(script)
            data = b'{"error":{"message":"stub overloaded","type":"server_error"}}'
            self.send_response(500)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return
        # HTTP/1.0 with no length: the stream ends when the connection does,
        # so each frame reaches the client as it is written.
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        if first_round and script == "run":
            self.think(["Let me run the command first.\n", "It prints thirty lines.\n",
                        "Then I can answer.\n"])
            command = "for i in $(seq 1 30); do echo line-$i; sleep 0.05; done"
            self.call("bash", json.dumps({"command": command, "description": "Print thirty lines"}), 0.12)
        elif first_round:
            self.think(["Plan the work before doing it.\n", "One task covers it.\n"])
            self.call("taskcreate", json.dumps({"subject": "Count the lines",
                                                "description": "Count them"}), 0.0)
        else:
            if script == "run":
                time.sleep(3)  # the next round's wait: what the reported screen showed
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
# Master mode: the command runs unasked, so nothing but the strip is on screen.
SR_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_PERMISSION_MODE=master ALTER_ZERO_PROVIDERS_FILE=$SR_PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"

# ---- 3. "run the command": the slow call, then the slow next round ----------
S153R="${S}_thoughthandoffreal"
launch -c "$(work_dir real)" "$S153R" 100 30 "$SR_APP"
submit "$S153R" "run the command"
# Only a rise before the call's cell is the hand-off's: the round's end may
# lift the box, its cell settling shorter than it ran. The blank-row rule
# holds throughout — the call being written, and the round boundary.
watch_turn "$S153R" 'stub-model ·' "Let me run the command first." "● Bash("
note "real backend: sampled $W_SAMPLES times; rises before the cell=$W_HOPS; held gaps=$W_GAPS; held thought seen=$W_HELD"
expect_watch "real backend"
real="$(wait_pane 20 "$S153R" -S -60 -- -F "Done.")" || fail "the stub's second round never answered"
dump "the settled real-backend session" "$real"
expect_eq "$(grep -cE '^Thought for [0-9]+s' <<<"$real")" "1" \
	"the stub's thought did not settle into exactly one 'Thought for' line"
expect_eq "$(grep -cF '● Bash(for i in' <<<"$real")" "1" "the stub's call is not there exactly once"
expect_lacks "$real" -F "Let me run the command first." "the stub's thought reached the conversation: only its line may commit"
keys "$S153R" C-c
wait_gone 10 "$S153R" || fail "the real-backend session did not quit on Ctrl+C"

# ---- 4. "plan the work": a thought, a task call with no cell, a retry -------
S153T="${S}_thoughthandofftask"
launch -c "$(work_dir task)" "$S153T" 100 30 "$SR_APP"
submit "$S153T" "plan the work"
wait_pane 25 "$S153T" -S -60 -- -F "Done." >/dev/null || fail "the retried second round never answered"
task="$(wait_settled 5 "$S153T" -S -60 -- -E "^$SUMMARY_RE")"
dump "the settled task-call session" "$task"
expect_has "$task" -F "Count the lines" "the task call never reached the checklist"
expect_eq "$(grep -cE '^Thought for [0-9]+s' <<<"$task")" "1" \
	"the thought before the task call lost its line: a retry dropped it as though the failed attempt had produced it"
keys "$S153T" C-c
wait_gone 10 "$S153T" || fail "the task-call session did not quit on Ctrl+C"
