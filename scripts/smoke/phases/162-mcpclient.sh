#!/usr/bin/env bash
# Phase 162 — the /mcp CLIENT FORM: an OAuth client typed in, turned away, fixed, forgotten

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The /mcp CLIENT FORM (docs/mcp.md *Pre-registered clients*). A server whose
# authorization server registers no clients itself — Google's, for its MCP
# servers — used to end Authenticate on a toast telling the user to edit a
# file. The auth page now turns into a form: the issuer, the redirect URI to register, a
# callback port, a client ID, a masked secret.
#
# A stub remote MCP server demands OAuth (a 401 naming its RFC 9728
# metadata) through an authorization server with NO registration endpoint,
# whose token endpoint knows one client, `smoke-client` / `right-secret`. The
# "browser" is this script: it reads the authorize URL off the raw byte
# stream (the page's OSC 8 link carries it whole) and requests the redirect
# itself. What this phase pins, on the real binary:
#
# - Authenticate ends on the FORM, naming the authorization server and the
#   redirect URI, which follows the port as it is typed;
# - the secret is never drawn — not on the pane, not in the byte stream;
# - a client the server turns away (a wrong secret, after the whole browser
#   round) reopens the form with the id kept and the server's own reason;
# - the right secret signs in: the server connects, its page names the
#   client and offers to change or forget it;
# - the client is kept with the tokens, owner-only, bound to its issuer and
#   port — and mcp.json is never written;
# - `mcp get` names the client and never its secret;
# - Forget OAuth client drops the client and the login together;
# - a sign-in that FAILS stays on its page with the whole reason.
S162="${S}_mcpclient"
free_port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }
AUTH_PORT="$(free_port)"
AUTH_BASE="http://127.0.0.1:$AUTH_PORT"
AUTH_LOG="$SMOKE_TMP/mcpclient.log"
python3 - "$AUTH_PORT" "$AUTH_LOG" <<'PY' &
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port, log = int(sys.argv[1]), sys.argv[2]
base = f"http://127.0.0.1:{port}"

class Stub(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def record(self, note=""):
        with open(log, "a") as f:
            f.write(f"{self.command} {self.path} {note}\n")

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
            self.reply(200, {"resource": f"{base}/mcp", "authorization_servers": [base]})
        elif self.path == "/.well-known/oauth-authorization-server":
            # No registration_endpoint: this authorization server registers
            # no clients itself.
            self.reply(200, {"issuer": base,
                             "authorization_endpoint": f"{base}/oauth/authorize",
                             "token_endpoint": f"{base}/oauth/token",
                             "response_types_supported": ["code"],
                             "code_challenge_methods_supported": ["S256"],
                             "token_endpoint_auth_methods_supported": ["client_secret_post"],
                             "service_documentation": f"{base}/docs/oauth"})
        else:
            self.reply(404, {})

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0))
        if self.path == "/mcp":
            if self.headers.get("Authorization") != "Bearer tok-ok":
                self.record("401")
                challenge = f'Bearer resource_metadata="{base}/.well-known/oauth-protected-resource"'
                self.reply(401, {"error": "unauthorized"}, [("WWW-Authenticate", challenge)])
                return
            request = json.loads(body or b"{}")
            method = request.get("method")
            self.record(str(method))
            if method == "server/discover":
                result = {"resultType": "complete", "supportedVersions": ["2026-07-28"],
                          "capabilities": {"tools": {}},
                          "_meta": {"io.modelcontextprotocol/serverInfo": {"name": "stub", "version": "0"}}}
            elif method == "tools/list":
                result = {"resultType": "complete",
                          "tools": [{"name": "one", "inputSchema": {"type": "object"}}]}
            else:
                self.send_response(202)
                self.send_header("content-length", "0")
                self.end_headers()
                return
            self.reply(200, {"jsonrpc": "2.0", "id": request.get("id"), "result": result})
        elif self.path == "/oauth/token":
            form = {k: v[0] for k, v in urllib.parse.parse_qs(body.decode()).items()}
            known = form.get("client_id") == "smoke-client" and form.get("client_secret") == "right-secret"
            # The log names the client and the redirect, never a secret.
            self.record(f"client_id={form.get('client_id')} redirect_uri={form.get('redirect_uri')} ok={known}")
            if known:
                self.reply(200, {"access_token": "tok-ok", "token_type": "Bearer",
                                 "expires_in": 3600, "refresh_token": "rt-1"})
            else:
                self.reply(401, {"error": "invalid_client",
                                 "error_description": "The OAuth client was not found."})
        else:
            self.record()
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
cp "$SMOKE_CFG/mcp.json" "$SMOKE_TMP/mcp.json.before"
STORE="$SMOKE_CFG/mcp-auth.json"

