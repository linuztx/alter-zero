#!/usr/bin/env bash
# Phase 146 — Verbose output off folds reads, searches and listings into one line

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# docs/tool-groups.md. The suite exports ALTER_ZERO_VERBOSE=1, so every other
# phase sees one cell per call; this phase's launches seed it 0 — the
# default — and drive the folded view itself.
#
# Offline first: the `explore` demo runs two rounds of calls that only look —
# `ls -la` and a `read`, a thought, then `rg` and a `cat`. While the turn
# runs the strip shows ONE live line counting up in the present tense, still
# there through the thought between the rounds, and none of the four calls
# reaches scrollback; once the answer starts, scrollback keeps exactly one dim
# `Searched for 1 pattern, read 2 files, listed 1 directory` line, under the
# two `Thought for` lines. A height change purge-rebuilds the conversation,
# which must draw it the same; `/settings` → Verbose output redraws every
# call as its own cell and back; Ctrl+O expands every call either way.
#
# Then the real backend against a stub provider, in auto mode: a listing and
# a search run with no prompt and WITHOUT the classifier ever being asked —
# the stub logs every request it gets — while the `touch` after them is
# classified, its cell committed under the group's line. Last, the same turn
# in manual mode, where every command asks: the prompt for the search keeps
# the held listing above it, and the run still settles into one line.
S146="${S}_toolgroups"
APP146="env $CFG_ENV_NOHIST ALTER_ZERO_VERBOSE=0 ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
GROUP_LINE='^Searched for 1 pattern, read 2 files, listed 1 directory \(ctrl\+o to expand\)$'

launch "$S146" 100 40 "$APP146"
submit "$S146" "explore the project before you answer"

# Between the rounds: the first round's listing and read are held, the
# group's line live in the strip over the model's thought about what to read
# next — counting in the present tense, naming the newest call it made. (The
# count includes a call still queued, so the line reads the same while the
# round runs: the thought is what says the round is over.)
between_rounds() {
	between="$(pane "$S146")"
	has "$between" -F "Reading 1 file, listing 1 directory…" && has "$between" -F "Thinking…"
}
poll 15 between_rounds || fail "the live group never showed over the thought between the rounds"
dump "the live group between the rounds" "$between"
# The bullet blinks: on the hidden half of the pulse, blanks of its width.
expect_has "$between" -E '^(● |  )Reading 1 file, listing 1 directory… \(ctrl\+o to expand\)$' \
	"the live group's line is not one blinking-bullet row with its hint"
expect_has "$between" -E '^  ⎿  about\.py$' "the live group does not name the file it just read"
expect_lacks "$between" -E '● (Bash|Read)\(' "a call of the open group reached the screen as its own cell"
expect_lacks "$between" -E '^(Read|Listed|Searched) ' "the open group was settled before it was over"

# The second round joins the same line.
second="$(wait_pane 15 "$S146" -E '^(● |  )Searching for 1 pattern, reading [12] files?, listing 1 directory…')" ||
	fail "the second round's search did not join the live group"
dump "the live group during the second round" "$second"
expect_lacks "$second" -E '● (Bash|Read)\(' "a call of the second round reached the screen as its own cell"

settled="$(wait_summaries 30 "$S146" 1 -S -120)" || fail "the explore turn never settled"
dump "the settled turn" "$settled"
expect_eq "$(grep -cE "$GROUP_LINE" <<<"$settled")" "1" "scrollback does not hold the group's one settled line"
expect_lacks "$settled" -E '● (Bash|Read)\(' "a member of the settled group was drawn as its own cell"
expect_lacks "$settled" -E '(Searching|Reading|Listing) .*…' "the live line was left behind in scrollback"
expect_lacks "$settled" -F '$ cat README.md' "the live group's hint row was left behind in scrollback"
# The thought between the rounds settled while the group was live, so its
# line sits above the group's, in scrollback as in history.
thought_row="$(grep -nF 'Thought for' <<<"$settled" | tail -1 | cut -d: -f1)"
group_row="$(grep -nE "$GROUP_LINE" <<<"$settled" | cut -d: -f1)"
expect_eq "$(grep -cF 'Thought for' <<<"$settled")" "2" "the two thoughts did not both settle"
if [ -z "$thought_row" ] || [ -z "$group_row" ] || [ "$thought_row" -ge "$group_row" ]; then
	fail "the thought between the rounds is not above the group's line"
