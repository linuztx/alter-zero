#!/usr/bin/env bash
# Phase 154 — auto mode's verdict on the Cloud Code wire: the listed id, a retry, the repair turn, a failed check's prompt

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Auto mode's classifier (docs/permissions.md) is a second request to the
# session's own provider, and on Google Antigravity that request used to fail
# every time: a grouped model's thinking level is the suffix of the id a
# request names (docs/antigravity.md), the classifier drops the thinking mode,
# and so it asked for `gemini-9-flash` where the catalog lists only
# `gemini-9-flash-high` — a 404, swallowed, and a prompt in its place. This
# phase drives the real backend against a Cloud Code stub that plays both
# halves: the turn (a `bash` call, then a closing line) and the verdict on
# that call, which it tells apart by the system instruction. What it pins:
#
# - a verdict is asked of the id the catalog lists, with no tools and no
#   thoughts asked for, and an allow runs the command under the classifier's
#   note with no prompt;
# - a provider that is busy for a moment (429) is asked again, and the
#   verdict still arrives;
# - a reply that is no verdict is answered by the REPAIR TURN — the reply
#   handed back with the correction — and the second reply is read;
# - a block rejects the call with the classifier's reason;
# - a check that fails for good raises the prompt WITH WHAT IT SAID, in the
#   place of the standing notice, and answering it runs the command.
S154="${S}_autoverdict"
AV_LOG="$SMOKE_TMP/autoverdict.log"
AV_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$AV_PORT" "$AV_LOG" <<'PY' &
import json, re, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port, log = int(sys.argv[1]), sys.argv[2]
asked = {}

CATALOG = {
    "models": {
        "gemini-9-flash-low": {"displayName": "Gemini 9 Flash (Low)", "maxTokens": 1048576,
                               "supportsImages": True, "supportsThinking": True,
                               "quotaInfo": {"remainingFraction": 0.9, "resetTime": "2099-01-01T00:00:00Z"}},
        "gemini-9-flash-high": {"displayName": "Gemini 9 Flash (High)", "maxTokens": 1048576,
                                "supportsImages": True, "supportsThinking": True,
                                "quotaInfo": {"remainingFraction": 0.75, "resetTime": "2099-01-01T00:00:00Z"}},
    },
    "agentModelSorts": [{"groups": [{"modelIds": ["gemini-9-flash-high", "gemini-9-flash-low"]}]}],
}

def texts(content):
    return " ".join(part.get("text", "") for part in content.get("parts", []))

def marker(text):
    found = re.findall(r"demo-[a-z]+", text)
    return found[-1] if found else ""

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.startswith("/oauth2/v1/userinfo"):
            self.reply(200, {"email": "dev@example.com", "verified_email": True})
        else:
            self.reply(404, {})

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0)).decode()
        if self.path == "/token":
            self.reply(200, {"access_token": "ya29.smoke-154", "expires_in": 3599})
        elif self.path == "/v1internal:loadCodeAssist":
            self.reply(200, {"cloudaicompanionProject": "proj-154",
                             "currentTier": {"id": "free-tier", "name": "Free"}})
        elif self.path == "/v1internal:fetchAvailableModels":
            self.reply(200, CATALOG)
        elif self.path == "/v1internal:streamGenerateContent?alt=sse":
            self.generate(json.loads(body))
        else:
            self.reply(404, {})

    def generate(self, envelope):
        request = envelope["request"]
        contents = request["contents"]
        system = texts(request.get("systemInstruction", {}))
        verdict = system.startswith("You are the safety classifier")
        which = marker(texts(contents[0]) if verdict else " ".join(
            texts(c) for c in contents if c["role"] == "user"))
        key = ("verdict" if verdict else "turn", which)
        asked[key] = asked.get(key, 0) + 1
        with open(log, "a") as f:
            f.write(json.dumps({
                "kind": key[0], "marker": which, "model": envelope["model"],
                "tools": "tools" in request,
                "thinking": "thinkingConfig" in request.get("generationConfig", {}),
                "contents": len(contents), "last": texts(contents[-1]),
                "system": system[:60],
            }) + "\n")
        if not verdict:
            answered = any("functionResponse" in part for part in contents[-1].get("parts", []))
            if answered:
                self.stream([{"text": f"Done with {which}."}])
            else:
                self.stream([{"functionCall": {"name": "bash", "args": {
                    "command": f"mkdir {which}", "description": "Create the demo directory"}}}])
        elif which == "demo-retry" and asked[key] == 1:
            self.reply(429, {"error": {"code": 429, "status": "RESOURCE_EXHAUSTED",
                                       "message": "You have exhausted your capacity on this model. Resets in 0s."}})
        elif which == "demo-repair" and len(contents) < 3:
            self.stream([{"text": "This only creates a directory, so it looks fine to me."}])
        elif which == "demo-block":
            self.stream([{"text": '{"verdict": "block", "reason": "The smoke stub says no."}'}])
        elif which == "demo-fail":
            self.reply(404, {"error": {"code": 404, "status": "NOT_FOUND",
                                       "message": "Requested entity was not found."}})
        else:
            self.stream([{"text": '{"verdict": "allow"}'}])

    def stream(self, parts):
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        frame = {"response": {
            "candidates": [{"content": {"role": "model", "parts": parts}, "finishReason": "STOP"}],
            "usageMetadata": {"promptTokenCount": 900, "candidatesTokenCount": 9, "totalTokenCount": 909}}}
        self.wfile.write(("data: " + json.dumps(frame) + "\n\n").encode())
        self.wfile.flush()

    def reply(self, status, obj):
        data = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
