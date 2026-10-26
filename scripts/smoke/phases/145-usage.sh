#!/usr/bin/env bash
# Phase 145 — the `/usage` page: what is left of each subscription

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The `/usage` page (docs/usage.md): a read-only, composer-replacing page
# listing every subscription this machine is signed in to, each read from
# its own provider's usage endpoint on a worker as the page opens — a bar per
# limit, the share used in the bar's colour, the reset, credits — with the
# one the session runs on first and tagged `in use`.
#
# Against a local stub standing in for all three backends that answer — the
# ChatGPT token mint and `/backend-api/wham/usage`
# (`ALTER_ZERO_OPENAI_ISSUER` + the provider's base), GitHub's API
# (`ALTER_ZERO_GITHUB_API_BASE`) and Cline's (`ALTER_ZERO_CLINE_API_BASE` +
# the provider's base) — with all four subscriptions signed in:
#   (a) the palette lists /usage, and it opens with a block per subscription:
#       ChatGPT's 5-hour and weekly windows and its credits, ClinePass's
#       three windows against the plan's caps, Copilot's refusal worded as
#       a sign-in to redo, and the Anthropic Console's row saying it bills
#       per token — that one asked nothing of any server;
#   (b) the spent ClinePass window's bar is red, the 76% weekly window's
#       amber, read off the raw SGR bytes;
#   (c) ↓ moves the highlight and the dashboard link follows it;
#   (d) `r` reads everything again and the new numbers land, the ChatGPT
#       read carrying the minted bearer and the account id the token names,
#       Copilot's the stored GitHub token itself — no exchange spent — and
#       the refresh token the mint rotated is written back to the store;
#   (e) a short terminal collapses the other subscriptions to their header
#       rows, each naming its tightest limit;
#   (f) Esc closes the page back to the composer;
# Then against the offline dummy, nothing signed in:
#   (g) /usage opens no page — a toast names /login.
S145="${S}_usage"
U_LOG="$SMOKE_TMP/usage.log"
U_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$U_PORT" "$U_LOG" <<'PY' &
import base64, datetime, json, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer

port, log = int(sys.argv[1]), sys.argv[2]
reads = {"wham": 0}
now = int(time.time())

def b64url(raw):
    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode()

def iso(at):
    return datetime.datetime.fromtimestamp(at, datetime.timezone.utc).isoformat().replace("+00:00", "Z")

# An access token whose claims name the seat; the app never verifies the
# signature (docs/chatgpt.md), only reads the payload.
claims = {
    "https://api.openai.com/auth": {"chatgpt_account_id": "acct-145", "chatgpt_plan_type": "plus"},
    "exp": 4102444800,
}
jwt = ".".join([b64url(b'{"alg":"none"}'), b64url(json.dumps(claims).encode()), b64url(b"sig")])

def wham():
    # The second read reports more of the 5-hour window used, so a refresh
    # is visible on screen.
    reads["wham"] += 1
    return {
        "email": "dev@example.com", "plan_type": "plus",
        "rate_limit": {"allowed": True, "limit_reached": False,
            "primary_window": {"used_percent": 42 if reads["wham"] == 1 else 57,
                               "limit_window_seconds": 18000, "reset_after_seconds": 3600,
                               "reset_at": now + 3600},
            "secondary_window": {"used_percent": 76, "limit_window_seconds": 604800,
                                 "reset_after_seconds": 4 * 86400, "reset_at": now + 4 * 86400}},
        "credits": {"has_credits": True, "unlimited": False, "balance": "1234.0",
                    "approx_local_messages": [300, 1500]},
    }

LIMITS = {"success": True, "data": {"limits": [
    {"type": "five_hour", "percentUsed": 12},
    {"type": "weekly", "percentUsed": 100, "resetsAt": iso(now + 86400 + 3600)},
    {"type": "monthly", "percentUsed": 91, "resetsAt": iso(now + 20 * 86400)}]}}
