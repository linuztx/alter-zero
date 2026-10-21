#!/usr/bin/env bash
# Phase 141 — /login's "Use a local model": each address tested, then saved

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# `/login` → Use a local model (docs/local-models.md). Three local stubs stand
# in for the servers: an LM Studio listing one language model and one
# embedding model over its native /api/v1/models, a second LM Studio set to
# require an API token (401 without `Bearer sk-lm-smoke`), and an Ollama
# answering /api/tags and /api/show. Against the real binary:
#   (a) the root offers a third way in; its list holds the local servers, and
#       the API-key list no longer offers the local Ollama a key field;
#   (b) an address nothing answers at is tested, refused in red on the page,
#       and saved nowhere — the page stays, to fix the address;
#   (c) the stub's address passes: the flow closes on a `Connected to
#       LM Studio` toast, .env holds LMSTUDIO_HOST, and /model lists the
#       stub's language model — never its embedding model;
#   (d) a server wanting a token asks for one on the same page, says so when
#       the one typed is refused, and saves the address and the token once
#       one passes;
#   (e) Ollama tests and saves its OLLAMA_HOST the same way;
#   (f) a session on LM Studio's model whose server has stopped ends the turn
#       on the test's own sentence, never on `Waiting for internet…`
#       (docs/offline.md);
#   (g) in that session, whose shell exports LMSTUDIO_HOST, a working address
#       saved under /login says the exported variable overrides it, instead
#       of confirming a connection the session will not use.
S141="${S}_local"
L_LOG="$SMOKE_TMP/local.log"
l_port() {
	python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'
}
L_OPEN="$(l_port)"
L_TOKEN="$(l_port)"
L_OLLAMA="$(l_port)"
# Bound and released: nothing listens here, so a connect is refused at once.
L_DEAD="$(l_port)"

# l_stub MODE PORT — one stub server in the background, killed at exit.
l_stub() {
	python3 - "$1" "$2" "$L_LOG" <<'PY' &
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

mode, port, log = sys.argv[1], int(sys.argv[2]), sys.argv[3]
TOKEN = "Bearer sk-lm-smoke"
LMSTUDIO = {"models": [
    {"type": "llm", "publisher": "qwen", "key": "qwen/qwen3-8b", "display_name": "Qwen3 8B",
     "loaded_instances": [{"id": "qwen/qwen3-8b", "config": {"context_length": 8192}}],
     "max_context_length": 131072, "format": "gguf",
     "capabilities": {"vision": False, "trained_for_tool_use": True}},
    {"type": "embedding", "publisher": "nomic-ai", "key": "text-embedding-nomic-embed-text-v1.5",
     "display_name": "Nomic Embed Text v1.5", "loaded_instances": [],
     "max_context_length": 2048, "format": "gguf"},
]}

class Stub(BaseHTTPRequestHandler):
    def reply(self, status, body):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def note(self):
        with open(log, "a") as f:
            f.write(f"{mode} {self.command} {self.path} {self.headers.get('authorization')}\n")

    def do_GET(self):
        self.note()
        if mode == "lmstudio-token" and self.headers.get("authorization") != TOKEN:
            return self.reply(401, {"error": "Missing or invalid API token"})
        if mode.startswith("lmstudio") and self.path == "/api/v1/models":
            return self.reply(200, LMSTUDIO)
        if mode == "ollama" and self.path == "/api/tags":
            return self.reply(200, {"models": [
                {"name": "qwen3:8b", "model": "qwen3:8b", "details": {"family": "qwen3"}}]})
        self.reply(404, {"error": "Unexpected endpoint or method."})

    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length") or 0))
        self.note()
        if mode == "ollama" and self.path == "/api/show":
            return self.reply(200, {
                "capabilities": ["completion", "tools"], "parameters": "",
                "details": {"family": "qwen3"},
                "model_info": {"general.architecture": "qwen3", "qwen3.context_length": 40960}})
        self.reply(404, {"error": "not found"})

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
	smoke_on_exit kill "$!"
}
l_stub lmstudio "$L_OPEN"
l_stub lmstudio-token "$L_TOKEN"
l_stub ollama "$L_OLLAMA"
for port in "$L_OPEN" "$L_TOKEN" "$L_OLLAMA"; do # wait for each to listen
	for _ in $(seq 1 30); do
		if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $port)) == 0 else 1)" 2>/dev/null; then
			break
		fi
		sleep 0.1
	done
done

L_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $APP"
L_ENV="$SMOKE_CFG/.env"

