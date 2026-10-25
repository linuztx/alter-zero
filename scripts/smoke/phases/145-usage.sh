#!/usr/bin/env bash
# Phase 145 — the `/usage` page: every signed-in subscription's limits, against a local stub

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# `/usage` (docs/usage-limits.md) reads each signed-in subscription's limits
# from the provider's own usage endpoint. One stub here answers all three
# wire shapes — ChatGPT's `/wham/usage` (behind its token refresh, through
# ALTER_ZERO_OPENAI_ISSUER and a standalone providers file pointing the
# ChatGPT base at it), Copilot's `copilot_internal/user`
# (ALTER_ZERO_GITHUB_API_BASE) and ClinePass's `usage-limits` + `plan`
# (ALTER_ZERO_CLINE_API_BASE) — while the Anthropic Console sign-in needs no
# endpoint at all: it bills per token, so the page says so and asks nobody.
# What this phase pins:
#
# - the page opens over the four sign-ins, each section under its name and
#   plan: a five-hour window at 80% beside a weekly one, a used-up ClinePass
#   week in words, the Console's pay-as-you-go sentence;
# - each request is authenticated the way the provider's own client does it,
#   off the stub's log: ChatGPT's minted bearer and its account header,
#   Copilot's stored GitHub token itself (`token …`, the API version pinned),
#   Cline's `workos:` bearer — and ChatGPT's rotated refresh token is written
#   back to the key store;
# - a refusal is worded, never a raw body: the stub refuses Copilot's first
#   read with a 401, which names /login; `r` fetches again and the section
#   recovers to its premium requests and their counts;
# - `c` copies the highlighted section's usage link with a toast, the page
#   staying open; Esc returns the composer;
# - the page opens mid-turn under the running turn's status line, and on a
#   terminal too short for it, as one title row over the folded sections;
# - with no subscription signed in, `/usage` opens nothing: a toast says how
#   to sign in.
S145="${S}_usage"
U_LOG="$SMOKE_TMP/usage.log"
U_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$U_PORT" "$U_LOG" <<'PY' &
import base64, json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port, log = int(sys.argv[1]), sys.argv[2]
copilot_reads = 0

def b64(obj):
    return base64.urlsafe_b64encode(json.dumps(obj).encode()).decode().rstrip("=")

# A token whose claims name the ChatGPT account the backend routes on.
ACCESS = ".".join([b64({"alg": "none"}), b64({
    "exp": 4102444800,
    "https://api.openai.com/auth": {"chatgpt_account_id": "acct-145", "chatgpt_plan_type": "plus"},
}), "sig"])
NOW = __import__("time").time()

def iso(seconds_ahead):
    stamp = __import__("time").gmtime(NOW + seconds_ahead)
    return __import__("time").strftime("%Y-%m-%dT%H:%M:%SZ", stamp)

class Stub(BaseHTTPRequestHandler):
    def note(self):
        auth = self.headers.get("authorization") or ""
        head = auth.split(" ", 1)[0]
        cred = auth.split(" ", 1)[1][:8] if " " in auth else ""
        extra = ""
        for name in ("chatgpt-account-id", "x-github-api-version", "editor-version"):
            if self.headers.get(name):
                extra += f" {name}={self.headers.get(name)}"
        with open(log, "a") as f:
            f.write(f"{self.command} {self.path} auth={head}:{cred}{extra}\n")

    def do_GET(self):
        global copilot_reads
        self.note()
        if self.path == "/backend-api/wham/usage":
            self.reply(200, {"plan_type": "plus", "rate_limit": {
                "allowed": True, "limit_reached": False,
                "primary_window": {"used_percent": 80, "limit_window_seconds": 18000,
                                   "reset_after_seconds": 4000, "reset_at": int(NOW) + 4000},
                "secondary_window": {"used_percent": 30, "limit_window_seconds": 604800,
                                     "reset_after_seconds": 300000, "reset_at": int(NOW) + 300000}},
                "credits": {"has_credits": False, "unlimited": False, "balance": None}})
        elif self.path == "/copilot_internal/user":
            copilot_reads += 1
            if copilot_reads == 1:
                self.reply(401, {"message": "Bad credentials"})
                return
            self.reply(200, {"access_type_sku": "monthly_subscriber_quota", "copilot_plan": "individual",
                             "quota_reset_date_utc": iso(18 * 86400 + 600),
                             "quota_snapshots": {
                                 "chat": {"entitlement": 0, "unlimited": True, "percent_remaining": 100},
                                 "completions": {"entitlement": 0, "unlimited": True, "percent_remaining": 100},
                                 "premium_interactions": {"entitlement": 300, "quota_remaining": 152.0,
                                                          "percent_remaining": 50.6, "unlimited": False}}})
        elif self.path == "/api/v1/users/me/plan/usage-limits":
            self.reply(200, {"success": True, "data": {"limits": [
                {"type": "five_hour", "percentUsed": 10, "resetsAt": iso(2 * 3600 + 600)},
                {"type": "weekly", "percentUsed": 100, "resetsAt": iso(2 * 86400 + 600)},
                {"type": "monthly", "percentUsed": 81, "resetsAt": iso(18 * 86400 + 600)}]}})
        elif self.path == "/api/v1/users/me/plan":
            self.reply(200, {"success": True, "data": {"plan": {"displayName": "Cline Pass (Monthly)",
                                                                "interval": "Monthly"}}})
        else:
            self.reply(404, {})

    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length") or 0))
        self.note()
        if self.path == "/oauth/token":
            self.reply(200, {"access_token": ACCESS, "refresh_token": "chatgpt-refresh-145b",
                             "id_token": ACCESS, "expires_in": 3600})
        elif self.path == "/api/v1/auth/refresh":
            self.reply(200, {"success": True, "data": {
                "accessToken": ACCESS, "refreshToken": "cline-refresh-145",
                "tokenType": "Bearer", "expiresAt": "2099-01-01T00:00:00.000Z"}})
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
U_SERVER=$!
smoke_on_exit kill "$U_SERVER"
for _ in $(seq 1 30); do # wait for the stub to listen
	if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $U_PORT)) == 0 else 1)" 2>/dev/null; then
		break
	fi
	sleep 0.1
