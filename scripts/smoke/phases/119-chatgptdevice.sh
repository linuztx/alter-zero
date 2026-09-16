#!/usr/bin/env bash
# Phase 119 — ChatGPT Codex's sign-in METHOD choice, and its DEVICE-CODE flow

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# ChatGPT Codex offers two ways in (docs/chatgpt.md): the browser PKCE flow
# Phase 104 drives, and — for a headless machine — Codex's own device code.
# Enter on its subscription row therefore opens a CHOICE rather than a page,
# and the device row runs a flow that talks to OpenAI's auth server: a code
# request, a poll until the user approves the code on OpenAI's page, then the
# token exchange. A local Python stub stands in for that server, reached
# through ALTER_ZERO_OPENAI_ISSUER, answering Codex's own wire shapes: the
# code with a STRING interval, two "not yet" polls (a 403 and then a 404 —
# the reference reads either as pending), then the grant carrying the
# server-minted PKCE verifier, and a token set whose access token claims a
# `pro` plan. What this phase pins: the choice's exact shape (a title naming
# the subscription, the two rows with the browser row marked as the default,
# the root's hint), Esc stepping back to the subscription list, the device
# page worded for a CODE (`Visit`, the box, `Waiting for approval`, `c copy
# code`) and never for a browser, the sign-in completing into the `Signed in
# to ChatGPT Codex (Pro)` toast with the composer back, the refresh token
# landing in the key store — and, off the stub's log, the poll presenting the
# pair the code request issued and the exchange repeating the SERVER's
# verifier at the device callback rather than a loopback port of our own.
S119="${S}_chatgptdevice"
DC_LOG="$SMOKE_TMP/device.log"
DC_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$DC_PORT" "$DC_LOG" <<'PY' &
import base64, json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port, log = int(sys.argv[1]), sys.argv[2]
polls = 0

def b64url(raw):
    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode()

# An access token whose claims name the seat; the app never verifies the
# signature (docs/chatgpt.md), only reads the payload.
claims = {
    "https://api.openai.com/auth": {"chatgpt_account_id": "acct-119", "chatgpt_plan_type": "pro"},
    "exp": 4102444800,
}
jwt = ".".join([b64url(b'{"alg":"none"}'), b64url(json.dumps(claims).encode()), b64url(b"sig")])

class Stub(BaseHTTPRequestHandler):
    def do_POST(self):
        global polls
        body = self.rfile.read(int(self.headers.get("content-length") or 0)).decode()
        with open(log, "a") as f:
            f.write(f"{self.path} {self.headers.get('originator') or '-'} {body}\n")
        if self.path == "/api/accounts/deviceauth/usercode":
            self.reply(200, {"device_auth_id": "device-auth-119", "user_code": "SMOK-E119", "interval": "1"})
        elif self.path == "/api/accounts/deviceauth/token":
            polls += 1
            if polls == 1:
                self.reply(403, {})
            elif polls == 2:
                self.reply(404, {"error": "not yet"})
            else:
                self.reply(200, {"authorization_code": "poll-code-119", "code_challenge": "cc-119", "code_verifier": "code-verifier-119"})
        elif self.path == "/oauth/token":
            self.reply(200, {"id_token": jwt, "access_token": jwt, "refresh_token": "refresh-token-119"})
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
DC_SERVER=$!
for _ in $(seq 1 30); do # wait for the stub to listen
	if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $DC_PORT)) == 0 else 1)" 2>/dev/null; then
		break
	fi
	sleep 0.1
done
# NO_PROXY keeps a developer's HTTP proxy out of the loopback requests.
DC_ISSUER="http://127.0.0.1:$DC_PORT"
launch "$S119" 100 32 "env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 ALTER_ZERO_OPENAI_ISSUER=$DC_ISSUER $APP"

# Into the subscription list, and onto the ChatGPT Codex row by NAME (Phase
# 104's rule: the list is alphabetical by provider id, so counting Downs would
# land on whichever subscription was added ahead of it).
open_chatgpt_row() {
	type_text "$S119" "chatgpt"
	sleep 0.3
	keys "$S119" Enter
	sleep 0.5
}
submit "$S119" "/login"
sleep 0.5
keys "$S119" Enter # Use a subscription
sleep 0.4
open_chatgpt_row
method="$(tmux capture-pane -t "$S119" -p)"
dump "the sign-in method choice" "$method"
for want in "Select ChatGPT Codex login method:" "Browser login (default)" \
	"Device code login (headless)" "enter select" "escape/ctrl+c cancel"; do
	expect_has "$method" -F "$want" "the method choice did not show \"$want\""
done
# The browser row is the default and opens highlighted; the choice is a
# question, not a list — no filter, no counter — and it opened no page.
expect_has "$method" -E '^ *→ Browser login \(default\)' "the browser row is not the highlighted default"
expect_lacks "$method" -F "(1/2)" "the method choice carries a counter"
expect_lacks "$method" -F "❯" "the method choice carries a filter"
for unwanted in "Waiting for" "Requesting a code" "Sign in to ChatGPT Codex"; do
	expect_lacks "$method" -F "$unwanted" "Enter on the row opened a page instead of the choice (\"$unwanted\")"
done

