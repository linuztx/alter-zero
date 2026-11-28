#!/usr/bin/env bash
# Phase 174 — the xAI Grok sign-in: the device code, the merged listing, Ctrl+T, a turn on the Responses wire, xAI's 403s and /usage, against a local stub

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# xAI Grok (docs/xai.md) signs in with the device code Grok's own CLI runs
# (RFC 8628) and speaks the Responses API on xAI's own host. The auth host
# and the API are one stub here, reached through ALTER_ZERO_XAI_ISSUER and a
# standalone provider file pointing the row's base at it, answering the wire
# shapes the two reference clients pin. What this phase pins:
#
# - the subscription list offers the xAI Grok row, and Enter opens the
#   DEVICE page at once, linking the PREFILLED page (the code baked in) over
#   the code in its box — asking to confirm the code rather than enter it;
# - the code request names the public client id and the scopes and nothing
#   else, and the poll waits an interval before it asks — off the stub's log
#   — and goes on past a proxy's 503, which says nothing about the code;
# - the sign-in completes into the account-naming toast (the id_token's
#   email), and the refresh token lands in the key store;
# - /model lists `/language-models`' rows — not the image model `/models`
#   carries — each with the window `/models` lends it: picked, the footer
#   carries the thinking level and the gauge those give it, and Ctrl+T steps
#   the ladder xAI listed; the listing is fetched with a bearer the refresh
#   grant minted, and the rotated refresh token is written back;
# - a turn streams its reply over `/responses` with the minted bearer, and
#   the request is xAI's shape: instructions, store false, the cache key,
#   the effort Ctrl+T chose, no `include`, and a schema keyword xAI refuses
#   taken out;
# - a 403 refusing the bearer reads as "send your message again" and the
#   next turn mints a new one — presenting the ROTATED refresh token — while
#   a 403 for a missing subscription reads as what to do about it, never as
#   the JSON envelope it came in;
# - /usage names the subscription and links the Grok dashboard, asking xAI
#   for nothing: it publishes no usage figures.
S174="${S}_xai"
XA_LOG="$SMOKE_TMP/xai.log"
XA_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$XA_PORT" "$XA_LOG" <<'PY' &
import base64, json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port, log = int(sys.argv[1]), sys.argv[2]
base = f"http://127.0.0.1:{port}"
polls = 0
refreshes = 0
turns = 0

def b64(obj):
    return base64.urlsafe_b64encode(json.dumps(obj).encode()).decode().rstrip("=")

# An unsigned id_token: the client reads the email and checks no signature.
ID_TOKEN = f"{b64({'alg': 'none'})}.{b64({'email': 'dev@example.com', 'sub': 'u-174'})}.sig"
PERMISSION = "The caller does not have permission to execute the specified operation"

def sse(frames):
    return "".join(f"data: {json.dumps(frame)}\n\n" for frame in frames).encode()

