#!/usr/bin/env bash
# Phase 173 — a subagent's background compaction, watched from its session view

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# A subagent compacts its own context (docs/background-compaction.md
# *Subagents*), driven through the REAL backend against a stub provider whose
# usage frames set the measure: a 100k window puts the agent's ceiling at 84k
# and a cold cycle's start at 42k.
#
# The lead launches one background agent and answers. The agent's rounds
# report 30k, 45k, 60k, 90k: its summary starts at the third round top and
# takes the stub six seconds, so its ceiling at the fifth comes first. From
# the agent's session view, opened while it works: its status line reads
# `Compacting…` while it waits, the `Context compacted · … · background` cell
# commits live in the view, and the agent finishes on the list the splice
# left — the request that follows carrying the summary with the rounds since
# it started kept as they were. Its lead hears it finish.
S173="${S}_agentcompact"
STUB_LOG="$SMOKE_TMP/stub173.log"
STUB_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$STUB_PORT" "$STUB_LOG" <<'PY' &
import json, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer
from socketserver import ThreadingMixIn

port, log = int(sys.argv[1]), sys.argv[2]
# What the lead asks the agent to do: what tells the agent's requests from
# the lead's, which quotes it only in its `agent` call.
TASK = "smoke173-agent read the logs"
# Each agent round's usage: the measure its round tops decide with.
USAGE = [30000, 45000, 60000, 90000]

def text(message):
    content = message.get("content")
    if isinstance(content, list):
        return " ".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

def sse(frames, prompt_tokens):
    frames = frames + [{"choices": [], "usage": {"prompt_tokens": prompt_tokens, "completion_tokens": 20}}]
    return ("".join("data: %s\n\n" % json.dumps(f) for f in frames) + "data: [DONE]\n\n").encode()

def answer(words):
    return [{"choices": [{"delta": {"content": words}}]},
            {"choices": [{"delta": {}, "finish_reason": "stop"}]}]

def call(name, arguments, call_id):
    tool_call = {"index": 0, "id": call_id, "type": "function",
                 "function": {"name": name, "arguments": json.dumps(arguments)}}
    return [{"choices": [{"delta": {"tool_calls": [tool_call]}}]},
            {"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}]

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send(b'{"object":"list","data":[{"id":"stub-model"}]}', "application/json")

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0)).decode()
        request = json.loads(body)
        messages = request.get("messages", [])
        summary = not request.get("tools")
        agent = any(m.get("role") == "user" and TASK in text(m) for m in messages)
        with open(log, "a") as f:
            f.write("%s\t%s\t%s\n" % ("summary" if summary else "round", "agent" if agent else "lead",
                                      body.replace("\n", " ")))
        if summary:
            time.sleep(6)  # written while the agent goes on
            self.send(sse(answer("SUMMARY-agent: the agent read the logs."), 20000), "text/event-stream")
            return
        if agent:
            done = sum(1 for m in messages if m.get("role") == "tool")
            if any("SUMMARY-agent" in text(m) for m in messages) or done >= len(USAGE):
                self.send(sse(answer("Done agent."), 30000), "text/event-stream")
                return
            time.sleep(0.8)  # a round's work: time to open its view
            frames = call("bash", {"command": "echo agent round %d" % (done + 1)}, "call_173_agent_%d" % done)
            self.send(sse(frames, USAGE[done]), "text/event-stream")
            return
        launched = any(tc.get("function", {}).get("name") == "agent"
                       for m in messages for tc in (m.get("tool_calls") or []))
        if not launched:
            frames = call("agent", {"description": "Read the logs", "prompt": TASK,
                                    "subagent_type": "general-purpose", "run_in_background": True},
                          "call_173_lead")
            self.send(sse(frames, 3000), "text/event-stream")
            return
        last = messages[-1] if messages else {}
        self.send(sse(answer("Done lead." if last.get("role") == "tool" else "Heard the agent."), 4000),
                  "text/event-stream")

    def send(self, data, kind):
        try:
            self.send_response(200)
            self.send_header("content-type", kind)
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def log_message(self, *_args):
        pass

class Server(ThreadingMixIn, HTTPServer):
    daemon_threads = True

Server(("127.0.0.1", port), Stub).serve_forever()
PY
STUB_PID=$!
smoke_on_exit kill "$STUB_PID"
PROVIDERS="$SMOKE_TMP/providers173.toml"
cat >"$PROVIDERS" <<EOF
[providers.stub]
name = "Stub"

