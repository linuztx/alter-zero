#!/usr/bin/env bash
# Phase 135 — a command's spinner keeps its running cell's shape

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The reported flicker (docs/tool-streaming.md *A spinner keeps the cell's
# shape*), driven through the REAL backend, a real terminal session and a
# stub provider. `npm install`'s spinner draws every frame as three writes —
# `ESC[1G`, `ESC[0K`, then the glyph — and the running cell flipped between
# `⎿ Running… (Ns · wait …)` and the glyph about once a second, moving
# everything below it each time. The stand-in below draws exactly that,
# write for write, in Python so the suite needs no Node.
#
# 1. A `bash` launch (`wait: 2`) runs it until the call hands it back running.
# 2. A `bashwait` (`wait: 8`) waits on the same session to its exit. The
#    launch's report handed the model one glyph, and the spinner comes back
#    round to it every lap: the stream used to drop the line there.
#
# The pane is sampled as fast as tmux answers while each call runs; once a
# call's live cell has shown the spinner it must never fall back to
# `Running…`.
S135="${S}_spinner"
WORK="$(work_dir spinner)"
SPINNER="$SMOKE_TMP/spinner135.py"
cat >"$SPINNER" <<'PY'
import os, time

frames = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"
time.sleep(0.2)
index, end = 0, time.monotonic() + 7
while time.monotonic() < end:
    index = (index + 1) % len(frames)
    os.write(2, b"\x1b[1G")
    os.write(2, b"\x1b[0K")
    os.write(2, frames[index].encode())
    time.sleep(0.08)
os.write(2, b"\x1b[1G\x1b[0K")
os.write(1, b"added 3 packages in 7s\n")
PY
STUB_LOG="$SMOKE_TMP/stub135.log"
STUB_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$STUB_PORT" "$STUB_LOG" "$SPINNER" <<'PY' &
import json, re, shlex, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port, log, spinner = int(sys.argv[1]), sys.argv[2], sys.argv[3]

def sse(frames):
    body = "".join("data: %s\n\n" % json.dumps(f) for f in frames) + "data: [DONE]\n\n"
    return body.encode()

def text(message):
    content = message.get("content")
    if isinstance(content, list):
        return " ".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

def answer(words):
    return [{"choices": [{"delta": {"content": words}}]},
            {"choices": [{"delta": {}, "finish_reason": "stop"}]}]

def calling(name, args, n):
    call = {"index": 0, "id": "call_135_%d" % n, "type": "function",
            "function": {"name": name, "arguments": json.dumps(args)}}
    return [{"choices": [{"delta": {"tool_calls": [call]}}]},
            {"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}]

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send(b'{"object":"list","data":[{"id":"stub-model"}]}', "application/json")

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0)).decode()
        with open(log, "a") as f:
            f.write(body.replace("\n", " ") + "\n")
        messages = json.loads(body).get("messages", [])
        last, n = messages[-1], len(messages)
        said = text(last)
        calls = json.dumps([m.get("tool_calls") for m in messages])
        session = re.search(r"session (b[0-9a-z]{8})", said)
        if last.get("role") == "user" and "smoke135 spin" in said:
            frames = calling("bash", {"command": "python3 -u %s" % shlex.quote(spinner),
                                      "description": "Install packages", "wait": 2}, n)
        elif last.get("role") == "tool" and session and "bashwait" not in calls:
            frames = calling("bashwait", {"session_id": session.group(1), "wait": 8}, n)
        else:
            frames = answer("Done.")
        self.send(sse(frames), "text/event-stream")

    def send(self, data, kind):
        self.send_response(200)
        self.send_header("content-type", kind)
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
STUB_PID=$!
smoke_on_exit kill "$STUB_PID"
PROVIDERS="$SMOKE_TMP/providers135.toml"
cat >"$PROVIDERS" <<EOF
[providers.stub]
name = "Stub"

[providers.stub.kwargs]
api_base = "http://127.0.0.1:$STUB_PORT/v1"
EOF
# No permission gate: nobody is there to answer one. NO_PROXY keeps a
# developer's proxy off the loopback.
REAL_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_PERMISSIONS=0 ALTER_ZERO_PROVIDERS_FILE=$PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"
launch -c "$WORK" "$S135" 120 40 "$REAL_APP"

# The live cell of the call whose clock row reads CLOCK: from the last row
# naming HEADER down to that clock row — never a committed cell above it,
# which may hold a glyph of its own.
live_cell() {
	awk -v head="$2" -v clock="$3" '
		index($0, head) { block = ""; on = 1 }
		on { block = block $0 "\n" }
		on && index($0, clock) { printf "%s", block; exit }
	' <<<"$1"
}

GLYPH='⎿  (⠋|⠙|⠹|⠸|⠼|⠴|⠦|⠧|⠇|⠏)[[:space:]]*$'
RUNNING='⎿  Running… \('
submit "$S135" "smoke135 spin"
declare -A spun=() relapses=() frames=()
relapse=""
deadline=$((SECONDS + 30))
while [ "$SECONDS" -lt "$deadline" ]; do
	frame="$(pane "$S135")"
	for call in "Bash(:· wait 2s)" "BashWait(:· wait 8s)"; do
		cell="$(live_cell "$frame" "${call%%:*}" "${call#*:}")"
		[ -n "$cell" ] || continue
		frames[$call]=$((${frames[$call]:-0} + 1))
		if has "$cell" -E "$GLYPH"; then
			spun[$call]=1
		elif [ -n "${spun[$call]:-}" ] && has "$cell" -E "$RUNNING"; then
			relapses[$call]=$((${relapses[$call]:-0} + 1))
			relapse="$cell"
		fi
	done
	has "$frame" -F "Done." && break
done
settled="$(wait_settled 20 "$S135" -S -200 -- -F "Done.")" || fail "the turn never settled"
dump "the settled turn" "$settled"
for call in "Bash(:· wait 2s)" "BashWait(:· wait 8s)"; do
	name="${call%%(*}"
	note "$name: ${frames[$call]:-0} live frames sampled, ${relapses[$call]:-0} back on Running…"
	[ "${frames[$call]:-0}" -ge 10 ] || fail "too few frames of the $name cell were sampled to tell (${frames[$call]:-0})"
	[ -n "${spun[$call]:-}" ] || fail "the $name cell never showed the spinner"
	[ "${relapses[$call]:-0}" -eq 0 ] ||
		fail "the $name cell fell back to Running… ${relapses[$call]} times after showing the spinner"
done
[ -z "$relapse" ] || dump "a relapse" "$relapse"
# The wait saw the command out: its last words reached the model.
expect_file_has "$STUB_LOG" -F "added 3 packages in 7s" "the bashwait never reported the command's exit"
tmux kill-session -t "$S135" 2>/dev/null