class Stub(BaseHTTPRequestHandler):
    def record(self, body):
        with open(log, "a") as f:
            f.write(json.dumps({"method": self.command, "path": self.path,
                                "auth": self.headers.get("authorization") or "",
                                "body": body}) + "\n")

    def do_GET(self):
        self.record("")
        if self.path == "/v1/language-models":
            self.reply(200, {"models": [
                {"id": "grok-4.7", "input_modalities": ["text", "image"], "output_modalities": ["text"],
                 "capabilities": {"reasoning_effort": ["low", "medium", "high", "xhigh"],
                                  "default_reasoning_effort": "high"}},
                {"id": "grok-4.3", "input_modalities": ["text"], "output_modalities": ["text"],
                 "capabilities": {"reasoning_effort": ["none", "low", "medium"],
                                  "default_reasoning_effort": "low"}}]})
        elif self.path == "/v1/models":
            self.reply(200, {"object": "list", "data": [
                {"id": "grok-4.7", "object": "model", "context_length": 500000},
                {"id": "grok-4.3", "object": "model", "context_length": 1000000},
                {"id": "grok-imagine-image", "object": "model", "context_length": None}]})
        else:
            self.reply(404, {"code": "not-found", "error": "Not found"})

    def do_POST(self):
        global polls, refreshes, turns
        body = self.rfile.read(int(self.headers.get("content-length") or 0)).decode()
        self.record(body)
        if self.path == "/oauth2/device/code":
            self.reply(200, {"device_code": "device-174", "user_code": "SMOK-X174",
                             "verification_uri": f"{base}/device",
                             "verification_uri_complete": f"{base}/device?user_code=SMOK-X174",
                             "expires_in": 300, "interval": 1})
        elif self.path == "/oauth2/token" and "grant_type=refresh_token" in body:
            refreshes += 1
            # Single-use: every refresh rotates the pair.
            self.reply(200, {"access_token": f"xai-access-174-{refreshes}",
                             "refresh_token": f"xai-refresh-174-r{refreshes}",
                             "expires_in": 3600, "token_type": "Bearer"})
        elif self.path == "/oauth2/token":
            polls += 1
            if polls == 1:
                self.reply(400, {"error": "authorization_pending"})
            elif polls == 2:
                # A proxy failing in front of the token endpoint: no answer
                # about the code, which is still good.
                page = b"<html>503 Service Unavailable</html>"
                self.send_response(503)
                self.send_header("content-type", "text/html")
                self.send_header("content-length", str(len(page)))
                self.end_headers()
                self.wfile.write(page)
            else:
                self.reply(200, {"access_token": "xai-access-174-0", "refresh_token": "xai-refresh-174",
                                 "id_token": ID_TOKEN, "expires_in": 3600, "token_type": "Bearer"})
        elif self.path == "/v1/responses":
            turns += 1
            if turns == 1:
                data = sse([{"type": "response.created", "sequence_number": 0,
                             "response": {"id": "resp_174", "status": "in_progress"}},
                            {"type": "response.output_text.delta", "sequence_number": 1,
                             "delta": "Hello from the Grok stub."},
                            {"type": "response.completed", "sequence_number": 2,
                             "response": {"id": "resp_174", "status": "completed",
                                          "usage": {"input_tokens": 1200, "output_tokens": 6,
                                                    "total_tokens": 1206,
                                                    "input_tokens_details": {"cached_tokens": 0},
                                                    "output_tokens_details": {"reasoning_tokens": 0}}}}])
                self.send_response(200)
                self.send_header("content-type", "text/event-stream")
                self.send_header("content-length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)
            elif turns == 2:
                # What xAI answers an access token it can no longer validate.
                self.reply(403, {"code": PERMISSION,
                                 "error": "The OAuth2 access token could not be validated. [WKE=unauthenticated:bad-credentials]"})
            else:
                self.reply(403, {"code": PERMISSION,
                                 "error": "You have either run out of available resources or do not have an active Grok subscription. Manage subscriptions at https://grok.com"})
        else:
            self.reply(404, {"code": "not-found", "error": "Not found"})

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
XA_SERVER=$!
for _ in $(seq 1 30); do # wait for the stub to listen
	if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $XA_PORT)) == 0 else 1)" 2>/dev/null; then
		break
	fi
	sleep 0.1
done
XA_BASE="http://127.0.0.1:$XA_PORT"
# The row as shipped, but for its base: xAI's API is the stub.
XA_PROVIDERS="$SMOKE_TMP/providers-174.toml"
cat >"$XA_PROVIDERS" <<EOF
[providers.xai_grok]
name = "xAI Grok"
auth = "xai_grok"
wire_api = "responses"
description = "Sign in with your xAI account (smoke stub)"
api_key_env = "XAI_GROK_REFRESH_TOKEN"

[providers.xai_grok.kwargs]
api_base = "$XA_BASE/v1"
EOF
# A short cwd (`~/work`): the footer lays the cwd out ahead of the context
# gauge this phase reads, and truncates from the right.
XA_WORK="$(work_dir)"
# NO_PROXY keeps a developer's HTTP proxy out of the loopback requests.
launch -c "$XA_WORK" "$S174" 110 34 "env NO_PROXY=127.0.0.1,localhost no_proxy=127.0.0.1,localhost $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS ALTER_ZERO_PROVIDERS_FILE=$XA_PROVIDERS ALTER_ZERO_XAI_ISSUER=$XA_BASE $BIN_ABS"

# Into the subscription half; onto the row by name (counting Downs would land
# on whichever subscription was added ahead of it). One flow is listed, so
# Enter opens the device page at once.
submit "$S174" "/login"
sleep 0.5
keys "$S174" Enter # Sign in with an account
subs="$(wait_pane 5 "$S174" -F "xAI Grok")"
dump "the subscription list" "$subs"
expect_has "$subs" -F "xAI Grok · ◯ unconfigured" "the subscription list did not offer the xAI Grok sign-in"
type_text "$S174" "grok"
sleep 0.3
keys "$S174" Enter
device="$(wait_pane 8 "$S174" -F "SMOK-X174")"
dump "the device-code page" "$device"
for want in "Sign in to xAI Grok" "Visit $XA_BASE/device?user_code=SMOK-X174" "and confirm this one-time code" "Waiting for approval" "c copy code"; do
	expect_has "$device" -F "$want" "the device page did not show \"$want\""
