#!/usr/bin/env bash
# Phase 145 — the `/usage` page: every subscription's plan limits

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The `/usage` page (docs/usage.md): a read-only, composer-replacing page
# that reads every signed-in subscription off its provider — ChatGPT's
# `/api/codex/usage` (the stub is not the ChatGPT backend, so codex's other
# path style), Copilot's `copilot_internal/user` with the GitHub token itself,
# and ClinePass's plan and usage limits — a section each, the one in use
# first, a bar per limit and when it resets.
#
# Against one local stub that plays all three (ALTER_ZERO_PROVIDERS_FILE
# points ChatGPT at it, ALTER_ZERO_GITHUB_API_BASE GitHub's API, and
# ALTER_ZERO_CLINE_API_BASE Cline's), the session running on ClinePass:
#   (a) the palette lists /usage; it opens with the three sections, ClinePass
#       first and tagged in use, every limit's share, the spent weekly
#       window's red note, ChatGPT's plan and account, Copilot's counts —
#       and no footer under it;
#   (b) every read carried its credential: ChatGPT's minted bearer and its
#       account header, Copilot's stored token under the `token` scheme,
#       Cline's `workos:` bearer — and Cline's plan was read before its
#       windows;
#   (c) `r` reads every subscription again and the page shows the new
#       numbers; a refresh the provider refuses keeps the last numbers
#       under an amber reason;
#   (d) ↓ moves the highlight; Esc closes the page back to the composer;
#   (e) a 12-row pane folds every section but the highlighted one, the
#       whole page on screen;
# Then against the offline dummy with nothing signed in:
#   (f) /usage opens mid-turn with the status line still above it, and says
#       how to sign in.
S145="${S}_usage"
U_LOG="$SMOKE_TMP/usage.log"
U_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$U_PORT" "$U_LOG" <<'PY' &
import base64, datetime, json, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer

port, log = int(sys.argv[1]), sys.argv[2]
now = int(time.time())
# Resets a few days out, so the page's countdowns read like real ones.
in_days = lambda days: datetime.datetime.fromtimestamp(now + days * 86400, datetime.timezone.utc)
reset_day = in_days(9).strftime("%Y-%m-%d")
reset_instant = in_days(2).strftime("%Y-%m-%dT%H:%M:%SZ")
seen = {"chatgpt": 0}

def b64url(raw):
    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode()

claims = {
    "https://api.openai.com/auth": {"chatgpt_account_id": "acct-145", "chatgpt_plan_type": "pro"},
    "exp": 4102444800,
}
jwt = ".".join([b64url(b'{"alg":"none"}'), b64url(json.dumps(claims).encode()), b64url(b"sig")])

def chatgpt(five_hour):
    return {
        "email": "smoke@example.com",
        "plan_type": "pro",
        "rate_limit": {
            "allowed": True, "limit_reached": False,
            "primary_window": {"used_percent": five_hour, "limit_window_seconds": 18000,
                               "reset_after_seconds": 7200, "reset_at": now + 7200},
            "secondary_window": {"used_percent": 42, "limit_window_seconds": 604800,
                                 "reset_after_seconds": 345600, "reset_at": now + 345600},
        },
        "credits": {"has_credits": True, "unlimited": False, "balance": "62500.0000000000"},
        "rate_limit_reset_credits": {"available_count": 2},
    }

copilot = {
    "copilot_plan": "individual",
    "quota_reset_date": reset_day,
    "quota_snapshots": {
        "premium_interactions": {"entitlement": 300, "remaining": 207, "percent_remaining": 69.0,
                                 "unlimited": False, "overage_count": 0, "overage_permitted": False},
        "chat": {"entitlement": 0, "percent_remaining": 100, "unlimited": True},
    },
}