done
U_BASE="http://127.0.0.1:$U_PORT"

# The four sign-ins, stand-alone (ALTER_ZERO_PROVIDERS_FILE replaces the
# shipped table): ChatGPT's base on the stub, the rest on their own hosts —
# which the overrides above move — or none at all (the Console's).
U_PROVIDERS="$SMOKE_TMP/providers.toml"
cat >"$U_PROVIDERS" <<TOML
[providers.chatgpt_codex]
name = "ChatGPT Codex"
auth = "chatgpt_codex"
wire_api = "responses"
api_key_env = "CHATGPT_CODEX_REFRESH_TOKEN"
[providers.chatgpt_codex.kwargs]
api_base = "$U_BASE/backend-api/codex"

[providers.github_copilot]
name = "GitHub Copilot"
auth = "github_copilot"
api_key_env = "GITHUB_COPILOT_TOKEN"
[providers.github_copilot.kwargs]
api_base = "$U_BASE/copilot"

[providers.cline_pass]
name = "ClinePass"
auth = "cline_account"
api_key_env = "CLINE_ACCOUNT_REFRESH_TOKEN"
[providers.cline_pass.kwargs]
api_base = "$U_BASE/api/v1"

[providers.anthropic_console]
name = "Anthropic Console"
auth = "anthropic_console"
wire_api = "anthropic"
api_key_env = "ANTHROPIC_CONSOLE_REFRESH_TOKEN"
[providers.anthropic_console.kwargs]
api_base = "$U_BASE/anthropic/v1"
TOML
# The stored sign-ins — none of them real. The variables are unset first:
# an exported one outranks the store, and the suite's scrub only takes the
# `*_API_KEY` ones.
unset GITHUB_COPILOT_TOKEN CHATGPT_CODEX_REFRESH_TOKEN OPENAI_CHATGPT_REFRESH_TOKEN \
	CLINE_ACCOUNT_REFRESH_TOKEN ANTHROPIC_CONSOLE_REFRESH_TOKEN
cat >"$SMOKE_CFG/.env" <<'ENV'
GITHUB_COPILOT_TOKEN=ghu-smoke-145
CHATGPT_CODEX_REFRESH_TOKEN=chatgpt-refresh-145
CLINE_ACCOUNT_REFRESH_TOKEN=cline-refresh-145
ANTHROPIC_CONSOLE_REFRESH_TOKEN=console-refresh-145
ENV
# The dummy stays the backend (ALTER_ZERO_DUMMY), so nothing here chats with
# the stub — the page is the only thing that reads it. NO_PROXY keeps a
# developer's HTTP proxy out of the loopback requests.
U_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 ALTER_ZERO_DUMMY=1 ALTER_ZERO_PROVIDERS_FILE=$U_PROVIDERS ALTER_ZERO_OPENAI_ISSUER=$U_BASE ALTER_ZERO_GITHUB_API_BASE=$U_BASE ALTER_ZERO_CLINE_API_BASE=$U_BASE ALTER_ZERO_CLINE_AUTH_BASE=$U_BASE $APP_ABS"
U_WORK="$(work_dir)"
launch -c "$U_WORK" "$S145" 120 44 "$U_APP"

# (a) The palette row, then the page over the four sign-ins.
type_text "$S145" "/usa"
palette="$(wait_pane 5 "$S145" -F "Show your subscriptions' usage limits")" ||
	fail "the palette did not offer /usage"