AV_SERVER=$!
for _ in $(seq 1 30); do # wait for the stub to listen
	if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $AV_PORT)) == 0 else 1)" 2>/dev/null; then
		break
	fi
	sleep 0.1
done
AV_BASE="http://127.0.0.1:$AV_PORT"
AV_PROVIDERS="$SMOKE_TMP/providers-154.toml"
cat >"$AV_PROVIDERS" <<EOF
[providers.antigravity]
name = "Google Antigravity"
auth = "google_antigravity"
wire_api = "cloud_code"
description = "Sign in with your Google account (smoke stub)"
api_key_env = "ANTIGRAVITY_REFRESH_TOKEN"

[providers.antigravity.kwargs]
api_base = "$AV_BASE"
EOF
AV_WORK="$(work_dir)"
# Signed in already (the token in the environment), on the grouped model, in
# AUTO mode — the suite's base env pins manual, so the later assignment wins.
launch -c "$AV_WORK" "$S154" 100 40 "env NO_PROXY=127.0.0.1,localhost no_proxy=127.0.0.1,localhost $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS ALTER_ZERO_PROVIDERS_FILE=$AV_PROVIDERS ALTER_ZERO_GOOGLE_OAUTH_BASE=$AV_BASE ANTIGRAVITY_REFRESH_TOKEN=1//refresh-154 ALTER_ZERO_PROVIDER=antigravity ALTER_ZERO_MODEL=gemini-9-flash ALTER_ZERO_PERMISSION_MODE=auto $BIN_ABS"
# The startup probe reads the catalog and seats the thinking level the id
# is listed under; a turn sent before it would name no level at all.
footer="$(wait_pane 15 "$S154" -F "gemini-9-flash high")"
dump "the footer once the probe has read the catalog" "$footer"
expect_has "$footer" -F "gemini-9-flash high" "the grouped model never got its thinking level"
expect_has "$footer" -E "auto[[:space:]]*$" "the session is not in auto mode"

# av_turn MARKER — one turn asking for `mkdir MARKER`, settled on the stub's
# closing line.
av_turn() {
	submit "$S154" "please make $1"
	wait_pane 30 "$S154" -F "Done with $1."
}

# 1. An allow: the command runs under the classifier's note, unprompted.
allowed="$(av_turn demo-allow)"
dump "an allowed command" "$allowed"
expect_has "$allowed" -F "Bash(mkdir demo-allow)" "the command's cell is missing"
expect_has "$allowed" -F "Allowed by auto mode classifier" "the allow did not note the classifier"
expect_lacks "$allowed" -F "Do you want to proceed?" "auto mode prompted for a command its classifier allowed"
[ -d "$AV_WORK/demo-allow" ] || fail "the allowed command did not run"

