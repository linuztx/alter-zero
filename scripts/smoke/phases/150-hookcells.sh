#!/usr/bin/env bash
# Phase 150 — every hook at a turn's edge shows what it said, through a real backend

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The hook-run cell (docs/hooks.md *The hook-run cell*) through the real
# path — the runner, the agent loop, the loop's commit — which the offline
# demo cannot reach. A stub provider answers every request with a numbered
# line of text, or refuses the prompt that asks it to; a hooks.json makes
# every turn-level event say something. What it proves:
#   - a SessionStart hook's systemMessage and a UserPromptSubmit hook's
#     context are cells under the prompt that read them;
#   - a Stop hook that blocks is a `● Ran 2 Stop hooks` cell under the
#     reply, the feedback the agent continues on right under its header and
#     a sibling that broke as a red error row naming it — and the round it
#     bought follows the cell;
#   - the second firing, where only the broken sibling speaks, is a cell of
#     its own before the turn's end;
#   - the model reads the feedback and the context, and none of what was
#     the user's alone (the message, the errors);
#   - Ctrl+O lists both Stop hooks that ran, each with how long it ran, the
#     error row under the broken one without its name a second time;
#   - a turn that ends on an error shows its StopFailure hook's cell above
#     the red error;
#   - a /compact shows its PreCompact and PostCompact hooks' cells above the
#     `Context compacted` line — the compact turn's branch of the loop.
S150="${S}_hookcells"
WORK="$(work_dir hookcells)"
HC_BODIES="$SMOKE_TMP/bodies150.jsonl"
HC_MARK="$SMOKE_TMP/stop150.blocked"
HC_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$HC_PORT" "$HC_BODIES" <<'PY' &
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port, bodies = int(sys.argv[1]), sys.argv[2]
answered = 0

def sse(frames):
    body = "".join("data: %s\n\n" % json.dumps(f) for f in frames) + "data: [DONE]\n\n"
    return body.encode()

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send(200, b'{"object":"list","data":[{"id":"stub-model"}]}', "application/json")

    def do_POST(self):
        global answered
        body = self.rfile.read(int(self.headers.get("content-length") or 0))
        with open(bodies, "ab") as out:
            out.write(body.replace(b"\n", b" ") + b"\n")
        # The turn that asks to fail is refused; the /compact after it —
        # whose request carries that prompt in its history, but no tools —
        # is answered.
        if b"smoke150 fail" in body and "tools" in json.loads(body):
            self.send(400, b'{"error":{"message":"smoke150 refused"}}', "application/json")
            return
        answered += 1
        frames = [{"choices": [{"delta": {"content": "Answer %d." % answered}}]},
                  {"choices": [{"delta": {}, "finish_reason": "stop"}]}]
        self.send(200, sse(frames), "text/event-stream")

    def send(self, status, data, kind):
        self.send_response(status)
        self.send_header("content-type", kind)
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
HC_STUB=$!
smoke_on_exit kill "$HC_STUB"
HC_PROVIDERS="$SMOKE_TMP/providers150.toml"
printf '[providers.stub]\nname = "Stub"\n\n[providers.stub.kwargs]\napi_base = "http://127.0.0.1:%s/v1"\n' "$HC_PORT" >"$HC_PROVIDERS"

HC_HOOKS="$SMOKE_TMP/hooks150.json"
cat >"$HC_HOOKS" <<HOOKS
{"hooks": {
  "SessionStart": [{"hooks": [{"type": "command",
    "command": "printf '%s' '{\"systemMessage\":\"smoke150 session hooks ready\"}'"}]}],
  "UserPromptSubmit": [{"hooks": [{"type": "command",
    "command": "echo 'smoke150 branch: main'"}]}],
  "Stop": [{"hooks": [
    {"type": "command",
     "command": "[ -f $HC_MARK ] && exit 0; touch $HC_MARK; echo 'smoke150 run the tests first' >&2; exit 2"},
    {"type": "command", "command": "echo 'smoke150 notifier offline' >&2; exit 1"}]}],
  "StopFailure": [{"hooks": [{"type": "command",
    "command": "echo 'smoke150 pager unreachable' >&2; exit 1"}]}],
  "PreCompact": [{"hooks": [{"type": "command",
    "command": "echo 'smoke150 keep the API notes'"}]}],
  "PostCompact": [{"hooks": [{"type": "command",
    "command": "printf '%s' '{\"systemMessage\":\"smoke150 summary archived\"}'"}]}]
}}
HOOKS
HC_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_HOOKS=1 ALTER_ZERO_HOOKS_FILE=$HC_HOOKS ALTER_ZERO_PROVIDERS_FILE=$HC_PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"

