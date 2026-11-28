#!/usr/bin/env bash
# Phase 175 — two sessions on one xAI sign-in: a refresh in the second never strands the first, and rotations land under the provider's own variable

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# xAI's refresh tokens are single-use (docs/xai.md *Two sessions, one
# sign-in*): a refresh rotates the pair and the token presented is spent.
# Two sessions sharing one key store both start from the token it holds, so
# once one of them refreshes, the other's token is spent. The stub here
# refuses a spent refresh token the way xAI does, and its bearers last a
# second, so every request refreshes. What this phase pins:
#
# - the sign-in stores its refresh token under the variable the provider
#   names — a user's own `api_key_env` here, not the shipped name — and every
#   rotation is written back under that variable and never the shipped one;
# - a second session sharing the config home (a headless `-p` run) that
#   refreshes between the first one's requests leaves the first one working:
#   its next refresh reads the store again and presents what the second one
#   wrote, so no spent token is ever presented — off the stub's log;
# - the lock keeping the two refreshes apart is a file beside the store.
S175="${S}_xaisessions"
XS_LOG="$SMOKE_TMP/xai175.log"
XS_LIVE="$SMOKE_TMP/xai175.live"
XS_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$XS_PORT" "$XS_LOG" "$XS_LIVE" <<'PY' &
import base64, json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs

port, log, live_file = int(sys.argv[1]), sys.argv[2], sys.argv[3]
base = f"http://127.0.0.1:{port}"
live = "xai-refresh-175"
issued = 0

def b64(obj):
    return base64.urlsafe_b64encode(json.dumps(obj).encode()).decode().rstrip("=")

ID_TOKEN = f"{b64({'alg': 'none'})}.{b64({'email': 'dev@example.com', 'sub': 'u-175'})}.sig"

def sse(frames):
    return "".join(f"data: {json.dumps(frame)}\n\n" for frame in frames).encode()

class Stub(BaseHTTPRequestHandler):
    def record(self, body, verdict=""):
        with open(log, "a") as f:
            f.write(json.dumps({"method": self.command, "path": self.path,
                                "auth": self.headers.get("authorization") or "",
                                "body": body, "verdict": verdict}) + "\n")

    def do_GET(self):
        self.record("")
        if self.path == "/v1/language-models":
            self.reply(200, {"models": [
                {"id": "grok-4.7", "input_modalities": ["text"], "output_modalities": ["text"],
                 "capabilities": {"reasoning_effort": ["low", "medium", "high"],
                                  "default_reasoning_effort": "medium"}}]})
        elif self.path == "/v1/models":
            self.reply(200, {"object": "list", "data": [
                {"id": "grok-4.7", "object": "model", "context_length": 500000}]})
        else:
            self.reply(404, {"code": "not-found", "error": "Not found"})

    def do_POST(self):
        global live, issued
        body = self.rfile.read(int(self.headers.get("content-length") or 0)).decode()
        if self.path == "/oauth2/device/code":
            self.record(body)
            self.reply(200, {"device_code": "device-175", "user_code": "SMOK-X175",
                             "verification_uri": f"{base}/device",
                             "verification_uri_complete": f"{base}/device?user_code=SMOK-X175",
                             "expires_in": 300, "interval": 1})
        elif self.path == "/oauth2/token" and "grant_type=refresh_token" in body:
            presented = parse_qs(body).get("refresh_token", [""])[0]
            if presented != live:
                # What xAI answers a refresh token already used: spent.
                self.record(body, "spent")
                self.reply(400, {"error": "invalid_grant",
                                 "error_description": "The refresh token has already been used"})
                return
            issued += 1
            live = f"xai-refresh-175-r{issued}"
            with open(live_file, "w") as f:
                f.write(live)
            self.record(body, "rotated")
            # A bearer good for a second: every request refreshes again.
            self.reply(200, {"access_token": f"xai-access-175-{issued}", "refresh_token": live,
                             "expires_in": 1, "token_type": "Bearer"})
        elif self.path == "/oauth2/token":
            self.record(body)
            self.reply(200, {"access_token": "xai-access-175-0", "refresh_token": live,
                             "id_token": ID_TOKEN, "expires_in": 1, "token_type": "Bearer"})
        elif self.path == "/v1/responses":
            self.record(body)
            data = sse([{"type": "response.created", "sequence_number": 0,
                         "response": {"id": "resp_175", "status": "in_progress"}},
                        {"type": "response.output_text.delta", "sequence_number": 1,
                         "delta": "Hello from the Grok stub."},
                        {"type": "response.completed", "sequence_number": 2,
                         "response": {"id": "resp_175", "status": "completed",
                                      "usage": {"input_tokens": 1200, "output_tokens": 6,
                                                "total_tokens": 1206}}}])
            self.send_response(200)
            self.send_header("content-type", "text/event-stream")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        else:
            self.record(body)
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
XS_SERVER=$!
for _ in $(seq 1 30); do # wait for the stub to listen
	if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $XS_PORT)) == 0 else 1)" 2>/dev/null; then
		break
	fi
	sleep 0.1
done
XS_BASE="http://127.0.0.1:$XS_PORT"
# The shipped row, but for its base — and the variable its key is stored
# under, renamed the way a user's own providers.toml may rename it.
XS_PROVIDERS="$SMOKE_TMP/providers-175.toml"
cat >"$XS_PROVIDERS" <<EOF
[providers.xai_grok]
name = "xAI Grok"
auth = "xai_grok"
wire_api = "responses"
description = "Sign in with your xAI account (smoke stub)"
api_key_env = "MY_GROK_TOKEN"

