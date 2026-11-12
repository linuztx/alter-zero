#!/usr/bin/env bash
# Phase 148 — the Google Antigravity sign-in, its catalog, a turn and /usage, against a local stub

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Google Antigravity (docs/providers/antigravity.md) is a browser sign-in, so the
# browser is played here: the phase reads the `state` off the sign-in page
# and makes the redirect itself, against the loopback port the page names.
# Google's token and userinfo hosts are one stub (ALTER_ZERO_GOOGLE_OAUTH_BASE)
# and so is Cloud Code (the provider's base, in a standalone providers
# file), answering the backend's own wire shapes. What this phase pins:
#
# - the row opens the BROWSER page — the link, not a code — whose authorize
#   URL asks for offline access on Antigravity's client with S256 PKCE;
# - the redirect completes the sign-in: the code is redeemed with the client
#   secret and the PKCE verifier, the account is named off userinfo, and an
#   account Cloud Code has never set up is ONBOARDED (asked again until it
#   is done), so the toast names the account and its plan, and the Google
#   refresh token lands in the key store;
# - /model lists the account's catalog with level siblings grouped into one
#   row, whose thinking ladder the footer shows;
# - a turn goes out as the Cloud Code envelope — the onboarded project, the
#   runtime id with its level, the agent fields, the bearer and the agy CLI
#   User-Agent, `includeThoughts`, and no Antigravity identity in the system
#   instruction — and the stub's SSE reply reaches the screen;
# - /usage draws the catalog's quota as a bar under its family.
S148="${S}_antigravity"
AG_LOG="$SMOKE_TMP/antigravity.log"
AG_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$AG_PORT" "$AG_LOG" <<'PY' &
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

port, log = int(sys.argv[1]), sys.argv[2]
onboards = 0

CATALOG = {
    "models": {
        "gemini-9-flash-low": {"displayName": "Gemini 9 Flash (Low)", "maxTokens": 1048576,
                               "supportsImages": True, "supportsThinking": True,
                               "quotaInfo": {"remainingFraction": 0.9, "resetTime": "2099-01-01T00:00:00Z"}},
        "gemini-9-flash-high": {"displayName": "Gemini 9 Flash (High)", "maxTokens": 1048576,
                                "supportsImages": True, "supportsThinking": True,
                                "quotaInfo": {"remainingFraction": 0.75, "resetTime": "2099-01-01T00:00:00Z"}},
        "chat_20706": {"model": "MODEL_CHAT_20706"},
    },
    "agentModelSorts": [{"groups": [{"modelIds": ["gemini-9-flash-high", "gemini-9-flash-low", "chat_20706"]}]}],
}

def frame(obj):
    return ("data: " + json.dumps({"response": obj}) + "\n\n").encode()

class Stub(BaseHTTPRequestHandler):
    def record(self, body):
        with open(log, "a") as f:
            f.write(json.dumps({
                "method": self.command, "path": self.path,
                "authorization": self.headers.get("authorization") or "",
                "user_agent": self.headers.get("user-agent") or "",
                "body": body,
            }) + "\n")

    def do_GET(self):
        self.record("")
        if self.path.startswith("/oauth2/v1/userinfo"):
            self.reply(200, {"email": "dev@example.com", "verified_email": True})
        else:
            self.reply(404, {})

    def do_POST(self):
        global onboards
        body = self.rfile.read(int(self.headers.get("content-length") or 0)).decode()
        self.record(body)
        if self.path == "/token":
            form = urllib.parse.parse_qs(body)
            if form.get("grant_type") == ["authorization_code"] and form.get("code") == ["code-148"]:
                self.reply(200, {"access_token": "ya29.smoke-148", "refresh_token": "1//refresh-148",
                                 "expires_in": 3599, "token_type": "Bearer"})
            elif form.get("grant_type") == ["refresh_token"]:
                self.reply(200, {"access_token": "ya29.smoke-148", "expires_in": 3599})
            else:
                self.reply(400, {"error": "invalid_grant"})
        elif self.path == "/v1internal:loadCodeAssist":
            # An account Cloud Code has never set up: no project, a default
            # tier to onboard on — until onboarding is done.
            answer = {"currentTier": {"id": "free-tier", "name": "Free"},
                      "allowedTiers": [{"id": "free-tier", "isDefault": True}]}
            if onboards >= 2:
                answer["cloudaicompanionProject"] = "proj-148"
            self.reply(200, answer)
        elif self.path == "/v1internal:onboardUser":
            onboards += 1
            if onboards == 1:
                self.reply(200, {"name": "operations/148"})
            else:
                self.reply(200, {"done": True,
                                 "response": {"cloudaicompanionProject": {"id": "proj-148"}}})
        elif self.path == "/v1internal:fetchAvailableModels":
            self.reply(200, CATALOG)
        elif self.path == "/v1internal:streamGenerateContent?alt=sse":
            self.send_response(200)
            self.send_header("content-type", "text/event-stream")
            self.end_headers()
            self.wfile.write(frame({"candidates": [{"content": {"role": "model", "parts": [
                {"text": "Weighing the greeting.", "thought": True}]}}]}))
            self.wfile.write(frame({"candidates": [{"content": {"role": "model", "parts": [
                {"text": "Hello from the Antigravity stub, phase 148."}]}, "finishReason": "STOP"}],
                "usageMetadata": {"promptTokenCount": 1200, "candidatesTokenCount": 12,
                                  "thoughtsTokenCount": 5, "totalTokenCount": 1217}}))
            self.wfile.flush()
        else:
            self.reply(404, {})

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
AG_SERVER=$!
for _ in $(seq 1 30); do # wait for the stub to listen
	if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $AG_PORT)) == 0 else 1)" 2>/dev/null; then
		break
	fi
	sleep 0.1
