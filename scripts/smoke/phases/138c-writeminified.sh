#!/usr/bin/env bash
# Phase 138c — a line longer than a file cell's ten rows is cut on the last of them

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# docs/long-lines.md §8. A file cell's peek shows a line whole while it fits
# its ten rows, and used to finish any line it started: an agent's write of a
# stylesheet whose fourth line was the whole sheet minified painted every row
# of that line inline, a few hundred rows between `Wrote 10 lines` and the
# hint. The ten rows are a ceiling now — the line they run out inside is cut
# on the last of them with a `…`, and the hint counts it.
#
# The REAL backend, against a stub provider whose model writes `styles.css`
# and reads it back. At 100 columns the cell's content column is 91 wide, so
# lines 1 and 3 (150 characters) are two rows each and line 2 one: the other
# five of the ten rows are left for line 4, twenty kilobytes ending in
# END138C.
# Proved:
#   - the committed `Write` cell and the `Read` cell are each exactly ten
#     body rows over `… +7 lines (ctrl+o to expand)`, the tenth ending in
#     `…`, and nothing of line 4's end or of lines 5 to 10 shows inline;
#   - the cell never grew past its ten rows and the hint while it streamed;
#   - Ctrl+O still shows line 4 to its last word.

PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$PORT" <<'PY' &
import json, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer

port = int(sys.argv[1])
minified = "*{box-sizing:border-box}" + ".c{color:#272932;margin:0 auto}" * 640 + "/*END138C*/"
lines = [
    "@import url('" + "f" * 134 + "');",
    "",
    ":root{" + "r" * 143 + "}",
    minified,
] + [".rule-%d{}" % i for i in range(5, 11)]
write_args = json.dumps({"path": "styles.css", "content": "\n".join(lines) + "\n"})
read_args = json.dumps({"path": "styles.css"})

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
            self.wfile.write(frame({"choices": [{"delta": {"content": "Writing the stylesheet now.\n\n"}}]}))
            self.wfile.write(frame({"choices": [{"delta": {"tool_calls": [call(0, "call_138c_w", "write", "")]}}]}))
            for at in range(0, len(write_args), 256):
                piece = {"index": 0, "function": {"arguments": write_args[at:at + 256]}}
                self.wfile.write(frame({"choices": [{"delta": {"tool_calls": [piece]}}]}))
                self.wfile.flush()
                time.sleep(0.01)
            self.wfile.write(frame({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}))
        elif results == 1:
            self.wfile.write(frame({"choices": [{"delta": {"tool_calls": [call(0, "call_138c_r", "read", read_args)]}}]}))
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
PROVIDERS="$SMOKE_TMP/providers138c.toml"
printf '[providers.stub]\nname = "Stub"\n\n[providers.stub.kwargs]\napi_base = "http://127.0.0.1:%s/v1"\n' "$PORT" >"$PROVIDERS"
WORK="$(work_dir minified)"
# No permission gate: the write and the read run unasked.
MIN_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_PERMISSIONS=0 ALTER_ZERO_PROVIDERS_FILE=$PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"
S138C="${S}_writeminified"
launch -c "$WORK" "$S138C" 100 40 "$MIN_APP"
submit "$S138C" "smoke138c write the stylesheet and read it back"

# Sample the newest Write cell until the turn settles: how many rows it has
# under its corner row, the hint included, while it streams, while it waits to
# run, and once it has committed.
SAMPLES="$SMOKE_TMP/samples138c.txt"
python3 - "$SMOKE_TMUX_SOCKET" "$S138C" "$SAMPLES" <<'PY'
import subprocess, sys, time

sock, target, out = sys.argv[1:]
end = time.time() + 40
with open(out, "w", encoding="utf-8") as f:
    while time.time() < end:
        rows = subprocess.run(["tmux", "-S", sock, "-f", "/dev/null", "capture-pane", "-t", target, "-p"],
                              capture_output=True, text=True).stdout.split("\n")
        if any("Done." in row for row in rows):
            break
        heads = [i for i, row in enumerate(rows) if "Write(styles.css)" in row]
        if heads:
            body = 0
            for row in rows[heads[-1] + 2:]:
                if not row.strip():
                    break
                body += 1
            f.write(f"{body}\n")
        time.sleep(0.02)
PY
settled="$(wait_settled 30 "$S138C" -S -80 -- -F "Done.")" || fail "the turn never settled"
dump "the settled turn" "$settled"
expect_has "$settled" -F "Wrote 10 lines to styles.css" "the streamed write ran and committed"
expect_has "$settled" -F "Read 10 lines" "the read ran and committed"

# Each cell: the rows from its corner row to its hint, exactly ten between.
mc_body_rows() { # SUMMARY → the rows between that corner row and the next hint
	awk -v head="$1" '
		index($0, head) { inside = 1; n = 0; next }
		inside && /ctrl\+o to expand/ { print n; exit }
		inside { n++ }
	' <<<"$settled"
}
expect_eq "$(mc_body_rows "Wrote 10 lines to styles.css")" "10" "the Write cell's peek is not ten rows"
expect_eq "$(mc_body_rows "Read 10 lines")" "10" "the Read cell's peek is not ten rows"
expect_eq "$(grep -c -F '… +7 lines (ctrl+o to expand)' <<<"$settled")" "2" \
	"both cells hint line 4 and the six lines after it"
expect_eq "$(grep -c -E '^ {6,}[^ ].*…$' <<<"$settled")" "2" \
	"each cell closes the row its peek cut line 4 on with a …"
expect_lacks "$settled" -F "END138C" "the end of the cut line showed inline"
expect_lacks "$settled" -F ".rule-" "a line after the cut one was started inline"

# The cell as it streamed and waited: never past its ten rows and the hint.
mc_max="$(sort -n "$SAMPLES" | tail -1)"
note "$(wc -l <"$SAMPLES" | tr -d ' ') samples, at most ${mc_max:-0} rows under the corner"
[ -n "$mc_max" ] || fail "the write cell was never sampled"
[ "${mc_max:-0}" -le 11 ] || fail "the write cell grew to $mc_max rows under its corner (the ten-row peek and the hint are 11)"

# Ctrl+O: the expansion is where the whole line lives.
keys "$S138C" C-o
view="$(wait_pane 5 "$S138C" -F "END138C")" || fail "Ctrl+O never showed the end of the cut line"
dump "Ctrl+O at the end of the conversation" "$view"
keys "$S138C" C-o

smoke_finish
