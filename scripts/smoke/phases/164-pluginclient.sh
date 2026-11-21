#!/usr/bin/env bash
# Phase 164 — a codex plugin's OAUTH CLIENT: an unfilled one asked for in /mcp, a filled one used as written

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# A CODEX PLUGIN'S OAUTH CLIENT (docs/mcp.md *Pre-registered clients*,
# docs/plugins.md). codex's plugins write a remote server's sign-in in
# codex's own spellings — `oauth.client_id` / `client_secret` /
# `callback_port` / `callback_url`, the scopes beside the entry's `url` — and
# its catalog ships several with the client left as a template's slot
# (`<GMAIL_PUBLIC_CLIENT_ID>`). None of that used to be read.
#
# A marketplace in codex's own format (`.agents/plugins/marketplace.json`,
# `.codex-plugin/plugin.json`) holds two plugins, each with one remote
# server behind a stub authorization server that registers no clients:
#
# - `mail`: the Gmail plugin's shape — scopes, a callback port, and a client
#   id and secret that are both unfilled slots;
# - `hub`: the GitHub plugin's shape — a real client id and secret, a
#   callback port and the callback URL that client registered.
#
# What this phase pins, on the real binary:
#
# - `mail`: Authenticate ends on the client form, on the PLUGIN'S port, with
#   no trace of the placeholder; the client typed in signs in with the
#   plugin's scopes on that port and is kept with the tokens — the plugin's
#   file is never written;
# - `hub`: Authenticate asks for nothing — the plugin's own client signs in,
#   and the redirect it sends, on the authorize URL and at the token
#   exchange, is the plugin's callback URL, answered on that URL's path; its
#   secret is never drawn;
# - `hub` again: Change OAuth client opens the whole form over the plugin's
#   own client, and Enter through it unedited keeps nothing — the next
#   sign-in still sends the plugin's callback URL, where an entered copy of
#   the client would have sent http://localhost:{port}/callback.
S164="${S}_pluginclient"
free_port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }
AUTH_PORT="$(free_port)"
AUTH_BASE="http://127.0.0.1:$AUTH_PORT"
AUTH_LOG="$SMOKE_TMP/pluginclient.log"
MAIL_PORT="$(free_port)"
HUB_PORT="$(free_port)"
HUB_CALLBACK="http://127.0.0.1:$HUB_PORT/callback/smoke123"
python3 - "$AUTH_PORT" "$AUTH_LOG" <<'PY' &
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port, log = int(sys.argv[1]), sys.argv[2]
base = f"http://127.0.0.1:{port}"
# client id -> (its secret, the token it is given); a server's path -> the
# token it takes.
CLIENTS = {"smoke-client": ("right-secret", "tok-mail"), "hub-client": ("hub-secret", "tok-hub")}
SERVERS = {"/mail": "tok-mail", "/hub": "tok-hub"}
WELL_KNOWN = "/.well-known/oauth-protected-resource"

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
        if self.path.startswith(WELL_KNOWN) and self.path[len(WELL_KNOWN):] in SERVERS:
            # Each server's own document: its `resource` is its own address.
            self.reply(200, {"resource": base + self.path[len(WELL_KNOWN):],
                             "authorization_servers": [base]})
        elif self.path == "/.well-known/oauth-authorization-server":
            # No registration_endpoint: this authorization server registers
            # no clients itself.
            self.reply(200, {"issuer": base,
                             "authorization_endpoint": f"{base}/oauth/authorize",
                             "token_endpoint": f"{base}/oauth/token",
                             "response_types_supported": ["code"],
                             "code_challenge_methods_supported": ["S256"],
                             "token_endpoint_auth_methods_supported": ["client_secret_post"]})
        else:
            self.reply(404, {})

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0))
        if self.path in SERVERS:
            if self.headers.get("Authorization") != f"Bearer {SERVERS[self.path]}":
                self.record("401")
                challenge = f'Bearer resource_metadata="{base}{WELL_KNOWN}{self.path}"'
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
            secret, token = CLIENTS.get(form.get("client_id"), (None, None))
            known = secret is not None and form.get("client_secret") == secret
            # The log names the client and the redirect, never a secret.
            self.record(f"client_id={form.get('client_id')} redirect_uri={form.get('redirect_uri')} ok={known}")
            if known:
                self.reply(200, {"access_token": token, "token_type": "Bearer",
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

# The marketplace, in codex's own format.
MARKET="$SMOKE_TMP/market"
mkdir -p "$MARKET/.agents/plugins" "$MARKET/plugins/mail/.codex-plugin" "$MARKET/plugins/hub/.codex-plugin"
cat >"$MARKET/.agents/plugins/marketplace.json" <<'JSON'
{ "name": "codexish", "interface": { "displayName": "Codexish" }, "plugins": [
  { "name": "mail", "source": { "source": "local", "path": "./plugins/mail" },
    "policy": { "installation": "AVAILABLE", "authentication": "ON_INSTALL" }, "category": "Communication" },
  { "name": "hub", "source": { "source": "local", "path": "./plugins/hub" },
    "policy": { "installation": "AVAILABLE", "authentication": "ON_INSTALL" }, "category": "Developer Tools" } ] }
JSON
printf '{ "name": "mail", "version": "0.1.0", "description": "Mail over MCP" }\n' >"$MARKET/plugins/mail/.codex-plugin/plugin.json"
printf '{ "name": "hub", "version": "0.1.0", "description": "A code host over MCP" }\n' >"$MARKET/plugins/hub/.codex-plugin/plugin.json"
# The Gmail plugin's shape: the client an unfilled slot.
cat >"$MARKET/plugins/mail/.mcp.json" <<JSON
{
  "mcpServers": {
    "mail": {
      "type": "http",
      "url": "$AUTH_BASE/mail",
      "scopes": ["mail.read", "mail.send"],
      "oauth": {
        "callback_port": $MAIL_PORT,
        "client_id": "<MAIL_PUBLIC_CLIENT_ID>",
        "client_secret": "<MAIL_CLIENT_SECRET>"
      }
    }
  }
}
JSON
# The GitHub plugin's shape: a client of the plugin's own, with the callback
# URL it registered.
cat >"$MARKET/plugins/hub/.mcp.json" <<JSON
{
  "mcpServers": {
    "hub": {
      "type": "http",
      "url": "$AUTH_BASE/hub",
      "oauth": {
        "client_id": "hub-client",
        "client_secret": "hub-secret",
        "callback_port": $HUB_PORT,
        "callback_url": "$HUB_CALLBACK"
      }
    }
  }
}
JSON
cp "$MARKET/plugins/mail/.mcp.json" "$SMOKE_TMP/mail.mcp.json.before"
STORE="$SMOKE_CFG/mcp-auth.json"
# The `plugin` subcommand, on this phase's config home.
az() { env $CFG_ENV_NOHIST "$BIN" "$@"; }

# The newest authorize URL the page has linked: every OSC 8 open in the raw
# stream carries its whole target.
RAW=""
authorize_url() {
	grep -aoE $']8;id=az[0-9]+;[^\x1b]*' "$RAW" | sed -E 's/^\]8;id=az[0-9]+;//' |
		grep -F "$AUTH_BASE/oauth/authorize" | tail -n 1
}
new_url=""
next_authorize_url() {
	fresh() {
		new_url="$(authorize_url)"
		[ -n "$new_url" ]
	}
	poll 15 fresh
}
# Wait for an authorize URL other than `$1` (the previous attempt's).
next_authorize_url_after() {
	local previous="$1"
	fresher() {
		new_url="$(authorize_url)"
		[ -n "$new_url" ] && [ "$new_url" != "$previous" ]
	}
	poll 15 fresher
}
# Play the browser: follow `$1`'s redirect back to the app with `$2` as the
# authorization server's answer.
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
# One query parameter of an authorize URL, decoded.
url_param() {
	python3 - "$1" "$2" <<'PY'
import sys, urllib.parse
values = urllib.parse.parse_qs(urllib.parse.urlparse(sys.argv[1]).query).get(sys.argv[2])
print(values[0] if values else "")
PY
}
# Launch on the one plugin installed, answer the review over its server, and
# open that server's page in /mcp.
open_server_page() {
	local session="$1" server="$2"
	# The review stands in the composer's place on the first frame.
	if ! launch -w "found in your plugins" "$session" 100 44 "$APP"; then
		dump "$session"
		fail "the review did not open over the plugin's server"
	fi
	tmux pipe-pane -t "$session" -o "cat >> '$RAW'"
	expect_has "$(pane "$session")" -F "$server · http" "the review does not name the plugin's server"
	keys "$session" Enter # enable it
	if ! wait_for 5 "$session" -E "^❯ *$"; then
		dump "$session"
		fail "the review did not close"
	fi
	type_text "$session" "/mcp"
	sleep "$SMOKE_TYPE_SETTLE"
	keys "$session" Enter
	if ! wait_for 15 "$session" -F "needs authentication"; then
		dump "$session"
		fail "$server never read as needing authentication"
	fi
	keys "$session" Enter # the server's detail
	if ! wait_for 5 "$session" -F "1. Authenticate"; then
		dump "$session"
		fail "$server's page offers no Authenticate action"
	fi
}

# --- mail: the client is an unfilled slot -----------------------------------
az plugin marketplace add "$MARKET" >/dev/null 2>&1 || fail "the marketplace could not be added"
az plugin install mail@codexish >/dev/null 2>&1 || fail "the mail plugin could not be installed"
RAW="$SMOKE_TMP/mail.raw"
: >"$RAW"
open_server_page "$S164" "plugin:mail:mail"
keys "$S164" Enter # Authenticate
if ! wait_for 15 "$S164" -F "OAuth client for plugin:mail:mail"; then
	dump "$S164"
	fail "Authenticate on a plugin's server with an unfilled client did not end on the client form"
fi
form="$(pane "$S164")"
note "the client form over a plugin's server"
printf '%s\n' "$form"
expect_has "$form" -F "Redirect URI:  http://localhost:$MAIL_PORT/callback" "the redirect URI is not on the plugin's callback port"
expect_has "$form" -F "Callback port > $MAIL_PORT" "the port field is not prefilled with the plugin's callback port"
expect_has "$form" -E "Client ID +> *$" "the client ID field is not empty"
expect_lacks "$form" -F "MAIL_PUBLIC_CLIENT_ID" "THE PLACEHOLDER IS SHOWN AS A CLIENT ID"
expect_lacks "$form" -F "MAIL_CLIENT_SECRET" "the placeholder secret is on the page"

type_text "$S164" "smoke-client" # the caret opens on the client ID
keys "$S164" Enter               # on to the secret
type_text "$S164" "right-secret"
sleep "$SMOKE_TYPE_SETTLE"
expect_lacks "$(pane "$S164")" -F "right-secret" "THE SECRET IS ON SCREEN"
keys "$S164" Enter # submit: kept, and the sign-in starts
if ! next_authorize_url; then
	dump "$S164"
	fail "no authorize URL followed the form"
fi
note "the authorize URL the entered client signs in with"
printf '%s\n' "$new_url"
expect_eq "$(url_param "$new_url" client_id)" "smoke-client" "the sign-in does not present the entered client"
expect_eq "$(url_param "$new_url" redirect_uri)" "http://localhost:$MAIL_PORT/callback" "the redirect is not on the plugin's callback port"
expect_eq "$(url_param "$new_url" scope)" "mail.read mail.send" "the sign-in does not ask for the plugin's scopes"
browser "$new_url" "code=smoke-code-1"
if ! wait_for 15 "$S164" -F "Authenticated with plugin:mail:mail"; then
	dump "$S164"
	fail "the entered client did not sign in to the plugin's server"
fi
if ! wait_for 15 "$S164" -F "✔ connected"; then
	dump "$S164"
	fail "the plugin's server did not connect after the sign-in"
fi
tmux kill-session -t "$S164" 2>/dev/null

# Kept with the tokens, under the server's address, bound to its issuer and
# the plugin's port — and the plugin's file is as it was.
python3 - "$STORE" "$AUTH_BASE" "$MAIL_PORT" <<'PY' || fail "the token store does not hold the client entered for the plugin's server"
import json, sys
store, base, port = json.load(open(sys.argv[1])), sys.argv[2], int(sys.argv[3])
client = store.get("clients", {}).get(f"{base}/mail")
if client is None:
    sys.exit(f"no client under {base}/mail: {sorted(store.get('clients', {}))}")
want = {"client_id": "smoke-client", "client_secret": "right-secret", "callback_port": port, "issuer": base}
for key, value in want.items():
    if client.get(key) != value:
        sys.exit(f"{key}: {client.get(key)!r}, expected {value!r}")
print("the entered client is kept under the plugin server's address")
PY
cmp -s "$MARKET/plugins/mail/.mcp.json" "$SMOKE_TMP/mail.mcp.json.before" || fail "the plugin's .mcp.json was written"
grep -qF "client_id=smoke-client redirect_uri=http://localhost:$MAIL_PORT/callback ok=True" "$AUTH_LOG" ||
	fail "the token exchange did not present the entered client on the plugin's port"
if grep -aqF "right-secret" "$RAW"; then
	fail "THE SECRET IS IN THE BYTE STREAM"
fi

# --- hub: the plugin names its own client and callback URL ------------------
az plugin uninstall mail@codexish >/dev/null 2>&1 || fail "the mail plugin could not be uninstalled"
az plugin install hub@codexish >/dev/null 2>&1 || fail "the hub plugin could not be installed"
RAW="$SMOKE_TMP/hub.raw"
: >"$RAW"
open_server_page "$S164" "plugin:hub:hub"
keys "$S164" Enter # Authenticate
if ! next_authorize_url; then
	dump "$S164"
	fail "a plugin's own client did not go straight to the authorize URL"
fi
note "the authorize URL the plugin's own client signs in with"
printf '%s\n' "$new_url"
expect_lacks "$(pane "$S164")" -F "OAuth client for" "a plugin that names its client was asked for one"
expect_eq "$(url_param "$new_url" client_id)" "hub-client" "the sign-in does not present the plugin's client"
expect_eq "$(url_param "$new_url" redirect_uri)" "$HUB_CALLBACK" "the redirect is not the callback URL the plugin's client registered"
browser "$new_url" "code=smoke-code-2"
if ! wait_for 15 "$S164" -F "Authenticated with plugin:hub:hub"; then
	dump "$S164"
	fail "the plugin's own client did not sign in on its callback URL"
fi
if ! wait_for 15 "$S164" -F "✔ connected"; then
	dump "$S164"
	fail "the plugin's server did not connect after the sign-in"
fi
expect_lacks "$(pane "$S164")" -F "hub-secret" "THE PLUGIN'S CLIENT SECRET IS ON SCREEN"
grep -qF "client_id=hub-client redirect_uri=$HUB_CALLBACK ok=True" "$AUTH_LOG" ||
	fail "the token exchange did not send the plugin's callback URL"

# --- hub: Change OAuth client, Enter through the unedited form --------------
# The form decides from its fields which client it describes: unedited, it
# is still the plugin's own, so nothing is kept and the next sign-in sends
# the callback URL that client registered.
hub_page="$(pane "$S164")"
note "the plugin server's page, signed in"
printf '%s\n' "$hub_page"
expect_has "$hub_page" -E "OAuth client: +hub-client" "the page does not name the plugin's client"
expect_lacks "$hub_page" -F "Forget OAuth client" "the page offers to forget the plugin's own client"
change="$(printf '%s\n' "$hub_page" | grep -oE '[0-9]+\. Change OAuth client' | grep -oE '^[0-9]+')"
if [ -z "$change" ]; then
	dump "$S164"
	fail "the page does not offer to change the plugin's client"
fi
keys "$S164" "$change"
if ! wait_for 15 "$S164" -F "Client ID     > hub-client"; then
	dump "$S164"
	fail "Change OAuth client did not open the form over the plugin's own client"
fi
form="$(pane "$S164")"
note "the form over the plugin's own client"
printf '%s\n' "$form"
expect_has "$form" -F "Callback port > $HUB_PORT" "the form does not show the plugin's callback port"
expect_has "$form" -F "Redirect URI:  $HUB_CALLBACK" "the form does not show the callback URL the plugin's client registered"
expect_has "$form" -F "Client secret > unchanged" "the form does not say the plugin's secret stays"
first_hub_url="$new_url"
keys "$S164" Enter # the caret opens on the secret: submit, unedited
if ! next_authorize_url_after "$first_hub_url"; then
	dump "$S164"
	fail "no authorize URL followed the unedited form"
fi
note "the authorize URL after Change OAuth client"
printf '%s\n' "$new_url"
expect_eq "$(url_param "$new_url" client_id)" "hub-client" "the sign-in after the unedited form does not present the plugin's client"
expect_eq "$(url_param "$new_url" redirect_uri)" "$HUB_CALLBACK" "the unedited form lost the callback URL the plugin's client registered"
browser "$new_url" "code=smoke-code-3"
signed_in_twice() {
	[ "$(grep -cF "client_id=hub-client redirect_uri=$HUB_CALLBACK ok=True" "$AUTH_LOG")" -ge 2 ]
}
if ! poll 15 signed_in_twice; then
	dump "$S164"
	fail "the plugin's client did not sign in again on its callback URL"
fi
python3 - "$STORE" "$AUTH_BASE" <<'PY' || fail "a client was kept for the plugin's server after an unedited Change"
import json, sys
store, base = json.load(open(sys.argv[1])), sys.argv[2]
kept = store.get("clients", {}).get(f"{base}/hub")
if kept is not None:
    sys.exit(f"kept for {base}/hub: {sorted(kept)}")
print("nothing was kept for the plugin's server")
PY
tmux kill-session -t "$S164" 2>/dev/null
if grep -aqF "hub-secret" "$RAW"; then
	fail "THE PLUGIN'S CLIENT SECRET IS IN THE BYTE STREAM"
fi
note "the stub's log"
cat "$AUTH_LOG"