# Esc steps BACK to the subscription list, not out of the flow.
keys "$S119" Escape
sleep 0.4
back="$(tmux capture-pane -t "$S119" -p)"
dump "Esc on the choice" "$back"
expect_has "$back" -F "ChatGPT Codex" "Esc on the choice did not return to the subscription list"
expect_has "$back" -F "enter sign in" "Esc on the choice did not return to the subscription list"
expect_lacks "$back" -F "login method" "Esc left the choice up"

# Back in, Down onto the device row, Enter: the device page, worded for a
# code and never for a browser, with the stub's code in its box.
open_chatgpt_row
keys "$S119" Down
sleep 0.2
moved="$(tmux capture-pane -t "$S119" -p)"
expect_has "$moved" -E '^ *→ Device code login \(headless\)' "Down did not move the highlight to the device row"
keys "$S119" Enter
device="$(wait_pane 8 "$S119" -F "SMOK-E119")"
dump "the device-code page" "$device"
for want in "Sign in to ChatGPT Codex" "Visit $DC_ISSUER/codex/device" \
	"and enter this one-time code" "SMOK-E119" "Waiting for approval" \
	"expires in 1" "c copy code"; do
	expect_has "$device" -F "$want" "the device page did not show \"$want\""
done
expect_has "$device" -F "╭" "the code is not in its box"
for unwanted in "Waiting for the browser" "this window continues by itself" "c copy link" "oauth/authorize"; do
	expect_lacks "$device" -F "$unwanted" "the device page wore the browser page's wording (\"$unwanted\")"
done

# The stub approves on the third poll (one a second): the sign-in completes
# into the toast naming the seat, and the composer comes back.
done_pane="$(wait_pane 15 "$S119" -F "Signed in to ChatGPT Codex")"
dump "the sign-in completes" "$done_pane"
expect_has "$done_pane" -F "Signed in to ChatGPT Codex (Pro) — run /model to use it" "the sign-in did not complete into the seat-naming toast"
expect_lacks "$done_pane" -F "Waiting for approval" "the device page stayed up after the sign-in completed"
expect_has "$done_pane" -E '^❯' "the composer did not come back after the sign-in"

# What the flow stored: the refresh token, under the provider's own variable
# in the same key store a pasted key lands in — nothing else.
dc_env="$(cat "$SMOKE_CFG/.env" 2>/dev/null)"
dump "the key store" "$dc_env"
expect_has "$dc_env" -F "OPENAI_CHATGPT_REFRESH_TOKEN=refresh-token-119" "the refresh token was not stored"
expect_lacks "$dc_env" -F "poll-code-119" "an authorization code leaked into the key store"

# And what went over the wire, off the stub's log: the code request naming
# the client id (with Codex's originator riding it), at least three polls
# presenting the pair the code request issued, and ONE exchange at the DEVICE
# callback with the server's own verifier — never a loopback redirect, since
# no port was ever opened.
dc_log="$(cat "$DC_LOG" 2>/dev/null)"
dump "the stub's log" "$dc_log"
expect_has "$dc_log" -E '^/api/accounts/deviceauth/usercode codex_cli_rs .*"client_id":"app_EMoamEEZ73f0CkXaXp7hrann"' "the code request did not carry the client id and the originator"
dc_polls="$(printf '%s\n' "$dc_log" | grep -c '^/api/accounts/deviceauth/token codex_cli_rs .*"device_auth_id":"device-auth-119".*"user_code":"SMOK-E119"' || true)"
if [ "${dc_polls:-0}" -lt 3 ]; then
	fail "expected at least three polls presenting the issued pair, saw ${dc_polls:-0}"
fi
dc_exchanges="$(printf '%s\n' "$dc_log" | grep -c '^/oauth/token ' || true)"
expect_eq "${dc_exchanges:-0}" "1" "the grant was not redeemed exactly once"
dc_exchange="$(printf '%s\n' "$dc_log" | grep '^/oauth/token ' || true)"
for want in "grant_type=authorization_code" "code=poll-code-119" "code_verifier=code-verifier-119" \
	"client_id=app_EMoamEEZ73f0CkXaXp7hrann" "redirect_uri=http%3A%2F%2F127.0.0.1%3A$DC_PORT%2Fdeviceauth%2Fcallback"; do
	expect_has "$dc_exchange" -F "$want" "the exchange is missing \"$want\""
done
expect_lacks "$dc_exchange" -F "localhost" "the exchange named a loopback redirect the device flow never opened"

# A second sign-in must not carry the old attempt: the poll counter above
# is the stub's, so this just confirms the flow can be reopened onto its
# choice again after completing — the row now reads configured.
submit "$S119" "/login"
sleep 0.5
keys "$S119" Enter
sleep 0.4
type_text "$S119" "chatgpt"
sleep 0.3
again="$(tmux capture-pane -t "$S119" -p)"
dump "the row after signing in" "$again"
expect_has "$again" -E 'ChatGPT Codex · ✔ configured' "the ChatGPT Codex row does not read configured after the sign-in"

kill "$DC_SERVER" 2>/dev/null
wait "$DC_SERVER" 2>/dev/null
tmux kill-session -t "$S119" 2>/dev/null
echo "==== Phase 119: the sign-in method choice opens, steps back, and the device-code flow signs in against the stub ===="
