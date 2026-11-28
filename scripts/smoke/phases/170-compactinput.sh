#!/usr/bin/env bash
# Phase 170 — /compact sends the summarizer the conversation only

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# What the summarizer reads (docs/background-compaction.md): the
# conversation, under the summarizer's own system prompt — never the agent's
# persona and environment blocks, never a `<system-reminder>`. A local stub
# stands in for an OpenAI-compatible provider and appends every chat request
# body to a log; the project's AGENTS.md carries a marker, so the turn's own
# request proves the reminder and the persona are there to leave out. One
# turn, then `/compact`, then both requests are read back off the log.
S170="${S}_compactinput"
C_LOG="$SMOKE_TMP/requests170.jsonl"
C_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$C_PORT" "$C_LOG" <<'PY' &
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port, log = int(sys.argv[1]), sys.argv[2]

def text_of(content):
    if isinstance(content, list):
        return "".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.startswith("/v1/models"):
            data = json.dumps({"data": [{"id": "stub-model", "context_length": 32000}]}).encode()
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        else:
            self.send_response(404)
            self.end_headers()

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0))
        with open(log, "ab") as f:
            f.write(body + b"\n")
        last = json.loads(body)["messages"][-1]
        summarizing = "CONTEXT CHECKPOINT COMPACTION" in text_of(last.get("content"))
        reply = "Stub summary." if summarizing else "Stub reply."
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        for frame in (
            {"choices": [{"index": 0, "delta": {"content": reply}}]},
            {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
        ):
            self.wfile.write(f"data: {json.dumps(frame)}\n\n".encode())
        self.wfile.write(b"data: [DONE]\n\n")

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
C_SERVER=$!
smoke_on_exit kill "$C_SERVER"
for _ in $(seq 1 30); do # wait for the stub to listen
	if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $C_PORT)) == 0 else 1)" 2>/dev/null; then
		break
	fi
	sleep 0.1
done
C_PROVIDERS="$SMOKE_TMP/providers170.toml"
cat >"$C_PROVIDERS" <<TOML
[providers.stub]
name = "Stub"
api_key_env = "STUB_API_KEY"

[providers.stub.kwargs]
api_base = "http://127.0.0.1:$C_PORT/v1"
TOML

# A project of its own, with an AGENTS.md the reminder will carry. The suite
# turns project docs off (lib.sh); this phase turns them back on.
C_PROJ="$SMOKE_TMP/proj170"
mkdir -p "$C_PROJ"
printf 'SMOKE170 house rule: tabs, never spaces.\n' >"$C_PROJ/AGENTS.md"

launch -c "$C_PROJ" "$S170" 100 40 "env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 ALTER_ZERO_PROJECT_DOC_MAX_BYTES=32768 ALTER_ZERO_PROVIDERS_FILE=$C_PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=smoke-170 $APP_ABS"
submit "$S170" "hello there"
replied="$(wait_pane 10 "$S170" -F "Stub reply.")" || fail "the stub's reply never streamed in"
dump "the turn against the stub" "$replied"
type_text "$S170" "/compact"
sleep 0.3
keys "$S170" Enter
compacted="$(wait_pane 15 "$S170" -S -60 -- -F "Context compacted")" || fail "/compact never landed its marker"
dump "after /compact" "$compacted"

# The first request is the turn's, the last the summarizer's.
report="$(python3 - "$C_LOG" <<'PY'
import json, sys

def text_of(content):
    if isinstance(content, list):
        return "".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

bodies = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
if len(bodies) < 2:
    sys.exit(f"expected the turn's request and the summarizer's, got {len(bodies)}")
for name, body in (("turn", bodies[0]), ("summary", bodies[-1])):
    messages = body["messages"]
    system = "".join(text_of(m.get("content")) for m in messages if m["role"] == "system")
    rest = [text_of(m.get("content")) for m in messages if m["role"] != "system"]
    print(f"{name} system-environment {'## Environment' in system}")
    print(f"{name} system-summarizer {system.startswith('You write context checkpoints.')}")
    print(f"{name} reminder {any('<system-reminder>' in t for t in rest)}")
    print(f"{name} agents-md {any('SMOKE170 house rule' in t for t in rest)}")
    print(f"{name} hello {any('hello there' in t for t in rest)}")
    print(f"{name} last-is-compact-prompt {rest[-1].startswith('You are performing a CONTEXT CHECKPOINT COMPACTION')}")
    print(f"{name} tools {bool(body.get('tools'))}")
PY
)" || fail "the request log could not be read: $report"
dump "what each request carried" "$report"
tmux kill-session -t "$S170" 2>/dev/null

echo "==== Phase 170: the summarizer reads the conversation only ===="
# The turn: the agent's own system prompt, the reminder with AGENTS.md.
expect_has "$report" -xF "turn system-environment True" "the turn's request has no environment block — the check below proves nothing"
expect_has "$report" -xF "turn agents-md True" "the turn's request does not carry AGENTS.md — the check below proves nothing"
# The summary: its own system prompt, no reminder, the conversation, the prompt last.
expect_has "$report" -xF "summary system-summarizer True" "the summarizer was not sent its own system prompt"
expect_has "$report" -xF "summary system-environment False" "the summarizer was sent the agent's environment block"
expect_has "$report" -xF "summary reminder False" "the summarizer was sent a <system-reminder>"
expect_has "$report" -xF "summary agents-md False" "the summarizer was sent AGENTS.md"
expect_has "$report" -xF "summary hello True" "the summarizer was not sent the conversation"
expect_has "$report" -xF "summary last-is-compact-prompt True" "the compact prompt is not the summarizer's last message"
expect_has "$report" -xF "summary tools False" "the summarizer was offered tools"
