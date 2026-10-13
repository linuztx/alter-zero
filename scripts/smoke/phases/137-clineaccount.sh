#!/usr/bin/env bash
# Phase 137 — Cline's ACCOUNT sign-in from both its rows, against a local WorkOS/Cline stub

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Cline ships two ways in (docs/cline.md): the pasted key Phase 136 drives,
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
# poll presenting the issued device code, and one registration per sign-in
# carrying that sign-in's WorkOS pair.
#
# Then the same again from the **ClinePass** row, which shares the sign-in
# and the stored token: the sign-in is chosen by the row's auth scheme, so
# this row must reach the very same stub — it once fell through to GitHub
# Copilot's device flow, whose token would have been stored as a Cline
# refresh token and sent to Cline's API.
#
# And last the /model picker, which needs no network either: ClinePass lists
# the plan's models Cline publishes (the stub's list carries one the shipped
# fallback cannot), read with no credential, while the Cline Account row's
# listing needs an access token, and the stub refuses the refresh exactly as
# the live server refuses a dead token — a 400 `invalid_grant` that must read
# as a sign-in to redo, never as the JSON envelope it came in.
S137="${S}_clineaccount"
CC_LOG="$SMOKE_TMP/cline.log"
CC_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$CC_PORT" "$CC_LOG" <<'PY' &
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs

port, log = int(sys.argv[1]), sys.argv[2]
base = f"http://127.0.0.1:{port}"
# One sign-in per row, in the order the phase drives them: each code request
# issues the next code, every code is approved on its second poll, and each
# registration hands back the refresh token named after the WorkOS pair it
# was given — so the key store says which sign-in wrote it.
codes = [("device-137", "SMOK-C137", "137"), ("device-p137", "SMOK-P137", "p137")]
issued = 0
polls = {}

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
        global issued
        body = self.rfile.read(int(self.headers.get("content-length") or 0)).decode()
        with open(log, "a") as f:
            f.write(f"{self.path} {body}\n")
        if self.path == "/user_management/authorize/device":
            device, user, _ = codes[min(issued, len(codes) - 1)]
            issued += 1
            self.reply(200, {"device_code": device, "user_code": user,
                             "verification_uri": f"{base}/device",
                             "verification_uri_complete": f"{base}/device?user_code={user}",
                             "expires_in": 300, "interval": 1})
        elif self.path == "/user_management/authenticate":
            device = parse_qs(body).get("device_code", [""])[0]
            tag = next((t for d, _, t in codes if d == device), None)
            polls[device] = polls.get(device, 0) + 1
            if tag is None:
                self.reply(400, {"error": "invalid_grant"})
            elif polls[device] == 1:
                self.reply(400, {"error": "authorization_pending"})
            else:
                self.reply(200, {"access_token": f"workos-access-{tag}",
                                 "refresh_token": f"workos-refresh-{tag}",
                                 "token_type": "Bearer", "authentication_method": "GoogleOAuth"})
        elif self.path == "/api/v1/auth/refresh":
            # What the live server answers a dead refresh token with.
            self.reply(400, {"data": "", "error": "failed to refresh token: invalid_grant",
                             "success": False})
        elif self.path == "/api/v1/auth/register":
            tag = json.loads(body or "{}").get("accessToken", "").removeprefix("workos-access-")
            self.reply(200, {"success": True, "data": {
                "accessToken": f"workos-access-{tag}", "refreshToken": f"cline-refresh-{tag}",
                "tokenType": "Bearer", "expiresAt": "2999-01-01T00:00:00.000Z",
                "userInfo": {"subject": "u-137", "email": "dev@example.com",
                             "name": "Dev", "clineUserId": "u-137", "accounts": None}}})
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

# Into the subscription half, and onto the Cline Account row by NAME (Phase
# 104's rule: counting Downs would land on whichever subscription was added
# ahead of it). One flow is listed, so Enter opens the device page at once.
submit "$S137" "/login"
sleep 0.5
keys "$S137" Enter # Use a subscription
sleep 0.4
type_text "$S137" "cline account"
sleep 0.3
keys "$S137" Enter
sleep 0.5
device="$(wait_pane 8 "$S137" -F "SMOK-C137")"
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
done_pane="$(wait_pane 15 "$S137" -F "Signed in to Cline Account")"
dump "the sign-in completes" "$done_pane"
expect_has "$done_pane" -F "Signed in to Cline Account (dev@example.com) — run /model to use it" "the sign-in did not complete into the account-naming toast"
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

