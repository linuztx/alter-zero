#!/usr/bin/env bash
# Phase 163 — a Codex plugin's placeholder OAuth client is asked for in /mcp

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# A CODEX PLUGIN'S OAUTH CLIENT (docs/mcp.md *Pre-registered clients*,
# docs/plugins.md). OpenAI's gmail plugin writes its server's client in
# codex's spelling — `client_id`, `client_secret`, `callback_port` — with
# placeholders (`<GMAIL_PUBLIC_CLIENT_ID>`) where the client goes, and the
# scopes on the entry itself. A placeholder is no client: the sign-in asks
# for one in /mcp, on the plugin's port, and asks the authorization server
# for the plugin's scopes.
#
# A local codex-format marketplace holds one plugin whose `.mcp.json` is the
# gmail shape pointed at a stub MCP server behind an authorization server
# that registers no clients. Installed with `alter-zero plugin`, its server
# is enabled in the startup review; then /mcp → Authenticate:
#
# - the form opens on the plugin's callback port (not the suggested 8976);
# - a client typed there signs in: the authorize URL asks for the plugin's
#   scopes, its redirect on the plugin's port; the server reads connected;
# - the client is kept in mcp-auth.json under the server's URL, and the
#   plugin's `.mcp.json` is byte-identical afterwards.
S163="${S}_mcppluginclient"
CLIENT_ID="smoke-163-client.apps.example"
SECRET="GOCSPX-smoke163-secret"
free_port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }
AUTH_PORT="$(free_port)"
CB_PORT="$(free_port)"
AUTH_BASE="http://127.0.0.1:$AUTH_PORT"
AUTH_LOG="$SMOKE_TMP/mcppluginclient.log"
STORE="$SMOKE_CFG/mcp-auth.json"
python3 - "$AUTH_PORT" "$AUTH_LOG" "$CLIENT_ID" "$SECRET" <<'PY' &
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port, log, client_id, secret = int(sys.argv[1]), sys.argv[2], sys.argv[3], sys.argv[4]
base = f"http://127.0.0.1:{port}"
TOKEN = "smoke-163-access-token"

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
            self.reply(200, {"resource": f"{base}/mcp", "authorization_servers": [base]})
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
                                 "expires_in": 3600, "refresh_token": "smoke-163-refresh"})
            else:
                self.reply(401, {"error": "invalid_client"})
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
poll 5 stub_up || fail "the stub MCP server never came up on port $AUTH_PORT"

# A codex-format marketplace in a directory: its plugin loads in place.
MARKET="$SMOKE_TMP/codex-market"
WORK="$SMOKE_TMP/work163"
mkdir -p "$MARKET/.agents/plugins" "$MARKET/plugins/mail/.codex-plugin" "$WORK"
cat >"$MARKET/.agents/plugins/marketplace.json" <<'JSON'
{ "name": "cx", "plugins": [
  { "name": "mail", "source": { "source": "local", "path": "./plugins/mail" } } ] }
JSON
cat >"$MARKET/plugins/mail/.codex-plugin/plugin.json" <<'JSON'
{ "name": "mail", "version": "0.1.0", "description": "Mail over a stub" }
JSON
PLUGIN_MCP="$MARKET/plugins/mail/.mcp.json"
cat >"$PLUGIN_MCP" <<MCPJSON
{ "mcpServers": { "gmail": {
    "type": "http",
    "url": "$AUTH_BASE/mcp",
    "scopes": ["https://mail.example/read", "https://mail.example/modify"],
    "oauth": { "callback_port": $CB_PORT,
               "client_id": "<GMAIL_PUBLIC_CLIENT_ID>",
               "client_secret": "<GMAIL_CLIENT_SECRET>" } } } }
MCPJSON
MCP_BEFORE="$(cat "$PLUGIN_MCP")"

cli() { (cd "$WORK" && env $CFG_ENV_NOHIST "$BIN_ABS" plugin "$@" 2>&1); }
out="$(cli marketplace add "$MARKET")" || {
	dump "plugin marketplace add" "$out"
	fail "the codex marketplace could not be added"
}
out="$(cli install mail)" || {
	dump "plugin install" "$out"
	fail "the codex plugin could not be installed"
}

launch -n -c "$WORK" "$S163" 100 44 "env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"
review="$(wait_pane 15 "$S163" -F "found in your plugins")" || {
	dump "the startup screen" "$(pane "$S163")"
	fail "the plugin's server was not put to the startup review"
}
dump "the review over the plugin's server" "$review"
expect_has "$review" -F "plugin:mail:gmail" "the review does not name the plugin's server"
keys "$S163" Enter
wait_for 10 "$S163" -E "^❯" || fail "the review did not close on the composer"