# l_saved VAR VALUE — does the key store hold VAR=VALUE?
l_saved() { [ -f "$L_ENV" ] && grep -qxF "$1=$2" "$L_ENV"; }
# l_clear SESSION COUNT — Backspace COUNT times.
l_clear() {
	local _
	for _ in $(seq 1 "$2"); do keys "$1" BSpace; done
	sleep 0.2
}
# l_server SESSION NAME DOWNS — /login → Use a local model → the server DOWNS
# rows down the list, and print its page.
l_server() {
	local _
	submit "$1" "/login"
	wait_for 5 "$1" -F "Use a local model" || fail "/login did not open"
	keys "$1" Up # the root's last row
	sleep 0.2
	keys "$1" Enter
	wait_for 5 "$1" -F "enter connect" || fail "Use a local model did not open the server list"
	for _ in $(seq 1 "$3"); do keys "$1" Down; done
	sleep 0.2
	keys "$1" Enter
	wait_pane 5 "$1" -F "Enter your $2 server URL"
}

# ---- (a) a third way in, and the local servers out of the API-key list ----
launch "$S141" 100 32 "$L_APP"
submit "$S141" "/login"
root="$(wait_pane 5 "$S141" -F "Use a local model")" || fail "/login's root does not offer a local model"
dump "the /login root" "$root"
for want in "Use a subscription" "Use an API key" "Use a local model"; do
	expect_has "$root" -F "$want" "the /login root did not show \"$want\""
done
keys "$S141" Down
sleep 0.2
keys "$S141" Enter
key_list="$(wait_pane 5 "$S141" -F "Keys are saved to")" || fail "the API-key list did not open"
dump "the API-key list" "$key_list"
expect_has "$key_list" -F "Ollama Cloud" "Ollama Cloud, a pasted key, left the API-key list"
expect_lacks "$key_list" -F "Ollama · " "the local Ollama is still offered a key field"
expect_lacks "$key_list" -F "LM Studio" "LM Studio is offered a key field"
keys "$S141" Escape
sleep 0.3
keys "$S141" Up
sleep 0.2
keys "$S141" Enter
local_list="$(wait_pane 5 "$S141" -F "enter connect")" || fail "Use a local model did not open the server list"
dump "the local server list" "$local_list"
expect_has "$local_list" -F "LM Studio · ◯ unconfigured" "LM Studio is not an unconfigured row"
expect_has "$local_list" -F "Ollama · ◯ unconfigured" "the local Ollama is not an unconfigured row"
expect_lacks "$local_list" -F "Ollama Cloud" "a pasted-key provider is offered as a local server"

# ---- (b) an address nothing answers at is refused, and saved nowhere ----
keys "$S141" Enter
page="$(wait_pane 5 "$S141" -F "Enter your LM Studio server URL")" || fail "LM Studio's page did not open"
dump "LM Studio's page" "$page"
expect_has "$page" -F "❯ http://127.0.0.1:1234" "the field does not offer LM Studio's own address"
expect_has "$page" -F "Install it from https://lmstudio.ai/download" "the page does not say where LM Studio comes from"
dead="127.0.0.1:$L_DEAD"
type_text "$S141" "$dead"
sleep 0.2
keys "$S141" Enter
refused="$(wait_pane 15 "$S141" -F "Nothing is answering at $dead")" ||
	fail "an address nothing answers at was not refused on the page"
dump "a refused address" "$refused"
expect_has "$refused" -F "is LM Studio's server running?" "the refusal does not say what to check"
expect_has "$refused" -F "Enter your LM Studio server URL" "the refusal closed the page"
if [ -f "$L_ENV" ] && grep -q "^LMSTUDIO_HOST=" "$L_ENV"; then
	fail "a refused address was saved"
fi

# ---- (c) the stub's address passes: saved, and listed by /model ----
l_clear "$S141" "${#dead}"
open="127.0.0.1:$L_OPEN"
type_text "$S141" "$open"
sleep 0.2
keys "$S141" Enter
passed="$(wait_pane 15 "$S141" -F "Connected to LM Studio")" || fail "a passing address did not confirm"
dump "a passing address" "$passed"
expect_has "$passed" -F "Connected to LM Studio — 1 model · run /model to use it" \
	"the confirmation does not count the one language model"
expect_lacks "$passed" -F "Enter your LM Studio server URL" "a passing test left the page open"
l_saved LMSTUDIO_HOST "$open" || fail "LMSTUDIO_HOST=$open was not saved to .env"
grep -qF "lmstudio GET /api/v1/models" "$L_LOG" || fail "the test did not read LM Studio's native listing"
submit "$S141" "/model"
picker="$(wait_pane 10 "$S141" -F "qwen/qwen3-8b")" || fail "/model did not list the saved server's model"
dump "/model over the saved server" "$picker"
expect_has "$picker" -E "qwen/qwen3-8b +\[lmstudio\]" "the model is not tagged with LM Studio"
expect_lacks "$picker" -F "text-embedding" "/model offers LM Studio's embedding model"
expect_lacks "$picker" -F "unavailable" "a provider fetch failed"
keys "$S141" Escape
sleep 0.3

