#!/usr/bin/env bash
# scripts/text_layout.sh — the idle text layout's tooling (docs/text-layout.md).
#
#   scripts/text_layout.sh profile [--idle SECS] [--no-build] [--names DIR]
#       Rewrite layout/idle-functions.txt from what an idle session runs:
#       build the release and the dist binary, run each idle under
#       valgrind's callgrind in every scenario below, and list what each
#       executed, one group a scenario. Build again afterwards and the
#       linker gathers the new list. --names keeps the raw lists of
#       executed symbols in DIR.
#   scripts/text_layout.sh measure [--runs N] [--settle SECS] [--first-run | --plugins |
#                                  --config-home DIR [--env KEY=VALUE]...] BINARY...
#       Each binary's resident memory at rest — launched, nothing typed —
#       freshly written and read back from disk, beside its idle section.
#       A configured session unless another scenario is named; --config-home
#       starts from a copy of DIR instead (--env adds to the environment).
#   scripts/text_layout.sh section BINARY...
#       The idle section's size beside the rest of the code.
#
# The scenarios, each a config home the binary under test makes for itself by
# being driven through a short session, so no file here is in a format this
# script knows:
#
#   a configured session   the state nearly every session is in: a provider
#                          signed in to and a model chosen, a turn taken, a
#                          theme, a setting and a permission mode changed, in
#                          a git checkout with an AGENTS.md
#   a first run            an empty config home, the offline demo backend
#   with plugins           the configured session with a plugin on that
#                          brings a skill, a command, an agent and a hook
#
# Nothing leaves the machine. Every launch is in a private tmux server, in an
# environment holding nothing but PATH, TERM and what is set below; the
# provider is a stub on the loopback, which also stands in for the telemetry
# collector and the release page, so the first launch's daily ping and update
# check land there and the launches that are measured find both done for the
# day — as a session that is not the day's first does.
#
# Needs tmux, python3 and binutils (nm, readelf); `profile` needs valgrind.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LIST="$ROOT/layout/idle-functions.txt"
SECTION=".text.idle"

die() {
	printf 'text_layout: %s\n' "$*" >&2
	exit 1
}
need() {
	local tool
	for tool in "$@"; do
		command -v "$tool" >/dev/null 2>&1 || die "$tool is needed and is not on PATH"
	done
}

WORK=""
SOCK="az-layout-$$"
STUB_PID=""
STUB_PORT=""
# Variables `measure --env` adds to every launch.
EXTRA_ENV=()
cleanup() {
	tmux -L "$SOCK" kill-server 2>/dev/null || true
	# tmux leaves a dead server's socket behind.
	rm -f "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/$SOCK"
	[ -z "$STUB_PID" ] || kill "$STUB_PID" 2>/dev/null || true
	[ -z "$WORK" ] || rm -rf "$WORK"
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# The fixture: a home, a config home, a project, a stub on the loopback.
# ---------------------------------------------------------------------------

# One process for everything a session would reach over the network: an
# OpenAI-compatible provider listing one model and streaming one reply, the
# telemetry collector (any other POST: 204) and the release page (a HEAD of
# /releases/latest redirects to the running version's tag: nothing newer).
start_stub() {
	local version="$1" i
	STUB_PORT="$(python3 -I -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
	python3 -I - "$STUB_PORT" "$version" <<'PY' &
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port, version = int(sys.argv[1]), sys.argv[2]
# A record the listing's readers learn everything from, so nothing is left
# for a startup probe to ask: the window, that it sees images, no reasoning.
MODELS = {
    "data": [
        {
            "id": "layout-model",
            "context_length": 200000,
            "architecture": {"input_modalities": ["text", "image"]},
            "supported_parameters": ["tools"],
        }
    ]
}
REPLY = [
    {"choices": [{"index": 0, "delta": {"content": "Stub reply."}}]},
    {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
]


class Stub(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def answer(self, code, body=b"", headers=()):
        self.send_response(code)
        for name, value in headers:
            self.send_header(name, value)
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        if body and self.command != "HEAD":
            self.wfile.write(body)

    def do_HEAD(self):
        if self.path.rstrip("/").endswith("/releases/latest"):
            self.answer(302, headers=[("location", f"/releases/tag/v{version}")])
        else:
            self.answer(200)

    def do_GET(self):
        if self.path.startswith("/v1/models"):
            body = json.dumps(MODELS).encode()
            self.answer(200, body, [("content-type", "application/json")])
        else:
            self.answer(404)

    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length") or 0))
        if self.path.startswith("/v1/chat"):
            frames = "".join(f"data: {json.dumps(frame)}\n\n" for frame in REPLY)
            body = (frames + "data: [DONE]\n\n").encode()
            self.answer(200, body, [("content-type", "text/event-stream")])
        else:
            self.answer(204)

    def log_message(self, *_args):
        pass


ThreadingHTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
	STUB_PID=$!
	for i in $(seq 1 50); do
		if python3 -I -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $STUB_PORT)) == 0 else 1)" 2>/dev/null; then
			return 0
		fi
		sleep 0.1
	done
	die "the stub provider did not start"
}

