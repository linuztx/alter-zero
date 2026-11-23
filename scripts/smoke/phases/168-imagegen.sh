#!/usr/bin/env bash
# Phase 168 — imagegen: a ChatGPT sign-in turns on the tool, its built-in skill and its /settings row, and a turn makes an image

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The `imagegen` tool and skill, ported from OpenAI Codex (docs/imagegen.md).
# Two halves. Signed out, the built-in `imagegen` skill is SEEDED like every
# built-in but WITHHELD from the walk: `/skills` must not list it, since its
# whole default path is a tool the session does not offer. Signed in to
# ChatGPT Codex — a local stub standing in for OpenAI's token mint
# (`ALTER_ZERO_OPENAI_ISSUER`), the `/models` catalog, the Responses stream
# and the Images endpoint, all on a smoke provider file's `api_base` — the
# skill is listed, the model's `imagegen` call POSTs Codex's body to
# `/images/generations` with the sign-in's bearer and account, the answer's
# base64 PNG is saved under the config home's `generated_images/<session>/`,
# the cell names the file and draws it, and the next request hands the model
# the fact line and the picture.

# ---- half one: signed out, the seeded skill is withheld ----
S168="${S}_imagegen"
I_CFG="$SMOKE_TMP/cfg-signed-out"
mkdir -p "$I_CFG"
# No skills override: the built-ins are seeded into the config home, which is
# what this half must see withheld. HOME is the suite's empty one.
I_APP_OUT="env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS ALTER_ZERO_CONFIG_DIR=$I_CFG ALTER_ZERO_SKILLS_DIR= $BIN_ABS"
launch "$S168" 100 24 "$I_APP_OUT"
type_text "$S168" "/skills"
keys "$S168" Enter
sleep 0.7
out_menu="$(tmux capture-pane -t "$S168" -p)"
dump "/skills signed out" "$out_menu"
expect_has "$out_menu" -F "flaredantic" "/skills lists no built-ins at all"
expect_lacks "$out_menu" -F "imagegen" "/skills lists the imagegen skill with nobody signed in"
keys "$S168" Escape
sleep 0.3
# The /settings row: unavailable with nobody signed in, and Enter says why.
type_text "$S168" "/settings"
keys "$S168" Enter
wait_pane 5 "$S168" -F "Type to search" >/dev/null || fail "/settings did not open signed out"
type_text "$S168" "generation"
out_row="$(wait_pane 5 "$S168" -E "Image generation +false \(unavailable\)")" \
	|| fail "the Image generation row is not unavailable with nobody signed in"
dump "/settings signed out" "$out_row"
keys "$S168" Enter
wait_pane 5 "$S168" -F "it needs a ChatGPT sign-in" >/dev/null \
	|| fail "the unavailable Image generation row does not say how to make it available"
# Esc clears the search, then closes the page.
keys "$S168" Escape
keys "$S168" Escape
wait_pane 5 "$S168" -E '^❯' >/dev/null || fail "Esc did not close /settings signed out"
tmux kill-session -t "$S168" 2>/dev/null
if ! grep -q "^name: imagegen" "$I_CFG/skills/imagegen/SKILL.md" 2>/dev/null; then
	fail "the imagegen built-in was not seeded while signed out"
fi
for seeded in LICENSE.txt references/cli.md references/image-api.md references/prompting.md references/sample-prompts.md scripts/image_gen.py scripts/remove_chroma_key.py; do
	if ! cmp -s "$SMOKE_ROOT/prompts/skills/imagegen/$seeded" "$I_CFG/skills/imagegen/$seeded"; then
		fail "the seed did not write the authored imagegen/$seeded"
	fi
done

# ---- half two: signed in, a turn makes an image ----
S168B="${S}_imagegen_signed_in"
I_LOG="$SMOKE_TMP/imagegen.log"
I_PNG="$SMOKE_TMP/served.png"
I_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$I_PORT" "$I_LOG" "$I_PNG" <<'PY' &
import base64, json, struct, sys, zlib
from http.server import BaseHTTPRequestHandler, HTTPServer

port, log, served = int(sys.argv[1]), sys.argv[2], sys.argv[3]

def b64url(raw):
    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode()