done
expect_has "$device" -F "╭" "the code is not in its box"
# The link carries the code, so the page asks to confirm it, not enter it.
for unwanted in "enter this one-time code" "Waiting for the browser" "this window continues by itself" "login method"; do
	expect_lacks "$device" -F "$unwanted" "the device page wore another sign-in's wording (\"$unwanted\")"
done

# The stub approves on the third poll, one interval apart, after a pending
# and a 503: the sign-in completes into the toast naming the id_token's
# account.
done_pane="$(wait_pane 15 "$S174" -F "Signed in to xAI Grok")"
dump "the sign-in completes" "$done_pane"
expect_has "$done_pane" -F "Signed in to xAI Grok (dev@example.com) — run /model to use it" "the sign-in did not complete into the account-naming toast"
expect_has "$done_pane" -E '^❯' "the composer did not come back after the sign-in"
xa_env="$(cat "$SMOKE_CFG/.env" 2>/dev/null)"
dump "the key store after signing in" "$xa_env"
expect_has "$xa_env" -F "XAI_GROK_REFRESH_TOKEN=xai-refresh-174" "the refresh token was not stored"

# Off the stub's log: the code request names the client id and the scopes
# and nothing else, and every poll — the one the 503 answered too — presents
# the device-code grant.
xa_log="$(cat "$XA_LOG" 2>/dev/null)"
dump "the stub's log after the sign-in" "$xa_log"
xa_code_body="$(python3 -c 'import json,sys
for line in open(sys.argv[1]):
    r = json.loads(line)
    if r["path"] == "/oauth2/device/code":
        print(r["body"]); break' "$XA_LOG")"
expect_eq "$xa_code_body" "client_id=b1a00492-073a-47ea-816f-4c329264a828&scope=openid%20profile%20email%20offline_access%20grok-cli%3Aaccess%20api%3Aaccess" "the code request was not the client id and the scopes alone"
xa_polls="$(python3 -c 'import json,sys
want = "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code&client_id=b1a00492-073a-47ea-816f-4c329264a828&device_code=device-174"
print(sum(1 for line in open(sys.argv[1]) if json.loads(line)["body"] == want))' "$XA_LOG")"
expect_eq "$xa_polls" "3" "the poll did not present the device-code grant once per interval until the approval"

# /model: the merged listing — the rows `/language-models` names, each with
# the window `/models` lends it, and no row for the image model.
submit "$S174" "/model"
picker="$(wait_pane 10 "$S174" -F "grok-4.7 [xai_grok]")"
dump "the /model picker" "$picker"
expect_has "$picker" -F "grok-4.3 [xai_grok]" "the listing did not offer grok-4.3"
expect_lacks "$picker" -F "grok-imagine-image" "an image model /models carries was listed as a chat model"
xa_log="$(cat "$XA_LOG" 2>/dev/null)"
expect_has "$xa_log" -F '"path": "/v1/language-models", "auth": "Bearer xai-access-174-1"' "the listing was not fetched with the bearer the refresh grant minted"
expect_has "$xa_log" -F '"path": "/v1/models", "auth": "Bearer xai-access-174-1"' "/models was not read for the windows"
xa_env="$(cat "$SMOKE_CFG/.env" 2>/dev/null)"
expect_has "$xa_env" -F "XAI_GROK_REFRESH_TOKEN=xai-refresh-174-r1" "the rotated refresh token was not written back"

# Pick grok-4.7: its thinking level and window are the listings' — the footer
# shows the level beside the model and gauges against the window.
type_text "$S174" "grok-4.7"
sleep 0.3
keys "$S174" Enter
wait_for 5 "$S174" -F "Switched model to grok-4.7" || fail "grok-4.7 was not selected"
footer="$(wait_pane 5 "$S174" -F "/500k")"
dump "the footer on grok-4.7" "$footer"
expect_has "$footer" -F "grok-4.7 medium" "the footer did not carry the ladder's thinking level"
expect_has "$footer" -E "[0-9.]+k?/500k" "the footer did not gauge against the lent window"
# Ctrl+T steps the ladder xAI listed — medium to high — and the turn sends it.
keys "$S174" C-t
wait_for 5 "$S174" -F "grok-4.7 high" || fail "Ctrl+T did not step grok-4.7 to high"

# A turn: the reply streams back over `/responses`, with the cached bearer.
submit "$S174" "$USER_MSG"
reply="$(wait_pane 15 "$S174" -F "Hello from the Grok stub.")"
dump "a turn on grok-4.7" "$reply"
expect_has "$reply" -F "Hello from the Grok stub." "the streamed reply did not reach the screen"
xa_request="$(python3 -c 'import json,sys
for line in open(sys.argv[1]):
    r = json.loads(line)
    if r["path"] == "/v1/responses":
        print(json.dumps({"auth": r["auth"], **json.loads(r["body"])})); break' "$XA_LOG")"