PLAN = {"success": True, "data": {"plan": {
    "displayName": "Cline Pass (Monthly)",
    "entitlements": {"cline_pass": {"enabled": True, "inferenceCapThreshold": {
        "last5HoursUsageCostUSDPerUser": 1000000000,
        "last7daysUsageCostUSDPerUser": 2500000000,
        "last30daysUsageCostUSDPerUser": 5000000000}}}}}}

class Stub(BaseHTTPRequestHandler):
    def note(self, method):
        auth = self.headers.get("authorization") or "-"
        account = self.headers.get("chatgpt-account-id") or "-"
        with open(log, "a") as f:
            f.write(f"{method} {self.path} auth={auth} account={account}\n")

    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length") or 0))
        self.note("POST")
        if self.path == "/oauth/token":
            # The mint rotates the refresh token, as OpenAI may.
            self.reply(200, {"access_token": jwt, "refresh_token": "rt-145-rotated",
                             "expires_in": 3600})
        elif self.path == "/api/v1/auth/refresh":
            self.reply(200, {"success": True, "data": {
                "accessToken": "cline-access-145", "refreshToken": "cline-refresh-145",
                "tokenType": "Bearer", "expiresAt": "2999-01-01T00:00:00.000Z"}})
        else:
            self.reply(404, {})

    def do_GET(self):
        self.note("GET")
        if self.path == "/backend-api/wham/usage":
            self.reply(200, wham())
        elif self.path.startswith("/backend-api/codex/models"):
            self.reply(200, {"models": [{"slug": "gpt-5.6-sol", "display_name": "GPT-5.6 Sol"}]})
        elif self.path == "/api/v1/users/me/plan/usage-limits":
            self.reply(200, LIMITS)
        elif self.path == "/api/v1/users/me/plan":
            self.reply(200, PLAN)
        elif self.path == "/copilot_internal/user":
            # A token GitHub no longer honours.
            self.reply(401, {"message": "Bad credentials"})
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
# The four subscriptions, every base at the stub, so no request can leave the
# machine — the Anthropic Console's included, though its row asks nothing.
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
api_base = "$U_BASE"

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
api_base = "$U_BASE/v1"
TOML
# NO_PROXY keeps a developer's HTTP proxy out of the loopback requests. The
# provider and model are pinned so ChatGPT is the one in use.
U_DIR="$(work_dir usage)"
U_ENV="NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 ALTER_ZERO_OPENAI_ISSUER=$U_BASE ALTER_ZERO_GITHUB_API_BASE=$U_BASE ALTER_ZERO_CLINE_API_BASE=$U_BASE ALTER_ZERO_CLINE_AUTH_BASE=$U_BASE ALTER_ZERO_PROVIDERS_FILE=$U_PROVIDERS ALTER_ZERO_PROVIDER=chatgpt_codex ALTER_ZERO_MODEL=gpt-5.6-sol CHATGPT_CODEX_REFRESH_TOKEN=rt-145 GITHUB_COPILOT_TOKEN=ghu_145 CLINE_ACCOUNT_REFRESH_TOKEN=cline-145 ANTHROPIC_CONSOLE_REFRESH_TOKEN=sk-ant-ort01-145"
launch -c "$U_DIR" "$S145" 100 40 "env $U_ENV $APP_ABS"

