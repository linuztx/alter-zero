#!/usr/bin/env bash
# Phase 138 — a write streams into its cell: the newest ten rows while it arrives, the first ten once it has

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# docs/write-streaming.md. A `write` call's content is the longest thing a
# model generates in a turn, and it used to reach the screen only once the
# whole round had finished. The cell goes up the moment the call is named
# now, and the file streams into it:
#   - while the content arrives the cell shows its NEWEST rows, under a
#     `Writing N lines…` corner counting them — never the file's first line
#     once more than ten have arrived;
#   - once it has all arrived it shows the FIRST ten over the
#     `… +N lines (ctrl+o to expand)` hint, the committed cell's own peek, so
#     the cell does not change shape again when the call resolves.
# Driven twice: through the offline dummy (its file-change demo), and through
# the REAL backend against a stub provider that streams a 30-line write a
# few characters at a time — the Chat Completions slots, the backend's
# pieces and the boundary's flush, end to end.

# --- the offline demo ---
S138="${S}_writestream"
launch "$S138" 100 40 "env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS ALTER_ZERO_CHUNK_DELAY_MS=40 $BIN"
submit "$S138" "write fizzbuzz for me"
tail_screen="$(wait_pane 30 "$S138" -E 'Writing 1[2-5] lines…')" ||
	fail "the demo's write never streamed past ten lines"
dump "the demo's write, mid-stream" "$tail_screen"
expect_has "$tail_screen" -F "Write(fizzbuzz.py)" "the streaming cell names its file"
expect_lacks "$tail_screen" -F '1 #!/usr/bin/env python3' \
	"past ten lines the window shows the newest rows, not the file's first"
settled="$(wait_settled 40 "$S138" -S -80 -- -E "$SUMMARY_RE")" || fail "the demo turn never settled"
expect_has "$settled" -F "Wrote 16 lines to fizzbuzz.py" "the write committed"
expect_has "$settled" -F '1 #!/usr/bin/env python3' "the committed cell shows the file's head"
expect_lacks "$settled" -F "Writing " "no streaming corner survives the commit"

# --- the real backend, against a stub provider ---
WS_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$WS_PORT" <<'PY' &
import json, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer

port = int(sys.argv[1])
content = "".join("row %02d = %d\n" % (i, i * i) for i in range(1, 31))
arguments = json.dumps({"path": "squares.py", "content": content})

def frame(payload):
    return ("data: %s\n\n" % json.dumps(payload)).encode()

class Stub(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

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
        self.send_header("connection", "close")
        self.end_headers()
        if messages[-1].get("role") == "user":
            self.wfile.write(frame({"choices": [{"delta": {"content": "Writing the table now.\n\n"}}]}))
            # The call named first, then its arguments a few characters at a
            # time, slowly enough that the window visibly moves.
            call = {"index": 0, "id": "call_138", "type": "function",
                    "function": {"name": "write", "arguments": ""}}
            self.wfile.write(frame({"choices": [{"delta": {"tool_calls": [call]}}]}))
            for at in range(0, len(arguments), 6):
                piece = {"index": 0, "function": {"arguments": arguments[at:at + 6]}}
                self.wfile.write(frame({"choices": [{"delta": {"tool_calls": [piece]}}]}))
                self.wfile.flush()
                time.sleep(0.03)
            self.wfile.write(frame({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}))
        else:
            self.wfile.write(frame({"choices": [{"delta": {"content": "Done."}}]}))
            self.wfile.write(frame({"choices": [{"delta": {}, "finish_reason": "stop"}]}))
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
WORK="$(work_dir writestream)"
# No permission gate: the write runs unasked once the round ends.
WS_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_PERMISSIONS=0 ALTER_ZERO_PROVIDERS_FILE=$WS_PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"
S138L="${S}_writestreamlive"
launch -c "$WORK" "$S138L" 100 40 "$WS_APP"
submit "$S138L" "smoke138 write the squares"

# Sample the newest Write cell until the turn settles: its corner row and the
# first and last line numbers its body shows.
WS_SAMPLES="$SMOKE_TMP/samples138.tsv"
python3 - "$SMOKE_TMUX_SOCKET" "$S138L" "$WS_SAMPLES" <<'PY'
import re, subprocess, sys, time

sock, target, out = sys.argv[1:]
numbered = re.compile(r"^\s+(\d+) row ")
end = time.time() + 40
with open(out, "w", encoding="utf-8") as f:
    while time.time() < end:
        rows = subprocess.run(["tmux", "-S", sock, "-f", "/dev/null", "capture-pane", "-t", target, "-p"],
                              capture_output=True, text=True).stdout.split("\n")
        if any("Done." in row for row in rows):
            break
        heads = [i for i, row in enumerate(rows) if "Write(squares.py)" in row]
        if heads:
            cell = rows[heads[-1] + 1:]
            corner = cell[0].strip() if cell else ""
            numbers = [int(m.group(1)) for row in cell[1:] for m in [numbered.match(row)] if m]
            if numbers:
                f.write(f"{corner}\t{numbers[0]}\t{numbers[-1]}\n")
        time.sleep(0.02)
PY
live="$(wait_settled 30 "$S138L" -S -80 -- -F "Done.")" || fail "the live turn never settled"
dump "the live turn" "$live"
expect_has "$live" -F "Wrote 30 lines to squares.py" "the streamed write ran and committed"
expect_has "$live" -F "+20 lines (ctrl+o to expand)" "the committed cell shows its head over the hint"
expect_has "$live" -F "Writing the table now." "the text before the call kept its place"

# The window, as it moved: while lines streamed past ten the newest row was
# always at the bottom and line 1 out of view; once the content closed the
# cell showed lines 1 to 10.
ws_streaming() { awk -F'\t' '$1 ~ /^⎿  Writing [0-9]+ lines…$/ && $3 > 10' "$WS_SAMPLES" | wc -l | tr -d ' '; }
ws_tail_from_one() { awk -F'\t' '$1 ~ /^⎿  Writing/ && $3 > 10 && $2 == 1' "$WS_SAMPLES" | wc -l | tr -d ' '; }
ws_ends() { awk -F'\t' '$1 ~ /^⎿  Writing/ && $3 > 10 { print $3 }' "$WS_SAMPLES" | sort -un | wc -l | tr -d ' '; }
ws_head() { awk -F'\t' '$2 == 1 && $3 == 10' "$WS_SAMPLES" | wc -l | tr -d ' '; }
note "$(ws_streaming) samples past ten lines, $(ws_ends) distinct bottom rows, $(ws_head) head samples"
[ "$(ws_streaming)" -ge 5 ] || fail "the live write never streamed past ten lines on screen"
[ "$(ws_ends)" -ge 5 ] || fail "the window's bottom row never moved down the file"
expect_eq "$(ws_tail_from_one)" "0" "a window past ten lines still showed line 1"
[ "$(ws_head)" -ge 1 ] || fail "the finished cell never showed lines 1 to 10"

smoke_finish
