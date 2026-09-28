#!/usr/bin/env bash
# Phase 126 — the `/secrets` page and placeholder secrets

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the `/secrets` page (docs/secrets.md): the user stores a credential — a
# name, a value, a line of context — and the agent uses it by placeholder,
# `<secret:NAME>`, without the value ever reaching the screen or the model.
# Driven end to end: the palette entry; the empty page; the form (a typed
# name normalized to ROOT-style and previewing its placeholder, the value
# PASTED and shown one dot per character, the context); the save — a toast,
# the list row with its fixed mask, `secrets.json` owner-only; a relaunch
# that lists it again; a `!` command whose placeholder EXPANDS for the shell
# (proved by the value's hash, which only the real value produces) and whose
# echoed value comes back REDACTED; the placeholder in the Ctrl+D reminder;
# the edit form keeping the value unloaded; and a two-step delete. Through
# all of it the value itself must never appear on screen, scrollback
# included.
S126="${S}_secrets"
VALUE="s3cr3t-demo-value-42"
VALUE_SHA="$(printf '%s' "$VALUE" | sha256sum | cut -c1-16)"
FILE="$SMOKE_CFG/secrets.json"
never_shows_value() { # $1 label
	local screen
	screen="$(pane "$S126" -S -400)"
	if printf '%s\n' "$screen" | grep -qF "$VALUE"; then
		dump "the value leaked ($1)" "$screen"
		fail "the secret's value is on screen ($1)"
	fi
}

launch "$S126" 90 40
type_text "$S126" "/secrets"
palette="$(wait_pane 5 "$S126" -F "Store credentials the agent uses but never sees")" ||
	fail "/secrets is missing from the slash-command palette"
dump "the palette filtered to /secrets" "$palette"
keys "$S126" Enter
empty="$(wait_pane 5 "$S126" -F "+ Add a secret")"
dump "the page with no secrets" "$empty"
for expect in "Secrets" "Credentials the agent uses by placeholder" "No secrets yet" \
	"❯ + Add a secret" "enter add a secret"; do
	expect_has "$empty" -F "$expect" "the empty page is missing '$expect'"
done

# The form: a name typed the way a person would say it.
keys "$S126" Enter
wait_for 5 "$S126" -F "New secret" || fail "Enter on the add row did not open the form"
type_text "$S126" "demo token"
form="$(wait_pane 5 "$S126" -F "Use it as <secret:DEMO_TOKEN>")" ||
	fail "the typed name did not normalize and preview its placeholder"