fi

# conversation CAPTURE — the rows from the user's message to the summary.
conversation() { sed -nE "/^❯ explore the project/,/^$SUMMARY_RE/p" <<<"$1"; }
before="$(conversation "$settled")"

# A height change purge-rebuilds the whole conversation from history: the
# folded walk must draw exactly what the commits wrote.
tmux resize-window -t "$S146" -x 100 -y 30
rebuilt="$(wait_settled 10 "$S146" -S -120 -- -E "^$SUMMARY_RE")" || fail "the rebuild never settled"
dump "the conversation after a height change" "$rebuilt"
expect_eq "$(conversation "$rebuilt")" "$before" "the rebuild drew the folded conversation differently"

# /settings → Verbose output (the second row) → true: every call its own
# cell again, the group's line gone.
submit "$S146" "/settings"
wait_for 5 "$S146" -F "Enter/Space to change" || fail "/settings did not open"
keys "$S146" Down
wait_for 5 "$S146" -E '^→ Verbose output +false' || fail "the Verbose output row is not under Hide thinking"
keys "$S146" Enter
wait_for 5 "$S146" -E '^→ Verbose output +true' || fail "Verbose output did not cycle to true"
keys "$S146" Escape
verbose="$(wait_settled 10 "$S146" -S -120 -- -F "● Bash(cat README.md)")" ||
	fail "turning Verbose output on did not redraw the calls as cells"
dump "the conversation with Verbose output on" "$verbose"
for header in "● Bash(ls -la)" "● Read(about.py)" '● Bash(rg -n "def " .)' "● Bash(cat README.md)"; do
	expect_has "$verbose" -F "$header" "Verbose output on did not draw $header"
done
expect_lacks "$verbose" -E "$GROUP_LINE" "Verbose output on still drew the group's line"

# …and back off: the line returns, the cells go.
submit "$S146" "/settings"
wait_for 5 "$S146" -F "Enter/Space to change" || fail "/settings did not reopen"
keys "$S146" Down
keys "$S146" Enter
wait_for 5 "$S146" -E '^→ Verbose output +false' || fail "Verbose output did not cycle back to false"
keys "$S146" Escape
folded="$(wait_settled 10 "$S146" -S -120 -- -E "$GROUP_LINE")" ||
	fail "turning Verbose output off did not fold the calls again"
expect_lacks "$folded" -E '● (Bash|Read)\(' "Verbose output off still drew a member as its own cell"

# Ctrl+O shows every call expanded, whatever the inline view folds.
keys "$S146" C-o
wait_for 5 "$S146" -F "T R A N S C R I P T" || fail "Ctrl+O did not open the transcript"
keys "$S146" Home
sleep 0.3
transcript="$(pane "$S146")"
for _ in 1 2 3; do
	keys "$S146" PageDown
	sleep 0.3
	transcript="$transcript"$'\n'"$(pane "$S146")"
done
dump "the Ctrl+O transcript, top to bottom" "$transcript"
for header in "● Bash(ls -la)" "● Read(about.py)" '● Bash(rg -n "def " .)' "● Bash(cat README.md)"; do
	expect_has "$transcript" -F "$header" "Ctrl+O does not expand $header"
done
expect_has "$transcript" -F "about.py:10:def card() -> str:" "Ctrl+O does not show the search's output"
keys "$S146" q
wait_for 5 "$S146" -F "dummy_model_name" || fail "q did not close the transcript"
tmux kill-session -t "$S146" 2>/dev/null

