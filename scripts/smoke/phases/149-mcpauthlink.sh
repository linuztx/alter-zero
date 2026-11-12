#!/usr/bin/env bash
# Phase 149 — the /mcp SIGN-IN URL IS A CLICKABLE LINK

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The /mcp SIGN-IN URL IS A CLICKABLE LINK (docs/ui/links.md, docs/tools/mcp.md
# *Authenticate*). The /login browser page's authorize URL opened whole when
# clicked; the /mcp auth page's did not. It printed the URL as plain rows, so
# the terminal's own per-row URL detection opened the FIRST row's truncated
# fragment and the rows under it were no links at all.
#
# A stub remote MCP server demands OAuth — a 401 naming its RFC 9728
# metadata, RFC 8414 authorization-server metadata, dynamic registration — so
# the REAL flow builds a real authorize URL; the 60-col pane hard-breaks it;
# and, like Phases 87 and 113, the assertion reads the RAW byte stream
# (pipe-pane), since the escape paints as nothing. What this phase pins:
#
# - the page spells the URL across several rows, as it always did;
# - the stream carries an OSC 8 open whose URI is that WHOLE URL, at least
#   once per row it is painted on — and no open anywhere carries a piece of
#   it: the only other target is the server page's `URL:` row, a link to the
#   server's own address;
# - the link opens exactly what the browser was handed (a stand-in xdg-open
#   records it);
# - the link-id carrier never leaks as an SGR 58 underline colour.
S149="${S}_mcpauthlink"
AUTH_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
AUTH_BASE="http://127.0.0.1:$AUTH_PORT"
AUTH_LOG="$SMOKE_TMP/mcpauth.log"
python3 - "$AUTH_PORT" "$AUTH_LOG" <<'PY' &
import json, sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port, log = int(sys.argv[1]), sys.argv[2]
base = f"http://127.0.0.1:{port}"

class Stub(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def record(self):
        with open(log, "a") as f:
            f.write(f"{self.command} {self.path}\n")

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
            self.reply(200, {"issuer": base,
                             "authorization_endpoint": f"{base}/oauth/authorize",
                             "token_endpoint": f"{base}/oauth/token",
                             "registration_endpoint": f"{base}/oauth/register",
                             "response_types_supported": ["code"],
                             "code_challenge_methods_supported": ["S256"],
                             "token_endpoint_auth_methods_supported": ["none"]})
        else:
            self.reply(404, {})

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0))
        self.record()
        if self.path == "/mcp":
            challenge = f'Bearer resource_metadata="{base}/.well-known/oauth-protected-resource"'
            self.reply(401, {"error": "unauthorized"}, [("WWW-Authenticate", challenge)])
        elif self.path == "/oauth/register":
            asked = json.loads(body or b"{}")
            self.reply(201, {"client_id": "smoke-149-dynamically-registered-client",
                             "token_endpoint_auth_method": "none",
                             "redirect_uris": asked.get("redirect_uris", [])})
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

# The browser the flow opens: a stand-in on PATH that records the URL it was
# handed (`xdg-open` on Linux, `open` on macOS — llm::mcp::manager).
AUTH_BIN="$SMOKE_TMP/bin"
BROWSER_LOG="$SMOKE_TMP/browser.log"
mkdir -p "$AUTH_BIN"
for opener in xdg-open open; do
	cat >"$AUTH_BIN/$opener" <<OPENER
#!/bin/sh
printf '%s\n' "\$1" >> '$BROWSER_LOG'
OPENER
	chmod +x "$AUTH_BIN/$opener"
done

cat >"$SMOKE_CFG/mcp.json" <<MCPJSON
{"mcpServers": {"stub": {"type": "http", "url": "$AUTH_BASE/mcp"}}}
MCPJSON

AUTH_RAW="$SMOKE_TMP/mcpauth.raw"
: >"$AUTH_RAW"
launch "$S149" 60 40 "env PATH='$AUTH_BIN:$PATH' $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux pipe-pane -t "$S149" -o "cat >> '$AUTH_RAW'"
type_text "$S149" "/mcp"
sleep "$SMOKE_TYPE_SETTLE"
keys "$S149" Enter
# The startup connect runs on a worker; the 401 resolves the row.
if ! wait_for 15 "$S149" -F "needs authentication"; then
	dump "$S149"
	fail "the stub server never read as needing authentication"
fi
keys "$S149" Enter # the server's detail
if ! wait_for 5 "$S149" -F "1. Authenticate"; then
	dump "$S149"
	fail "the server page offers no Authenticate action"
