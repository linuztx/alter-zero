#!/usr/bin/env bash
# Phase 174 — xAI Grok: the device-code sign-in, then /model, Ctrl+T, a turn and /usage, against a stub of xAI

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The xAI Grok sign-in (docs/xai.md) end to end, against one local Python
# stub standing in for both of xAI's hosts: the issuer (`auth.x.ai`, reached
# through ALTER_ZERO_XAI_ISSUER) and the API (`api.x.ai/v1`, a smoke provider
# file's `api_base`). The issuer answers RFC 8628's shapes: a device code
# with an `https` verification page, then a pending poll, a `slow_down`
# naming its interval, a 503 error page (a trip that failed, polled again),
# and the grant — an access token whose claims name the account, and a
# refresh token. The API serves `/language-models` (the models that chat,
# their effort lists), `/models` (their windows, beside an image generator),
# and the Responses stream, each only to the bearer a refresh minted.
#
# What this phase pins: the row opening the device page at once (one flow,
# so no method choice), the page worded for a code, the toast naming the
# account, the refresh token in the key store; `/model` listing the chat
# models and not the generator, the footer carrying the default effort and
# the window joined in from `/models`, Ctrl+T stepping the ladder; a turn
# going out on the Responses wire with that effort and the minted bearer; the
# refresh's rotation written back to the store; and `/usage` pointing at the
# Grok dashboard without asking xAI anything.
S174="${S}_xai"
X_LOG="$SMOKE_TMP/xai.log"
X_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$X_PORT" "$X_LOG" <<'PY' &
import base64, json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs

port, log = int(sys.argv[1]), sys.argv[2]
polls = 0
refreshes = 0

def b64url(raw):
    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode()

def jwt(serial):
    # The app reads the payload for the account and never verifies the
    # signature (docs/xai.md).
    claims = {"sub": "user-174", "email": "grok@example.com", "exp": 4102444800, "n": serial}
    return ".".join([b64url(b'{"alg":"none"}'), b64url(json.dumps(claims).encode()), b64url(b"sig")])

SIGN_IN_ACCESS = jwt(0)
MINTED_ACCESS = jwt(1)

LANGUAGE_MODELS = {"models": [
    {"id": "grok-4.7", "object": "model", "owned_by": "xai", "aliases": [],
     "input_modalities": ["text", "image"], "output_modalities": ["text"],
     "capabilities": {"reasoning_effort": ["low", "medium", "high", "xhigh"],
                      "default_reasoning_effort": "high"}},
    {"id": "grok-4.20-0309-reasoning", "object": "model", "owned_by": "xai", "aliases": [],
     "input_modalities": ["text", "image"], "output_modalities": ["text"]},
]}
MODELS = {"object": "list", "data": [
    {"id": "grok-4.7", "aliases": [], "context_length": 500000, "object": "model", "owned_by": "xai"},
    {"id": "grok-4.20-0309-reasoning", "aliases": [], "context_length": 1000000,
     "object": "model", "owned_by": "xai"},
    {"id": "grok-imagine-image", "aliases": [], "context_length": 1024, "object": "model",
     "owned_by": "xai", "image_price": 200000000},
]}

def sse(frames):
    return "".join(f"data: {json.dumps(frame)}\n\n" for frame in frames).encode()