type_text "$S163" "/mcp"
sleep "$SMOKE_TYPE_SETTLE"
keys "$S163" Enter
list="$(wait_pane 15 "$S163" -F "plugin:mail:gmail · △ needs authentication")" || {
	dump "/mcp" "$(pane "$S163")"
	fail "the plugin's server never read as needing authentication"
}
dump "/mcp" "$list"
# Walk to the server's row (the plugin servers are listed last).
for _ in 1 2 3 4 5 6; do
	pane "$S163" | grep -qE "❯ .*plugin:mail:gmail" && break
	keys "$S163" Down
	sleep 0.2
done
keys "$S163" Enter
wait_for 5 "$S163" -F "Set OAuth client" || {
	dump "the server page" "$(pane "$S163")"
	fail "the plugin server's page did not open"
}
keys "$S163" Enter # Authenticate
form="$(wait_pane 15 "$S163" -F "registers no clients itself")" || {
	dump "the page after Authenticate" "$(pane "$S163")"
	fail "Authenticate did not turn into the client form"
}
dump "the client form" "$form"
expect_has "$form" -F "Redirect URI: http://localhost:$CB_PORT/callback" \
	"the form does not open on the plugin's callback port"
expect_lacks "$form" -F "8976" "the form suggested its own port over the plugin's"
expect_lacks "$form" -F "GMAIL_PUBLIC_CLIENT_ID" "the placeholder was offered as a client"

type_text "$S163" "$CLIENT_ID"
sleep "$SMOKE_TYPE_SETTLE"
keys "$S163" Enter
wait_for 5 "$S163" -F "❯ Client secret" || fail "Enter did not move to the secret field"
tmux set-buffer -b client "$SECRET"
tmux paste-buffer -p -b client -t "$S163"
sleep "$SMOKE_TYPE_SETTLE"
keys "$S163" Enter
wait_for 5 "$S163" -F "❯ Callback port" || fail "Enter did not move to the callback port"
keys "$S163" Enter # the plugin's port, as the form opened on it

auth_url=""
read_url() {
	auth_url="$(pane "$S163" | tr -d ' \n' | grep -oE "${AUTH_BASE}/oauth/authorize\?[^[:space:]]*scope=[A-Za-z0-9%._-]*modify" | head -1)"
	[ -n "$auth_url" ]
}
poll 15 read_url || {
	dump "the page after the form" "$(pane "$S163")"
	fail "the sign-in never showed an authorize URL"
}
python3 - "$auth_url" "$CLIENT_ID" "$CB_PORT" <<'PY' || fail "the authorize URL is not the plugin's (see above)"
import sys, urllib.parse
url, client_id, port = sys.argv[1:4]
query = urllib.parse.parse_qs(urllib.parse.urlparse(url).query)
assert query["client_id"] == [client_id], query
assert query["scope"] == ["https://mail.example/read https://mail.example/modify"], query["scope"]
assert query["redirect_uri"] == [f"http://localhost:{port}/callback"], query["redirect_uri"]
print("authorize URL: ok")
PY
state="$(printf '%s' "$auth_url" | grep -oE 'state=[A-Za-z0-9_-]+' | head -1 | cut -d= -f2)"
[ -n "$state" ] || fail "no state could be read off the authorize URL"
curl -s --noproxy '*' "http://127.0.0.1:$CB_PORT/callback?code=code-163&state=$state" >/dev/null || true
signed_in="$(wait_pane 20 "$S163" -F "✔ connected")" || {
	dump "the page after approving" "$(pane "$S163")"
	fail "the plugin's server never connected after signing in"
}
dump "the server page, signed in" "$signed_in"
expect_has "$signed_in" -F "$CLIENT_ID (entered here)" "the page does not name the entered client"
if pane "$S163" -S -600 | grep -qF "$SECRET"; then
	fail "the client secret is on screen"
fi

python3 - "$STORE" "$AUTH_BASE" "$CLIENT_ID" "$SECRET" "$CB_PORT" <<'PY' || fail "the entered client was not kept as expected (see above)"
import json, sys
store, base, client_id, secret, port = sys.argv[1:6]
client = json.load(open(store))["clients"][f"{base}/mcp"]
assert client == {"client_id": client_id, "client_secret": secret, "issuer": base,
                  "callback_port": int(port)}, client
print("kept client: ok")
PY
expect_eq "$(cat "$PLUGIN_MCP")" "$MCP_BEFORE" "the plugin's .mcp.json was written to"
tmux kill-session -t "$S163" 2>/dev/null