dump "the form with a name" "$form"
expect_has "$form" -F "DEMO_TOKEN" "the name did not normalize to DEMO_TOKEN"
keys "$S126" Enter
# The value, PASTED (bracketed): into the field, one dot per character.
tmux set-buffer -b secret "$VALUE"
tmux paste-buffer -p -b secret -t "$S126"
dots="$(printf '%*s' "${#VALUE}" '' | sed 's/ /•/g')"
masked="$(wait_pane 5 "$S126" -F "$dots")" || fail "the pasted value is not masked one dot per character"
dump "the form with the value pasted" "$masked"
never_shows_value "the value field"
keys "$S126" Enter
type_text "$S126" "Demo token for the smoke test"
sleep "$SMOKE_TYPE_SETTLE"
keys "$S126" Enter
saved="$(wait_pane 5 "$S126" -F "<secret:DEMO_TOKEN>  ••••••••  Demo token for the smoke test")" ||
	fail "the saved secret is not listed with its fixed mask and context"
dump "the list after the save" "$saved"
expect_has "$saved" -F "Saved <secret:DEMO_TOKEN>" "no confirming toast"
never_shows_value "the list"

# On disk: owner-only, the value stored (it is the one place it lives).
if [ ! -f "$FILE" ]; then
	fail "secrets.json was not written"
else
	expect_eq "$(stat -c %a "$FILE")" "600" "secrets.json is not owner-only"
	expect_file_has "$FILE" -F "\"name\": \"DEMO_TOKEN\"" "secrets.json lacks the name"
	expect_file_has "$FILE" -F "$VALUE" "secrets.json lacks the value"
fi

# A relaunch lists it again.
keys "$S126" Escape
sleep 0.3
keys "$S126" C-c
wait_gone 5 "$S126" || fail "the app did not quit"
launch "$S126" 90 40
type_text "$S126" "/secrets"
sleep "$SMOKE_TYPE_SETTLE"
keys "$S126" Enter
relisted="$(wait_pane 5 "$S126" -F "<secret:DEMO_TOKEN>")" || fail "the secret did not survive a relaunch"
dump "the list after a relaunch" "$relisted"
keys "$S126" Escape
wait_for 5 "$S126" -E '^❯' || fail "Esc did not bring the composer back"

# A `!` command: the placeholder expands for the shell — only the real value
# hashes to VALUE_SHA — and the echoed value comes back as the placeholder.
submit "$S126" "!printf '%s' '<secret:DEMO_TOKEN>' | sha256sum | cut -c1-16"
hashed="$(wait_pane 10 "$S126" -F "$VALUE_SHA")" || fail "the placeholder did not expand for the ! command"
dump "the ! command hashing the expanded value" "$hashed"
submit "$S126" "!printf 'token=%s\\n' '<secret:DEMO_TOKEN>'"
echoed="$(wait_pane 10 "$S126" -F "token=<secret:DEMO_TOKEN>")" || fail "the echoed value was not redacted"
dump "the ! command echoing the value" "$echoed"
never_shows_value "a ! command's output"

# A placeholder naming nothing stored runs nothing — a slip would otherwise
# reach the command as its literal text — and the cell names the stored ones.
submit "$S126" "!printf 'ran-%s\\n' '<secret:DEMO_TOKN>'"
refused="$(wait_pane 10 "$S126" -F "Not run: <secret:DEMO_TOKN> is not a stored secret")" ||
	fail "a ! command naming a secret that is not stored was not refused"
dump "the ! command naming a secret that is not stored" "$refused"
expect_has "$refused" -F "Stored: <secret:DEMO_TOKEN>" "the refusal does not name the stored secret"
if printf '%s\n' "$refused" | grep -qF "ran-<secret:DEMO_TOKN>"; then
	fail "the refused command ran"
fi

# The reminder names the placeholder and its context — never the value.
keys "$S126" C-d
sleep 0.8
keys "$S126" Home
sleep 0.5
context="$(pane "$S126")"
dump "the derived context" "$context"
expect_has "$context" -F "The user's secrets, as placeholders" "the reminder has no secrets section"
expect_has "$context" -F "<secret:DEMO_TOKEN>: Demo token for the smoke test" "the reminder does not list the secret"
if printf '%s\n' "$context" | grep -qF "$VALUE"; then
	fail "the value is in the derived context"
fi
keys "$S126" q
wait_for 5 "$S126" -E '^❯' || fail "the context view did not close"

# ---- the real backend against a stub provider ----
# A second launch on the same config home drives the REAL backend and the
# real executor, so the boundary's wiring is what is under test: the secret
# loaded from secrets.json at startup, the reminder leading the first request,
# the store attached to the backend a session builds. The stub's model calls
# `bash` with the placeholder: the permission prompt shows the placeholder,
# the command writes the VALUE into a file, the cell shows its output masked
# back, and the stub's log proves no request carried the value. Then a call
# naming a secret that is not stored: refused before it runs — no prompt,
# no marker file — with the refusal the model reads naming the stored one.
S126B="${S126}_real"
WORK="$(work_dir secrets)"
STUB_LOG="$SMOKE_TMP/stub.log"
OUT_FILE="$WORK/token.out"
WRONG_MARKER="$WORK/wrong-ran"
STUB_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$STUB_PORT" "$STUB_LOG" "$OUT_FILE" "$WRONG_MARKER" <<'PY' &
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port, log, out_file, marker = int(sys.argv[1]), sys.argv[2], sys.argv[3], sys.argv[4]
RIGHT = "printf 'token=%%s\\n' '<secret:DEMO_TOKEN>' | tee %s" % out_file
WRONG = "touch %s && echo '<secret:DEMO_TOKN>'" % marker

def sse(frames):
    body = "".join("data: %s\n\n" % json.dumps(f) for f in frames) + "data: [DONE]\n\n"
    return body.encode()

def text(message):
    content = message.get("content")
    if isinstance(content, list):
        return " ".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send(b'{"object":"list","data":[{"id":"stub-model"}]}', "application/json")

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0)).decode()
        with open(log, "a") as f:
            f.write(body.replace("\n", " ") + "\n")
        messages = json.loads(body).get("messages", [])
        if messages and messages[-1].get("role") == "tool":
            frames = [
                {"choices": [{"delta": {"content": "Done."}}]},
                {"choices": [{"delta": {}, "finish_reason": "stop"}]},
            ]
        else:
            command = WRONG if "wrong name" in text(messages[-1]) else RIGHT
            call = {"index": 0, "id": "call_126_%d" % len(messages), "type": "function",
                    "function": {"name": "bash", "arguments": json.dumps(
                        {"command": command, "description": "Use the demo token"})}}
            frames = [
                {"choices": [{"delta": {"tool_calls": [call]}}]},
                {"choices": [{"delta": {}, "finish_reason": "tool_calls"}]},
            ]
        self.send(sse(frames), "text/event-stream")

    def send(self, data, kind):
        self.send_response(200)
        self.send_header("content-type", kind)
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
STUB_PID=$!
smoke_on_exit kill "$STUB_PID"
PROVIDERS="$SMOKE_TMP/providers.toml"
cat >"$PROVIDERS" <<EOF
[providers.stub]
name = "Stub"