# ---- (a) the palette lists it, and it opens over every subscription ----
type_text "$S145" "/usa"
palette="$(wait_pane 5 "$S145" -E '^ */usage')" || fail "the palette did not list /usage"
dump "the palette on /usa" "$palette"
expect_has "$palette" -F "Show your subscriptions' usage limits" "the /usage row lost its description"
keys "$S145" Enter
page="$(wait_pane 10 "$S145" -F "as of")" || fail "the /usage page never finished reading"
dump "the /usage page" "$page"
expect_has "$page" -F "Subscription usage" "the page lost its title"
expect_has "$page" -F "4 subscriptions · as of" "the subtitle does not count the four sign-ins"
expect_has "$page" -E '❯ ChatGPT Codex  Plus · dev@example.com +in use$' "the subscription in use is not first, highlighted and tagged"
expect_has "$page" -E '5-hour limit +[█░]+ +42% used  resets [0-9]+:[0-9]{2} [AP]M' "ChatGPT's 5-hour window is missing its bar, share or reset"
expect_has "$page" -E 'Weekly limit +[█░]+ +76% used  resets [A-Z][a-z]{2} [0-9]+:[0-9]{2} [AP]M' "ChatGPT's weekly window is missing its bar, share or reset"
expect_has "$page" -E 'Credits +1,234  ≈300–1,500 messages' "the credits balance is missing"
expect_has "$page" -F "ClinePass  Cline Pass (Monthly)" "ClinePass lost its plan's name"
expect_has "$page" -E '5-hour limit +[█░]+ +12% used  \$10 cap' "ClinePass's 5-hour window is missing"
expect_has "$page" -E 'Weekly limit +[█░]+ +100% used  \$25 cap · resets tomorrow' "the spent weekly window is missing"
expect_has "$page" -E 'Monthly limit +[█░]+ +91% used  \$50 cap · resets [A-Z][a-z]{2} [0-9]+' "the monthly window is missing"
expect_has "$page" -F "✗ GitHub Copilot refused the sign-in — /login signs in again." "Copilot's refusal is not worded as a sign-in to redo"
expect_has "$page" -F "Billed per token to your API organization — there's no plan limit to show." "the Console's row is missing"
expect_has "$page" -F "Details: https://chatgpt.com/settings/usage" "the dashboard of the subscription in use is missing"
expect_has "$page" -F "r refresh  ↑↓ select  esc close" "the hint is missing"
u_log="$(cat "$U_LOG" 2>/dev/null)"
expect_lacks "$u_log" -E '^[A-Z]+ /v1/' "the Anthropic Console's row made a request"

# ---- (b) the bars wear their thresholds ----
# u_bar_colour SESSION NEEDLE — the truecolour (R;G;B) of the first `█` cell
# on the first row whose plain text holds NEEDLE, read off the raw SGR bytes
# (`capture-pane -e`): the share and the bar are separate runs, so the row is
# found on its text with the escapes stripped.
u_bar_colour() {
	tmux capture-pane -t "$1" -p -e | python3 -c '
import re, sys

needle = sys.argv[1]
for line in sys.stdin:
    if needle not in re.sub(r"\x1b\[[0-9;]*m", "", line):
        continue
    fg = None
    for sgr, run in re.findall(r"\x1b\[([0-9;]*)m|([^\x1b]+)", line):
        if run:
            if "█" in run:
                print(fg or "")
                sys.exit(0)
            continue
        params, i = sgr.split(";"), 0
        while i < len(params):
            if params[i] == "38" and params[i + 1 : i + 2] == ["2"]:
                fg, i = ";".join(params[i + 2 : i + 5]), i + 5
            elif params[i] == "38" and params[i + 1 : i + 2] == ["5"]:
                fg, i = None, i + 3
            elif params[i] in ("", "0", "39"):
                fg, i = None, i + 1
            else:
                i += 1
    sys.exit(0)
' "$2"
}
# Mocha's green, yellow and red, the theme the smoke suite runs in.
expect_eq "$(u_bar_colour "$S145" '$25 cap')" "243;139;168" "the spent window's bar is not red"
expect_eq "$(u_bar_colour "$S145" '76% used')" "249;226;175" "the 76% window's bar is not amber"
expect_eq "$(u_bar_colour "$S145" '42% used')" "166;227;161" "the 42% window's bar is not green"

# ---- (c) the highlight moves, the dashboard follows ----
keys "$S145" Down
moved="$(wait_pane 5 "$S145" -F "Details: https://platform.claude.com/usage")" || fail "↓ did not move the highlight to the next subscription"
dump "after ↓" "$moved"
expect_has "$moved" -F "❯ Anthropic Console" "the second subscription did not take the marker"