# A fresh fixture for BINARY: an empty home and config home, a project that
# is a git checkout with an AGENTS.md, and the stub.
fixture() {
	local bin="$1" version
	tmux -L "$SOCK" kill-server 2>/dev/null || true
	[ -z "$STUB_PID" ] || kill "$STUB_PID" 2>/dev/null || true
	STUB_PID=""
	[ -z "$WORK" ] || rm -rf "$WORK"
	WORK="$(mktemp -d)"
	mkdir -p "$WORK/home" "$WORK/cfg" "$WORK/bin" "$WORK/proj/.git"
	printf '# Project\n\nA small project. Keep changes small and run the tests.\n' >"$WORK/proj/AGENTS.md"
	version="$("$bin" --version 2>/dev/null | awk '{ print $NF }')"
	start_stub "${version:-0.0.0}"
}

# The sanitized environment, as `env -i` words.
session_env() {
	printf '%s\n' \
		"PATH=$PATH" "TERM=xterm-256color" "LANG=C.UTF-8" "HOME=$WORK/home" \
		"ALTER_ZERO_CONFIG_DIR=$WORK/cfg" "ALTER_ZERO_SESSIONS_DIR=$WORK/sessions" \
		"ALTER_ZERO_TELEMETRY_URL=http://127.0.0.1:$STUB_PORT/ping" \
		"ALTER_ZERO_UPDATE_URL=http://127.0.0.1:$STUB_PORT" \
		"NO_PROXY=127.0.0.1" "no_proxy=127.0.0.1" \
		"${EXTRA_ENV[@]}"
}

# launch COMMAND... — the command in a new 120x40 pane of the private server,
# in the project directory and the sanitized environment.
launch() {
	local cmd="exec env -i" word
	while IFS= read -r word; do
		[ -z "$word" ] || cmd+=" $(printf '%q' "$word")"
	done < <(session_env)
	for word in "$@"; do
		cmd+=" $(printf '%q' "$word")"
	done
	tmux -L "$SOCK" new-session -d -x 120 -y 40 -c "$WORK/proj" "$cmd" 2>/dev/null ||
		{
			sleep 1
			tmux -L "$SOCK" new-session -d -x 120 -y 40 -c "$WORK/proj" "$cmd"
		}
}
# Run COMMAND... to its end in the same environment, outside tmux.
run_plain() {
	local words=() word
	while IFS= read -r word; do
		[ -z "$word" ] || words+=("$word")
	done < <(session_env)
	(cd "$WORK/proj" && env -i "${words[@]}" "$@")
}
# Take the server down and wait until it is gone: a launch that follows too
# closely finds the old server still leaving ("server exited unexpectedly").
stop() {
	local i
	tmux -L "$SOCK" kill-server 2>/dev/null || true
	for i in $(seq 1 50); do
		tmux -L "$SOCK" list-sessions >/dev/null 2>&1 || break
		sleep 0.1
	done
	sleep 0.3
}
screen() { tmux -L "$SOCK" capture-pane -p 2>/dev/null || true; }
# wait_screen ERE [SECONDS] — until the pane shows it.
wait_screen() {
	local pattern="$1" limit="${2:-30}" i
	for i in $(seq 1 $((limit * 5))); do
		if screen | grep -Eq -- "$pattern"; then
			return 0
		fi
		sleep 0.2
	done
	screen | tail -12 >&2
	die "the session never showed: $pattern"
}
type_text() {
	tmux -L "$SOCK" send-keys -l "$1"
	sleep 0.3
}
press() {
	tmux -L "$SOCK" send-keys "$@"
	sleep 0.3
}
# Ctrl+C on an empty composer quits; wait for the pane to go.
quit_session() {
	local i
	press C-c
	for i in $(seq 1 50); do
		tmux -L "$SOCK" list-sessions >/dev/null 2>&1 || break
		sleep 0.1
	done
	stop
}
# The pid running BINARY under the pane (the pane's own process, or a child).
app_pid() {
	local bin="$1" pane p
	pane="$(tmux -L "$SOCK" list-panes -a -F '#{pane_pid}' | head -1)"
	for p in "$pane" $(pgrep -P "$pane" 2>/dev/null || true); do
		if [ "$(readlink "/proc/$p/exe" 2>/dev/null)" = "$bin" ]; then
			printf '%s\n' "$p"
			return 0
		fi
	done
	return 1
}