RAW="$SMOKE_TMP/mcpclient.raw"
: >"$RAW"
launch "$S162" 100 44 "$APP"
tmux pipe-pane -t "$S162" -o "cat >> '$RAW'"

# The newest authorize URL the page has linked: every OSC 8 open in the raw
# stream carries its whole target.
authorize_url() {
	grep -aoE $']8;id=az[0-9]+;[^\x1b]*' "$RAW" | sed -E 's/^\]8;id=az[0-9]+;//' |
		grep -F "$AUTH_BASE/oauth/authorize" | tail -n 1
}
# Wait for an authorize URL other than `$1` (the previous attempt's).
new_url=""
next_authorize_url() {
	local previous="$1"
	fresh() {
		new_url="$(authorize_url)"
		[ -n "$new_url" ] && [ "$new_url" != "$previous" ]
	}
	poll 15 fresh
}
# Play the browser: follow `$1`'s redirect back to the app with `$2` as the
# authorization server's answer (`code=…`, or `error=…`).
browser() {
	python3 - "$1" "$2" <<'PY'
import sys, urllib.parse, urllib.request
query = urllib.parse.parse_qs(urllib.parse.urlparse(sys.argv[1]).query)
target = f"{query['redirect_uri'][0]}?{sys.argv[2]}&state={urllib.parse.quote(query['state'][0])}"
try:
    urllib.request.urlopen(target, timeout=10).read()
except Exception as error:  # the page may answer an error status; the app has the redirect
    print(f"browser: {error}", file=sys.stderr)
PY
}

type_text "$S162" "/mcp"
sleep "$SMOKE_TYPE_SETTLE"
keys "$S162" Enter
if ! wait_for 15 "$S162" -F "needs authentication"; then
	dump "$S162"
	fail "the stub server never read as needing authentication"
fi
keys "$S162" Enter # the server's detail
if ! wait_for 5 "$S162" -F "1. Authenticate"; then
	dump "$S162"
	fail "the server page offers no Authenticate action"
fi
if pane_has "$S162" -F "OAuth client"; then
	dump "$S162"
	fail "a server with no client of the user's shows client rows"
fi
keys "$S162" Enter # Authenticate

# --- the form ---------------------------------------------------------------
if ! wait_for 15 "$S162" -F "OAuth client for stub"; then
	dump "$S162"
	fail "Authenticate did not end on the client form"
fi
form="$(pane "$S162")"
note "the client form"
printf '%s\n' "$form"
expect_has "$form" -F "Issuer:        $AUTH_BASE" "the form does not show the issuer as discovery named it"
expect_lacks "$form" -F "doesn't register this app" "the form still describes the provider"
expect_has "$form" -F "Docs:          $AUTH_BASE/docs/oauth" "the form does not show the server's documentation link"
# The documentation row is a link to exactly the URL it shows.
docs_linked() {
	grep -aoE $']8;id=az[0-9]+;[^\x1b]*' "$RAW" | sed -E 's/^\]8;id=az[0-9]+;//' | grep -qxF "$AUTH_BASE/docs/oauth"
}
if ! poll 5 docs_linked; then
	fail "the documentation row carries no OSC 8 link to $AUTH_BASE/docs/oauth"
fi
expect_has "$form" -F "Redirect URI:  http://localhost:8976/callback" "the form does not show the redirect URI on the default port"
expect_has "$form" -F "Callback port > 8976" "the port field is not prefilled"
expect_has "$form" -F "Client ID     >" "the client ID field is missing"
expect_has "$form" -F "Client secret > optional" "the empty secret field does not say it is optional"
expect_has "$form" -F "Tab to switch fields" "the form's keys are missing"
expect_lacks "$form" -F "URL > " "the paste field is still on the page"

# The port field: the redirect URI follows it as it is typed.
CB_PORT="$(free_port)"
keys "$S162" Up # the port, above the client ID
keys "$S162" C-u
type_text "$S162" "$CB_PORT"
if ! wait_for 5 "$S162" -F "Redirect URI:  http://localhost:$CB_PORT/callback"; then
	dump "$S162"
	fail "the redirect URI did not follow the port typed"
fi
# Ctrl+Y copies it (OSC 52 here: the suite runs headless).
keys "$S162" C-y
if ! wait_for 5 "$S162" -F "Copied redirect URI"; then
	dump "$S162"
	fail "Ctrl+Y did not copy the redirect URI"
fi

# A submit with no client ID says so, and sends nothing.
keys "$S162" Down  # the client ID
keys "$S162" Down  # the secret: the last field
keys "$S162" Enter # submit
if ! wait_for 5 "$S162" -F "Enter the client ID"; then
	dump "$S162"
	fail "a submit with no client ID was not refused on the page"