class Stub(BaseHTTPRequestHandler):
    def record(self, body):
        headers = {k.lower(): v for k, v in self.headers.items()}
        with open(log, "a") as f:
            f.write(json.dumps({"method": self.command, "path": self.path,
                                "headers": headers, "body": body}) + "\n")

    def do_POST(self):
        global polls, refreshes
        body = self.rfile.read(int(self.headers.get("content-length") or 0)).decode()
        self.record(body)
        form = parse_qs(body)
        if self.path == "/oauth2/device/code":
            self.reply(200, {"device_code": "device-174", "user_code": "GROK-1740",
                             "verification_uri": "https://accounts.x.ai.invalid/oauth2/device",
                             "verification_uri_complete":
                                 "https://accounts.x.ai.invalid/oauth2/device?user_code=GROK-1740",
                             "expires_in": 600, "interval": 1})
        elif self.path == "/oauth2/token":
            grant = form.get("grant_type", [""])[0]
            if grant == "urn:ietf:params:oauth:grant-type:device_code":
                polls += 1
                if polls == 1:
                    self.reply(400, {"error": "authorization_pending"})
                elif polls == 2:
                    self.reply(400, {"error": "slow_down", "interval": 1})
                elif polls == 3:
                    self.raw(503, "text/html", b"<html>upstream busy</html>")
                else:
                    self.reply(200, {"access_token": SIGN_IN_ACCESS, "refresh_token": "refresh-token-174",
                                     "expires_in": 21600, "token_type": "Bearer"})
            elif grant == "refresh_token":
                refreshes += 1
                self.reply(200, {"access_token": MINTED_ACCESS,
                                 "refresh_token": f"refresh-token-174-r{refreshes}", "expires_in": 21600})
            else:
                self.reply(400, {"error": "unsupported_grant_type"})
        elif self.path == "/v1/responses":
            if self.headers.get("authorization") != f"Bearer {MINTED_ACCESS}":
                self.reply(401, {"error": "invalid_token"})
                return
            frames = [
                {"type": "response.created", "response": {"id": "resp_174", "status": "in_progress"}},
                {"type": "response.output_text.delta", "delta": "Grok here, "},
                {"type": "response.output_text.delta", "delta": "from the stub."},
                {"type": "response.completed", "response": {"status": "completed", "usage": {
                    "input_tokens": 1200, "input_tokens_details": {"cached_tokens": 1024},
                    "output_tokens": 7, "total_tokens": 1207}}},
            ]
            self.raw(200, "text/event-stream", sse(frames))
        else:
            self.reply(404, {})

    def do_GET(self):
        self.record("")
        if self.headers.get("authorization") != f"Bearer {MINTED_ACCESS}":
            self.reply(401, {"error": "invalid_token"})
        elif self.path == "/v1/language-models":
            self.reply(200, LANGUAGE_MODELS)
        elif self.path == "/v1/models":
            self.reply(200, MODELS)
        else:
            self.reply(404, {})

    def reply(self, status, obj):
        self.raw(status, "application/json", json.dumps(obj).encode())

    def raw(self, status, kind, data):
        self.send_response(status)
        self.send_header("content-type", kind)
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
X_SERVER=$!
smoke_on_exit kill "$X_SERVER"
for _ in $(seq 1 30); do # wait for the stub to listen
	if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $X_PORT)) == 0 else 1)" 2>/dev/null; then
		break
	fi
	sleep 0.1
done
X_BASE="http://127.0.0.1:$X_PORT"
# The shipped block, but for its base: the API is the stub.
X_PROVIDERS="$SMOKE_TMP/providers-174.toml"
cat >"$X_PROVIDERS" <<EOF
[providers.xai_grok]
name = "xAI Grok"
auth = "xai_grok"
wire_api = "responses"
description = "Sign in with your Grok subscription (SuperGrok), with a device code"
api_key_env = "XAI_GROK_REFRESH_TOKEN"

[providers.xai_grok.kwargs]
api_base = "$X_BASE/v1"
EOF
X_WORK="$(work_dir)"
# NO_PROXY keeps a developer's HTTP proxy out of the loopback requests.
launch -c "$X_WORK" "$S174" 110 36 "env NO_PROXY=127.0.0.1,localhost no_proxy=127.0.0.1,localhost $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS ALTER_ZERO_PROVIDERS_FILE=$X_PROVIDERS ALTER_ZERO_XAI_ISSUER=$X_BASE $BIN_ABS"

# The row, by name; one flow is listed, so Enter opens the device page at
# once — no method choice.
submit "$S174" "/login"
sleep 0.5
keys "$S174" Enter # Sign in with an account
subs="$(wait_pane 5 "$S174" -F "xAI Grok")"
dump "the subscription list" "$subs"
expect_has "$subs" -F "xAI Grok · ◯ unconfigured" "the subscription list did not offer the xAI sign-in"
type_text "$S174" "grok"
sleep 0.3
keys "$S174" Enter
page="$(wait_pane 8 "$S174" -F "GROK-1740")"
dump "the device-code page" "$page"
for want in "Sign in to xAI Grok" "Visit https://accounts.x.ai.invalid/oauth2/device" \
	"and enter this one-time code" "GROK-1740" "Waiting for approval" "expires in" "c copy code"; do
	expect_has "$page" -F "$want" "the device page did not show \"$want\""
done
expect_has "$page" -F "╭" "the code is not in its box"
for unwanted in "login method" "Waiting for the browser" "c copy link" "user_code=GROK-1740"; do
	expect_lacks "$page" -F "$unwanted" "the device page showed \"$unwanted\""
done

# The stub approves on the fourth poll, one a second: the pending answer,
# the slow-down and the error page all keep the page waiting.
done_pane="$(wait_pane 20 "$S174" -F "Signed in to xAI Grok")"
dump "the sign-in completes" "$done_pane"
expect_has "$done_pane" -F "Signed in to xAI Grok (grok@example.com) — run /model to use it" \
	"the sign-in did not complete into the toast naming the account"
expect_lacks "$done_pane" -F "Waiting for approval" "the device page stayed up after the sign-in"
expect_has "$done_pane" -E '^❯' "the composer did not come back after the sign-in"
x_env="$(cat "$SMOKE_CFG/.env" 2>/dev/null)"
dump "the key store after the sign-in" "$x_env"
expect_has "$x_env" -F "XAI_GROK_REFRESH_TOKEN=refresh-token-174" "the refresh token was not stored"