# ---------------------------------------------------------------------------
# The scenarios: each leaves the fixture's config home as its sessions find it.
# ---------------------------------------------------------------------------

# A first run: one plain launch, so the launches that count are ordinary
# ones — the first writes the built-in agents and skills into the config
# home, shows the telemetry notice and makes the day's ping and update check.
scenario_first_run() {
	launch "$1"
	wait_screen "Alter Zero" 60
	sleep 3
	quit_session
}

# A configured session: a provider with a key, a model chosen from its
# listing, a turn taken, a theme, a setting and a permission mode changed.
scenario_configured() {
	local bin="$1"
	cat >"$WORK/cfg/providers.toml" <<TOML
[providers.layout]
name = "Layout"
api_key_env = "LAYOUT_API_KEY"

[providers.layout.kwargs]
api_base = "http://127.0.0.1:$STUB_PORT/v1"
TOML
	printf 'LAYOUT_API_KEY=layout\n' >"$WORK/cfg/.env"
	launch "$bin"
	wait_screen "Alter Zero" 60
	type_text "/model"
	press Enter
	wait_screen "layout-model" 30
	press Enter
	# The footer names the model once the session runs on it.
	wait_screen "^ *layout-model .*·" 30
	type_text "hello"
	press Enter
	wait_screen "Stub reply" 60
	wait_screen "^[A-Z][a-z]+ for [0-9]+s" 60
	type_text "/theme"
	press Enter
	wait_screen "Select theme" 20
	press Down
	press Enter
	type_text "/settings"
	press Enter
	wait_screen "Settings" 20
	press Down
	press Enter
	press Escape
	press BTab
	sleep 0.5
	quit_session
	[ -s "$WORK/cfg/config.json" ] || die "the session saved no model selection"
}

# With plugins: the configured session, plus a plugin from a folder
# marketplace that brings one of each thing a plugin can bring without
# starting a process — and its hook trusted in the review, as a plugin a
# user has had for a while is.
scenario_plugins() {
	local bin="$1" market="$WORK/market"
	scenario_configured "$bin"
	mkdir -p "$market/.claude-plugin" "$market/plugins/hello/.claude-plugin" \
		"$market/plugins/hello/skills/greet" "$market/plugins/hello/commands" \
		"$market/plugins/hello/agents" "$market/plugins/hello/hooks"
	cat >"$market/.claude-plugin/marketplace.json" <<'JSON'
{"name": "layout-market", "owner": {"name": "layout"},
 "plugins": [{"name": "hello", "source": "./plugins/hello", "description": "Says hello"}]}
JSON
	printf '{"name": "hello", "version": "1.0.0", "description": "Says hello"}\n' \
		>"$market/plugins/hello/.claude-plugin/plugin.json"
	printf -- '---\nname: greet\ndescription: Greets\n---\nSay hello.\n' \
		>"$market/plugins/hello/skills/greet/SKILL.md"
	printf -- '---\ndescription: Says hi\n---\nSay hi.\n' >"$market/plugins/hello/commands/hi.md"
	printf -- '---\nname: helper\ndescription: Helps\n---\nHelp.\n' >"$market/plugins/hello/agents/helper.md"
	printf '{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "true"}]}]}}\n' \
		>"$market/plugins/hello/hooks/hooks.json"
	run_plain "$bin" plugin marketplace add "$market" >/dev/null ||
		die "this binary has no plugin commands (measure it without --plugins)"
	run_plain "$bin" plugin install hello@layout-market >/dev/null
	# The review asks about the new hook on the next launch: trust it.
	launch "$bin"
	wait_screen "new hook" 60
	press Enter
	sleep 1
	quit_session
}