# The access token: claims naming a paid seat and its account. The app reads
# the payload and never verifies the signature (docs/chatgpt.md).
claims = {
    "https://api.openai.com/auth": {"chatgpt_account_id": "acct-168", "chatgpt_plan_type": "plus"},
    "exp": 4102444800,
}
jwt = ".".join([b64url(b'{"alg":"none"}'), b64url(json.dumps(claims).encode()), b64url(b"sig")])

def png(width, height, rgb):
    """A real PNG, built by hand: the stdlib has no image writer."""
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)
    raw = b"".join(b"\x00" + bytes(rgb) * width for _ in range(height))
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw))
            + chunk(b"IEND", b""))

picture = png(16, 16, (220, 30, 30))
with open(served, "wb") as f:
    f.write(picture)

catalog = {"models": [{
    "slug": "gpt-stub",
    "display_name": "GPT Stub",
    "context_window": 200000,
    "input_modalities": ["text", "image"],
}]}

def sse(frames):
    return "".join(f"data: {json.dumps(frame)}\n\n" for frame in frames).encode()

USAGE = {"input_tokens": 10, "output_tokens": 5, "total_tokens": 15}
CALL = {"type": "function_call", "id": "fc_1", "call_id": "call_168",
        "name": "imagegen", "arguments": json.dumps({"prompt": "a red square"})}

class Stub(BaseHTTPRequestHandler):
    def record(self, body):
        headers = {k.lower(): v for k, v in self.headers.items()}
        with open(log, "a") as f:
            f.write(json.dumps({"method": self.command, "path": self.path,
                                "headers": headers, "body": body}) + "\n")

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0)).decode()
        self.record(body)
        if self.path == "/oauth/token":
            self.reply(200, {"access_token": jwt, "refresh_token": "rt-168-rotated", "expires_in": 3600})
        elif self.path.startswith("/images/generations"):
            self.reply(200, {"created": 1, "background": "opaque", "output_format": "png",
                             "data": [{"b64_json": base64.b64encode(picture).decode(),
                                       "revised_prompt": "a red square"}]})
        elif self.path.startswith("/responses"):
            answered = '"function_call_output"' in body
            frames = ([{"type": "response.output_text.delta", "delta": "Here is your red square."}]
                      if answered else
                      [{"type": "response.output_item.added", "output_index": 0,
                        "item": dict(CALL, arguments="")},
                       {"type": "response.output_item.done", "output_index": 0, "item": CALL}])
            frames.append({"type": "response.completed",
                           "response": {"status": "completed", "usage": USAGE}})
            data = sse(frames)
            self.send_response(200)
            self.send_header("content-type", "text/event-stream")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        else:
            self.reply(404, {})

    def do_GET(self):
        self.record("")
        if self.path.startswith("/models"):
            self.reply(200, catalog)
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
I_SERVER=$!
smoke_on_exit kill "$I_SERVER"
for _ in $(seq 1 30); do # wait for the stub to listen
	if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $I_PORT)) == 0 else 1)" 2>/dev/null; then
		break
	fi
	sleep 0.1
done
I_BASE="http://127.0.0.1:$I_PORT"
# The real ChatGPT Codex block with its base pointed at the stub, so every
# request — the turn's and the image's — goes there and nowhere else.
I_PROVIDERS="$SMOKE_TMP/providers.toml"
cat >"$I_PROVIDERS" <<TOML
[providers.chatgpt_codex]
name = "ChatGPT Codex"
auth = "chatgpt_codex"
wire_api = "responses"
description = "Sign in with your ChatGPT Plus/Pro account"
api_key_env = "CHATGPT_CODEX_REFRESH_TOKEN"

[providers.chatgpt_codex.extra_headers]
originator = "codex_cli_rs"

[providers.chatgpt_codex.kwargs]
api_base = "$I_BASE"
TOML
I_CFG2="$SMOKE_TMP/cfg-signed-in"
mkdir -p "$I_CFG2"
I_DIR="$(work_dir)"
# Half-blocks at a fixed cell, so the picture is drawn in the pane's own text
# (docs/images.md); NO_PROXY keeps a developer's proxy off the loopback.
launch -c "$I_DIR" "$S168B" 100 40 "env $CFG_ENV_NOHIST ALTER_ZERO_CONFIG_DIR=$I_CFG2 ALTER_ZERO_SKILLS_DIR= NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 ALTER_ZERO_OPENAI_ISSUER=$I_BASE ALTER_ZERO_PROVIDERS_FILE=$I_PROVIDERS ALTER_ZERO_PROVIDER=chatgpt_codex ALTER_ZERO_MODEL=gpt-stub CHATGPT_CODEX_REFRESH_TOKEN=rt-168 ALTER_ZERO_IMAGE_PROTOCOL=halfblocks ALTER_ZERO_IMAGE_CELL_SIZE=5x10 $BIN_ABS"

