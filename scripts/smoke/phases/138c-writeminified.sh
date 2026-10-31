#!/usr/bin/env bash
# Phase 138c — a minified line past the file peek's tenth row is cut there, never painted whole

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# docs/long-lines.md §8. The numbered `read`/`write`/`edit` cells showed every
# line their peek started to its end, so a `write` of a stylesheet whose
# fourth line was the whole sheet minified painted that one line across well
# over a hundred rows, under a `… +6 lines` hint counting the lines after it.
# The peek's ten rows are a ceiling now: a line the rows left can hold shows
# whole, and the one that needs more is cut on the tenth row with a `…`.
#
# The REAL backend, against a stub provider whose model writes the reported
# shape — lines 1–3 short enough to show whole, line 4 some twelve thousand
# columns ending in the word END138C, lines 5–10 short — streamed in pieces,
# then reads the file back and answers. Proved:
#   - neither the streaming cell nor its head once the content has arrived
#     ever shows more than ten body rows;
#   - the committed `Write` cell and the `Read` cell after it each show
#     exactly ten body rows, the tenth ending in `…`, over
#     `… +6 lines (ctrl+o to expand)`, and line 4's last word is nowhere
#     inline;
#   - Ctrl+O still shows line 4 to its end.

PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$PORT" <<'PY' &
import json, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer

port = int(sys.argv[1])
sheet = ("*{box-sizing:border-box}body{margin:0;min-width:360px}button{font:inherit;cursor:pointer}"
         ".site-header{height:80px;max-width:1440px;padding:0 54px;margin:auto;display:flex}") * 75
lines = [
    "@import url('https://fonts.googleapis.com/css2?family=DM+Sans:wght@400;500;700&family=Manrope:wght@400;600;800&display=swap');",
    "",
    ":root{font-family:'DM Sans',sans-serif;color:#272932;background:#fafbfc;--purple:#6c5ce7;--line:#e8e9ee}",
    sheet + ".end{content:'END138C'}",
    "@keyframes pulse{50%{opacity:.4}}",
    "@keyframes fadeIn{from{opacity:0}}",
    "@keyframes slideUp{from{transform:translateY(8px)}}",
    "@keyframes toastIn{from{opacity:0}}",
    "@media(max-width:900px){.workspace{grid-template-columns:1fr}}",
    "@media(max-width:600px){.site-header{padding:0 18px}}",
]
write_args = json.dumps({"path": "src/styles.css", "content": "\n".join(lines) + "\n"})
read_args = json.dumps({"path": "src/styles.css"})

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
            for at in range(0, len(write_args), 240):
                piece = {"index": 0, "function": {"arguments": write_args[at:at + 240]}}
                self.wfile.write(frame({"choices": [{"delta": {"tool_calls": [piece]}}]}))
                self.wfile.flush()
                time.sleep(0.02)
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
mkdir -p "$WORK/src"
# No permission gate: the write and the read run unasked.
MW_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_PERMISSIONS=0 ALTER_ZERO_PROVIDERS_FILE=$PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"
S138C="${S}_writeminified"
launch -c "$WORK" "$S138C" 100 40 "$MW_APP"
submit "$S138C" "smoke138c write the stylesheet and read it back"

# Sample the newest Write cell until the turn settles: its corner row and how
# many body rows sit under it before the blank row that ends the cell.
SAMPLES="$SMOKE_TMP/samples138c.tsv"
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
        heads = [i for i, row in enumerate(rows) if "Write(src/styles.css)" in row]
        if heads:
            cell = rows[heads[-1] + 1:]
            corner = cell[0].strip() if cell else ""
            body = 0
            for row in cell[1:]:
                if not row.strip() or "ctrl+o to expand" in row:
                    break
                body += 1
            f.write(f"{corner}\t{body}\n")
        time.sleep(0.02)
PY
settled="$(wait_settled 30 "$S138C" -S -80 -- -F "Done.")" || fail "the turn never settled"
dump "the settled turn" "$settled"
expect_has "$settled" -F "Wrote 10 lines to src/styles.css" "the streamed write ran and committed"
expect_has "$settled" -F "Read 10 lines" "the read ran and committed"

# Each committed cell: the rows between its corner row and its hint.
mw_body() { awk -v head="$1" 'index($0, head) { on = 1; next } on && /ctrl\+o to expand/ { exit } on' <<<"$settled"; }
for corner in "Wrote 10 lines to src/styles.css" "Read 10 lines"; do
	body="$(mw_body "$corner")"
	dump "the body under '$corner'" "$body"
	expect_eq "$(grep -c '' <<<"$body")" "10" "the cell under '$corner' is not ten rows"
	expect_has "$(tail -n 1 <<<"$body")" -E '…$' "the cell under '$corner' does not close its tenth row with a …"
done
expect_eq "$(grep -c -F '… +6 lines (ctrl+o to expand)' <<<"$settled")" "2" \
	"each cell's hint should count lines 5–10"
expect_lacks "$settled" -F "END138C" "line 4 was painted inline to its last word"
expect_lacks "$settled" -F "@keyframes" "a line after the cut one was started"

# The cell as it streamed and waited: never past its ten rows.
mw_tallest() { awk -F'\t' 'BEGIN { m = 0 } $2 > m { m = $2 } END { print m }' "$SAMPLES"; }
note "$(wc -l <"$SAMPLES" | tr -d ' ') samples, the tallest body $(mw_tallest) rows"
[ "$(wc -l <"$SAMPLES" | tr -d ' ')" -ge 1 ] || fail "the Write cell was never sampled while it streamed"
[ "$(mw_tallest)" -le 10 ] || fail "the streaming cell showed $(mw_tallest) body rows, past its ten"

# Ctrl+O holds the line the peek cut, to its end.
keys "$S138C" C-o
overlay="$(wait_pane 5 "$S138C" -F "END138C")" || fail "Ctrl+O does not show line 4's last word"
dump "Ctrl+O" "$overlay"
expect_has "$overlay" -F "T R A N S C R I P T" "Ctrl+O did not open the transcript"

smoke_finish
