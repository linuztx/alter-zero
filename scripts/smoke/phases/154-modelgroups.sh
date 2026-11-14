#!/usr/bin/env bash
# Phase 154 — /model groups a multi-provider list by provider

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# #25: a `/model` list spanning more than one provider is grouped under a
# bold provider heading, sorted by provider then id, and the `[provider]` tag
# is dropped from the rows (the heading says it). Selection still steps over
# models only, so ↓ from the last model of one provider lands on the first
# model of the next, never on its heading. A search that narrows the list to
# one provider drops the headings and brings the tag back.
S154="${S}_modelgroups"
M_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$M_PORT" <<'PY' &
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port = int(sys.argv[1])

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.startswith("/alpha/v1/models"):
            models = [{"id": f"alpha/model-{i:02}", "name": f"Alpha: Model {i}"} for i in range(3)]
        elif self.path.startswith("/beta/v1/models"):
            models = [{"id": f"beta/model-{i:02}", "name": f"Beta: Model {i}"} for i in range(3)]
        else:
            self.send_response(404)
            self.end_headers()
            return
        data = json.dumps({"data": models}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
M_SERVER=$!
smoke_on_exit kill "$M_SERVER"
for _ in $(seq 1 30); do # wait for the stub to listen
	if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $M_PORT)) == 0 else 1)" 2>/dev/null; then
		break
	fi
	sleep 0.1
done
M_PROVIDERS="$SMOKE_TMP/providers.toml"
cat >"$M_PROVIDERS" <<TOML
[providers.alpha]
name = "Alpha"
api_key_env = "STUB_API_KEY"

[providers.alpha.kwargs]
api_base = "http://127.0.0.1:$M_PORT/alpha/v1"

[providers.beta]
name = "Beta"
api_key_env = "STUB_API_KEY"

[providers.beta.kwargs]
api_base = "http://127.0.0.1:$M_PORT/beta/v1"
TOML
launch "$S154" 90 24 "env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 ALTER_ZERO_PROVIDERS_FILE=$M_PROVIDERS STUB_API_KEY=smoke-138 $APP"
submit "$S154" "/model"
grouped="$(wait_pane 10 "$S154" -F "beta/model-00")"
echo "==== Phase 154: /model over two providers ===="
printf '%s\n' "$grouped"
expect_has "$grouped" -E '^  alpha *$' "/model lost the alpha heading"
expect_has "$grouped" -E '^  beta *$' "/model lost the beta heading"
expect_has "$grouped" -E '^  → alpha/model-00' "/model lost the highlighted first model"
expect_lacks "$grouped" -F "[alpha]" "the grouped list still tags rows with [alpha]"
expect_lacks "$grouped" -F "[beta]" "the grouped list still tags rows with [beta]"
# Three ↓ from alpha/model-00: alpha-01, alpha-02, then beta/model-00 — the
# beta heading in between is not a step.
tmux send-keys -t "$S154" Down Down Down
sleep 0.4
grouped_down="$(pane "$S154")"
echo "==== Phase 154: /model after three ↓ ===="
printf '%s\n' "$grouped_down"
expect_has "$grouped_down" -E '^  → beta/model-00' "↓ did not land on the next provider's first model"
expect_has "$grouped_down" -E '^    \(4/6\)' "the counter did not count the third model"
# Typing "beta" narrows the list to one provider: the headings go (a heading
# over a single provider is noise) and the tag comes back on the rows.
tmux send-keys -t "$S154" -l "beta"
sleep 0.6
filtered="$(pane "$S154")"
echo "==== Phase 154: /model filtered to one provider ===="
printf '%s\n' "$filtered"
expect_has "$filtered" -E '^  → beta/model-00 \[beta\]' "the filtered rows lost their [beta] tag"
expect_lacks "$filtered" -E '^  beta *$' "a one-provider result still shows a heading"
expect_lacks "$filtered" -E '^  alpha *$' "the filtered list still shows the alpha heading"
tmux send-keys -t "$S154" Escape
sleep 0.4
