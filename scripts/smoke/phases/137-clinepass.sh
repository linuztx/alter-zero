#!/usr/bin/env bash
# Phase 137 — the ClinePass sign-in: Cline's account flow, its models, and a dead token, against a local stub

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Cline ships two ways in (docs/cline.md): the pasted key Phase 136 drives,
# for Cline's usage-billed models, and **ClinePass** — the subscription — on
# the ACCOUNT sign-in the Cline extension and CLI run: a WorkOS device code,
# then a registration with Cline's own API whose refresh token the key store
# keeps. Both hosts are one stub here, reached through
# ALTER_ZERO_CLINE_AUTH_BASE / ALTER_ZERO_CLINE_API_BASE, answering the
# reference server's own wire shapes. What this phase pins:
#
# - the subscription list offers the Cline sign-in ONCE, as ClinePass (a
#   second row once ran the same flow for the usage-billed catalog);
# - the row reaches the device page, worded for a CODE (Visit, the box,
#   `c copy code`) and never for a browser — the stub's code, never GitHub's
#   (the row once fell through to Copilot's device flow);
# - the sign-in completes into the account-naming toast with the composer
#   back, and the **Cline** refresh token lands in the key store, not the
#   WorkOS one; off the stub's log, the code request names Cline's client id,
#   every poll presents the issued device code, and one registration carries
#   the WorkOS pair;
# - /model lists the plan's models from the list Cline publishes (the stub's
#   carries one the shipped fallback cannot), read with no credential;
# - a turn on a ClinePass model whose refresh token the server no longer
#   honours — the stub refuses it as the live server does, a 400
#   `invalid_grant` — reads as a sign-in to redo, never as the JSON envelope.
#   The refresh fails before any chat request is built, so nothing here
#   reaches a real host.
S137="${S}_clinepass"
CC_LOG="$SMOKE_TMP/cline.log"
CC_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$CC_PORT" "$CC_LOG" <<'PY' &
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port, log = int(sys.argv[1]), sys.argv[2]
base = f"http://127.0.0.1:{port}"
polls = 0

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        auth = "yes" if self.headers.get("authorization") else "no"
        with open(log, "a") as f:
            f.write(f"GET {self.path} auth={auth}\n")
        if self.path == "/api/v1/ai/cline/recommended-models":
            # The live page's shape: four lists, ClinePass's among them.
            self.reply(200, {"recommended": [{"id": "anthropic/claude-sonnet-5", "name": "anthropic/claude-sonnet-5"}],
                             "free": [],
                             "clinePass": [{"id": "cline-pass/smoke-137", "name": "cline-pass/smoke-137",
                                            "description": "", "tags": []},
                                           {"id": "cline-pass/glm-5.3", "name": "cline-pass/glm-5.3",
                                            "description": "", "tags": []}],
                             "clineCloud": []})
        else:
            self.reply(404, {})

    def do_POST(self):
        global polls
        body = self.rfile.read(int(self.headers.get("content-length") or 0)).decode()
        with open(log, "a") as f:
            f.write(f"{self.path} {body}\n")
        if self.path == "/user_management/authorize/device":
            self.reply(200, {"device_code": "device-137", "user_code": "SMOK-C137",
                             "verification_uri": f"{base}/device",
                             "verification_uri_complete": f"{base}/device?user_code=SMOK-C137",
                             "expires_in": 300, "interval": 1})
        elif self.path == "/user_management/authenticate":
            polls += 1
            if polls == 1:
                self.reply(400, {"error": "authorization_pending"})
            else:
                self.reply(200, {"access_token": "workos-access-137",
                                 "refresh_token": "workos-refresh-137",
                                 "token_type": "Bearer", "authentication_method": "GoogleOAuth"})
        elif self.path == "/api/v1/auth/register":
            self.reply(200, {"success": True, "data": {
                "accessToken": "workos-access-137", "refreshToken": "cline-refresh-137",
                "tokenType": "Bearer", "expiresAt": "2999-01-01T00:00:00.000Z",
                "userInfo": {"subject": "u-137", "email": "dev@example.com",
                             "name": "Dev", "clineUserId": "u-137", "accounts": None}}})
        elif self.path == "/api/v1/auth/refresh":
            # What the live server answers a dead refresh token with.
            self.reply(400, {"data": "", "error": "failed to refresh token: invalid_grant",
                             "success": False})
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
CC_SERVER=$!
for _ in $(seq 1 30); do # wait for the stub to listen
	if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $CC_PORT)) == 0 else 1)" 2>/dev/null; then
		break
	fi
	sleep 0.1
done
# NO_PROXY keeps a developer's HTTP proxy out of the loopback requests.
CC_BASE="http://127.0.0.1:$CC_PORT"
launch "$S137" 100 32 "env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 ALTER_ZERO_CLINE_AUTH_BASE=$CC_BASE ALTER_ZERO_CLINE_API_BASE=$CC_BASE $APP"

# Into the subscription half: Cline is ONE row there, ClinePass.
submit "$S137" "/login"
sleep 0.5
keys "$S137" Enter # Use a subscription
subs="$(wait_pane 5 "$S137" -F "ClinePass")"
dump "the subscription list" "$subs"
expect_has "$subs" -F "ClinePass · ◯ unconfigured" "the subscription list did not offer the ClinePass sign-in"
expect_lacks "$subs" -F "Cline Account" "the Cline sign-in was offered twice"