[providers.xai_grok.kwargs]
api_base = "$XS_BASE/v1"
EOF
XS_WORK="$(work_dir)"
# Both sessions run with this environment, from one config home.
XS_ENV="NO_PROXY=127.0.0.1,localhost no_proxy=127.0.0.1,localhost $CFG_ENV_NOHIST ALTER_ZERO_PROVIDERS_FILE=$XS_PROVIDERS ALTER_ZERO_XAI_ISSUER=$XS_BASE"
launch -c "$XS_WORK" "$S175" 110 34 "env $XS_ENV ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"

# Sign in, as Phase 174 does: the stub approves the first poll.
submit "$S175" "/login"
sleep 0.5
keys "$S175" Enter # Sign in with an account
wait_for 5 "$S175" -F "xAI Grok" || fail "the subscription list did not offer the xAI Grok sign-in"
type_text "$S175" "grok"
sleep 0.3
keys "$S175" Enter
signed="$(wait_pane 15 "$S175" -F "Signed in to xAI Grok")"
dump "the sign-in completes" "$signed"
expect_has "$signed" -F "Signed in to xAI Grok (dev@example.com)" "the sign-in did not complete"
xs_env="$(cat "$SMOKE_CFG/.env" 2>/dev/null)"
dump "the key store after signing in" "$xs_env"
expect_has "$xs_env" -F "MY_GROK_TOKEN=xai-refresh-175" "the sign-in was not stored under the provider's own variable"

# /model lists with a bearer the refresh grant minted, which rotates the
# token: the rotation lands under the provider's variable.
submit "$S175" "/model"
wait_for 10 "$S175" -F "grok-4.7 [xai_grok]" || fail "the listing did not offer grok-4.7"
type_text "$S175" "grok-4.7"
sleep 0.3
keys "$S175" Enter
wait_for 5 "$S175" -F "Switched model to grok-4.7" || fail "grok-4.7 was not selected"
xs_env="$(cat "$SMOKE_CFG/.env" 2>/dev/null)"
dump "the key store after the listing" "$xs_env"
expect_has "$xs_env" -F "MY_GROK_TOKEN=xai-refresh-175-r" "the rotation was not written back under the provider's own variable"
expect_lacks "$xs_env" -F "XAI_GROK_REFRESH_TOKEN" "a rotation was written under the shipped name"

# The second session: a headless run from the same config home and the same
# directory. It refreshes too, spending the token the first one holds.
XS_OUT="$SMOKE_TMP/xai175-headless.out"
XS_ERR="$SMOKE_TMP/xai175-headless.err"
(cd "$XS_WORK" && env $XS_ENV ALTER_ZERO_PROVIDER=xai_grok ALTER_ZERO_MODEL=grok-4.7 "$BIN_ABS" -p "hello from the other session" >"$XS_OUT" 2>"$XS_ERR" </dev/null)
xs_code=$?
dump "the second session's run (stdout, then stderr)" "$(cat "$XS_OUT")
---
$(cat "$XS_ERR")"
expect_eq "$xs_code" "0" "the second session's headless run failed"
expect_has "$(cat "$XS_OUT")" -F "Hello from the Grok stub." "the second session did not get its reply"
xs_rotated="$(python3 -c 'import json,sys
print(sum(1 for l in open(sys.argv[1]) if json.loads(l)["verdict"] == "rotated"))' "$XS_LOG")"
dump "refreshes so far" "$xs_rotated"

# Back in the first session: its bearer is long gone, so the turn refreshes
# — and the token it knew is the one the second session spent.
sleep 1.2
submit "$S175" "$USER_MSG"
reply="$(wait_pane 15 "$S175" -F "Hello from the Grok stub.")" || fail "the first session's turn never answered"
dump "the first session's turn after the second session's refresh" "$reply"
expect_lacks "$reply" -F "sign in again" "the first session presented the token the second one spent"
xs_after="$(python3 -c 'import json,sys
print(sum(1 for l in open(sys.argv[1]) if json.loads(l)["verdict"] == "rotated"))' "$XS_LOG")"
[ "$xs_after" -gt "$xs_rotated" ] || fail "the first session's turn did not refresh (rotations: $xs_rotated then $xs_after)"

# Off the stub's log: no refresh ever presented a spent token.
xs_presented="$(python3 -c 'import json,sys
print("\n".join(json.loads(l)["verdict"] + " " + json.loads(l)["body"].split("refresh_token=")[-1]
                for l in open(sys.argv[1]) if "grant_type=refresh_token" in json.loads(l)["body"]))' "$XS_LOG")"
dump "every refresh, in order (verdict, token presented)" "$xs_presented"
expect_lacks "$xs_presented" -F "spent" "a session presented a refresh token already spent"
# The store holds the newest token the stub handed out, under the
# provider's variable, and the lock sits beside it.
xs_env="$(cat "$SMOKE_CFG/.env" 2>/dev/null)"
dump "the key store at the end" "$xs_env"
expect_has "$xs_env" -F "MY_GROK_TOKEN=$(cat "$XS_LIVE")" "the store does not hold the newest refresh token"
expect_lacks "$xs_env" -F "XAI_GROK_REFRESH_TOKEN" "a rotation was written under the shipped name"
[ -e "$SMOKE_CFG/.env.MY_GROK_TOKEN.lock" ] || fail "no credential lock beside the store"

kill "$XS_SERVER" 2>/dev/null
wait "$XS_SERVER" 2>/dev/null
tmux kill-session -t "$S175" 2>/dev/null
echo "==== Phase 175: two sessions share one xAI sign-in without spending each other's refresh token, and rotations land under the provider's own variable ===="
