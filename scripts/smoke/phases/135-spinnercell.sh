#!/usr/bin/env bash
# Phase 135 — a spinner keeps its row in a running command's cell, launch and bashwait alike

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The reported flicker (docs/interactive-shell.md *Streaming the running
# cell*), driven through the REAL backend against a stub provider: `bash`
# starts a program drawing npm's spinner — each frame a column move, an
# erase and the glyph, three writes every 80 ms — the call comes back with
# it still running, and a `bashwait` waits on it. The running cell used to
# jump between the spinner and `⎿ Running…`, moving the box a row each time:
#   - the wait dropped the spinner's row whenever the cycle came back round
#     to the frame the launch's report had handed the model;
#   - an update sampled between a frame's erase and its glyph showed the
#     line blank.
# Sampled every few milliseconds, the cell must show the spinner and never
# fall back to `Running…` once it has.
S135="${S}_spinnercell"
WORK="$(work_dir spinner)"
SPINNER="$WORK/spinner.py"
cat >"$SPINNER" <<'PY'
import os, sys, time
frames = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"
os.write(1, b"installing deps\n")
end, i = time.time() + float(sys.argv[1]), 0
while time.time() < end:
    # npm 10's Progress: cursorTo(0), clearLine(1), the frame — three writes.
    os.write(2, b"\x1b[1G")
    os.write(2, b"\x1b[0K")
    os.write(2, frames[i % len(frames)].encode())
    i += 1
    time.sleep(0.08)
os.write(2, b"\x1b[1G\x1b[0K")
os.write(1, b"added 5 packages\n")
PY
SC_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$SC_PORT" <<'PY' &
import json, re, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port = int(sys.argv[1])

def sse(frames):
    return ("".join("data: %s\n\n" % json.dumps(f) for f in frames) + "data: [DONE]\n\n").encode()

def text(message):
    content = message.get("content")
    if isinstance(content, list):
        return " ".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

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
        messages = json.loads(body).get("messages", [])
        n = len(messages)
        made = [call["function"]["name"] for m in messages for call in (m.get("tool_calls") or [])]
        session = next((found.group(1) for m in messages if m.get("role") == "tool"
                        for found in [re.search(r"session (b[0-9a-z]{8})", text(m))] if found), None)
        if messages[-1].get("role") == "user":
            frames = calling("bash", {"command": "python3 spinner.py 30",
                                      "description": "Install deps", "wait": 4}, n)
        elif session and "bashwait" not in made:
            frames = calling("bashwait", {"session_id": session, "wait": 5}, n)
        elif session and "bashkill" not in made:
            frames = calling("bashkill", {"session_id": session}, n)
        else:
            frames = [{"choices": [{"delta": {"content": "Done."}}]},
                      {"choices": [{"delta": {}, "finish_reason": "stop"}]}]
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
SC_STUB=$!
smoke_on_exit kill "$SC_STUB"
SC_PROVIDERS="$SMOKE_TMP/providers135.toml"
printf '[providers.stub]\nname = "Stub"\n\n[providers.stub.kwargs]\napi_base = "http://127.0.0.1:%s/v1"\n' "$SC_PORT" >"$SC_PROVIDERS"
# No permission gate: the calls run unasked, so nothing but the spinner moves.
SC_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_PERMISSIONS=0 ALTER_ZERO_PROVIDERS_FILE=$SC_PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"
launch -c "$WORK" "$S135" 120 40 "$SC_APP"
submit "$S135" "smoke135 install"

# Sample the pane until the turn settles: per sample, which call's cell is
# the newest on screen and what it shows.
SC_SAMPLES="$SMOKE_TMP/samples135.tsv"
python3 - "$SMOKE_TMUX_SOCKET" "$S135" "$SC_SAMPLES" <<'PY'
import re, subprocess, sys, time

sock, target, out = sys.argv[1:]
glyph = re.compile("[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]")
end = time.time() + 40
with open(out, "w", encoding="utf-8") as f:
    while time.time() < end:
        rows = subprocess.run(["tmux", "-S", sock, "-f", "/dev/null", "capture-pane", "-t", target, "-p"],
                              capture_output=True, text=True).stdout.split("\n")
        if any("Done." in row for row in rows):
            break
        heads = [i for i, row in enumerate(rows) if re.search(r"\b(BashWait|Bash)\(", row)]
        if heads:
            # The newest cell: its rows down to the blank gap above the
            # status line, whose own spinner is braille too.
            cell = []
            for row in rows[heads[-1] + 1:]:
                if not row.strip():
                    break
                cell.append(row.strip())
            call = "BashWait" if "BashWait(" in rows[heads[-1]] else "Bash"
            body = " | ".join(cell)
            state = "running" if "Running…" in body else ("spinner" if glyph.search(body) else "other")
            f.write(f"{call}\t{state}\t{body}\n")
        time.sleep(0.015)
PY
settled="$(wait_settled 30 "$S135" -S -80 -- -F "Done.")" || fail "the turn never settled"
dump "the settled turn" "$settled"

# sc_count CALL STATE — samples of CALL's running cell in STATE.
sc_count() { awk -F'\t' -v c="$1" -v s="$2" '$1 == c && $2 == s' "$SC_SAMPLES" | wc -l | tr -d ' '; }
# sc_relapses CALL — samples of CALL's cell back on `Running…` after it had
# shown the spinner.
sc_relapses() {
	awk -F'\t' -v c="$1" '$1 == c { if ($2 == "spinner") seen = 1; else if (seen && $2 == "running") n++ } END { print n + 0 }' "$SC_SAMPLES"
}
for call in Bash BashWait; do
	note "$call: $(sc_count "$call" spinner) spinner samples, $(sc_count "$call" running) on Running…, $(sc_relapses "$call") after the spinner showed"
	[ "$(sc_count "$call" spinner)" -ge 5 ] || fail "the $call cell never showed the spinner turning"
	expect_eq "$(sc_relapses "$call")" "0" "the $call cell fell back to Running… after showing the spinner"
done
expect_has "$settled" -F "● BashWait(python3" "the wait's cell is missing"