fi
type_text "$S162" "smoke-client" # the caret went to the field at fault
keys "$S162" Enter               # on to the secret
type_text "$S162" "wrong-secret"
sleep "$SMOKE_TYPE_SETTLE"
typed="$(pane "$S162")"
expect_has "$typed" -F "Client ID     > smoke-client" "the typed client ID is not on its row"
expect_has "$typed" -F "Client secret > ••••••••••••" "the secret is not drawn as a mask, a mark a character"
expect_lacks "$typed" -F "wrong-secret" "THE SECRET IS ON SCREEN"
keys "$S162" Enter # submit: kept, and the sign-in starts again

# --- the browser round with a client the server turns away -------------------
if ! next_authorize_url ""; then
	dump "$S162"
	fail "no authorize URL followed the form"
fi
first_url="$new_url"
note "the authorize URL the entered client signs in with"
printf '%s\n' "$first_url"
expect_has "$first_url" -F "client_id=smoke-client" "the sign-in does not present the entered client"
expect_has "$first_url" -F "redirect_uri=http%3A%2F%2Flocalhost%3A$CB_PORT%2Fcallback" "the redirect is not on the port the form named"
browser "$first_url" "code=smoke-code-1"
if ! wait_for 15 "$S162" -F "turned this client away"; then
	dump "$S162"
	fail "a client the server turned away did not reopen the form with the reason"
fi
back="$(pane "$S162")"
note "the form, back with the server's reason"
printf '%s\n' "$back"
expect_has "$back" -F "invalid_client" "the reason lacks the server's error"
# The reason wraps at the pane's width: read it with its rows joined.
back_flat="$(printf '%s\n' "$back" | tr '\n' ' ' | tr -s ' ')"
expect_has "$back_flat" -F "check its ID and its secret" "the reason does not say what to do"
expect_has "$back_flat" -F "The OAuth client was not found." "the reason lacks the server's own words"
expect_has "$back" -F "Client ID     > smoke-client" "the client ID was not kept on the form"
expect_has "$back" -F "Callback port > $CB_PORT" "the port was not kept on the form"
expect_has "$back" -F "Client secret > unchanged" "the form does not say the kept secret stays"
expect_lacks "$back" -F "wrong-secret" "THE SECRET IS ON SCREEN"

# --- the right secret --------------------------------------------------------
type_text "$S162" "right-secret" # the caret is on the secret
keys "$S162" Enter
if ! next_authorize_url "$first_url"; then
	dump "$S162"
	fail "no second authorize URL followed the corrected client"
fi
browser "$new_url" "code=smoke-code-2"
if ! wait_for 15 "$S162" -F "Authenticated with stub"; then
	dump "$S162"
	fail "the corrected client did not sign in"
fi
if ! wait_for 15 "$S162" -F "✔ connected"; then
	dump "$S162"
	fail "the server did not connect after the sign-in"
fi
signed_in="$(pane "$S162")"
note "the server's page, signed in"
printf '%s\n' "$signed_in"
expect_has "$signed_in" -E "OAuth client: +smoke-client" "the page does not name the client it signs in with"
expect_has "$signed_in" -F "Change OAuth client" "the page does not offer to change the client"
expect_has "$signed_in" -F "Forget OAuth client" "the page does not offer to forget the client"

# The client is kept with the tokens — whole, owner-only, bound to its issuer
# — and the config file was never written.
python3 - "$STORE" "$AUTH_BASE" "$CB_PORT" <<'PY' || fail "the token store does not hold the entered client as expected"
import json, os, sys
store, base, port = sys.argv[1], sys.argv[2], int(sys.argv[3])
mode = oct(os.stat(store).st_mode & 0o777)
assert mode == "0o600", f"the store is {mode}, not owner-only"
doc = json.load(open(store))
client = doc["clients"][f"{base}/mcp"]
want = {"client_id": "smoke-client", "client_secret": "right-secret",
        "callback_port": port, "issuer": base}
assert client == want, f"kept {client}, expected {want}"
grant = doc["servers"][f"{base}/mcp"]
assert grant["access_token"] == "tok-ok" and grant["client_id"] == "smoke-client", grant
PY
cmp -s "$SMOKE_CFG/mcp.json" "$SMOKE_TMP/mcp.json.before" || fail "mcp.json was written by the form"

# `mcp get` names the client, and where its secret is — never the secret.
got="$(env $CFG_ENV_NOHIST "$BIN" mcp get stub)"
note "mcp get stub"
printf '%s\n' "$got"
expect_has "$got" -F "OAuth client: smoke-client (entered in /mcp)" "mcp get does not name the entered client"
expect_has "$got" -F "Callback port: $CB_PORT" "mcp get does not name the client's port"
expect_has "$got" -F "Client secret: kept with the tokens" "mcp get does not say where the secret is"
expect_lacks "$got" -F "right-secret" "mcp get printed the secret"