# scenario NAME BINARY — a fresh fixture left as that scenario's sessions find it.
scenario() {
	local name="$1" bin="$2"
	fixture "$bin"
	case "$name" in
	configured) scenario_configured "$bin" ;;
	first-run) scenario_first_run "$bin" ;;
	plugins) scenario_plugins "$bin" ;;
	*) die "no scenario named $name" ;;
	esac
}

# ---------------------------------------------------------------------------
# profile
# ---------------------------------------------------------------------------

# executed BINARY IDLE_SECS OUT — every function an idle session of BINARY
# executes in the fixture as it stands, by its raw symbol name, one a line.
executed() {
	local bin="$1" idle="$2" out="$3" pid
	launch valgrind --tool=callgrind --demangle=no --dump-instr=no --collect-jumps=no \
		"--callgrind-out-file=$WORK/callgrind.out" "$bin"
	wait_screen "Alter Zero" 300
	sleep "$idle"
	pid="$(pgrep -f "callgrind.*$bin" | head -1 || true)"
	[ -n "$pid" ] || die "the profiled session is gone"
	callgrind_control -d "$pid" >/dev/null 2>&1 || true
	sleep 2
	stop
	sleep 1
	cat "$WORK"/callgrind.out* 2>/dev/null | python3 -I -c '
import re, sys
names, ids = set(), {}
for line in sys.stdin:
    m = re.match(r"^c?fn=\((\d+)\)(?: (.*))?$", line.rstrip("\n"))
    if m:
        number, name = m.group(1), m.group(2)
        if name:
            ids[number] = name
        names.add(ids.get(number, ""))
names.discard("")
print("\n".join(sorted(names)))
' >"$out"
	rm -f "$WORK"/callgrind.out*
	[ -s "$out" ] || die "callgrind recorded nothing for $bin"
}

cmd_profile() {
	local idle=150 build=1 keep=""
	while [ $# -gt 0 ]; do
		case "$1" in
		--idle)
			idle="${2:?--idle takes seconds}"
			shift 2
			;;
		--no-build)
			build=0
			shift
			;;
		--names)
			keep="${2:?--names takes a directory}"
			shift 2
			;;
		*) die "profile: unknown argument '$1'" ;;
		esac
	done
	need tmux python3 nm valgrind callgrind_control
	local target="${CARGO_TARGET_DIR:-$ROOT/target}"
	local names
	if [ -n "$keep" ]; then
		mkdir -p "$keep"
		names="$(cd "$keep" && pwd)"
	else
		names="$(mktemp -d)"
	fi
	local profile bin name secs runs=()
	for profile in release dist; do
		bin="$target/$profile/alter-zero"
		if [ "$build" = 1 ]; then
			printf 'building the %s binary\n' "$profile" >&2
			(cd "$ROOT" && cargo build --profile "$profile" --bin alter-zero)
		fi
		[ -x "$bin" ] || die "$bin is not built (drop --no-build)"
		bin="$(readlink -f "$bin")"
		# The configured session is watched the longest: long enough for the
		# banner mascot's nap, which every scenario would run the same.
		for name in configured first-run plugins; do
			secs=30
			[ "$name" != configured ] || secs="$idle"
			printf 'profiling the %s binary, %s, idle for %ss\n' "$profile" "$name" "$secs" >&2
			scenario "$name" "$bin"
			executed "$bin" "$secs" "$names/$profile-$name.txt"
			runs+=("$name:$names/$profile-$name.txt:$bin")
		done
	done
	python3 -I - "$LIST" "${runs[@]}" <<'PY'
import re
import subprocess
import sys

HEADER = """\
# The functions an idle session runs (docs/text-layout.md): build.rs has
# the linker put them side by side, so the code a session keeps in memory is
# what it runs and not whatever the default order left beside it.
#
# A [group] is one run of code, in this order: what a configured session
# runs, then what only a first run adds, then what a session with plugins
# adds. One symbol pattern a line, sorted within its group; a legacy-mangled
# Rust name keeps its path and leaves its hash to the `*`.
#
# Written by scripts/text_layout.sh profile from idle sessions of the
# release and the dist build — take it again there, never edit it by hand.
"""

# The groups, in the order the linker lays them out, each named after the
# scenario whose sessions run it.
GROUPS = [
    ("configured", "a configured session"),
    ("first-run", "a first run"),
    ("plugins", "with plugins"),
]

