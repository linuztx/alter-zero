#!/usr/bin/env bash
# Phase 136 — Cline's ACCOUNT sign-in: the subscription row, the device page, and
# the WorkOS/register wire against a local stub

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Cline ships two ways in (docs/cline.md): the pasted key Phase 135 drives,
# and the ACCOUNT sign-in the Cline extension and CLI run — a WorkOS device
# code, then a registration with Cline's own API whose refresh token the key
# store keeps. Both hosts are one stub here, reached through
# ALTER_ZERO_CLINE_AUTH_BASE / ALTER_ZERO_CLINE_API_BASE, answering the
# reference server's own wire shapes: a code request, one
# `authorization_pending` poll, then the WorkOS pair, then the `{success,
# data}` registration naming the account. What this phase pins: the row
# reaching the device page from the subscription half, the page worded for a
# CODE (Visit, the box, `c copy code`) and never for a browser, the sign-in
# completing into the account-naming toast with the composer back, the
# **Cline** refresh token landing in the key store (not the WorkOS one), and
# — off the stub's log — the code request naming Cline's client id, every
# poll presenting the issued device code, and exactly one registration
# carrying the WorkOS pair.
S136="${S}_clineaccount"
CC_LOG="$SMOKE_TMP/cline.log"
CC_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$CC_PORT" "$CC_LOG" <<'PY' &
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port, log = int(sys.argv[1]), sys.argv[2]
polls = 0
base = f"http://127.0.0.1:{port}"

class Stub(BaseHTTPRequestHandler):
    def do_POST(self):
        global polls
        body = self.rfile.read(int(self.headers.get("content-length") or 0)).decode()
        with open(log, "a") as f:
            f.write(f"{self.path} {body}\n")
        if self.path == "/user_management/authorize/device":
            self.reply(200, {"device_code": "device-136", "user_code": "SMOK-C136",
                             "verification_uri": f"{base}/device",
                             "verification_uri_complete": f"{base}/device?user_code=SMOK-C136",
                             "expires_in": 300, "interval": 1})
        elif self.path == "/user_management/authenticate":
            polls += 1
            if polls == 1:
                self.reply(400, {"error": "authorization_pending"})
            else:
                self.reply(200, {"access_token": "workos-access-136",
                                 "refresh_token": "workos-refresh-136",
                                 "token_type": "Bearer", "authentication_method": "GoogleOAuth"})
        elif self.path == "/api/v1/auth/register":
            self.reply(200, {"success": True, "data": {
                "accessToken": "workos-access-136", "refreshToken": "cline-refresh-136",
                "tokenType": "Bearer", "expiresAt": "2999-01-01T00:00:00.000Z",
                "userInfo": {"subject": "u-136", "email": "dev@example.com",
                             "name": "Dev", "clineUserId": "u-136", "accounts": None}}})
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
launch "$S136" 100 32 "env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 ALTER_ZERO_CLINE_AUTH_BASE=$CC_BASE ALTER_ZERO_CLINE_API_BASE=$CC_BASE $APP"

# Into the subscription half, and onto the Cline Account row by NAME (Phase
# 104's rule: counting Downs would land on whichever subscription was added
# ahead of it). One flow is listed, so Enter opens the device page at once.
submit "$S136" "/login"
sleep 0.5
keys "$S136" Enter # Use a subscription
sleep 0.4
type_text "$S136" "cline"
sleep 0.3
keys "$S136" Enter
sleep 0.5
device="$(wait_pane 8 "$S136" -F "SMOK-C136")"
dump "the device-code page" "$device"
for want in "Sign in to Cline Account" "Visit $CC_BASE/device" "Waiting for approval" "c copy code"; do
	expect_has "$device" -F "$want" "the device page did not show \"$want\""
done
expect_has "$device" -F "╭" "the code is not in its box"
for unwanted in "Waiting for the browser" "this window continues by itself" "c copy link" "login method"; do
	expect_lacks "$device" -F "$unwanted" "the device page wore the browser page's wording (\"$unwanted\")"
done

# The stub approves on the second poll (one a second): the sign-in completes
# into the toast naming the account, and the composer comes back.
done_pane="$(wait_pane 15 "$S136" -F "Signed in to Cline Account")"
dump "the sign-in completes" "$done_pane"
expect_has "$done_pane" -F "Signed in to Cline Account (dev@example.com) — run /model to use it" "the sign-in did not complete into the account-naming toast"
expect_lacks "$done_pane" -F "Waiting for approval" "the device page stayed up after the sign-in completed"
expect_has "$done_pane" -E '^❯' "the composer did not come back after the sign-in"

# What the flow stored: the **Cline** refresh token under the provider's own
# variable, and not the WorkOS one — only the registration's token can mint
# API access, and the WorkOS pair is consumed by the sign-in itself.
cc_env="$(cat "$SMOKE_CFG/.env" 2>/dev/null)"
dump "the key store" "$cc_env"
expect_has "$cc_env" -F "CLINE_ACCOUNT_REFRESH_TOKEN=cline-refresh-136" "the Cline refresh token was not stored"
expect_lacks "$cc_env" -F "workos-refresh-136" "the WorkOS refresh token leaked into the key store"

# And what went over the wire, off the stub's log: the code request naming
# Cline's public client id, at least two polls presenting the issued device
# code, and ONE registration carrying both WorkOS tokens.
cc_log="$(cat "$CC_LOG" 2>/dev/null)"
dump "the stub's log" "$cc_log"
expect_has "$cc_log" -E '^/user_management/authorize/device client_id=client_01K3A541FN8TA3EPPHTD2325AR$' "the code request did not carry Cline's client id"
cc_polls="$(printf '%s\n' "$cc_log" | grep -c '^/user_management/authenticate grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code&device_code=device-136&client_id=' || true)"
if [ "${cc_polls:-0}" -lt 2 ]; then
	fail "expected at least two polls presenting the issued device code, saw ${cc_polls:-0}"
fi
cc_registers="$(printf '%s\n' "$cc_log" | grep -c '^/api/v1/auth/register ' || true)"
expect_eq "${cc_registers:-0}" "1" "the registration was not made exactly once"
cc_register="$(printf '%s\n' "$cc_log" | grep '^/api/v1/auth/register ' || true)"
for want in '"accessToken":"workos-access-136"' '"refreshToken":"workos-refresh-136"'; do
	expect_has "$cc_register" -F "$want" "the registration is missing $want"
done

# The row now reads configured — the ✓ a second /login shows for a stored
# credential, which is also what the next launch's key resolution finds.
submit "$S136" "/login"
sleep 0.5
keys "$S136" Enter
sleep 0.4
type_text "$S136" "cline"
sleep 0.3
again="$(tmux capture-pane -t "$S136" -p)"
dump "the row after signing in" "$again"
expect_has "$again" -E 'Cline Account · ✔ configured' "the Cline Account row does not read configured after the sign-in"

kill "$CC_SERVER" 2>/dev/null
wait "$CC_SERVER" 2>/dev/null
tmux kill-session -t "$S136" 2>/dev/null
echo "==== Phase 136: the Cline account sign-in opens, completes against the stub, and stores the Cline refresh token ===="
