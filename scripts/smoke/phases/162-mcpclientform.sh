#!/usr/bin/env bash
# Phase 162 — /mcp ASKS FOR THE OAUTH CLIENT a server can't register

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# /mcp ASKS FOR THE OAUTH CLIENT (docs/mcp.md *Pre-registered clients*). An
# authorization server that registers no clients itself — Google's, for its
# MCP servers — left a sign-in one way forward, a client the user creates
# with the provider, and one way to give it: the config file or the `mcp
# add` flags. The spec's registration order ends on "prompt the user to
# enter the client information"; the Authenticate page now does.
#
# A stub remote MCP server demands OAuth (a 401 naming its RFC 9728
# metadata) behind an authorization server whose RFC 8414 metadata offers NO
# registration endpoint and takes its client's secret in the form body. Its
# token endpoint knows one client — an id and a secret — and turns any other
# away with `invalid_client`. Driven end to end, the browser played by curl:
#
# - Authenticate ends in the FORM, not a failure: who asks and why, the
#   three fields, the redirect URI to register;
# - a typed id, a PASTED secret shown one dot per character and never as
#   text — the WRONG one first: the token endpoint turns the client away and
#   the form reopens on the id, with the server's own words;
# - the right secret signs in: the authorize URL names the typed client, the
#   token request carries its id and secret, the server reads connected;
# - the client is kept in mcp-auth.json — owner-only, under its server,
#   bound to its issuer — and never in mcp.json; the server's page names it
#   (`entered here`) and `mcp get` names it without its secret;
# - Change OAuth client reopens the form on what is kept, a secret left
#   empty keeping the one kept, a callback port typed there moving the
#   redirect to exactly that port;
# - Forget OAuth client removes it and the login made with it;
# - a sign-in that FAILS keeps its reason on the page it was started from
#   (docs/mcp.md *A sign-in that failed*): a client given a callback port
#   something else is listening on cannot be signed in with, and the page
#   says so whole — it used to close under a one-row toast cut at the
#   terminal's width — with Enter trying again once the port is free.
#
# Through all of it neither secret may appear on screen, scrollback included.
S162="${S}_mcpclientform"
CLIENT_ID="smoke-162-client.apps.example"
SECRET="GOCSPX-smoke162-right-secret"
WRONG="GOCSPX-smoke162-wrong-secret"
AUTH_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
FIXED_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
AUTH_BASE="http://127.0.0.1:$AUTH_PORT"
AUTH_LOG="$SMOKE_TMP/mcpclient.log"
STORE="$SMOKE_CFG/mcp-auth.json"
python3 - "$AUTH_PORT" "$AUTH_LOG" "$CLIENT_ID" "$SECRET" <<'PY' &
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port, log, client_id, secret = int(sys.argv[1]), sys.argv[2], sys.argv[3], sys.argv[4]
base = f"http://127.0.0.1:{port}"
TOKEN = "smoke-162-access-token"

class Stub(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def record(self, body=""):
        with open(log, "a") as f:
            f.write(json.dumps({"method": self.command, "path": self.path, "body": body}) + "\n")

    def reply(self, status, obj, headers=()):
        data = json.dumps(obj).encode()
        self.send_response(status)
        for key, value in headers:
            self.send_header(key, value)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self.record()
        if self.path.startswith("/.well-known/oauth-protected-resource"):
            self.reply(200, {"resource": f"{base}/mcp",
                             "authorization_servers": [base],
                             "scopes_supported": ["mcp:tools"]})
        elif self.path == "/.well-known/oauth-authorization-server":
            # No registration_endpoint: this server registers no clients.
            self.reply(200, {"issuer": base,
                             "authorization_endpoint": f"{base}/oauth/authorize",
                             "token_endpoint": f"{base}/oauth/token",
                             "response_types_supported": ["code"],
                             "code_challenge_methods_supported": ["S256"],
                             "token_endpoint_auth_methods_supported": ["client_secret_post"]})
        else:
            self.reply(404, {})

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0)).decode()
        self.record(body)
        if self.path == "/oauth/token":
            form = urllib.parse.parse_qs(body)
            if form.get("client_id") == [client_id] and form.get("client_secret") == [secret]:
                self.reply(200, {"access_token": TOKEN, "token_type": "Bearer",
                                 "expires_in": 3600, "refresh_token": "smoke-162-refresh"})
            else:
                self.reply(401, {"error": "invalid_client",
                                 "error_description": "The OAuth client was not found."})
        elif self.path == "/mcp":
            if self.headers.get("authorization") != f"Bearer {TOKEN}":
                challenge = f'Bearer resource_metadata="{base}/.well-known/oauth-protected-resource"'
                self.reply(401, {"error": "unauthorized"}, [("WWW-Authenticate", challenge)])
                return
            request = json.loads(body or "{}")
            method = request.get("method")
            if method == "server/discover":
                result = {"resultType": "complete", "supportedVersions": ["2026-07-28"],
                          "capabilities": {"tools": {}},
                          "_meta": {"io.modelcontextprotocol/serverInfo":
                                    {"name": "stub", "version": "0"}}}
            elif method == "tools/list":
                result = {"resultType": "complete",
                          "tools": [{"name": "ping", "description": "Ping.",
                                     "inputSchema": {"type": "object"}}]}
            else:
                self.send_response(202)
                self.send_header("content-length", "0")
                self.end_headers()
                return
            self.reply(200, {"jsonrpc": "2.0", "id": request.get("id"), "result": result})
        else:
            self.reply(404, {})

    def do_DELETE(self):
        self.reply(405, {})

ThreadingHTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
smoke_on_exit kill "$!"
stub_up() { python3 -c "import socket; socket.create_connection(('127.0.0.1', $AUTH_PORT), 0.2)" 2>/dev/null; }
if ! poll 5 stub_up; then
	fail "the stub MCP server never came up on port $AUTH_PORT"
fi

cat >"$SMOKE_CFG/mcp.json" <<MCPJSON
{"mcpServers": {"stub": {"type": "http", "url": "$AUTH_BASE/mcp"}}}
MCPJSON
CONFIG_BEFORE="$(cat "$SMOKE_CFG/mcp.json")"

never_shows_a_secret() { # $1 label
	local screen
	screen="$(pane "$S162" -S -600)"
	if printf '%s\n' "$screen" | grep -qF -e "$SECRET" -e "$WRONG"; then
		dump "a secret leaked ($1)" "$screen"
		fail "a client secret is on screen ($1)"
	fi
}
paste() { # TEXT — a bracketed paste into the focused field
	tmux set-buffer -b client "$1"
	tmux paste-buffer -p -b client -t "$S162"
}
# The authorize URL as the page spells it, squeezed back together (it holds
# no spaces), once it is whole — its last parameter is the scope.
auth_url=""
read_url() {
	auth_url="$(pane "$S162" | tr -d ' \n' | grep -oE "${AUTH_BASE}/oauth/authorize\?[^[:space:]]*scope=mcp%3Atools" | head -1)"
	[ -n "$auth_url" ]
}
# Play the browser: approve, and follow the redirect to the loopback listener.
approve() {
	local state cb_port
	state="$(printf '%s' "$auth_url" | grep -oE 'state=[A-Za-z0-9_-]+' | head -1 | cut -d= -f2)"
	cb_port="$(printf '%s' "$auth_url" | grep -oE 'localhost%3A[0-9]+' | head -1 | sed 's/.*%3A//')"
	[ -n "$state" ] || fail "no state could be read off the authorize URL"
	[ -n "$cb_port" ] || fail "no callback port could be read off the authorize URL"
	REDIRECT_PORT="$cb_port"
	curl -s --noproxy '*' "http://127.0.0.1:$cb_port/callback?code=code-162&state=$state" >/dev/null || true
}

launch "$S162" 100 44 "env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
type_text "$S162" "/mcp"
sleep "$SMOKE_TYPE_SETTLE"
keys "$S162" Enter
if ! wait_for 15 "$S162" -F "needs authentication"; then
	dump "$S162"
	fail "the stub server never read as needing authentication"
fi
keys "$S162" Enter # the server's detail
detail="$(wait_pane 5 "$S162" -F "2. Set OAuth client")" ||
	fail "the server page does not offer Set OAuth client under the sign-in"
dump "the server page before any client" "$detail"
expect_lacks "$detail" -F "OAuth client:" "the page names a client before one was given"