cline_plan = {"success": True, "data": {"plan": {"displayName": "Cline Pass (Monthly)"}}}
cline_limits = {"success": True, "data": {"limits": [
    {"type": "five_hour", "percentUsed": 3},
    {"type": "weekly", "percentUsed": 100, "resetsAt": reset_instant},
    {"type": "monthly", "percentUsed": 81, "resetsAt": in_days(17).strftime("%Y-%m-%dT%H:%M:%SZ")},
]}}

class Stub(BaseHTTPRequestHandler):
    def note(self):
        auth = self.headers.get("authorization", "-")
        account = self.headers.get("chatgpt-account-id", "-")
        with open(log, "a") as f:
            f.write(f"{self.command} {self.path} auth={auth} account={account}\n")

    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length") or 0))
        self.note()
        if self.path == "/oauth/token":
            self.reply(200, {"access_token": jwt, "expires_in": 864000})
        elif self.path == "/api/v1/auth/refresh":
            self.reply(200, {"success": True, "data": {"accessToken": "cline-access-145",
                                                       "expiresAt": "2099-01-01T00:00:00Z"}})
        else:
            self.reply(404, {})

    def do_GET(self):
        self.note()
        if self.path == "/api/codex/usage":
            seen["chatgpt"] += 1
            if seen["chatgpt"] == 3:
                self.reply(503, {"error": {"message": "usage service down"}})
            else:
                self.reply(200, chatgpt(12 if seen["chatgpt"] == 1 else 13))
        elif self.path == "/copilot_internal/user":
            self.reply(200, copilot)
        elif self.path == "/api/v1/users/me/plan":
            self.reply(200, cline_plan)
        elif self.path == "/api/v1/users/me/plan/usage-limits":
            self.reply(200, cline_limits)
        elif self.path == "/api/v1/ai/cline/recommended-models":
            self.reply(200, {"clinePass": [{"id": "cline-pass/glm-5.3", "name": "GLM 5.3"}]})
        elif self.path == "/api/v1/ai/cline/models":
            self.reply(200, {"data": []})
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
U_PROVIDERS="$SMOKE_TMP/providers.toml"
cat >"$U_PROVIDERS" <<TOML
[providers.chatgpt_codex]
name = "ChatGPT Codex"
auth = "chatgpt_codex"
wire_api = "responses"
api_key_env = "CHATGPT_CODEX_REFRESH_TOKEN"

[providers.chatgpt_codex.extra_headers]
originator = "codex_cli_rs"

[providers.chatgpt_codex.kwargs]
api_base = "$U_BASE"

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
TOML
U_DIR="$(work_dir usage)"
# NO_PROXY keeps a developer's HTTP proxy out of the loopback requests; the
# three tokens are spelled out so a developer's own exported ones never
# reach the stub — or a real provider.
U_APP="env -u ANTHROPIC_CONSOLE_REFRESH_TOKEN -u OPENAI_CHATGPT_REFRESH_TOKEN NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS ALTER_ZERO_PROVIDERS_FILE=$U_PROVIDERS ALTER_ZERO_OPENAI_ISSUER=$U_BASE ALTER_ZERO_GITHUB_API_BASE=$U_BASE ALTER_ZERO_CLINE_API_BASE=$U_BASE ALTER_ZERO_PROVIDER=cline_pass ALTER_ZERO_MODEL=cline-pass/glm-5.3 CHATGPT_CODEX_REFRESH_TOKEN=rt-145 GITHUB_COPILOT_TOKEN=ghu_smoke145 CLINE_ACCOUNT_REFRESH_TOKEN=clr-145 $BIN_ABS"

# ---- (a) the palette lists /usage; the page reads all three ----
launch -c "$U_DIR" "$S145" 110 44 "$U_APP"
wait_for 10 "$S145" -F "cline-pass/glm-5.3" || fail "the ClinePass model never reached the footer"
type_text "$S145" "/usa"
palette="$(wait_pane 5 "$S145" -F "Show your subscriptions' usage limits")" ||
	fail "/usage is missing from the slash-command palette"
