#!/usr/bin/env bash
# Phase 138b — a long file line shows whole in the write and read cells, never cut at three rows

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# docs/long-lines.md §7. The numbered `read`/`write`/`edit` cells used to cut
# any line longer than three rows and end the third with a `…` — a markdown
# paragraph, a long HTML tag, a string literal, cut mid-word while the file
# streamed into its cell and again once it was written. A file cell now shows
# a line to its end while the peek's ten rows hold it (the one line that needs
# more is cut on the tenth, docs/long-lines.md §8 and Phase 138c); a line
# taller than a streaming write's ten-row window scrolls through it instead.
#
# The REAL backend, against a stub provider whose model writes `notes.md` — a
# title, a blank line, and one 467-character paragraph line ending in the
# word END138B, which lands at the start of the paragraph's sixth row at 100
# columns — a few characters at a time, then reads it back. Proved:
#   - while the content streams, the cell shows more of the paragraph than
#     the old three-row cut ever did, and no row of its body ends in `…`;
#   - once it has run, the committed `Write` cell and the `Read` cell after
#     it each show the paragraph's last word, and no file row is cut.

PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$PORT" <<'PY' &
import json, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer

port = int(sys.argv[1])
# 460 columns of filler is exactly five rows of the cell's 92-column content
# at 100 columns, so the marker opens the sixth row whole.
paragraph = ("lorem ipsum dolor sit amet " * 30)[:460] + "END138B"
write_args = json.dumps({"path": "notes.md", "content": "# Notes\n\n" + paragraph + "\n"})
read_args = json.dumps({"path": "notes.md"})

def frame(payload):
    return ("data: %s\n\n" % json.dumps(payload)).encode()

def call(index, call_id, name, arguments):
    return {"index": index, "id": call_id, "type": "function",
            "function": {"name": name, "arguments": arguments}}

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
        results = sum(1 for m in messages if m.get("role") == "tool")
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("connection", "close")
        self.end_headers()
        if results == 0:
            self.wfile.write(frame({"choices": [{"delta": {"content": "Writing the notes now.\n\n"}}]}))
            self.wfile.write(frame({"choices": [{"delta": {"tool_calls": [call(0, "call_138b_w", "write", "")]}}]}))
            for at in range(0, len(write_args), 6):
                piece = {"index": 0, "function": {"arguments": write_args[at:at + 6]}}
                self.wfile.write(frame({"choices": [{"delta": {"tool_calls": [piece]}}]}))
                self.wfile.flush()
                time.sleep(0.02)
            self.wfile.write(frame({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}))
        elif results == 1:
            self.wfile.write(frame({"choices": [{"delta": {"tool_calls": [call(0, "call_138b_r", "read", read_args)]}}]}))
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
STUB=$!
smoke_on_exit kill "$STUB"
PROVIDERS="$SMOKE_TMP/providers138b.toml"
printf '[providers.stub]\nname = "Stub"\n\n[providers.stub.kwargs]\napi_base = "http://127.0.0.1:%s/v1"\n' "$PORT" >"$PROVIDERS"
WORK="$(work_dir longline)"
# No permission gate: the write and the read run unasked.
LL_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_PERMISSIONS=0 ALTER_ZERO_PROVIDERS_FILE=$PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"
S138B="${S}_writelongline"
launch -c "$WORK" "$S138B" 100 40 "$LL_APP"
submit "$S138B" "smoke138b write the notes and read them back"

# Sample the newest Write cell until the turn settles: its corner row, how
# many of its body rows carry the paragraph, and whether any body row ends in
# the old cut's `…`.
SAMPLES="$SMOKE_TMP/samples138b.tsv"
python3 - "$SMOKE_TMUX_SOCKET" "$S138B" "$SAMPLES" <<'PY'
import subprocess, sys, time

sock, target, out = sys.argv[1:]
end = time.time() + 40
with open(out, "w", encoding="utf-8") as f:
    while time.time() < end:
        rows = subprocess.run(["tmux", "-S", sock, "-f", "/dev/null", "capture-pane", "-t", target, "-p"],
                              capture_output=True, text=True).stdout.split("\n")
        if any("Done." in row for row in rows):
            break
        heads = [i for i, row in enumerate(rows) if "Write(notes.md)" in row]
        if heads:
            cell = rows[heads[-1] + 1:]
            corner = cell[0].strip() if cell else ""
            body = []
            for row in cell[1:]:
                if not row.strip():
                    break
                body.append(row.rstrip())
            paragraph = sum(1 for row in body if "lorem" in row or "END138B" in row)
            cut = int(any(row.endswith("…") for row in body))
            f.write(f"{corner}\t{paragraph}\t{cut}\n")
        time.sleep(0.02)
PY
settled="$(wait_settled 30 "$S138B" -S -80 -- -F "Done.")" || fail "the turn never settled"
dump "the settled turn" "$settled"
expect_has "$settled" -F "Wrote 3 lines to notes.md" "the streamed write ran and committed"
expect_has "$settled" -F "Read(notes.md)" "the read ran and committed"
expect_has "$settled" -F "Read 3 lines" "the read cell shows its summary"
expect_eq "$(grep -c -F 'END138B' <<<"$settled")" "2" \
	"the paragraph's last word shows in both the write cell and the read cell"
expect_lacks "$settled" -E '^ {6,}[^ ].*…$' "a file row is still cut short with a …"

# The cell as it streamed: past the old three-row cut, never marked as cut.
ll_streaming() { awk -F'\t' '$1 ~ /^⎿  Writing/ && $2 > 3' "$SAMPLES" | wc -l | tr -d ' '; }
ll_cut() { awk -F'\t' '$3 == 1' "$SAMPLES" | wc -l | tr -d ' '; }
note "$(wc -l <"$SAMPLES" | tr -d ' ') samples, $(ll_streaming) streaming past three paragraph rows, $(ll_cut) with a cut row"
[ "$(ll_streaming)" -ge 1 ] || fail "the streaming cell never showed more than three rows of the paragraph"
expect_eq "$(ll_cut)" "0" "a streaming cell ended a row of the file with the old cut's …"

smoke_finish