# Signed in, the skill is listed — the same session that withheld nothing.
type_text "$S168B" "/skills"
keys "$S168B" Enter
in_menu="$(wait_pane 8 "$S168B" -F "imagegen")" || fail "/skills does not list the imagegen skill while signed in"
dump "/skills signed in" "$in_menu"
keys "$S168B" Escape
sleep 0.4
# …and so is the /settings row, on until a directory turns it off.
type_text "$S168B" "/settings"
keys "$S168B" Enter
wait_pane 5 "$S168B" -F "Type to search" >/dev/null || fail "/settings did not open signed in"
type_text "$S168B" "generation"
in_row="$(wait_pane 5 "$S168B" -E "Image generation +true")" \
	|| fail "the Image generation row is not on with a sign-in"
dump "/settings signed in" "$in_row"
keys "$S168B" Escape
keys "$S168B" Escape
wait_pane 5 "$S168B" -E '^❯' >/dev/null || fail "Esc did not close /settings signed in"

# The turn: the stub's model calls `imagegen`, the image is made and saved,
# and the model's reply follows.
submit "$S168B" "make me a small red square image"
done_pane="$(wait_pane 20 "$S168B" -F "Here is your red square.")" || fail "the turn never finished"
dump "the turn that made an image" "$done_pane"
expect_has "$done_pane" -F "● ImageGen(a red square)" "the cell does not name the tool and its prompt"
expect_has "$done_pane" -F "Generated image (PNG, 16x16" "the cell does not state the image's facts"
expect_has "$done_pane" -F "Saved to" "the cell does not say where the image is"
# The picture itself: half-block cells in the served square's own red — a
# uniform colour draws as coloured cells a plain capture shows blank, so the
# colour is what is looked for (`capture-pane -e`).
drawn="$(tmux capture-pane -t "$S168B" -p -e)"
if ! printf '%s\n' "$drawn" | grep -qE '(38|48);2;220;30;30'; then
	dump "the escaped pane" "$drawn"
	fail "no picture was drawn under the cell"
fi

# The file: the PNG the stub served, numbered in this session's folder.
saved="$(ls "$I_CFG2"/generated_images/*/1.png 2>/dev/null | head -1)"
if [ -z "$saved" ]; then
	fail "no image was saved under the config home's generated_images folder"
elif ! cmp -s "$I_PNG" "$saved"; then
	fail "the saved image is not the PNG the backend sent"
fi

# The wire: Codex's body and the sign-in's credential on the image request,
# and the fact line and the picture on the request after it.
python3 - "$I_LOG" <<'PY' || fail "the requests were not what Codex sends"
import json, sys
entries = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
images = [e for e in entries if e["path"].startswith("/images/generations")]
assert len(images) == 1, f"{len(images)} image requests"
request = images[0]
headers = request["headers"]
assert headers["authorization"].startswith("Bearer "), headers
assert headers["chatgpt-account-id"] == "acct-168", headers
assert headers["originator"] == "codex_cli_rs", headers
assert "version" not in headers, "Codex sends no version header: %s" % headers
assert headers.get("x-codex-image-turn-id"), headers
body = json.loads(request["body"])
assert body == {"prompt": "a red square", "background": "opaque", "model": "gpt-image-2",
                "quality": "auto", "size": "auto"}, body
turns = [e for e in entries if e["path"].startswith("/responses")]
assert len(turns) == 2, f"{len(turns)} turn requests"
second = json.loads(turns[1]["body"])
output = next(item for item in second["input"] if item.get("type") == "function_call_output")
assert output["output"].startswith("Generated image (PNG, 16x16"), output
assert "Generated images are saved to" in output["output"], output
pictures = [part for item in second["input"] if item.get("type") == "message"
            for part in item.get("content", []) if part.get("type") == "input_image"]
assert pictures and pictures[-1]["image_url"].startswith("data:image/png;base64,"), second["input"]
print("ok: Codex's body, the sign-in's headers, and the picture handed back")
PY

submit "$S168B" "/quit"
sleep 0.6
tmux kill-session -t "$S168B" 2>/dev/null