dump "the palette filtered to /usa" "$palette"
keys "$S145" Enter
page="$(wait_pane 10 "$S145" -E "Premium requests .* 31% used")" || fail "the page never showed every subscription"
page="$(wait_pane 10 "$S145" -F "3 subscriptions · updated")" || fail "the page never finished its first round"
dump "the page, read" "$page"
expect_has "$page" -E "^  Usage" "the page has no title"
expect_has "$page" -E "❯ ClinePass +Cline Pass \(Monthly\) +in use$" \
	"ClinePass is not first, highlighted and tagged in use"
# (A glyph is several bytes and the suite's grep counts bytes, so a run of
# them is a group: `(█)+`, never `█+`.)
expect_has "$page" -E "5-hour limit +(█|░)+ +3% used" "ClinePass's 5-hour window is missing"
expect_has "$page" -E "Weekly limit +(█){20} 100% used +resets in (1d 23h|2d)" \
	"ClinePass's spent weekly window is not a full bar with its countdown"
expect_has "$page" -E "Monthly limit +(█|░)+ +81% used +resets in 1[67]d" "ClinePass's monthly window is missing"
expect_has "$page" -F "⚠ Your weekly limit has been reached." "the spent window has no note"
expect_has "$page" -E "ChatGPT Codex +Pro · smoke@example\.com" "ChatGPT's plan and account are missing"
expect_has "$page" -E "5-hour limit +(█|░)+ +12% used +resets in (1h 59m|2h)" \
	"ChatGPT's 5-hour window or its countdown is missing"
expect_has "$page" -E "Weekly limit +(█|░)+ +42% used +resets in (3d 23h|4d)" "ChatGPT's weekly window is missing"
expect_has "$page" -E "Credits +62,500 credits" "ChatGPT's credits are missing"
expect_has "$page" -E "Limit resets +2 available" "ChatGPT's limit resets are missing"
expect_has "$page" -E "GitHub Copilot +Pro" "Copilot's plan is missing"
expect_has "$page" -E "Premium requests .* 31% used +93 of 300 · resets in 8d" "Copilot's counts or reset are missing"
expect_has "$page" -E "Chat messages +Unlimited" "Copilot's unlimited chat is missing"
expect_has "$page" -F "Details at https://chatgpt.com/settings/usage" "the providers' own pages are not linked"
expect_lacks "$page" -F "cline-pass/glm-5.3 medium" "the footer is still showing under the page"
expect_lacks "$page" -F "cline-pass/glm-5.3 ·" "the footer is still showing under the page"
# The spent window is red and the 81% one amber — read off the raw SGR.
sgr="$(tmux capture-pane -t "$S145" -p -e)"
expect_has "$sgr" -E "38;2;243;139;168m(█){20}" "the spent window's bar is not red"
expect_has "$sgr" -E "38;2;249;226;175m(█){8,}" "the 81% bar is not amber"

# ---- (b) every read carried its credential ----
log="$(cat "$U_LOG")"
dump "the stub's requests" "$log"
expect_has "$log" -E "^GET /api/codex/usage auth=Bearer [A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+ account=acct-145$" \
	"ChatGPT's read did not carry its minted bearer and account"
expect_has "$log" -F "GET /copilot_internal/user auth=token ghu_smoke145" \
	"Copilot's read did not present the stored GitHub token"
expect_has "$log" -F "GET /api/v1/users/me/plan/usage-limits auth=Bearer workos:cline-access-145" \
	"ClinePass's windows were not read with the workos bearer"
plan_line="$(grep -n "GET /api/v1/users/me/plan " "$U_LOG" | head -n 1 | cut -d: -f1)"
limits_line="$(grep -n "GET /api/v1/users/me/plan/usage-limits" "$U_LOG" | head -n 1 | cut -d: -f1)"
if [ -z "$plan_line" ] || [ -z "$limits_line" ] || [ "$plan_line" -gt "$limits_line" ]; then
	fail "ClinePass's plan (line ${plan_line:-none}) was not read before its windows (line ${limits_line:-none})"
fi