# A legacy-mangled Rust symbol: `_ZN{path}17h{16 hex}E`, and after it
# whatever a local symbol gained on the way (`.llvm.123`, `.6455`).
LEGACY = re.compile(r"^(_ZN.*17h)[0-9a-f]{16}E(?:\.[A-Za-z0-9_.]+)?$")
# What build.rs takes a pattern to be made of.
PATTERN = re.compile(r"^[A-Za-z0-9_$.*]+$")


def pattern(name):
    """The name with what moves between builds left to a `*`."""
    legacy = LEGACY.match(name)
    if legacy:
        return legacy.group(1) + "*"
    if name.startswith("_R"):
        # A v0 name's crate disambiguators (the allocator shims have them).
        return re.sub(r"Cs[0-9A-Za-z]+_", "Cs*_", name)
    return name


SIZES = {}


def functions(binary):
    """The functions the binary itself defines (not libc's), with their sizes."""
    if binary not in SIZES:
        out = subprocess.run(
            ["nm", "--defined-only", "-S", binary], capture_output=True, text=True, check=True
        ).stdout
        sizes = {}
        for row in (line.split() for line in out.splitlines()):
            if len(row) == 4 and row[2] in "tTwW":
                sizes[row[3]] = int(row[1], 16)
        SIZES[binary] = sizes
    return SIZES[binary]


ran_by = {key: set() for key, _ in GROUPS}
for run in sys.argv[2:]:
    scenario, names, binary = run.split(":", 2)
    defined = functions(binary)
    ran = [line.strip() for line in open(names) if line.strip()]
    own = [name for name in ran if name in defined]
    usable = {pattern(name) for name in own if PATTERN.match(pattern(name))}
    # What the patterns take along: a generic function's instances share a
    # name but for the hash, so the one an idle session runs brings the rest.
    taken = sum(size for name, size in defined.items() if pattern(name) in usable)
    print(
        f"{binary}, {scenario}: ran {len(ran)} functions, {len(own)} its own"
        f" ({sum(defined[name] for name in own) // 1024} kB);"
        f" {len(usable)} patterns, which take {taken // 1024} kB of it",
        file=sys.stderr,
    )
    ran_by[scenario] |= usable

listed, groups = set(), []
for key, title in GROUPS:
    fresh = sorted(ran_by[key] - listed)
    listed |= set(fresh)
    groups.append((title, fresh))
if len(groups[0][1]) < 500:
    sys.exit(f"only {len(groups[0][1])} functions found: the profile did not work")
with open(sys.argv[1], "w") as out:
    out.write(HEADER)
    for title, patterns in groups:
        out.write(f"[{title}]\n" + "".join(f"{p}\n" for p in patterns))
for title, patterns in groups:
    print(f"[{title}]: {len(patterns)} functions", file=sys.stderr)
PY
	[ -n "$keep" ] || rm -rf "$names"
	printf 'build again (cargo build --release) and the binary is linked with the new list\n' >&2
}

# ---------------------------------------------------------------------------
# section
# ---------------------------------------------------------------------------

# section_sizes BINARY — "IDLE_BYTES TEXT_BYTES" (0 for a missing section).
section_sizes() {
	readelf -SW "$1" | python3 -I -c '
import sys
want = {sys.argv[1]: 0, ".text": 0}
for line in sys.stdin:
    cells = line.replace("[", " ").replace("]", " ").split()
    if len(cells) > 5 and cells[1] in want and cells[2] == "PROGBITS":
        want[cells[1]] = int(cells[5], 16)
print(want[sys.argv[1]], want[".text"])
' "$SECTION"
}