# hc_under PANE HEADER — the line right under the first line of PANE that
# contains HEADER (fixed string).
hc_under() {
	printf '%s\n' "$1" | awk -v header="$2" 'index($0, header) {if ((getline line) > 0) print line; exit}'
}

launch -n -c "$WORK" "$S150" 120 60 "$HC_APP"
trust_hooks "$S150"
submit "$S150" "smoke150 go"
hc_pane="$(wait_settled 30 "$S150" -S -200 -- -E "$SUMMARY_RE")" || fail "the turn never settled"
dump "the turn, its hooks' cells and the round the Stop hook bought" "$hc_pane"

expect_has "$(hc_under "$hc_pane" "● Ran 1 SessionStart hook (ctrl+o to expand)")" -F \
	"⎿  smoke150 session hooks ready" "the SessionStart hook's message left no cell under the prompt"
expect_has "$(hc_under "$hc_pane" "● Ran 1 UserPromptSubmit hook (ctrl+o to expand)")" -F \
	"⎿  Context added by UserPromptSubmit hook" "the UserPromptSubmit hook's context left no cell under the prompt"
# The first Stop firing: the feedback the agent continues on leads, the
# broken sibling's error follows.
hc_stop="$(printf '%s\n' "$hc_pane" | awk 'index($0, "● Ran 2 Stop hooks (ctrl+o to expand)") {for (i = 0; i < 2 && (getline line) > 0; i++) print line; exit}')"
expect_has "$(printf '%s\n' "$hc_stop" | sed -n 1p)" -F "⎿  Stop hook feedback: smoke150 run the tests first" \
	"the Stop hook's block left no feedback row under its cell (saw: $hc_stop)"
# The error row says what broke and which hook it was: collapsed, the rows
# stand without their hooks.
expect_has "$(printf '%s\n' "$hc_stop" | sed -n 2p)" -F "⎿  Stop hook error: echo 'smoke150 notifier offline' >&2; exit 1 failed (exit 1): smoke150 notifier offline" \
	"the broken Stop hook left no error row naming it (saw: $hc_stop)"
# The order the turn ran in: the answer, the block's cell, the round it
# bought, the second firing's cell (only the error this time), the receipt.
hc_order="$(printf '%s\n' "$hc_pane" | grep -n -E "Answer 1\.|Answer 2\.|● Ran 2 Stop hooks|$SUMMARY_RE" | cut -d: -f2- | sed -E 's/^● //; s/ \(ctrl\+o to expand\)//; s/ for [0-9].*//')"
note "the turn's order"
printf '%s\n' "$hc_order"
expect_eq "$(printf '%s\n' "$hc_order" | tr '\n' '|')" "Answer 1.|Ran 2 Stop hooks|Answer 2.|Ran 2 Stop hooks|$(printf '%s\n' "$hc_order" | tail -1)|" \
	"the Stop hook's cells out of order with the rounds"
hc_second="$(printf '%s\n' "$hc_pane" | awk '/Answer 2\./ {seen = 1} seen && index($0, "● Ran 2 Stop hooks") {if ((getline line) > 0) print line; exit}')"
expect_has "$hc_second" -F "Stop hook error:" "the second firing's cell lost its error row (saw: $hc_second)"
expect_lacks "$hc_second" -F "Stop hook feedback" "the second firing blocked nothing, yet its cell shows feedback"

# What the model read: the feedback and the context, nothing that was the
# user's alone.
expect_file_has "$HC_BODIES" -F "Stop hook feedback: smoke150 run the tests first" "the model never read the Stop hook's feedback"
expect_file_has "$HC_BODIES" -F "smoke150 branch: main" "the model never read the UserPromptSubmit hook's context"
grep -q -F "smoke150 notifier offline" "$HC_BODIES" && fail "a broken hook's error reached the model"
grep -q -F "smoke150 session hooks ready" "$HC_BODIES" && fail "a hook's message to the user reached the model"