# ---- (c) r reads them all again; a refused refresh keeps the numbers ----
keys "$S145" r
again="$(wait_pane 10 "$S145" -E "5-hour limit +(█|░)+ +13% used")" || fail "r did not read ChatGPT again"
again="$(wait_pane 10 "$S145" -F "3 subscriptions · updated")" || fail "the refresh never finished"
dump "after r" "$again"
[ "$(grep -c "GET /api/codex/usage" "$U_LOG")" -ge 2 ] || fail "r sent no second ChatGPT read"
keys "$S145" r
refused="$(wait_pane 10 "$S145" -F "Couldn't refresh:")" || fail "a refused refresh did not say so"
dump "a refused refresh" "$refused"
expect_has "$refused" -F "Couldn't refresh: HTTP 503" "the refusal does not name the provider's answer"
expect_has "$refused" -E "5-hour limit +(█|░)+ +13% used" "a refused refresh dropped the last numbers"

# ---- (d) ↓ moves the highlight; Esc closes ----
keys "$S145" Down
moved="$(wait_pane 5 "$S145" -E "❯ ChatGPT Codex")" || fail "↓ did not move the highlight to ChatGPT"
dump "↓ on the page" "$moved"
keys "$S145" Escape
closed="$(wait_pane 5 "$S145" -F "cline-pass/glm-5.3")" || fail "Esc did not close the page back to the composer"
expect_lacks "$closed" -F "3 subscriptions" "the page is still on screen after Esc"
tmux kill-session -t "$S145" 2>/dev/null

# ---- (e) a short pane folds the other sections ----
S145S="${S}_usageshort"
launch -c "$U_DIR" "$S145S" 110 12 "$U_APP"
submit "$S145S" "/usage"
short="$(wait_pane 10 "$S145S" -E "ChatGPT Codex +Pro · .*% used")" || fail "the short page did not fold ChatGPT to a summary"
dump "the page on 12 rows" "$short"
expect_has "$short" -E "❯ ClinePass" "the highlighted section is not on screen"
expect_has "$short" -E "GitHub Copilot +Pro · Premium requests 31% used" "Copilot is not folded to its fullest limit"
expect_lacks "$short" -F "Details at" "a 12-row page kept the links"
tmux kill-session -t "$S145S" 2>/dev/null

# ---- (f) the dummy, mid-turn, nothing signed in ----
S145D="${S}_usagedummy"
launch -c "$U_DIR" "$S145D" 100 30 "env -u GITHUB_COPILOT_TOKEN -u CHATGPT_CODEX_REFRESH_TOKEN -u CLINE_ACCOUNT_REFRESH_TOKEN -u ANTHROPIC_CONSOLE_REFRESH_TOKEN -u OPENAI_CHATGPT_REFRESH_TOKEN $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=2500 $BIN_ABS"
submit "$S145D" "$USER_MSG"
wait_for 5 "$S145D" -F "esc to interrupt" || fail "the dummy's turn never started"
submit "$S145D" "/usage"
mid="$(wait_pane 5 "$S145D" -F "No subscription is signed in")" || fail "/usage did not open mid-turn"
dump "the page mid-turn" "$mid"
expect_has "$mid" -F "esc to interrupt" "the status line left the strip above the page"
status_row="$(printf '%s\n' "$mid" | grep -n "esc to interrupt" | head -n 1 | cut -d: -f1)"
title_row="$(printf '%s\n' "$mid" | grep -nE "^  Usage$" | head -n 1 | cut -d: -f1)"
if [ -z "$status_row" ] || [ -z "$title_row" ] || [ "$status_row" -gt "$title_row" ]; then
	fail "the status line (row ${status_row:-none}) is not above the page (row ${title_row:-none})"
fi
expect_has "$mid" -F "/login" "the page does not say how to sign in"
keys "$S145D" Escape
wait_for 5 "$S145D" -F "esc to interrupt" || fail "Esc closed more than the page"
tmux kill-session -t "$S145D" 2>/dev/null