dump "the first /responses request (abridged)" "$(printf '%s' "$xa_request" | cut -c1-600)"
python3 - "$xa_request" <<'PY' || fail "the /responses request was not xAI's shape (see above)"
import json, sys
r = json.loads(sys.argv[1])
problems = []
if r.get("auth") != "Bearer xai-access-174-1": problems.append(f"bearer {r.get('auth')!r} is not the cached one")
if r.get("model") != "grok-4.7": problems.append("model")
if r.get("store") is not False: problems.append("store is not false")
if not isinstance(r.get("instructions"), str) or not r["instructions"]: problems.append("no instructions")
if not r.get("prompt_cache_key"): problems.append("no prompt_cache_key")
if r.get("reasoning") != {"effort": "high", "summary": "auto"}: problems.append(f"reasoning {r.get('reasoning')!r}")
if "include" in r: problems.append("include was asked for")
def walk(node):
    if isinstance(node, dict):
        for key in ("pattern", "format"):
            if key in node and isinstance(node[key], str): yield key
        for value in node.get("properties", {}).values(): yield from walk(value)
        for key in ("items", "additionalProperties"):
            if isinstance(node.get(key), dict): yield from walk(node[key])
        for key in ("anyOf", "oneOf", "allOf"):
            for value in node.get(key, []): yield from walk(value)
for tool in r.get("tools", []):
    for keyword in walk(tool.get("parameters", {})):
        problems.append(f"tool {tool.get('name')} kept a {keyword} keyword")
if problems:
    print("; ".join(problems)); sys.exit(1)
PY

# The second turn: xAI rejects the bearer. The advice says to send again —
# and the cached bearer was dropped, so the next turn mints a new one.
submit "$S174" "and again"
stale="$(wait_pane 15 "$S174" -F "send your message again")"
dump "a turn on a rejected bearer" "$stale"
expect_has "$stale" -F "xAI no longer accepts this sign-in's access token" "a rejected bearer did not read as one to send again"
expect_lacks "$stale" -F "WKE=" "the refusal's envelope reached the screen"

# The third turn mints afresh — presenting the ROTATED refresh token, the
# first being spent — and meets the missing subscription.
submit "$S174" "once more"
unsub="$(wait_pane 15 "$S174" -F "no active Grok subscription")"
dump "a turn without a subscription" "$unsub"
expect_has "$unsub" -F "grok.com/?_s=usage" "a missing subscription did not say where to look"
expect_lacks "$unsub" -F '"code"' "the refusal's JSON envelope reached the screen"
xa_refreshes="$(python3 -c 'import json,sys
print("|".join(json.loads(l)["body"].split("refresh_token=")[-1] for l in open(sys.argv[1]) if "grant_type=refresh_token" in json.loads(l)["body"]))' "$XA_LOG")"
dump "the refresh tokens presented, in order" "$xa_refreshes"
expect_eq "$xa_refreshes" "xai-refresh-174|xai-refresh-174-r1" "the turn after a rejected bearer did not mint afresh with the rotated refresh token"
xa_bearers="$(python3 -c 'import json,sys
print("|".join(json.loads(l)["auth"] for l in open(sys.argv[1]) if json.loads(l)["path"] == "/v1/responses"))' "$XA_LOG")"
expect_eq "$xa_bearers" "Bearer xai-access-174-1|Bearer xai-access-174-1|Bearer xai-access-174-2" "the rejected bearer was sent again instead of a new one"

# /usage: xAI publishes no usage figures, so the page names the subscription
# and links the dashboard — and asks xAI for nothing, not even a bearer.
xa_asked_before="$(wc -l <"$XA_LOG")"
submit "$S174" "/usage"
usage="$(wait_pane 10 "$S174" -F "xAI publishes no usage figures")"
dump "the /usage page" "$usage"
expect_has "$usage" -F "xAI Grok" "the usage page did not name the subscription"
expect_has "$usage" -F "grok.com/?_s=usage" "the usage page did not link the Grok dashboard"
sleep 0.5
xa_asked_after="$(wc -l <"$XA_LOG")"
expect_eq "$xa_asked_after" "$xa_asked_before" "/usage asked xAI for something"
keys "$S174" Escape
wait_for 3 "$S174" -E '^❯' || fail "/usage did not close"

kill "$XA_SERVER" 2>/dev/null
wait "$XA_SERVER" 2>/dev/null
tmux kill-session -t "$S174" 2>/dev/null
echo "==== Phase 174: xAI Grok signs in past a 503, lists the merged catalog, steps its ladder, streams a turn, explains its 403s and points /usage at the dashboard ===="