# ---- (d) a server that wants a token asks for one, then saves both ----
page="$(l_server "$S141" "LM Studio" 0)" || fail "LM Studio's page did not reopen"
dump "LM Studio's page, pointed at" "$page"
expect_has "$page" -F "❯ $open" "the field does not offer the saved address"
guarded="127.0.0.1:$L_TOKEN"
type_text "$S141" "$guarded"
sleep 0.2
keys "$S141" Enter
asked="$(wait_pane 15 "$S141" -F "Enter your LM Studio API token")" ||
	fail "a server answering 401 did not ask for its token"
dump "the token step" "$asked"
expect_has "$asked" -F "LM Studio at $guarded wants an API token." "the token step does not say who asks"
type_text "$S141" "wrong"
sleep 0.2
keys "$S141" Enter
wrong="$(wait_pane 15 "$S141" -F "refused that token")" || fail "a refused token was not said"
dump "a refused token" "$wrong"
expect_lacks "$wrong" -F "wrong" "the token is shown in the clear"
l_clear "$S141" 5
type_text "$S141" "sk-lm-smoke"
sleep 0.2
keys "$S141" Enter
# (c)'s confirmation reads the same and a toast lives 4 s, so waiting on the
# toast could return before this test did: wait on the save itself (the
# token is written last), then on the page closing.
poll 15 l_saved LM_API_TOKEN "sk-lm-smoke" || fail "LM_API_TOKEN was not saved beside the address"
l_saved LMSTUDIO_HOST "$guarded" || fail "LMSTUDIO_HOST=$guarded was not saved"
l_token_page_gone() { ! pane_has "$S141" -F "Enter your LM Studio API token"; }
poll 5 l_token_page_gone || fail "a passing token left the page open"
grep -qF "lmstudio-token GET /api/v1/models Bearer sk-lm-smoke" "$L_LOG" ||
	fail "the token never rode the test's request as a bearer"

# ---- (e) Ollama: the same page, its own variable ----
page="$(l_server "$S141" "Ollama" 1)" || fail "Ollama's page did not open"
dump "Ollama's page" "$page"
expect_has "$page" -F "❯ http://127.0.0.1:11434" "the field does not offer Ollama's own address"
ollama="127.0.0.1:$L_OLLAMA"
type_text "$S141" "$ollama"
sleep 0.2
keys "$S141" Enter
wait_for 15 "$S141" -F "Connected to Ollama — 1 model" || fail "the Ollama stub did not pass"
l_saved OLLAMA_HOST "$ollama" || fail "OLLAMA_HOST=$ollama was not saved"
grep -qF "ollama POST /api/show" "$L_LOG" || fail "the Ollama test did not read the model's show record"
tmux kill-session -t "$S141" 2>/dev/null

# ---- (f) the server stops: advice in seconds, not a wait for the internet ----
# A saved selection on LM Studio's model (Phase 129's trick) and its host
# pointed at the dead port, the process env outranking the .env store: the
# turn's refused connect must come back as the test's sentence on the bounded
# retry, where the offline wait would never end.
S141F="${S}_local_stopped"
cat >"$SMOKE_CFG/config.json" <<'CONFIG'
{ "provider": "lmstudio", "model": "qwen/qwen3-8b" }
CONFIG
launch "$S141F" 100 32 "env LMSTUDIO_HOST=$dead $L_APP"
wait_for 10 "$S141F" -F "qwen/qwen3-8b" || fail "the session did not start on LM Studio's model"
submit "$S141F" "hello"
stopped="$(wait_pane 30 "$S141F" -F "is LM Studio's server running?")" ||
	fail "a stopped LM Studio was not reported (still waiting as if offline?)"
dump "a stopped LM Studio mid-session" "$stopped"
expect_has "$stopped" -F "Nothing is answering at $dead" "the advice does not name the server"
expect_lacks "$stopped" -F "Waiting for internet" "a stopped local server was waited out as an outage"

# ---- (g) an exported variable overrides the save, and the toast says so ----
page="$(l_server "$S141F" "LM Studio" 0)" || fail "LM Studio's page did not open over the exported host"
dump "LM Studio's page, the host exported" "$page"
expect_has "$page" -F "❯ $dead" "the field does not offer the exported address in force"
type_text "$S141F" "$open"
sleep 0.2
keys "$S141F" Enter
overridden="$(wait_pane 15 "$S141F" -F "which overrides it")" ||
	fail "a save the exported LMSTUDIO_HOST overrides was confirmed as if it would be used"
dump "a save the environment overrides" "$overridden"
expect_has "$overridden" -F "Saved — unset LMSTUDIO_HOST in your environment, which overrides it" \
	"the notice does not name the variable to unset"
expect_lacks "$overridden" -F "Connected to LM Studio" "the overridden save was confirmed"
l_saved LMSTUDIO_HOST "$open" || fail "the overridden address was not saved for when the variable is unset"
tmux kill-session -t "$S141F" 2>/dev/null
note "Use a local model tests each address, refuses the dead one, asks for a token, saves what passes, reports a stopped server, and names an exported override"