# Authenticate: no registration, no client — the page ASKS instead of failing.
keys "$S162" Enter
form="$(wait_pane 15 "$S162" -F "registers no clients itself")" || {
	dump "the page after Authenticate" "$form"
	fail "Authenticate did not turn into the client form"
}
dump "the client form" "$form"
for want in "OAuth client for stub" "127.0.0.1:$AUTH_PORT registers no clients itself" \
	"never in an mcp.json" "❯ Client ID" "type or paste the client ID" \
	"Client secret" "leave empty for a public client" "Callback port" "any free port" \
	"Redirect URI: http://localhost:PORT/callback" "Enter for the next field"; do
	expect_has "$form" -F "$want" "the form is missing \"$want\""
done
expect_lacks "$form" -F "URL > " "the form still shows the sign-in's redirect field"

# The id typed, the WRONG secret pasted: one dot a character, never its text.
type_text "$S162" "$CLIENT_ID"
sleep "$SMOKE_TYPE_SETTLE"
keys "$S162" Enter
wait_for 5 "$S162" -F "❯ Client secret" || fail "Enter did not move to the secret field"
paste "$WRONG"
dots="$(printf '%*s' "${#WRONG}" '' | sed 's/ /•/g')"
masked="$(wait_pane 5 "$S162" -F "$dots")" || fail "the pasted secret is not masked one dot per character"
dump "the form with the id typed and the secret pasted" "$masked"
expect_has "$masked" -F "$CLIENT_ID" "the typed client id is not on its row"
never_shows_a_secret "the secret field"
keys "$S162" Enter
hint="$(wait_pane 5 "$S162" -F "Enter to sign in")" || fail "the last field's hint does not say Enter signs in"
expect_has "$hint" -F "❯ Callback port" "Enter did not move to the callback port"
keys "$S162" Enter # no port: any free one

# The sign-in runs as the typed client; the browser approves; the token
# endpoint turns the client away — and the form comes back, on the id.
poll 15 read_url || {
	dump "the page after the form" "$(pane "$S162")"
	fail "the sign-in never showed an authorize URL for the entered client"
}
expect_has "$auth_url" -F "client_id=$CLIENT_ID" "the authorize URL does not name the entered client"
approve
again="$(wait_pane 15 "$S162" -F "turned this client away")" || {
	dump "the page after the wrong secret" "$again"
	fail "a client the token endpoint turned away did not reopen the form"
}
dump "the form after the client was turned away" "$again"
expect_has "$again" -F "HTTP 401: invalid_client: The OAuth client was not found." \
	"the form does not carry the server's own words"
expect_has "$again" -F "$CLIENT_ID" "the form did not start from the id that was tried"
expect_has "$again" -F "❯ Client secret" "the focus is not on the secret, the thing to put right"
# The pairing the server just refused is not offered to be kept.
expect_lacks "$again" -F "leave empty to keep" "the form offers to keep the secret that was just refused"
never_shows_a_secret "the reopened form"

# The right secret: signed in, and the server connects.
paste "$SECRET"
sleep "$SMOKE_TYPE_SETTLE"
keys "$S162" Enter
wait_for 5 "$S162" -F "❯ Callback port" || fail "Enter did not move to the callback port"
keys "$S162" Enter
auth_url=""
poll 15 read_url || fail "the second sign-in never showed its authorize URL"
approve
signed_in="$(wait_pane 20 "$S162" -F "✔ connected")" || {
	dump "the page after the right secret" "$signed_in"
	fail "the server never connected after signing in with the entered client"
}
dump "the server page, signed in" "$signed_in"
for want in "Authenticated with stub" "OAuth client:" "$CLIENT_ID (entered here)" \
	"Change OAuth client" "Forget OAuth client"; do
	expect_has "$signed_in" -F "$want" "the signed-in page is missing \"$want\""
done
never_shows_a_secret "the signed-in page"

# What went over the wire, and what is kept where.
python3 - "$AUTH_LOG" "$STORE" "$AUTH_BASE" "$CLIENT_ID" "$SECRET" "$WRONG" <<'PY' || fail "the sign-in's requests or the kept client were not as expected (see above)"
import json, os, stat, sys, urllib.parse
log, store, base, client_id, secret, wrong = sys.argv[1:7]
entries = [json.loads(line) for line in open(log)]
assert not any(e["path"].endswith("/register") for e in entries), "something was registered"
exchanges = [urllib.parse.parse_qs(e["body"]) for e in entries if e["path"] == "/oauth/token"]
assert len(exchanges) == 2, f"{len(exchanges)} token requests"
for exchange, sent in zip(exchanges, (wrong, secret)):
    assert exchange["grant_type"] == ["authorization_code"], exchange
    assert exchange["client_id"] == [client_id], exchange
    assert exchange["client_secret"] == [sent], exchange
    assert exchange["code_verifier"][0], exchange
kept = json.load(open(store))
client = kept["clients"][f"{base}/mcp"]
assert client == {"client_id": client_id, "client_secret": secret, "issuer": base}, client
assert kept["servers"][f"{base}/mcp"]["client_id"] == client_id, kept["servers"]
mode = stat.S_IMODE(os.stat(store).st_mode)
assert mode == 0o600, oct(mode)
print("wire and store: ok")
PY
expect_eq "$(cat "$SMOKE_CFG/mcp.json")" "$CONFIG_BEFORE" "the form wrote to mcp.json"

# `mcp get` names the kept client, never its secret.
got="$(env $CFG_ENV_NOHIST "$BIN" mcp get stub 2>&1)"
dump "mcp get stub" "$got"
expect_has "$got" -F "OAuth client: $CLIENT_ID (entered in /mcp)" "mcp get does not name the entered client"
expect_has "$got" -F "Client secret: kept with the tokens" "mcp get does not say a secret is kept"
expect_lacks "$got" -F "$SECRET" "mcp get printed the client secret"

# Change OAuth client: the form over what is kept — the id, an empty secret
# that keeps the kept one — and a callback port typed there is the port the
# sign-in listens on, exactly.
change_row="$(printf '%s\n' "$signed_in" | grep -oE '[0-9]\. Change OAuth client' | cut -c1)"
[ -n "$change_row" ] || fail "no numbered Change OAuth client row"
keys "$S162" "$change_row"
changing="$(wait_pane 15 "$S162" -F "Sign in as an OAuth client you created with")" || {
	dump "the page after Change OAuth client" "$changing"
	fail "Change OAuth client did not open the form"
}
dump "the form over the kept client" "$changing"
expect_has "$changing" -F "$CLIENT_ID" "the form did not start from the kept id"
expect_has "$changing" -F "leave empty to keep the secret already kept" \
	"the form does not offer to keep the kept secret"
keys "$S162" Enter # the secret left empty → the port
wait_for 5 "$S162" -F "❯ Callback port" || fail "Enter did not move to the callback port"
type_text "$S162" "$FIXED_PORT"
fixed="$(wait_pane 5 "$S162" -F "Redirect URI: http://localhost:$FIXED_PORT/callback")" ||
	fail "the redirect URI did not follow the port typed"
expect_lacks "$fixed" -F "PORT is any free port" "the any-port note stayed over a fixed port"
keys "$S162" Enter
auth_url=""
poll 15 read_url || fail "the sign-in on the fixed port never showed its authorize URL"
approve
expect_eq "$REDIRECT_PORT" "$FIXED_PORT" "the sign-in did not listen on the callback port typed"
wait_for 20 "$S162" -F "✔ connected" || fail "the server did not reconnect after the client was changed"
python3 - "$AUTH_LOG" "$STORE" "$AUTH_BASE" "$SECRET" "$FIXED_PORT" <<'PY' || fail "the changed client was not kept as expected (see above)"
import json, sys, urllib.parse
log, store, base, secret, port = sys.argv[1:6]
exchanges = [urllib.parse.parse_qs(json.loads(line)["body"]) for line in open(log)
             if json.loads(line)["path"] == "/oauth/token"]
assert exchanges[-1]["client_secret"] == [secret], "the kept secret was not sent"
assert exchanges[-1]["redirect_uri"] == [f"http://localhost:{port}/callback"], exchanges[-1]
client = json.load(open(store))["clients"][f"{base}/mcp"]
assert client["client_secret"] == secret and client["callback_port"] == int(port), client
print("changed client: ok")
PY
never_shows_a_secret "the changed client"

# Forget OAuth client: the client goes, and the login made with it.
page="$(pane "$S162")"
forget_row="$(printf '%s\n' "$page" | grep -oE '[0-9]\. Forget OAuth client' | cut -c1)"
[ -n "$forget_row" ] || {
	dump "the page before forgetting" "$page"
	fail "no numbered Forget OAuth client row"
}
keys "$S162" "$forget_row"
forgotten="$(wait_pane 15 "$S162" -F "needs authentication")" || {
	dump "the page after Forget OAuth client" "$forgotten"
	fail "forgetting the client did not sign the server out"
}
dump "the server page after forgetting" "$forgotten"
expect_has "$forgotten" -F "Forgot stub's OAuth client and signed out of it" "no toast said what was forgotten"
expect_lacks "$forgotten" -F "OAuth client:" "the page still names the forgotten client"
expect_has "$forgotten" -F "Set OAuth client" "the page does not offer to set a client again"
python3 - "$STORE" <<'PY' || fail "the token store still keeps the forgotten client or its login"
import json, sys
kept = json.load(open(sys.argv[1]))
assert not kept.get("clients"), kept
assert not kept.get("servers"), kept
PY
never_shows_a_secret "after forgetting"

# A sign-in that fails: Set OAuth client gives the server a client whose
# callback port is TAKEN. The sign-in cannot listen there — and the reason
# stays on its page, whole, instead of closing it under a toast.
SQUAT_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 -c 'import socket, sys, time
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", int(sys.argv[1])))
s.listen()
time.sleep(300)' "$SQUAT_PORT" &
SQUAT_PID="$!"
smoke_on_exit kill "$SQUAT_PID"
port_taken() { python3 -c "import socket; socket.create_connection(('127.0.0.1', $SQUAT_PORT), 0.2)" 2>/dev/null; }
poll 5 port_taken || fail "the port squatter never came up on $SQUAT_PORT"
keys "$S162" 2 # Set OAuth client
wait_for 15 "$S162" -F "Sign in as an OAuth client you created with" ||
	fail "Set OAuth client did not open the form"
type_text "$S162" "$CLIENT_ID"
sleep "$SMOKE_TYPE_SETTLE"
keys "$S162" Enter
wait_for 5 "$S162" -F "❯ Client secret" || fail "Enter did not move to the secret field"
paste "$SECRET"
sleep "$SMOKE_TYPE_SETTLE"
keys "$S162" Enter
wait_for 5 "$S162" -F "❯ Callback port" || fail "Enter did not move to the callback port"
type_text "$S162" "$SQUAT_PORT"
wait_for 5 "$S162" -F "http://localhost:$SQUAT_PORT/callback" || fail "the redirect did not follow the taken port"
keys "$S162" Enter
failed="$(wait_pane 15 "$S162" -F "Couldn't authenticate with stub")" || {
	dump "the page after the sign-in failed" "$failed"
	fail "a failed sign-in did not keep its page"
}
dump "the failed sign-in's page" "$failed"
expect_has "$failed" -F "the callback port $SQUAT_PORT isn't free" "the page does not carry the whole reason"
expect_has "$failed" -F "Enter to try again · Esc to go back" "the page does not say how to go on"
expect_lacks "$failed" -F "URL > " "the failed page still shows the sign-in's redirect field"
expect_lacks "$failed" -F "stub: the callback port" "the reason was toasted over the page that shows it"
never_shows_a_secret "the failed sign-in"

# The port freed, Enter tries again — with the client that was kept — and
# the sign-in listens on exactly that port.
kill "$SQUAT_PID" 2>/dev/null
wait "$SQUAT_PID" 2>/dev/null || true
port_free() { ! port_taken; }
poll 5 port_free || fail "the squatted port was never released"
keys "$S162" Enter
auth_url=""
poll 15 read_url || {
	dump "the page after trying again" "$(pane "$S162")"
	fail "Enter on the failed page did not start the sign-in again"
}
expect_has "$auth_url" -F "client_id=$CLIENT_ID" "the retry did not sign in as the kept client"
expect_has "$auth_url" -F "localhost%3A$SQUAT_PORT%2Fcallback" "the retry did not listen on the kept callback port"
keys "$S162" Escape
wait_for 5 "$S162" -F "$CLIENT_ID (entered here)" || fail "Esc did not return to the server page naming the kept client"
tmux kill-session -t "$S162" 2>/dev/null