# 2. A busy provider: asked again, and the verdict still arrives.
retried="$(av_turn demo-retry)"
dump "a verdict that needed a retry" "$retried"
expect_lacks "$retried" -F "Do you want to proceed?" "a 429 on the verdict's request became a prompt"
[ -d "$AV_WORK/demo-retry" ] || fail "the command behind the retried verdict did not run"

# 3. A reply that is no verdict: the repair turn reads the second one.
repaired="$(av_turn demo-repair)"
dump "a verdict that needed the repair turn" "$repaired"
expect_lacks "$repaired" -F "Do you want to proceed?" "a reply that was no verdict became a prompt"
[ -d "$AV_WORK/demo-repair" ] || fail "the command behind the repaired verdict did not run"

# 4. A block: rejected with the classifier's reason, never run.
blocked="$(av_turn demo-block)"
dump "a blocked command" "$blocked"
expect_has "$blocked" -F "Denied by auto mode classifier" "the block did not reject the call"
expect_has "$blocked" -F "Reason: The smoke stub says no." "the block's reason did not reach the cell"
[ ! -e "$AV_WORK/demo-block" ] || fail "a blocked command ran"

# 5. A check that fails for good: the prompt says what it said.
submit "$S154" "please make demo-fail"
asked="$(wait_pane 30 "$S154" -F "Do you want to proceed?")"
dump "the prompt a failed check falls back to" "$asked"
expect_has "$asked" -F "Auto mode's safety check failed: HTTP 404" "the prompt did not say the check failed"
# Wrapped, never cut: the notice's tail — the part that says what to do —
# is on the row below.
expect_has "$asked" -F "another with /model." "the prompt cut what the check said"
expect_lacks "$asked" -F "This command requires approval" "the standing notice was not replaced"
[ ! -e "$AV_WORK/demo-fail" ] || fail "the command ran before the prompt was answered"
keys "$S154" Enter
answered="$(wait_pane 30 "$S154" -F "Done with demo-fail.")"
dump "the prompt answered" "$answered"
expect_lacks "$answered" -F "Do you want to proceed?" "the prompt did not close"
[ -d "$AV_WORK/demo-fail" ] || fail "the approved command did not run"

# What went over the wire, off the stub's log.
python3 - "$AV_LOG" <<'PY' || fail "the verdict's requests were not the expected shapes (see above)"
import json, sys
entries = [json.loads(line) for line in open(sys.argv[1])]
verdicts = [e for e in entries if e["kind"] == "verdict"]
def of(marker):
    return [e for e in verdicts if e["marker"] == marker]
for e in verdicts:
    # The id the catalog lists — the session's own — never the bare one.
    assert e["model"] == "gemini-9-flash-high", e
    assert not e["tools"], f"a verdict was asked with tools: {e}"
    assert not e["thinking"], f"a verdict asked for thoughts: {e}"
assert len(of("demo-allow")) == 1, of("demo-allow")
assert len(of("demo-retry")) == 2, f"the busy request was asked {len(of('demo-retry'))} times"
repair = of("demo-repair")
assert [e["contents"] for e in repair] == [1, 3], repair
assert repair[1]["last"].startswith("That reply was not in the output format"), repair[1]
assert len(of("demo-block")) == 1, of("demo-block")
# A request the provider refuses for what it asks is not asked again.
assert len(of("demo-fail")) == 1, f"the refused request was asked {len(of('demo-fail'))} times"
turns = [e for e in entries if e["kind"] == "turn"]
assert all(e["model"] == "gemini-9-flash-high" and e["tools"] for e in turns), turns
print("verdict wire: ok")
PY

kill "$AV_SERVER" 2>/dev/null
wait "$AV_SERVER" 2>/dev/null
tmux kill-session -t "$S154" 2>/dev/null
echo "==== Phase 154: auto mode's verdict names the listed id, retries a busy provider, repairs a reply and says why a failed check asks ===="