# --- Change opens the form over the kept client; Esc leaves it be -----------
change="$(printf '%s\n' "$signed_in" | grep -oE '[0-9]+\. Change OAuth client' | grep -oE '^[0-9]+')"
keys "$S162" "$change"
if ! wait_for 5 "$S162" -F "Client ID     > smoke-client"; then
	dump "$S162"
	fail "Change OAuth client did not open the form over the kept client"
fi
expect_has "$(pane "$S162")" -F "Client secret > unchanged" "the form does not say the kept secret stays"
keys "$S162" Escape
if ! wait_for 5 "$S162" -F "Forget OAuth client"; then
	dump "$S162"
	fail "Esc did not return from the form to the server page"
fi

# --- Forget drops the client and the login together -------------------------
forget="$(pane "$S162" | grep -oE '[0-9]+\. Forget OAuth client' | grep -oE '^[0-9]+')"
keys "$S162" "$forget"
if ! wait_for 5 "$S162" -F "Forgot the OAuth client for stub"; then
	dump "$S162"
	fail "Forget OAuth client did not say it forgot the client"
fi
if ! wait_for 15 "$S162" -F "needs authentication"; then
	dump "$S162"
	fail "the server is still signed in after its client was forgotten"
fi
forgotten() { ! pane_has "$S162" -F "Forget OAuth client" && ! pane_has "$S162" -F "OAuth client:"; }
if ! poll 5 forgotten; then
	dump "$S162"
	fail "the page still names a client after it was forgotten"
fi
python3 - "$STORE" "$AUTH_BASE" <<'PY' || fail "the token store still holds the forgotten client or its login"
import json, sys
doc = json.load(open(sys.argv[1]))
url = f"{sys.argv[2]}/mcp"
assert url not in doc.get("clients", {}), doc
assert url not in doc.get("servers", {}), doc
PY

# --- a sign-in that fails stays on its page with the whole reason ----------
keys "$S162" 1 # Authenticate
if ! wait_for 15 "$S162" -F "OAuth client for stub"; then
	dump "$S162"
	fail "the sign-in did not ask for a client again once it was forgotten"
fi
keys "$S162" Up
keys "$S162" C-u
type_text "$S162" "$CB_PORT"
keys "$S162" Down
type_text "$S162" "smoke-client"
keys "$S162" Enter
type_text "$S162" "right-secret"
keys "$S162" Enter
if ! next_authorize_url "$new_url"; then
	dump "$S162"
	fail "no authorize URL followed the client entered again"
fi
browser "$new_url" "error=access_denied&error_description=The%20user%20said%20no"
if ! wait_for 15 "$S162" -F "Couldn't authenticate with stub"; then
	dump "$S162"
	fail "a refused sign-in did not stay on its page"
fi
failed="$(pane "$S162")"
note "the failed sign-in's page"
printf '%s\n' "$failed"
failed_flat="$(printf '%s\n' "$failed" | tr '\n' ' ' | tr -s ' ')"
expect_has "$failed_flat" -F "access_denied" "the failed page lacks the server's error"
expect_has "$failed_flat" -F "The user said no" "the failed page lacks the server's own words"
expect_has "$failed" -F "Enter to try again" "the failed page does not say how to retry"
keys "$S162" Escape
if ! wait_for 5 "$S162" -F "1. Authenticate"; then
	dump "$S162"
	fail "Esc did not return from the failed page to the server page"
fi

# --- no secret anywhere ------------------------------------------------------
# The byte stream is everything the app ever wrote to the terminal.
for secret in wrong-secret right-secret; do
	if grep -aqF "$secret" "$RAW"; then
		fail "the secret \"$secret\" reached the terminal's byte stream"
	fi
	if grep -qF "$secret" "$AUTH_LOG"; then
		fail "the stub's log holds a secret; the phase's own check is broken"
	fi
done
note "the stub's requests"
cat "$AUTH_LOG"
expect_file_has "$AUTH_LOG" -F "client_id=smoke-client redirect_uri=http://localhost:$CB_PORT/callback ok=False" "the first exchange did not present the entered client on its port"
expect_file_has "$AUTH_LOG" -F "client_id=smoke-client redirect_uri=http://localhost:$CB_PORT/callback ok=True" "the corrected exchange did not present the right secret"
if grep -qF "/oauth/register" "$AUTH_LOG"; then
	fail "the sign-in tried to register a client with a server that registers none"
fi
tmux kill-session -t "$S162" 2>/dev/null