cmd_section() {
	need readelf python3
	[ $# -gt 0 ] || die "section: name a binary"
	local bin idle text
	for bin in "$@"; do
		[ -f "$bin" ] || die "$bin: no such file"
		read -r idle text < <(section_sizes "$bin")
		if [ "$idle" = 0 ]; then
			printf '%s: no %s section — not laid out (%d kB of code)\n' "$bin" "$SECTION" "$((text / 1024))"
		else
			printf '%s: %s %d kB, the rest of the code %d kB\n' "$bin" "$SECTION" "$((idle / 1024))" "$((text / 1024))"
		fi
	done
}

# ---------------------------------------------------------------------------
# measure
# ---------------------------------------------------------------------------

status_field() { awk -v key="$2:" '$1 == key { print $2 }' "/proc/$1/status"; }
median() { printf '%s\n' "$@" | sort -n | awk '{ row[NR] = $1 } END { print row[int((NR + 1) / 2)] }'; }

# The resident part of the binary's own executable mapping, in kB.
text_rss() {
	awk -v bin="$2" '
		/^[0-9a-f]+-[0-9a-f]+ / { mine = ($6 == bin && $2 ~ /x/) }
		/^Rss:/ && mine { total += $2 }
		END { print total + 0 }' "/proc/$1/smaps"
}

# idle_once BINARY SETTLE — "VmRSS RssAnon RssFile TEXT" of one launch, kB.
idle_once() {
	local bin="$1" settle="$2" pid
	launch "$bin"
	wait_screen "Alter Zero" 60
	sleep "$settle"
	pid="$(app_pid "$bin")" || die "the session is gone"
	printf '%s %s %s %s\n' "$(status_field "$pid" VmRSS)" "$(status_field "$pid" RssAnon)" \
		"$(status_field "$pid" RssFile)" "$(text_rss "$pid" "$bin")"
	stop
}

# measure_state LABEL BINARY RUNS SETTLE — the median launch.
measure_state() {
	local label="$1" bin="$2" runs="$3" settle="$4"
	local rss=() anon=() file=() text=() r a f t i
	for i in $(seq 1 "$runs"); do
		read -r r a f t < <(idle_once "$bin" "$settle")
		rss+=("$r")
		anon+=("$a")
		file+=("$f")
		text+=("$t")
	done
	r="$(median "${rss[@]}")"
	printf '  %-14s %6d kB (%5.2f MB)   anonymous %5d kB   file-backed %6d kB   its code %5d kB\n' \
		"$label" "$r" "$(echo "$r / 1024" | bc -l)" "$(median "${anon[@]}")" "$(median "${file[@]}")" "$(median "${text[@]}")"
}

cmd_measure() {
	local runs=5 settle=8 name=configured config="" bins=()
	EXTRA_ENV=()
	while [ $# -gt 0 ]; do
		case "$1" in
		--runs)
			runs="${2:?--runs takes a count}"
			shift 2
			;;
		--settle)
			settle="${2:?--settle takes seconds}"
			shift 2
			;;
		--first-run)
			name=first-run
			shift
			;;
		--plugins)
			name=plugins
			shift
			;;
		--config-home)
			config="${2:?--config-home takes a directory}"
			[ -d "$config" ] || die "$config: no such directory"
			shift 2
			;;
		--env)
			EXTRA_ENV+=("${2:?--env takes KEY=VALUE}")
			shift 2
			;;
		-*) die "measure: unknown argument '$1'" ;;
		*)
			bins+=("$1")
			shift
			;;
		esac
	done
	need tmux python3 readelf bc
	[ ${#bins[@]} -gt 0 ] || die "measure: name a binary"
	local given bin copy idle text label
	for given in "${bins[@]}"; do
		[ -x "$given" ] || die "$given: not an executable"
		# The fixture is made by the binary as given; what is measured is a
		# copy of its own — a file nothing else has open, sitting in the page
		# cache exactly as this script put it there.
		given="$(readlink -f "$given")"
		if [ -n "$config" ]; then
			fixture "$given"
			cp -a "$config"/. "$WORK/cfg"/
			scenario_first_run "$given"
			label="the config home $config"
		else
			scenario "$name" "$given"
			label="$(printf '%s' "$name" | tr - ' ')"
		fi
		copy="$WORK/bin/alter-zero"
		cp "$given" "$copy"
		bin="$(readlink -f "$copy")"
		read -r idle text < <(section_sizes "$bin")
		printf '%s — %s, %s %d kB of %d kB of code\n' "$given" "$label" "$SECTION" "$((idle / 1024))" "$(((idle + text) / 1024))"
		# How much of a file is resident depends on how it reached the page
		# cache: the kernel maps the whole block a fault lands in, and a file
		# read from disk sits in larger blocks than one just written. Both
		# are states a user's binary is in — just installed, or after a
		# reboot — so both are measured.
		measure_state "just written" "$bin" "$runs" "$settle"
		sync "$bin"
		dd if="$bin" iflag=nocache count=0 status=none
		cat "$bin" >/dev/null
		measure_state "read from disk" "$bin" "$runs" "$settle"
	done
}

case "${1:-}" in
profile)
	shift
	cmd_profile "$@"
	;;
measure)
	shift
	cmd_measure "$@"
	;;
section)
	shift
	cmd_section "$@"
	;;
*)
	# The header comment, as the usage.
	awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "${BASH_SOURCE[0]}"
	exit 2
	;;
esac