fi
keys "$S149" Enter # Authenticate
# The browser stand-in is handed the authorize URL as soon as the flow has
# built it — the one URL the page and its link are held to below.
browser_url=""
if wait_file 15 "$BROWSER_LOG" -F "$AUTH_BASE/oauth/authorize"; then
	browser_url="$(head -1 "$BROWSER_LOG")"
else
	fail "the flow never handed the browser its URL"
fi

# The URL as the page spells it: the rows from the one its scheme opens down
# to the blank row under them, each without its inset. Polled until it is the
# browser's URL whole, since a capture taken while tmux is still part-way
# through a repaint (a loaded machine splits the frame) spells half of the
# new page over half of the old one.
auth_page=""
shown_url=""
shown_rows=0
spell_url() {
	local spelled
	auth_page="$(tmux capture-pane -t "$S149" -p)"
	spelled="$(printf '%s\n' "$auth_page" | awk -v head="$AUTH_BASE/oauth/authorize" '
		!on && index($0, head) { on = 1 }
		on && /^[[:space:]]*$/ { exit }
		on { sub(/^[[:space:]]+/, ""); sub(/[[:space:]]+$/, ""); printf "%s", $0; rows++ }
		END { printf "\n%d\n", rows }')"
	shown_url="$(printf '%s\n' "$spelled" | sed -n 1p)"
	shown_rows="$(printf '%s\n' "$spelled" | sed -n 2p)"
	[ -n "$browser_url" ] && [ "$shown_url" = "$browser_url" ]
}
if ! poll 10 spell_url; then
	fail "the page never spelled the URL the browser was handed (it spelled '$shown_url')"
fi
note "the auth page in a 60-col pane"
printf '%s\n' "$auth_page"
expect_has "$auth_page" -F "Authenticating with stub" "the auth page never opened"
expect_has "$auth_page" -E "^ *$AUTH_BASE/oauth/authorize\?" "the authorize URL does not open its own row"
note "the URL the page spells across ${shown_rows:-0} rows"
printf '%s\n' "$shown_url"
[ "${shown_rows:-0}" -ge 2 ] || fail "the URL fits one row; narrow the pane so this phase tests the split"
for want in "response_type=code" "client_id=smoke-149-dynamically-registered-client" \
	"code_challenge_method=S256" "&state=" "&resource=" "scope=mcp%3Atools"; do
	expect_has "$shown_url" -F "$want" "the authorize URL is missing \"$want\""
done

# The raw stream: every OSC 8 open, as its URI alone. pipe-pane copies the
# stream on its own schedule, so wait for the painted frame's opens to land.
link_targets=""
read_links() {
	link_targets="$(grep -aoE $']8;id=az[0-9]+;[^\x1b]*' "$AUTH_RAW" | sed -E 's/^\]8;id=az[0-9]+;//')"
	[ -n "$shown_url" ] &&
		[ "$(printf '%s\n' "$link_targets" | grep -cxF "$shown_url")" -ge "${shown_rows:-1}" ]
}
poll 5 read_links
note "the OSC 8 targets in the raw stream"
if [ -n "$link_targets" ]; then printf '%s\n' "$link_targets" | sort | uniq -c; else echo "(none)"; fi
# Every open is the whole authorize URL — the one the page spells and the
# browser was handed — or the server page's address; never a row's fragment
# of either.
strays="$(printf '%s\n' "$link_targets" | grep -v '^$' | grep -vxF -e "$shown_url" -e "$AUTH_BASE/mcp")"
expect_eq "$strays" "" "an OSC 8 open carries something other than a whole URL the walk shows"
opens="$(printf '%s\n' "$link_targets" | grep -cxF "$shown_url")"
[ "$opens" -ge "${shown_rows:-1}" ] ||
	fail "only $opens OSC 8 open(s) carry the whole URL; each of its ${shown_rows:-?} rows needs one"
expect_has "$link_targets" -xF "$AUTH_BASE/mcp" "the server page's URL: row is no link to the server's address"
if ! grep -aqF $'\x1b]8;;\x1b' "$AUTH_RAW"; then
	fail "the OSC 8 close is missing from the raw stream"
fi
if grep -aqF $'\x1b[58;' "$AUTH_RAW"; then
	fail "the link-id carrier leaked as an SGR 58 underline colour"
fi

# Esc backs out of the sign-in to the server page.
keys "$S149" Escape
if ! wait_for 5 "$S149" -F "1. Authenticate"; then
	dump "$S149"
	fail "Esc did not return to the server page"
fi
tmux kill-session -t "$S149" 2>/dev/null