[providers.stub.kwargs]
api_base = "http://127.0.0.1:$STUB_PORT/v1"
EOF
# NO_PROXY keeps a developer's HTTP proxy out of the loopback requests.
REAL_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_PROVIDERS_FILE=$PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"
launch -c "$WORK" "$S126B" 100 40 "$REAL_APP"
submit "$S126B" "use the demo token"
prompt="$(wait_pane 15 "$S126B" -F "Bash command")" || fail "the stub's call raised no permission prompt"
dump "the permission prompt" "$prompt"
expect_has "$prompt" -F "<secret:DEMO_TOKEN>" "the prompt does not show the placeholder"
expect_lacks "$prompt" -F "$VALUE" "the prompt showed the value"
keys "$S126B" Enter
done_pane="$(wait_settled 20 "$S126B" -F "Done.")" || fail "the stub's turn never settled"
dump "the turn settled" "$done_pane"
expect_has "$done_pane" -F "token=<secret:DEMO_TOKEN>" "the cell does not show the output masked"
if ! grep -qx "token=$VALUE" "$OUT_FILE" 2>/dev/null; then
	fail "the command did not run with the value (got: $(cat "$OUT_FILE" 2>/dev/null))"
fi
requests="$(wc -l <"$STUB_LOG" 2>/dev/null || echo 0)"
[ "$requests" -ge 2 ] || fail "the stub saw $requests request(s), not the tool round and its answer"
grep -qF 'token=<secret:DEMO_TOKEN>' "$STUB_LOG" ||
	fail "the tool result the model read is not masked to the placeholder"
head -1 "$STUB_LOG" | grep -qF '<secret:DEMO_TOKEN>: Demo token for the smoke test' ||
	fail "the first request's reminder does not name the stored secret"

# A placeholder naming nothing stored: refused before the prompt, so the
# refusal shows at once — a prompt would have left it waiting for an answer.
submit "$S126B" "now try the wrong name"
refused="$(wait_settled 15 "$S126B" -F "Not run: <secret:DEMO_TOKN> is not a stored secret")" ||
	fail "the call naming a secret that is not stored was not refused"
dump "the refused call" "$refused"
expect_lacks "$refused" -F "Bash command" "a refused call raised a permission prompt"
[ -e "$WRONG_MARKER" ] && fail "the refused command ran"
grep -qF 'is not a stored secret. Stored: <secret:DEMO_TOKEN>.' "$STUB_LOG" ||
	fail "the model was not told which secrets are stored"
if grep -qF "$VALUE" "$STUB_LOG"; then
	fail "a request carried the value to the provider"
fi

# Nowhere on screen, in the transcript or in the record.
everything="$(pane "$S126B" -S -500)"
expect_lacks "$everything" -F "$VALUE" "the scrollback holds the value"
keys "$S126B" C-o
transcript="$(wait_pane 5 "$S126B" -F "T R A N S C R I P T")" || fail "Ctrl+O did not open the transcript"
dump "Ctrl+O" "$transcript"
expect_lacks "$transcript" -F "$VALUE" "the Ctrl+O transcript holds the value"
keys "$S126B" q
if grep -rqF "$VALUE" "$SMOKE_SESSIONS" 2>/dev/null; then
	fail "the rollout recorded the value"
fi
tmux kill-session -t "$S126B" 2>/dev/null

# Editing never loads the value back.
type_text "$S126" "/secrets"
sleep "$SMOKE_TYPE_SETTLE"
keys "$S126" Enter
wait_for 5 "$S126" -F "<secret:DEMO_TOKEN>" || fail "the page did not reopen"
keys "$S126" Enter
edit="$(wait_pane 5 "$S126" -F "Edit <secret:DEMO_TOKEN>")" || fail "Enter on the row did not open the edit form"
dump "the edit form" "$edit"
expect_has "$edit" -F "leave empty to keep the current value" "the edit form loaded the value"
keys "$S126" Escape
sleep 0.3

# A delete asks first.
keys "$S126" d
asked="$(wait_pane 5 "$S126" -F "Delete <secret:DEMO_TOKEN>?")" || fail "d did not ask before deleting"
dump "the delete question" "$asked"
keys "$S126" d
gone="$(wait_pane 5 "$S126" -F "No secrets yet")" || fail "the second d did not delete"
dump "the list after the delete" "$gone"
expect_has "$gone" -F "Deleted <secret:DEMO_TOKEN>" "no confirming toast for the delete"
if grep -qF "DEMO_TOKEN" "$FILE" 2>/dev/null; then
	fail "the deleted secret is still in secrets.json"
fi
never_shows_value "the whole session"
keys "$S126" Escape
tmux kill-session -t "$S126" 2>/dev/null