# ---------------------------------------------------------------------------
# The real backend in auto mode: read-only commands skip the classifier.
# ---------------------------------------------------------------------------
S146B="${S}_toolgroups_auto"
WORK146="$(work_dir explore)"
printf 'TODO: ship the folded view\n' >"$WORK146/notes.txt"
TG_LOG="$SMOKE_TMP/requests146.tsv"
: >"$TG_LOG"
TG_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$TG_PORT" "$TG_LOG" <<'PY' &
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port, log = int(sys.argv[1]), sys.argv[2]

def sse(frames):
    return ("".join("data: %s\n\n" % json.dumps(f) for f in frames) + "data: [DONE]\n\n").encode()

def text(message):
    content = message.get("content")
    if isinstance(content, list):
        return " ".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

def calling(calls, n):
    deltas = [{"index": i, "id": "call_146_%d_%d" % (n, i), "type": "function",
               "function": {"name": name, "arguments": json.dumps(args)}}
              for i, (name, args) in enumerate(calls)]
    return [{"choices": [{"delta": {"tool_calls": deltas}}]},
            {"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}]

def saying(words):
    return [{"choices": [{"delta": {"content": words}}]},
            {"choices": [{"delta": {}, "finish_reason": "stop"}]}]

def note(kind, detail):
    with open(log, "a", encoding="utf-8") as f:
        f.write("%s\t%s\n" % (kind, " ".join(detail.split())))

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send(b'{"object":"list","data":[{"id":"stub-model"}]}', "application/json")

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0)).decode()
        messages = json.loads(body).get("messages", [])
        system = text(messages[0]) if messages and messages[0].get("role") == "system" else ""
        if system.startswith("You are the safety classifier"):
            # Only the action under review: the task context above it lists
            # the calls already made, read-only ones included.
            note("classifier", text(messages[-1]).split("## Action to review")[-1])
            frames = saying("<block>no</block>")
        else:
            results = sum(1 for m in messages if m.get("role") == "tool")
            note("chat", str(results))
            if results == 0:
                frames = calling([("bash", {"command": "ls -la"}),
                                  ("bash", {"command": "grep -rn TODO ."})], len(messages))
            elif results == 2:
                frames = calling([("bash", {"command": "touch made.txt"})], len(messages))
            else:
                frames = saying("Done.")
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
TG_STUB=$!
smoke_on_exit kill "$TG_STUB"
TG_PROVIDERS="$SMOKE_TMP/providers146.toml"
printf '[providers.stub]\nname = "Stub"\n\n[providers.stub.kwargs]\napi_base = "http://127.0.0.1:%s/v1"\n' "$TG_PORT" >"$TG_PROVIDERS"
TG_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_VERBOSE=0 ALTER_ZERO_PERMISSION_MODE=auto ALTER_ZERO_PROVIDERS_FILE=$TG_PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"
launch -c "$WORK146" "$S146B" 100 40 "$TG_APP"
submit "$S146B" "smoke146 look around, then touch a file"
auto_done="$(wait_settled 30 "$S146B" -S -80 -- -E '^Done\.|^● Done\.')" ||
	fail "the auto-mode turn never settled (a prompt left open?)"
dump "the auto-mode turn" "$auto_done"
dump "the requests the stub saw" "$(cat "$TG_LOG")"
expect_has "$auto_done" -E '^Searched for 1 pattern, listed 1 directory \(ctrl\+o to expand\)$' \
	"the listing and the search did not fold into one line"
expect_lacks "$auto_done" -E '● Bash\((ls -la|grep -rn TODO \.)\)' "a read-only command was drawn as its own cell"
expect_has "$auto_done" -F "● Bash(touch made.txt)" "the touch is missing its own cell"
expect_has "$auto_done" -F "Allowed by auto mode classifier" "the touch was not decided by the classifier"
expect_lacks "$auto_done" -F "Do you want to proceed?" "auto mode opened a permission prompt"
group_row="$(grep -nE '^Searched for 1 pattern' <<<"$auto_done" | cut -d: -f1)"
touch_row="$(grep -nF '● Bash(touch made.txt)' <<<"$auto_done" | cut -d: -f1)"
if [ -z "$group_row" ] || [ -z "$touch_row" ] || [ "$group_row" -ge "$touch_row" ]; then
	fail "the group's line is not committed above the call that ended it"