done
AG_BASE="http://127.0.0.1:$AG_PORT"
# The provider as shipped, but for its base: Cloud Code is the stub.
AG_PROVIDERS="$SMOKE_TMP/providers-148.toml"
cat >"$AG_PROVIDERS" <<EOF
[providers.antigravity]
name = "Google Antigravity"
auth = "google_antigravity"
wire_api = "cloud_code"
description = "Sign in with your Google account (smoke stub)"
api_key_env = "ANTIGRAVITY_REFRESH_TOKEN"

[providers.antigravity.kwargs]
api_base = "$AG_BASE"
EOF
AG_WORK="$(work_dir)"
# NO_PROXY keeps a developer's HTTP proxy out of the loopback requests.
launch -c "$AG_WORK" "$S148" 110 36 "env NO_PROXY=127.0.0.1,localhost no_proxy=127.0.0.1,localhost $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS ALTER_ZERO_PROVIDERS_FILE=$AG_PROVIDERS ALTER_ZERO_GOOGLE_OAUTH_BASE=$AG_BASE $BIN_ABS"

# The row, by name; one flow is listed, so Enter opens the browser page.
submit "$S148" "/login"
sleep 0.5
keys "$S148" Enter # Sign in with an account
subs="$(wait_pane 5 "$S148" -F "Google Antigravity")"
dump "the subscription list" "$subs"
expect_has "$subs" -F "Google Antigravity · ◯ unconfigured" "the subscription list did not offer the Antigravity sign-in"
type_text "$S148" "antigravity"
sleep 0.3
keys "$S148" Enter
page="$(wait_pane 8 "$S148" -F "Waiting for the browser")"
dump "the browser sign-in page" "$page"
for want in "Sign in to Google Antigravity" "this window continues by itself" "c copy link"; do
	expect_has "$page" -F "$want" "the sign-in page did not show \"$want\""
done
for unwanted in "enter this one-time code" "c copy code"; do
	expect_lacks "$page" -F "$unwanted" "the browser page wore the device-code wording (\"$unwanted\")"
done
# The URL wraps across rows; squeeze it back together (it holds no spaces).
url="$(printf '%s' "$page" | tr -d ' \n')"
for want in "https://accounts.google.com/o/oauth2/v2/auth?client_id=1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com" \
	"access_type=offline" "prompt=consent" "code_challenge_method=S256" \
	"redirect_uri=http%3A%2F%2Flocalhost%3A" "%2Foauth-callback"; do
	expect_has "$url" -F "$want" "the authorize URL is missing \"$want\""
done
ag_state="$(printf '%s' "$url" | grep -oE 'state=[A-Za-z0-9_-]+' | head -1 | cut -d= -f2)"
ag_cb_port="$(printf '%s' "$url" | grep -oE 'localhost%3A[0-9]+' | head -1 | sed 's/.*%3A//')"
[ -n "$ag_state" ] || fail "no state could be read off the authorize URL"
[ -n "$ag_cb_port" ] || fail "no callback port could be read off the authorize URL"

# Play the browser: the redirect back to the loopback listener.
cb_page="$(curl -s --noproxy '*' "http://127.0.0.1:$ag_cb_port/oauth-callback?code=code-148&state=$ag_state" || true)"
expect_has "$cb_page" -F "Signed in to Google Antigravity" "the callback page did not confirm the sign-in"

# Onboarding answers on its second ask, three seconds after the first.
done_pane="$(wait_pane 20 "$S148" -F "Signed in to Google Antigravity")"
dump "the sign-in completes" "$done_pane"
expect_has "$done_pane" -F "Signed in to Google Antigravity (dev@example.com, Free) — run /model to use it" "the sign-in did not complete into the toast naming the account and plan"
expect_has "$done_pane" -E '^❯' "the composer did not come back after the sign-in"
ag_env="$(cat "$SMOKE_CFG/.env" 2>/dev/null)"
expect_has "$ag_env" -F "ANTIGRAVITY_REFRESH_TOKEN=1//refresh-148" "the Google refresh token was not stored"