# ---- (d) r reads everything again ----
keys "$S145" r
# The new number lands first; the subtitle says `refreshing…` until the last
# row's answer is in, then when the numbers are from.
wait_for 10 "$S145" -F "57% used" || fail "r did not read ChatGPT's usage again"
again="$(wait_pane 10 "$S145" -F "4 subscriptions · as of")" || fail "the refresh never settled"
dump "after r" "$again"
expect_has "$again" -F "57% used" "the refreshed number did not stay"
u_log="$(cat "$U_LOG" 2>/dev/null)"
dump "what the stub was asked" "$u_log"
reads="$(grep -c '^GET /backend-api/wham/usage ' <<<"$u_log")"
expect_eq "$reads" 2 "ChatGPT's usage was not read once per open and once per r"
expect_has "$u_log" -E '^GET /backend-api/wham/usage auth=Bearer [A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+ account=acct-145$' "the ChatGPT read did not carry the minted bearer and the account id"
expect_has "$u_log" -F "GET /copilot_internal/user auth=token ghu_145 " "Copilot's read did not carry the stored GitHub token itself"
expect_lacks "$u_log" -F "/copilot_internal/v2/token" "a Copilot token exchange was spent on a usage read"
expect_has "$u_log" -F "GET /api/v1/users/me/plan/usage-limits auth=Bearer workos:cline-access-145 " "ClinePass's read did not carry the minted workos bearer"
expect_file_has "$SMOKE_CFG/.env" -F "CHATGPT_CODEX_REFRESH_TOKEN=rt-145-rotated" "the rotated refresh token was not written back to the store"

# ---- (e) a short terminal collapses the other subscriptions ----
tmux resize-window -t "$S145" -x 100 -y 16
short="$(wait_pane 5 "$S145" -F "ClinePass  Cline Pass (Monthly) · Weekly limit 100% used")" || fail "the short page did not collapse ClinePass to its tightest limit"
dump "the page on 16 rows" "$short"
expect_has "$short" -F "ChatGPT Codex  Plus · dev@example.com · Weekly limit 76% used" "ChatGPT did not collapse to its tightest limit"
expect_has "$short" -F "❯ Anthropic Console" "the highlighted subscription is not the one left whole"
expect_has "$short" -F "r refresh  ↑↓ select  esc close" "the hint went with the rows"
tmux resize-window -t "$S145" -x 100 -y 40

# ---- (f) Esc closes back to the composer ----
keys "$S145" Escape
closed="$(wait_pane 5 "$S145" -F "~/usage")" || fail "Esc did not close the page back to the composer"
dump "after Esc" "$closed"
expect_lacks "$closed" -F "Subscription usage" "the page is still on screen after Esc"
tmux kill-session -t "$S145" 2>/dev/null

# ---- (g) nothing signed in: a toast, no page ----
# A config home of its own: the first session's store now holds the tokens
# its sign-ins rotated, and a session reading that store would read those
# subscriptions — from the real servers, with tokens the stub minted.
S145B="${S}_usage_none"
U_NONE_CFG="$SMOKE_TMP/cfg-none"
mkdir -p "$U_NONE_CFG"
launch "$S145B" 100 24 "env $CFG_ENV_NOHIST ALTER_ZERO_CONFIG_DIR=$U_NONE_CFG ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"
submit "$S145B" "/usage"
none="$(wait_pane 5 "$S145B" -F "No subscription signed in")" || fail "/usage with nothing signed in raised no toast"
dump "/usage with nothing signed in" "$none"
expect_has "$none" -F "No subscription signed in — /login → Use a subscription" "the toast does not name /login"
expect_lacks "$none" -F "Subscription usage" "an empty page opened"
tmux kill-session -t "$S145B" 2>/dev/null