keys "$S145" Enter
wait_for 10 "$S145" -F "Usage limits" || fail "/usage did not open the page"
# Every section answers on its own worker: wait for the last of them.
wait_for 10 "$S145" -F "Monthly limit" || fail "the ClinePass read never answered"
wait_for 10 "$S145" -F "refused the stored sign-in" || fail "the Copilot read never answered"
page="$(wait_settled 10 "$S145" -F "5-hour limit")" || fail "the ChatGPT read never answered"
dump "the page" "$page"
expect_has "$page" -F "Usage limits" "the page has no title"
expect_has "$page" -F "4 subscriptions" "the subtitle does not count the sign-ins"
expect_has "$page" -F "ChatGPT Codex  Plus" "the ChatGPT section does not name its plan"
expect_has "$page" -E "5-hour limit +(█|░)+ +80% used  resets in 1h 7m" "the five-hour window is not its share and reset"
expect_has "$page" -E "Weekly limit +(█|░)+ +30% used  resets in 3d 11h" "the weekly window is not its share and reset"
expect_has "$page" -F "ClinePass  Monthly" "the ClinePass section does not name its plan"
expect_has "$page" -E "100% used  limit reached · resets in 2d" "the used-up ClinePass week does not say so"
expect_has "$page" -F "Anthropic Console  Pay as you go" "the Console section is not pay as you go"
expect_has "$page" -F "Billed per token to your API organisation — no plan limits" "the Console section does not say why"

# (b) Copilot's first read was refused: worded, naming /login.
expect_has "$page" -F "GitHub refused the stored sign-in — run /login to sign in again" \
	"the refused Copilot read was not worded"
expect_lacks "$page" -F "Bad credentials" "the raw refusal body reached the page"

# Each request authenticated the way the provider's own client does it.
log="$(cat "$U_LOG")"
dump "the stub's log" "$log"
expect_has "$log" -F "POST /oauth/token" "ChatGPT's access token was never minted"
expect_has "$log" -E "GET /backend-api/wham/usage auth=Bearer:eyJhbGci chatgpt-account-id=acct-145" \
	"the ChatGPT read lacked its bearer or its account header"
expect_has "$log" -E "GET /copilot_internal/user auth=token:ghu-smok x-github-api-version=2025-04-01 editor-version=vscode/" \
	"the Copilot read did not send the stored GitHub token with its identity"
expect_has "$log" -E "GET /api/v1/users/me/plan/usage-limits auth=Bearer:workos:e" \
	"the ClinePass read did not send the workos bearer"
expect_lacks "$log" -F "/anthropic" "the pay-as-you-go sign-in was asked about limits"
expect_has "$(cat "$SMOKE_CFG/.env")" -F "CHATGPT_CODEX_REFRESH_TOKEN=chatgpt-refresh-145b" \
	"ChatGPT's rotated refresh token was not written back"

# (c) `r` fetches again; the refused section recovers.
keys "$S145" r
fresh="$(wait_pane 10 "$S145" -F "148 of 300 requests")" || fail "r did not refetch the refused section"
dump "after r" "$fresh"
expect_has "$fresh" -E "Premium requests +(█|░)+ +49% used" "the recovered section lacks its premium requests"
expect_has "$fresh" -F "GitHub Copilot  Pro" "the recovered section lacks its plan"
expect_lacks "$fresh" -F "refused the stored sign-in" "the old refusal outlived the recovery"
expect_eq "$(grep -c "GET /copilot_internal/user" "$U_LOG")" 2 "r did not read Copilot exactly once more"

# (d) `c` copies the highlighted section's link, the page staying open.
keys "$S145" c
copied="$(wait_pane 5 "$S145" -F "Copied the usage link to clipboard")" || fail "c did not copy the usage link"
expect_has "$copied" -F "Usage limits" "copying closed the page"
keys "$S145" Escape
wait_for 5 "$S145" -E "^❯ *$" || fail "Esc did not return the composer"

# (e) Mid-turn: the page under the running turn's status line.
submit "$S145" "$USER_MSG"
wait_for 5 "$S145" -F "esc to interrupt" || fail "the dummy's turn never started"
submit "$S145" "/usage"
mid="$(wait_pane 5 "$S145" -F "Usage limits")" || fail "/usage did not open mid-turn"
dump "mid-turn" "$mid"
expect_has "$mid" -F "esc to interrupt" "the running turn's status line is not above the page"
keys "$S145" Escape
wait_summaries 20 "$S145" 1 >/dev/null || fail "the turn under the page never settled"

# (f) A terminal too short for the page: one title row over the folded rows.
tmux resize-window -t "$S145" -x 120 -y 9
sleep 0.5
submit "$S145" "/usage"
short="$(wait_pane 5 "$S145" -F "Usage limits · ")" || fail "the short page has no title row"
dump "on 9 rows" "$short"
expect_has "$short" -F "❯ Anthropic Console  Pay as you go" "the short page lacks the highlighted section"
expect_has "$short" -F "ClinePass  Monthly · Weekly limit 100%" "the short page does not fold a section to its tightest window"
expect_has "$short" -F "esc close" "the short page lacks its hint"
keys "$S145" Escape
tmux resize-window -t "$S145" -x 120 -y 44
tmux kill-session -t "$S145"

# (g) Nothing signed in: no page, a toast saying how to sign in.
: >"$SMOKE_CFG/.env"
S145E="${S145}_empty"
launch -c "$U_WORK" "$S145E" 100 24 "$U_APP"
submit "$S145E" "/usage"
empty="$(wait_pane 5 "$S145E" -F "No subscription signed in")" || fail "/usage with no sign-in raised no toast"
expect_lacks "$empty" -F "Usage limits" "/usage opened a page with nothing to show"
tmux kill-session -t "$S145E"