[providers.stub.kwargs]
api_base = "http://127.0.0.1:$STUB_PORT/v1"
EOF
REAL_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_PERMISSIONS=0 ALTER_ZERO_CONTEXT_WINDOW=100000 ALTER_ZERO_PROVIDERS_FILE=$PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"

launch -c "$(work_dir agentcompact)" "$S173" 120 50 "$REAL_APP"
submit "$S173" "smoke173 go"
wait_for 20 "$S173" -F "Done lead." || fail "the lead never finished its turn after launching the agent"
wait_for 10 "$S173" -F "Read the logs" || fail "the agent's row never reached the roster"
# ↓ opens the roster on `● main`, a second ↓ steps onto the agent, Enter
# opens its session view while it works.
keys "$S173" Down
sleep 0.2
keys "$S173" Down
sleep 0.2
keys "$S173" Enter
opened="$(wait_pane 10 "$S173" -F "smoke173-agent read the logs")" ||
	fail "the agent's session view never opened on its own transcript"
dump "the agent's session view, opened while it works" "$opened"
waiting="$(wait_pane 20 "$S173" -F "Compacting…")" ||
	fail "the agent's view never showed it waiting at its ceiling with 'Compacting…'"
dump "the agent waiting at its ceiling for its summary" "$waiting"
finished="$(wait_settled 30 "$S173" -S -200 -- -F "Done agent.")" ||
	fail "the agent never finished after its summary was spliced in"
dump "the agent's view once it finished" "$finished"
expect_has "$finished" -E "Context compacted · [0-9.]+k? → [0-9.]+k? tokens · [0-9]+s · background" \
	"the agent's view has no '· background' compaction cell"
cell_row="$(printf '%s\n' "$finished" | grep -nF "Context compacted" | tail -1 | cut -d: -f1)"
reply_row="$(printf '%s\n' "$finished" | grep -nF "Done agent." | tail -1 | cut -d: -f1)"
round_row="$(printf '%s\n' "$finished" | grep -nF "echo agent round 4" | tail -1 | cut -d: -f1)"
if [ -z "$cell_row" ] || [ -z "$reply_row" ] || [ -z "$round_row" ] ||
	[ "$cell_row" -le "$round_row" ] || [ "$cell_row" -ge "$reply_row" ]; then
	fail "the agent's compaction cell is not between its last round and its reply (round $round_row, cell $cell_row, reply $reply_row)"
fi
# Back on the main view, the lead heard the agent finish.
keys "$S173" Escape
wait_for 20 "$S173" -S -200 -- -F "Heard the agent." || fail "the lead never answered on the agent's result"
tmux kill-session -t "$S173" 2>/dev/null

python3 - "$STUB_LOG" <<'PY' || fail "the requests the stub saw are wrong (see above)"
import json, sys

lines = [line.rstrip("\n").split("\t", 2) for line in open(sys.argv[1]) if line.strip()]
agent = [(kind, json.loads(body)["messages"]) for kind, who, body in lines if who == "agent"]
kinds = [kind for kind, _ in agent]

def text(message):
    content = message.get("content")
    if isinstance(content, list):
        return " ".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

problems = []
if kinds.count("summary") != 1:
    problems.append("not one agent summary request: %s" % kinds)
else:
    asked = kinds.index("summary")
    if kinds[asked + 1:].count("round") < 2:
        problems.append("the agent did not go on while its summary was written: %s" % kinds)
    summary = agent[asked][1]
    rounds = [m for kind, m in agent if kind == "round"]
    if summary[0].get("role") != "system" or (rounds and text(summary[0]) == text(rounds[0][0])):
        problems.append("the summarizer read the agent's own system prompt")
    if any("system-reminder" in text(m) for m in summary):
        problems.append("the summarizer read a <system-reminder>")
spliced = [m for kind, m in agent if kind == "round" and any("SUMMARY-agent" in text(x) for x in m)]
if not spliced:
    problems.append("no agent request carries the summary")
else:
    m = spliced[0]
    bridge = next(i for i, x in enumerate(m) if "SUMMARY-agent" in text(x))
    if not any(x.get("role") == "user" and "smoke173-agent" in text(x) for x in m[:bridge]):
        problems.append("the agent's task was not kept ahead of the summary")
    kept = [x for x in m[bridge + 1:] if x.get("role") == "tool"]
    if not kept:
        problems.append("the rounds since it started were not kept after the summary: %s" % [x.get("role") for x in m])
for problem in problems:
    print("FAIL:", problem)
sys.exit(1 if problems else 0)
PY