# What went over the wire, off the stub's log.
python3 - "$AG_LOG" "$ag_cb_port" <<'PY' || fail "the sign-in's requests were not the expected shapes (see above)"
import json, sys, urllib.parse
entries = [json.loads(line) for line in open(sys.argv[1])]
port = sys.argv[2]
def first(path):
    return next(e for e in entries if e["path"].startswith(path))
exchange = urllib.parse.parse_qs(first("/token")["body"])
assert exchange["grant_type"] == ["authorization_code"], exchange
assert exchange["code"] == ["code-148"], exchange
assert exchange["client_secret"] == ["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"], exchange
assert exchange["code_verifier"][0], exchange
assert exchange["redirect_uri"] == [f"http://localhost:{port}/oauth-callback"], exchange
assert first("/oauth2/v1/userinfo")["authorization"] == "Bearer ya29.smoke-148"
load = first("/v1internal:loadCodeAssist")
assert json.loads(load["body"]) == {"metadata": {"ideType": "ANTIGRAVITY"}}, load
assert load["user_agent"].startswith("antigravity/cli/") and "auth_method=consumer" in load["user_agent"], load
onboards = [e for e in entries if e["path"] == "/v1internal:onboardUser"]
assert len(onboards) == 2, f"onboarding asked {len(onboards)} times"
assert json.loads(onboards[0]["body"])["tier_id"] == "free-tier", onboards[0]
print("sign-in wire: ok")
PY

# /model: the catalog's two levels are one row, its ladder the footer's.
submit "$S148" "/model"
picker="$(wait_pane 10 "$S148" -F "gemini-9-flash")"
dump "the /model picker" "$picker"
expect_has "$picker" -F "gemini-9-flash [antigravity]" "the catalog's level siblings were not one row"
expect_lacks "$picker" -F "gemini-9-flash-low" "a level sibling was listed as a row of its own"
expect_lacks "$picker" -F "chat_20706" "a router slot was listed"
type_text "$S148" "gemini-9-flash"
sleep 0.3
keys "$S148" Enter
wait_for 8 "$S148" -F "Switched model to gemini-9-flash" || fail "the Antigravity model was not selected"
footer="$(wait_pane 5 "$S148" -F "gemini-9-flash high")"
dump "the footer on the grouped model" "$footer"
expect_has "$footer" -F "gemini-9-flash high" "the footer did not carry the ladder's level"

# A turn: the stub's reply reaches the screen, its thought collapsed.
submit "$S148" "$USER_MSG"
reply="$(wait_pane 20 "$S148" -F "Hello from the Antigravity stub, phase 148.")"
dump "the turn's reply" "$reply"
expect_has "$reply" -F "Hello from the Antigravity stub, phase 148." "the stub's reply did not reach the screen"
python3 - "$AG_LOG" <<'PY' || fail "the turn did not go out as the Cloud Code envelope (see above)"
import json, sys
entries = [json.loads(line) for line in open(sys.argv[1])]
turn = [e for e in entries if e["path"] == "/v1internal:streamGenerateContent?alt=sse"][-1]
assert turn["authorization"] == "Bearer ya29.smoke-148", turn["authorization"]
assert turn["user_agent"].startswith("antigravity/cli/"), turn["user_agent"]
body = json.loads(turn["body"])
assert body["project"] == "proj-148", body["project"]
# The project is looked up once — at the sign-in, which set it up — and
# neither /model nor the turn asks again.
loads = [e for e in entries if e["path"] == "/v1internal:loadCodeAssist"]
assert len(loads) == 1, f"loadCodeAssist was asked {len(loads)} times"
assert body["model"] == "gemini-9-flash-high", body["model"]
assert body["requestType"] == "agent" and body["userAgent"] == "antigravity", body
assert body["requestId"].startswith("agent-"), body["requestId"]
request = body["request"]
assert request["generationConfig"]["thinkingConfig"] == {"includeThoughts": True}, request["generationConfig"]
system = request["systemInstruction"]["parts"][0]["text"]
assert "You are Antigravity" not in system, system[:200]
assert request["contents"][-1]["role"] == "user", request["contents"][-1]
assert any("parametersJsonSchema" in d for d in request["tools"][0]["functionDeclarations"])
print("turn wire: ok")
PY

# /usage: the catalog's quota, as a bar under its family.
submit "$S148" "/usage"
usage="$(wait_pane 10 "$S148" -F "Gemini 9 Flash")"
dump "the /usage page" "$usage"
expect_has "$usage" -F "Free" "the usage page did not name the plan"
expect_has "$usage" -F "Gemini 9 Flash" "the usage page did not draw the model's quota"
keys "$S148" Escape
wait_for 3 "$S148" -E '^❯' || fail "/usage did not close"

kill "$AG_SERVER" 2>/dev/null
wait "$AG_SERVER" 2>/dev/null
tmux kill-session -t "$S148" 2>/dev/null
echo "==== Phase 148: Google Antigravity signs in, onboards, lists, streams a turn and reports usage against the stub ===="
