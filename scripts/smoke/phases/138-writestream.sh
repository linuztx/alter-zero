#!/usr/bin/env bash
# Phase 138 — a write streams its file while the model generates it

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# A `write` call's cell streams its file into the strip as the call's
# arguments decode (docs/write-streaming.md): while the `content` string is
# open the cell shows its TAIL — the newest ten rows, the line being typed
# last — and once the string closes, its HEAD: the first ten rows over
# `… +N lines`, the shape the committed `Wrote N lines` cell takes. Before
# this the call was invisible until it ran.
#
# Sampled every few milliseconds by a recorder (the Phase 135 pattern), then
# judged over the whole recording: a tail frame, a head frame before the call
# ran, and the committed cell after. Twice — the offline demo, and the real
# backend against a stub provider that streams a 40-line write and holds the
# round open after the file closes, so the head is on screen for a while.

# record SESSION OUT STOP — capture the pane every 15 ms into OUT (one JSON
# array of rows per line) until STOP shows or 40 s pass.
record() {
	python3 - "$SMOKE_TMUX_SOCKET" "$1" "$2" "$3" <<'PY'
import json, subprocess, sys, time

sock, target, out, stop = sys.argv[1:]
end = time.time() + 40
with open(out, "w", encoding="utf-8") as f:
    while time.time() < end:
        rows = subprocess.run(["tmux", "-S", sock, "-f", "/dev/null", "capture-pane", "-t", target, "-p"],
                              capture_output=True, text=True).stdout.split("\n")
        f.write(json.dumps(rows) + "\n")
        if any(stop in row for row in rows):
            break
        time.sleep(0.015)
PY
}

# judge FRAMES TOTAL FIRST LAST — how many recorded frames show the live cell
# in each shape, as `tail head`. FIRST and LAST are the file's first line and
# a line from its end:
#   tail — the corner still counting (`Writing N lines…`, not yet `Wrote`),
#          with a late line on screen and the first one scrolled out;
#   head — the corner at TOTAL lines, not yet `Wrote`, the first line back
#          on screen over the `… +N lines` hint.
judge() {
	python3 - "$@" <<'PY'
import json, re, sys

path, total, first, last = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4]
tail = head = 0
for frame in open(path, encoding="utf-8"):
    rows = json.loads(frame)
    text = "\n".join(rows)
    if "Wrote " in text:
        continue
    corner = re.search(r"Writing (\d+) lines?…", text)
    if not corner:
        continue
    n = int(corner.group(1))
    if n < total and last in text and first not in text:
        tail += 1
    if n == total and first in text and re.search(r"… \+\d+ lines", text):
        head += 1
print(tail, head)
PY
}

# --- A: the offline demo's write --------------------------------------------
S138="${S}_writestream"
WORK="$(work_dir writes)"
launch -c "$WORK" "$S138" 100 40 "env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS ALTER_ZERO_CHUNK_DELAY_MS=120 $BIN_ABS"
submit "$S138" "write a fizzbuzz"
DEMO_FRAMES="$SMOKE_TMP/frames138-demo.jsonl"
record "$S138" "$DEMO_FRAMES" "Wrote 16 lines to fizzbuzz.py"
read -r demo_tail demo_head <<<"$(judge "$DEMO_FRAMES" 16 '#!/usr/bin/env python3' 'print(i)')"
note "demo: $demo_tail tail frames, $demo_head head frames"
[ "$demo_tail" -ge 1 ] || fail "the demo write never showed its streaming tail"
[ "$demo_head" -ge 1 ] || fail "the demo write never showed its head before it ran"
demo_done="$(wait_settled 30 "$S138" -S -80 -- -E "$SUMMARY_RE")" || fail "the demo turn never settled"
dump "the demo turn" "$demo_done"
expect_has "$demo_done" -F "Wrote 16 lines to fizzbuzz.py" "the demo write never committed its cell"
expect_lacks "$demo_done" -F "Writing " "a live draft row leaked into scrollback"
tmux kill-session -t "$S138" 2>/dev/null