# /model: the chat models, never the image generator.
submit "$S174" "/model"
picker="$(wait_pane 10 "$S174" -F "grok-4.7")"
dump "the /model picker" "$picker"
expect_has "$picker" -F "grok-4.7 [xai_grok]" "the picker did not list grok-4.7"
expect_has "$picker" -F "grok-4.20-0309-reasoning [xai_grok]" "the picker did not list the effort-less model"
expect_lacks "$picker" -F "grok-imagine-image" "the picker listed an image generator"
type_text "$S174" "grok-4.7"
sleep 0.3
keys "$S174" Enter
wait_for 8 "$S174" -F "Switched model to grok-4.7" || fail "grok-4.7 was not selected"
# The footer: the ladder's default rung (medium, as for every ladder that
# offers it), and the window joined in from /models.
footer="$(wait_pane 5 "$S174" -F "grok-4.7 medium")"
dump "the footer on grok-4.7" "$footer"
expect_has "$footer" -F "grok-4.7 medium" "the footer did not carry the effort ladder's default"
expect_has "$footer" -F "/500k (" "the footer's gauge did not use the window from /models"
# Ctrl+T steps the ladder, and never onto an Off the model has no way to take.
keys "$S174" C-t
wait_for 5 "$S174" -F "grok-4.7 high" || fail "Ctrl+T did not step grok-4.7 to high"

# A turn on the Responses wire.
submit "$S174" "$USER_MSG"
reply="$(wait_pane 20 "$S174" -F "Grok here, from the stub.")"
dump "the turn's reply" "$reply"
expect_has "$reply" -F "Grok here, from the stub." "the stub's reply did not reach the screen"

python3 - "$X_LOG" <<'PY' || fail "the requests were not the expected shapes (see above)"
import json, sys
from urllib.parse import parse_qs
entries = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
def form(entry):
    return {k: v[0] for k, v in parse_qs(entry["body"]).items()}
code = [e for e in entries if e["path"] == "/oauth2/device/code"]
assert len(code) == 1, f"{len(code)} device code requests"
asked = form(code[0])
assert asked["client_id"] == "b1a00492-073a-47ea-816f-4c329264a828", asked
scopes = asked["scope"].split()
for scope in ("openid", "offline_access", "api:access"):
    assert scope in scopes, asked
tokens = [form(e) for e in entries if e["path"] == "/oauth2/token"]
polls = [t for t in tokens if t["grant_type"] == "urn:ietf:params:oauth:grant-type:device_code"]
assert len(polls) == 4, f"{len(polls)} polls"
assert all(p["device_code"] == "device-174" for p in polls), polls
# One refresh, presenting the stored token: the cache serves every request
# after it.
refreshes = [t for t in tokens if t["grant_type"] == "refresh_token"]
assert len(refreshes) == 1, f"{len(refreshes)} refreshes"
assert refreshes[0]["refresh_token"] == "refresh-token-174", refreshes[0]
paths = [e["path"] for e in entries if e["method"] == "GET"]
assert "/v1/language-models" in paths and "/v1/models" in paths, paths
turn = [e for e in entries if e["path"] == "/v1/responses"][-1]
body = json.loads(turn["body"])
assert body["model"] == "grok-4.7", body["model"]
assert body["reasoning"]["effort"] == "high", body.get("reasoning")
assert body["store"] is False and body["stream"] is True, body
assert body["instructions"].strip(), "the system prompt is hoisted into instructions"
assert body.get("prompt_cache_key"), "the cache key routes the conversation"
assert "include" not in body, "encrypted reasoning is not asked for"
print("wire: ok")
PY

# The refresh's rotation was written back over the stored token.
x_env="$(cat "$SMOKE_CFG/.env" 2>/dev/null)"
dump "the key store after the turn" "$x_env"
expect_has "$x_env" -F "XAI_GROK_REFRESH_TOKEN=refresh-token-174-r1" "the rotated refresh token was not written back"

# /usage: a note pointing at the dashboard, and nothing asked of xAI.
gets_before="$(grep -c '"method": "GET"' "$X_LOG" || true)"
submit "$S174" "/usage"
usage="$(wait_pane 10 "$S174" -F "xAI shares no usage figures")"
dump "the /usage page" "$usage"
expect_has "$usage" -F "xAI Grok" "the usage page did not name the subscription"
expect_has "$usage" -F "grok.com/dashboard" "the usage page did not link the Grok dashboard"
gets_after="$(grep -c '"method": "GET"' "$X_LOG" || true)"
expect_eq "$gets_after" "$gets_before" "/usage asked xAI for something"
keys "$S174" Escape
wait_for 3 "$S174" -E '^❯' || fail "/usage did not close"

tmux kill-session -t "$S174" 2>/dev/null
echo "==== Phase 174: xAI Grok signs in with a device code, lists, steps its ladder, streams a turn and points /usage at its dashboard ===="