fi
[ -f "$WORK146/made.txt" ] || fail "the classified touch never ran"
expect_eq "$(grep -c '^classifier' "$TG_LOG")" "1" "the classifier was not asked exactly once"
expect_has "$(grep '^classifier' "$TG_LOG")" -F "touch made.txt" "the classifier reviewed something other than the touch"
expect_lacks "$(grep '^classifier' "$TG_LOG")" -E 'ls -la|grep -rn' "a read-only command was sent to the classifier"
tmux kill-session -t "$S146B" 2>/dev/null

# Manual mode asks for every command, read-only ones included — and the
# prompt replaces the strip that was drawing the group. So the group the
# turn is holding is drawn above the call being asked about, and the run
# still settles into ONE line once the `touch` ends it.
S146C="${S}_toolgroups_manual"
: >"$TG_LOG"
TG_MANUAL="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_VERBOSE=0 ALTER_ZERO_PROVIDERS_FILE=$TG_PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"
launch -c "$WORK146" "$S146C" 100 40 "$TG_MANUAL"
submit "$S146C" "smoke146 look around, then touch a file"
first_ask="$(wait_pane 15 "$S146C" -F "Do you want to proceed?")" || fail "manual mode never asked about the listing"
dump "the prompt for the listing" "$first_ask"
expect_has "$first_ask" -F "● Bash(ls -la)" "the first prompt is not about the listing"
keys "$S146C" Enter
# The listing ran and is held: the prompt for the search keeps it on screen.
held_ask() {
	asked="$(pane "$S146C")"
	has "$asked" -F "Do you want to proceed?" && has "$asked" -F "● Bash(grep -rn TODO .)"
}
poll 15 held_ask || fail "manual mode never asked about the search"
dump "the prompt for the search, the held listing above it" "$asked"
expect_has "$asked" -E '^● Listing 1 directory… \(ctrl\+o to expand\)$' "the held listing is not drawn above the prompt"
expect_has "$asked" -E '^  ⎿  \$ ls -la$' "the held group does not name the listing"
keys "$S146C" Enter
touch_ask() {
	asked="$(pane "$S146C" -S -60)"
	has "$asked" -F "Do you want to proceed?" && has "$asked" -F "● Bash(touch made.txt)"
}
poll 15 touch_ask || fail "manual mode never asked about the touch"
dump "the prompt for the touch, the group settled above it" "$asked"
expect_has "$asked" -E '^Searched for 1 pattern, listed 1 directory \(ctrl\+o to expand\)$' \
	"the group did not settle into its line when the touch ended it"
keys "$S146C" Enter
manual_done="$(wait_settled 30 "$S146C" -S -80 -- -E '^● Done\.')" || fail "the manual-mode turn never settled"
dump "the manual-mode turn" "$manual_done"
expect_eq "$(grep -cE '^Searched for 1 pattern, listed 1 directory \(ctrl\+o to expand\)$' <<<"$manual_done")" "1" \
	"the closed prompts' rebuilds did not leave the group's one line"
expect_lacks "$manual_done" -E '● Bash\((ls -la|grep -rn TODO \.)\)' "a read-only command was drawn as its own cell in manual mode"
expect_lacks "$manual_done" -E '(Listing|Searching) .*…' "the live group was left behind in scrollback"
expect_has "$manual_done" -F "● Bash(touch made.txt)" "the touch is missing its own cell"
expect_eq "$(grep -c '^classifier' "$TG_LOG")" "0" "manual mode asked the classifier"
tmux kill-session -t "$S146C" 2>/dev/null

smoke_finish