# Both rows now read configured — one stored token lights the pair — the ✓
# a second /login shows for a stored credential, which is also what the next
# launch's key resolution finds.
submit "$S137" "/login"
sleep 0.5
keys "$S137" Enter
sleep 0.4
type_text "$S137" "cline"
sleep 0.3
again="$(tmux capture-pane -t "$S137" -p)"
dump "the rows after signing in" "$again"
expect_has "$again" -E 'Cline Account · ✔ configured' "the Cline Account row does not read configured after the sign-in"
expect_has "$again" -E 'ClinePass · ✔ configured' "the ClinePass row does not read configured after the sign-in"

# Now the ClinePass row itself (Esc clears the filter first). Its sign-in is
# Cline's, chosen by the row's auth scheme rather than by its id: the page
# must show the stub's SECOND code, never a GitHub one, and complete into a
# toast naming ClinePass.
keys "$S137" Escape
sleep 0.3
type_text "$S137" "clinepass"
sleep 0.3
keys "$S137" Enter
pass_device="$(wait_pane 8 "$S137" -F "SMOK-P137")"
dump "the ClinePass device page" "$pass_device"
for want in "Sign in to ClinePass" "Visit $CC_BASE/device" "Waiting for approval" "c copy code"; do
	expect_has "$pass_device" -F "$want" "the ClinePass device page did not show \"$want\""
done
expect_lacks "$pass_device" -F "github.com" "the ClinePass row ran GitHub Copilot's sign-in"
pass_done="$(wait_pane 15 "$S137" -F "Signed in to ClinePass")"
dump "the ClinePass sign-in completes" "$pass_done"
expect_has "$pass_done" -F "Signed in to ClinePass (dev@example.com) — run /model to use it" "the ClinePass sign-in did not complete into the account-naming toast"

# The second sign-in replaced the first under the one shared variable.
cc_env="$(cat "$SMOKE_CFG/.env" 2>/dev/null)"
dump "the key store after the ClinePass sign-in" "$cc_env"
expect_has "$cc_env" -F "CLINE_ACCOUNT_REFRESH_TOKEN=cline-refresh-p137" "the ClinePass sign-in did not store its Cline refresh token"
expect_lacks "$cc_env" -F "CLINE_ACCOUNT_REFRESH_TOKEN=cline-refresh-137" "the first sign-in's token survived the second"
expect_lacks "$cc_env" -E 'gho_|ghu_|GITHUB' "a GitHub token reached the key store"
cc_log="$(cat "$CC_LOG" 2>/dev/null)"
dump "the stub's log after both sign-ins" "$cc_log"
cc_codes="$(printf '%s\n' "$cc_log" | grep -c '^/user_management/authorize/device ' || true)"
expect_eq "${cc_codes:-0}" "2" "the two rows did not each ask the stub for a code"
cc_pass_register="$(printf '%s\n' "$cc_log" | grep '^/api/v1/auth/register ' | grep -F '"accessToken":"workos-access-p137"' || true)"
expect_has "$cc_pass_register" -F '"refreshToken":"workos-refresh-p137"' "the ClinePass sign-in did not register its own WorkOS pair"

# The /model picker: ClinePass's rows are the stub's published list, and the
# Cline Account row's dead token reads as a sign-in to redo.
submit "$S137" "/model"
picker="$(wait_pane 10 "$S137" -F "cline-pass/smoke-137")"
dump "the /model picker" "$picker"
expect_has "$picker" -F "cline-pass/smoke-137" "ClinePass did not list the plan's published models"
picker="$(wait_pane 10 "$S137" -F "sign in again")"
dump "the /model picker, both fetches settled" "$picker"
expect_has "$picker" -F "Cline Account unavailable" "the Cline Account fetch did not fail on the dead token"
expect_has "$picker" -F "your Cline sign-in is no longer valid" "a dead refresh token did not read as a sign-in to redo"
expect_lacks "$picker" -F '"success"' "the refresh refusal's JSON envelope reached the picker"
cc_log="$(cat "$CC_LOG" 2>/dev/null)"
dump "the stub's log after /model" "$cc_log"
expect_has "$cc_log" -E '^GET /api/v1/ai/cline/recommended-models auth=no$' "the ClinePass list was not fetched, or was sent a credential"
cc_refresh="$(printf '%s\n' "$cc_log" | grep '^/api/v1/auth/refresh ' || true)"
expect_has "$cc_refresh" -F '"refreshToken":"cline-refresh-p137"' "the refresh did not present the stored Cline refresh token"
keys "$S137" Escape

kill "$CC_SERVER" 2>/dev/null
wait "$CC_SERVER" 2>/dev/null
tmux kill-session -t "$S137" 2>/dev/null
echo "==== Phase 137: both Cline rows sign in against the stub, store the Cline refresh token, and list the published ClinePass catalog ===="