# --- B: the real backend, streamed by a stub provider -----------------------
S138B="${S}_writestreamlive"
LIVE="$(work_dir livewrite)"
TARGET="$LIVE/streamed.txt"
WS_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$WS_PORT" "$TARGET" <<'PY' &
import json, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer

port, target = int(sys.argv[1]), sys.argv[2]
content = "".join(f"row {i:02d} of the streamed file\n" for i in range(1, 41))

def frame(f):
    return ("data: %s\n\n" % json.dumps(f)).encode()

def piece(arguments, name=None):
    function = {"arguments": arguments}
    if name:
        function["name"] = name
    call = {"index": 0, "function": function}
    if name:
        call.update({"id": "call_138", "type": "function"})
    return frame({"choices": [{"delta": {"tool_calls": [call]}}]})

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        body = b'{"object":"list","data":[{"id":"stub-model"}]}'
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0)).decode()
        messages = json.loads(body).get("messages", [])
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        if messages[-1].get("role") != "user":
            self.wfile.write(frame({"choices": [{"delta": {"content": "Done."}}]}))
            self.wfile.write(frame({"choices": [{"delta": {}, "finish_reason": "stop"}]}))
            self.wfile.write(b"data: [DONE]\n\n")
            return
        # The call opens with its name, then its arguments arrive a few
        # characters at a time — escapes split wherever the cut falls.
        self.wfile.write(piece("", name="write"))
        self.wfile.flush()
        arguments = json.dumps({"path": target, "content": content})
        head, closing = arguments[:-1], arguments[-1:]
        for at in range(0, len(head), 9):
            self.wfile.write(piece(head[at:at + 9]))
            self.wfile.flush()
            time.sleep(0.03)
        # The file has closed but the round has not: the head shows.
        time.sleep(1.5)
        self.wfile.write(piece(closing))
        self.wfile.write(frame({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}))
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
WS_STUB=$!
smoke_on_exit kill "$WS_STUB"
WS_PROVIDERS="$SMOKE_TMP/providers138.toml"
printf '[providers.stub]\nname = "Stub"\n\n[providers.stub.kwargs]\napi_base = "http://127.0.0.1:%s/v1"\n' "$WS_PORT" >"$WS_PROVIDERS"
# No permission gate: the write runs the moment its round ends.
WS_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_PERMISSIONS=0 ALTER_ZERO_PROVIDERS_FILE=$WS_PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"
launch -c "$LIVE" "$S138B" 100 40 "$WS_APP"
submit "$S138B" "smoke138 write the file"
LIVE_FRAMES="$SMOKE_TMP/frames138-live.jsonl"
record "$S138B" "$LIVE_FRAMES" "Done."
read -r live_tail live_head <<<"$(judge "$LIVE_FRAMES" 40 'row 01 of' 'row 3')"
note "live: $live_tail tail frames, $live_head head frames"
[ "$live_tail" -ge 3 ] || fail "the streamed write never showed its tail"
[ "$live_head" -ge 3 ] || fail "the streamed write never showed its head while the round was open"
live_done="$(wait_settled 30 "$S138B" -S -80 -- -F "Done.")" || fail "the live turn never settled"
dump "the live turn" "$live_done"
expect_has "$live_done" -F "Wrote 40 lines to streamed.txt" "the live write never committed its cell"
expect_has "$live_done" -F "row 01 of the streamed file" "the committed cell does not open on the file's first line"
expect_has "$live_done" -F "… +30 lines (ctrl+o to expand)" "the committed cell does not fold its tail"
expect_lacks "$live_done" -F "Writing " "a live draft row leaked into scrollback"
expect_eq "$(wc -l <"$TARGET" 2>/dev/null | tr -d ' ')" "40" "the streamed file is not on disk whole"
tmux kill-session -t "$S138B" 2>/dev/null