# Onto the row by NAME (Phase 104's rule: counting Downs would land on
# whichever subscription was added ahead of it). One flow is listed, so Enter
# opens the device page at once.
type_text "$S137" "clinepass"
sleep 0.3
keys "$S137" Enter
device="$(wait_pane 8 "$S137" -F "SMOK-C137")"
dump "the device-code page" "$device"
for want in "Sign in to ClinePass" "Visit $CC_BASE/device" "Waiting for approval" "c copy code"; do
	expect_has "$device" -F "$want" "the device page did not show \"$want\""
done
expect_has "$device" -F "╭" "the code is not in its box"
for unwanted in "Waiting for the browser" "this window continues by itself" "c copy link" "login method" "github.com"; do
	expect_lacks "$device" -F "$unwanted" "the device page wore another sign-in's wording (\"$unwanted\")"
done

# The stub approves on the second poll (one a second): the sign-in completes
# into the toast naming the account, and the composer comes back.
done_pane="$(wait_pane 15 "$S137" -F "Signed in to ClinePass")"
dump "the sign-in completes" "$done_pane"
expect_has "$done_pane" -F "Signed in to ClinePass (dev@example.com) — run /model to use it" "the sign-in did not complete into the account-naming toast"
expect_lacks "$done_pane" -F "Waiting for approval" "the device page stayed up after the sign-in completed"
expect_has "$done_pane" -E '^❯' "the composer did not come back after the sign-in"

# What the flow stored: the **Cline** refresh token under the provider's own
# variable, and not the WorkOS one — only the registration's token can mint
# API access, and the WorkOS pair is consumed by the sign-in itself.
cc_env="$(cat "$SMOKE_CFG/.env" 2>/dev/null)"
dump "the key store" "$cc_env"
expect_has "$cc_env" -F "CLINE_ACCOUNT_REFRESH_TOKEN=cline-refresh-137" "the Cline refresh token was not stored"
expect_lacks "$cc_env" -F "workos-refresh-137" "the WorkOS refresh token leaked into the key store"

# And what went over the wire, off the stub's log: the code request naming
# Cline's public client id, at least two polls presenting the issued device
# code, and ONE registration carrying both WorkOS tokens.
cc_log="$(cat "$CC_LOG" 2>/dev/null)"
dump "the stub's log" "$cc_log"
expect_has "$cc_log" -E '^/user_management/authorize/device client_id=client_01K3A541FN8TA3EPPHTD2325AR$' "the code request did not carry Cline's client id"
cc_polls="$(printf '%s\n' "$cc_log" | grep -c '^/user_management/authenticate grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code&device_code=device-137&client_id=' || true)"
if [ "${cc_polls:-0}" -lt 2 ]; then
	fail "expected at least two polls presenting the issued device code, saw ${cc_polls:-0}"
fi
cc_registers="$(printf '%s\n' "$cc_log" | grep -c '^/api/v1/auth/register ' || true)"
expect_eq "${cc_registers:-0}" "1" "the registration was not made exactly once"
cc_register="$(printf '%s\n' "$cc_log" | grep '^/api/v1/auth/register ' || true)"
for want in '"accessToken":"workos-access-137"' '"refreshToken":"workos-refresh-137"'; do
	expect_has "$cc_register" -F "$want" "the registration is missing $want"
done

# The row now reads configured — the ✓ a second /login shows for a stored
# credential, which is also what the next launch's key resolution finds.
submit "$S137" "/login"
sleep 0.5
keys "$S137" Enter
again="$(wait_pane 5 "$S137" -F "ClinePass")"
dump "the row after signing in" "$again"
expect_has "$again" -F "ClinePass · ✔ configured" "the ClinePass row does not read configured after the sign-in"
keys "$S137" Escape # back to the method root
sleep 0.3
keys "$S137" Escape # and out
wait_for 3 "$S137" -E '^❯' || fail "/login did not close"

# /model: the plan's models are the list the stub publishes, read with no
# credential.
submit "$S137" "/model"
picker="$(wait_pane 10 "$S137" -F "cline-pass/smoke-137")"
dump "the /model picker" "$picker"
expect_has "$picker" -F "cline-pass/smoke-137 [cline_pass]" "ClinePass did not list the plan's published models"
cc_log="$(cat "$CC_LOG" 2>/dev/null)"
expect_has "$cc_log" -E '^GET /api/v1/ai/cline/recommended-models auth=no$' "the ClinePass list was not fetched, or was sent a credential"

# Pick one and send a message: the turn mints an access token first, the
# stub refuses the refresh as the live server refuses a dead token, and the
# failure says what to do about it.
type_text "$S137" "smoke-137"
sleep 0.3
keys "$S137" Enter
wait_for 5 "$S137" -F "Switched model to cline-pass/smoke-137" || fail "the ClinePass model was not selected"
submit "$S137" "$USER_MSG"
failed="$(wait_pane 15 "$S137" -F "sign in again")"
dump "a turn on a dead sign-in" "$failed"
expect_has "$failed" -F "your Cline sign-in is no longer valid — run /login and sign in again" "a dead refresh token did not read as a sign-in to redo"
expect_lacks "$failed" -F '"success"' "the refresh refusal's JSON envelope reached the screen"
cc_log="$(cat "$CC_LOG" 2>/dev/null)"
dump "the stub's log after the turn" "$cc_log"
cc_refresh="$(printf '%s\n' "$cc_log" | grep '^/api/v1/auth/refresh ' || true)"
expect_has "$cc_refresh" -F '"refreshToken":"cline-refresh-137"' "the turn did not present the stored Cline refresh token"

kill "$CC_SERVER" 2>/dev/null
wait "$CC_SERVER" 2>/dev/null
tmux kill-session -t "$S137" 2>/dev/null
echo "==== Phase 137: ClinePass signs in against the stub, lists the published plan models, and explains a dead token ===="