# Ctrl+O: the first Stop cell, expanded, lists both hooks with their times.
keys "$S150" C-o
sleep 0.8
keys "$S150" Home
sleep 0.3
hc_overlay="$(tmux capture-pane -t "$S150" -p)"
dump "the Ctrl+O transcript" "$hc_overlay"
hc_expanded="$(printf '%s\n' "$hc_overlay" | awk '/● Ran 2 Stop hooks *$/ {for (i = 0; i < 8 && (getline line) > 0; i++) print line; exit}')"
# The cell elides the hook's command line with an ellipsis once the temp path
# under it is long (a canonical `/private/var/...`), so the marker's name can
# be cut: assert the command's shape and its time, not the tail of the path.
expect_has "$(printf '%s\n' "$hc_expanded" | sed -n 1p)" -E "⎿  \[ -f .* \([0-9.]+m?s\)$" \
	"the expanded cell lost the blocking hook or its time (saw: $hc_expanded)"
expect_has "$(printf '%s\n' "$hc_expanded" | sed -n 2p)" -F "Config: " "the expanded cell lost the hook's hooks.json (saw: $hc_expanded)"
expect_has "$hc_expanded" -E "⎿  echo 'smoke150 notifier offline' >&2; exit 1 \([0-9.]+m?s\)" \
	"the expanded cell lost the broken hook or its time (saw: $hc_expanded)"
# Under the broken hook's own line, its error row does not name it again.
expect_has "$hc_expanded" -F "⎿  Stop hook error: failed (exit 1): smoke150 notifier offline" \
	"the expanded cell lost the broken hook's error row (saw: $hc_expanded)"
expect_lacks "$hc_expanded" -F "Stop hook error: echo" "the expanded cell names the broken hook twice (saw: $hc_expanded)"
keys "$S150" q
sleep 0.3

# A turn that ends on an error: its StopFailure hook's cell, above the red
# error that ended it.
submit "$S150" "smoke150 fail"
hc_failed="$(wait_settled 30 "$S150" -S -200 -- -F "smoke150 refused")" || fail "the refused turn never ended"
dump "the refused turn" "$hc_failed"
expect_has "$(hc_under "$hc_failed" "● Ran 1 StopFailure hook (ctrl+o to expand)")" -F \
	"⎿  StopFailure hook error: echo 'smoke150 pager unreachable' >&2; exit 1 failed (exit 1): smoke150 pager unreachable" \
	"the StopFailure hook's error left no cell naming it"
hc_failure_order="$(printf '%s\n' "$hc_failed" | grep -n -E "● Ran 1 StopFailure hook|smoke150 refused" | cut -d: -f1 | tr '\n' ' ')"
read -r hc_cell_at hc_error_at _ <<<"$hc_failure_order"
[ -n "$hc_cell_at" ] && [ -n "$hc_error_at" ] && [ "$hc_cell_at" -lt "$hc_error_at" ] ||
	fail "the StopFailure cell is not above the red error (lines: $hc_failure_order)"

# A /compact: the summarization turn's own hooks, their cells above the
# `Context compacted` line it ends on.
submit "$S150" "/compact"
hc_compacted="$(wait_settled 30 "$S150" -S -200 -- -F "Context compacted")" || fail "the compaction never finished"
dump "the compaction" "$hc_compacted"
expect_has "$(hc_under "$hc_compacted" "● Ran 1 PreCompact hook (ctrl+o to expand)")" -F \
	"⎿  Context added by PreCompact hook" "the PreCompact hook's instructions left no cell"
expect_has "$(hc_under "$hc_compacted" "● Ran 1 PostCompact hook (ctrl+o to expand)")" -F \
	"⎿  smoke150 summary archived" "the PostCompact hook's message left no cell"
hc_compact_order="$(printf '%s\n' "$hc_compacted" | grep -E "● Ran 1 PreCompact hook|● Ran 1 PostCompact hook|Context compacted" | sed -E 's/^● //; s/ \(ctrl\+o to expand\)//; s/ ·.*//' | tr '\n' '|')"
expect_eq "$hc_compact_order" "Ran 1 PreCompact hook|Ran 1 PostCompact hook|Context compacted|" \
	"the compact hooks' cells are not above the compaction's line"
expect_file_has "$HC_BODIES" -F "smoke150 keep the API notes" "the summarization never read the PreCompact hook's instructions"

tmux kill-session -t "$S150" 2>/dev/null || true
