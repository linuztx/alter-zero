#!/usr/bin/env bash
# scripts/smoke/lib.sh — the shared half of the smoke suite.
#
# Every phase file under scripts/smoke/phases/ sources this and calls
# `smoke_begin`; the runner (scripts/smoke.sh) sources it for the phase
# discovery helpers. Nothing here drives the app — the phases do that — this
# file owns the FIXTURE: the sanitized environment, the private tmux server,
# the throwaway config home, the launch/poll/assert vocabulary, the fail
# accounting, and the teardown that takes it all down again.
#
# Isolation is per PHASE, not per suite. Each phase gets its own tmux server on
# its own socket, its own temp tree ($SMOKE_TMP) holding its own config home,
# skills/agents roots and work dirs, and its own $TMPDIR pointed into that tree
# — so a bare `mktemp -d` lands inside it and the whole thing is one `rm -rf`
# at exit. That is what lets the runner execute phases in parallel and lets a
# developer run one phase on its own (`bash scripts/smoke/phases/055-*.sh`):
# no phase can see, or leave behind, another phase's state.

[ -n "${SMOKE_LIB_LOADED:-}" ] && return 0
SMOKE_LIB_LOADED=1

set -uo pipefail

SMOKE_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SMOKE_ROOT="$(cd "$SMOKE_LIB_DIR/../.." && pwd)"
SMOKE_PHASES_DIR="$SMOKE_LIB_DIR/phases"

# ---------------------------------------------------------------------------
# Constants every phase reads.
# ---------------------------------------------------------------------------
USER_MSG="hello there"
# The dummy reply is deterministic per prompt (dummy_response: char-count % 3).
# "hello there" is 11 chars → responses[2], which opens with this phrase.
EXPECT_REPLY="Happy to help"
# Every canned demo reply CLOSES on the same hand-off paragraph — the two
# commands that swap the dummy for a real model (docs/dummy-backend.md). It is
# one shared sentence, pinned by
# `stream::tests::script::the_handoff_paragraph_closes_every_demo_reply`, so
# this one marker means "the turn finished streaming" whatever prompt was sent.
SETTLED_REPLY="Two commands away"
# The startup banner's tier-independent title word (docs/header.md).
HEADER_MARK="Alter Zero"
# The dummy AI pauses before streaming (so the status indicator shows first) —
# 3s by default. Every phase runs with a SHORT delay so the turns stream
# promptly; the phases that want a visible pause set their own.
SMOKE_STARTUP_MS="${SMOKE_STARTUP_MS:-200}"
# The cadence every polling helper samples the pane at.
SMOKE_POLL_INTERVAL="${SMOKE_POLL_INTERVAL:-0.1}"
# The pause between typing a message and pressing Enter: a burst of keys is
# what `paste::PasteBurst` treats as a paste, so an Enter riding straight on
# the text would be read as part of it.
SMOKE_TYPE_SETTLE="${SMOKE_TYPE_SETTLE:-0.2}"

# ---------------------------------------------------------------------------
# The environment the suite states rather than inherits (docs/design.md, "The
# smoke suite defines its own environment"). Every assertion is written
# against the DUMMY backend's canned turns, so a resolvable provider key, a
# stale `ALTER_ZERO_*` knob, `NO_COLOR`, a reachable display or a developer's
# `OLLAMA_HOST` each turn passing assertions into failures that say nothing
# about the app. Idempotent; run before the first launch of every phase.
# ---------------------------------------------------------------------------
smoke_sanitize_env() {
	local v
	for v in $(env | sed -n 's/^\([A-Za-z0-9_]*API_KEY\)=.*/\1/p'); do
		unset "$v"
	done
	for v in $(env | sed -n 's/^\(ALTER_ZERO_[A-Za-z0-9_]*\)=.*/\1/p'); do
		unset "$v"
	done
	# The Ollama provider is configured by being *pointed at* (docs/ollama.md):
	# a developer's own host would make the /model phases fetch from a server
	# the suite doesn't run.
	unset OLLAMA_HOST
	# Colour is the medium a dozen assertions read; NO_COLOR disables it
	# wholesale (crossterm memoizes it and emits no SGR at all).
	unset NO_COLOR
	# The clipboard phases are written headless ON PURPOSE: with a display the
	# native arboard path wins, the OSC 52 fallback they read back never fires,
	# and the suite overwrites the developer's real clipboard on its way past.
	unset DISPLAY WAYLAND_DISPLAY XAUTHORITY
	# Telemetry (docs/telemetry.md) is opt-out and pings a real collector once
	# a day per config home — off for the suite, both exported (the tmux
	# server inherits it) and spelled out in every launch string.
	export ALTER_ZERO_TELEMETRY=0
}

# ---------------------------------------------------------------------------
# Phase discovery — shared with the runner, so a phase's identity comes from
# ONE place: its file name (`055-permissions.sh` → id 55, `107b-…` → 107b) and
# its first comment line (`# Phase 55 — title`). Tags ride an optional
# `# smoke: tags=serial` line: `serial` phases run after the parallel pool,
# one at a time, for measurements a loaded machine would skew.
# ---------------------------------------------------------------------------
smoke_phase_id() { # file → "55" | "107b"
	local base
	base="$(basename "$1" .sh)"
	base="${base%%-*}"
	# strip the zero padding: 055 → 55, 107b → 107b
	while [ "${#base}" -gt 1 ] && [ "${base:0:1}" = "0" ]; do base="${base:1}"; done
	printf '%s' "$base"
}
smoke_phase_slug() { # file → "permissions"
	local base
	base="$(basename "$1" .sh)"
	printf '%s' "${base#*-}"
}
smoke_phase_title() { # file → the text after "# Phase N — " on the first such line
	# (An alternation, not a bracket: under a byte-wise C locale a bracket
	# matches one BYTE of the three-byte em dash.)
	sed -n -E '1,20{s/^# Phase [0-9]+[a-z]? (—|-) (.*)$/\2/p}' "$1" | head -1
}
smoke_phase_tags() { # file → "serial" (space-separated tags, possibly empty)
	sed -n -E '1,20{s/^# smoke: tags=(.*)$/\1/p}' "$1" | head -1 | tr ',' ' '
}

# ---------------------------------------------------------------------------
# Per-phase setup. Call once at the top of a phase file, after sourcing.
# ---------------------------------------------------------------------------
smoke_begin() {
	SMOKE_PHASE_FILE="${SMOKE_PHASE_FILE:-${BASH_SOURCE[1]:-$0}}"
	SMOKE_PHASE="$(smoke_phase_id "$SMOKE_PHASE_FILE")"
	SMOKE_PHASE_TITLE="$(smoke_phase_title "$SMOKE_PHASE_FILE")"
	SMOKE_PHASE_TITLE="${SMOKE_PHASE_TITLE:-$(smoke_phase_slug "$SMOKE_PHASE_FILE")}"
	SMOKE_FAILS=0
	status=0 # kept for hand-written checks that still set it; counted at exit
	SMOKE_T0="$(date +%s)"

	smoke_sanitize_env
	cd "$SMOKE_ROOT" || exit 2

	BIN="${SMOKE_BIN:-target/debug/alter-zero}"
	if [ ! -x "$BIN" ]; then
		echo "FAIL: binary not found at $BIN (run: cargo build)" >&2
		exit 2
	fi
	# Phases that run the app in a temp cwd (-c) need an ABSOLUTE binary path.
	BIN_ABS="$(readlink -f "$BIN" 2>/dev/null || realpath "$BIN")"

	# The phase's own temp tree: config home, skills/agents roots, work dirs,
	# recordings, the tmux socket — everything, so teardown is one rm -rf. The
	# name is kept SHORT: paths under it end up in the app's own rows (a
	# background command's notice headline names the script it ran), and a
	# row that wraps is a row a single-line assertion no longer finds.
	SMOKE_TMP="$(mktemp -d "${TMPDIR:-/tmp}/smoke-${SMOKE_PHASE}.XXXXXX")"
	export TMPDIR="$SMOKE_TMP"
	SMOKE_TMUX_SOCKET="$SMOKE_TMP/tmux.sock"
	# Isolate the config home (~/.alter-zero by default) so the dummy stays the
	# backend regardless of any real key/model a developer has saved there.
	SMOKE_CFG="$SMOKE_TMP/cfg"
	# ALTER_ZERO_SKILLS_DIR REPLACES the skills root list, so an empty directory
	# gives a hermetic, skill-free session with the feature itself still on (a
	# machine that HAS skills would inject their listing at the top of every
	# context and push the rows the phases assert on off the screen).
	SMOKE_SKILLS="$SMOKE_TMP/skills"
	# Agent definitions are discovered the same way; the two built-ins are seeded
	# into the temp dir at startup, so it is not empty — it is *known*.
	SMOKE_AGENTS="$SMOKE_TMP/agents"
	# Every conversation records a rollout (docs/resume.md). The app keeps them
	# under the config home now, but the suite says so explicitly: for a long
	# time they fell back to `$HOME/.alter-zero/sessions` whatever the config
	# dir was, and each run left ~100 dummy-backend sessions in the developer's
	# own `/resume` picker (`scripts/smoke.sh --sweep-leaked` clears those).
	SMOKE_SESSIONS="$SMOKE_TMP/sessions"
	# And HOME itself moves into the tree, so there is NO path by which the
	# binary under test — or tmux, or git, whatever a phase's own env string
	# says — can read or write the developer's home: `~/.alter-zero`,
	# `~/.claude/skills`, `~/.tmux.conf`, `~/.gitconfig` all resolve to an
	# empty directory that dies with the phase. (The checkpoint store supplies
	# its own git identity, so it needs nothing from there.) A phase that
	# tests HOME-relative behaviour sets its own.
	SMOKE_HOME="$SMOKE_TMP/home"
	# The developer's real home stays reachable by name for the one kind of
	# phase that needs a *conventional* one — the checkpoint scope rules read
	# `/tmp` as "an ancestor of your home directory" when HOME sits under it,
	# and a phase asserting the "shared scratch directory" refusal wants the
	# home a real user has (Phase 71). Nothing under it is ever written to.
	SMOKE_REAL_HOME="${HOME:-/nonexistent}"
	mkdir -p "$SMOKE_CFG" "$SMOKE_SKILLS" "$SMOKE_AGENTS" "$SMOKE_SESSIONS" "$SMOKE_HOME"
	export HOME="$SMOKE_HOME"
	# Filesystem checkpoints are OFF for every launch in the repo's cwd: a
	# checkpoint restore does `git reset --hard` + `git clean` on the working
	# directory. Only phases that run in a throwaway cwd re-enable them.
	# Lifecycle hooks and the project config layer are off for hermeticity too.
	CFG_ENV="ALTER_ZERO_CONFIG_DIR=$SMOKE_CFG ALTER_ZERO_SESSIONS_DIR=$SMOKE_SESSIONS ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_SKILLS_DIR=$SMOKE_SKILLS ALTER_ZERO_AGENTS_DIR=$SMOKE_AGENTS ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_TELEMETRY=0"
	# Persistence seeds the input history from a file on startup; /dev/null
	# gives every launch an EMPTY history so the ↑/↓ and Ctrl+R assertions are
	# unaffected by earlier submissions.
	CFG_ENV_NOHIST="$CFG_ENV ALTER_ZERO_HISTORY_FILE=/dev/null"
	APP="env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
	APP_ABS="env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"
	# The launch string the permission-prompt phases share: a 2.5s pre-stream
	# pause is the window their mid-turn keys land in (a fresh config home, so
	# no standing rule covers anything).
	APP_PERM="env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=2500 $BIN"
	# Session-name prefix. The server is private, so the names need only be
	# readable — every phase's sessions are `${S}_something`.
	S="smoke"

	SMOKE_EXIT_HOOKS=()
	trap smoke_finish EXIT
	# A Ctrl+C on a long phase is ordinary, and an untrapped signal kills bash
	# WITHOUT running the EXIT trap — which would strand the private server and
	# its temp tree. Funnel both into an ordinary exit so cleanup runs once.
	trap 'exit 130' INT
	trap 'exit 143' TERM

	echo "==== Phase $SMOKE_PHASE: $SMOKE_PHASE_TITLE ===="
}

# Register a command to run at teardown (a stub server to kill, say):
#   smoke_on_exit kill "$server_pid"
smoke_on_exit() {
	SMOKE_EXIT_HOOKS+=("$*")
}

smoke_finish() {
	local rc=$? elapsed hook
	trap - EXIT
	# $rc is the last command's status, and a phase's last command is often a
	# negative check — so a non-zero status here is not by itself a failure.
	# An abort bash reports (an unbound variable, a syntax error) prints its own
	# `file: line N:` message, which the runner treats as a failure of the phase.
	if [ "${status:-0}" -ne 0 ] && [ "$SMOKE_FAILS" -eq 0 ]; then
		fail "status=1 was set without a fail() call"
	fi
	for hook in ${SMOKE_EXIT_HOOKS[@]+"${SMOKE_EXIT_HOOKS[@]}"}; do
		eval "$hook" 2>/dev/null || true
	done
	# The private server itself, and with it every pane the phase opened. Its
	# socket file survives `kill-server`, so the tree holding it goes too.
	tmux kill-server 2>/dev/null
	if [ -n "${SMOKE_KEEP:-}" ]; then
		echo "==== Phase $SMOKE_PHASE: kept $SMOKE_TMP (SMOKE_KEEP) ===="
	else
		rm -rf "$SMOKE_TMP" 2>/dev/null
	fi
	elapsed=$(($(date +%s) - SMOKE_T0))
	if [ "$rc" -eq 130 ] || [ "$rc" -eq 143 ]; then
		echo "ABORT: Phase $SMOKE_PHASE — $SMOKE_PHASE_TITLE (signal, ${elapsed}s)" >&2
		exit "$rc"
	fi
	if [ "$SMOKE_FAILS" -eq 0 ]; then
		echo "PASS: Phase $SMOKE_PHASE — $SMOKE_PHASE_TITLE (${elapsed}s)"
		exit 0
	fi
	echo "FAIL: Phase $SMOKE_PHASE — $SMOKE_PHASE_TITLE: $SMOKE_FAILS check(s) failed (${elapsed}s)" >&2
	exit 1
}

# Every `tmux …` call in a phase routes to the phase's private socket, and the
# server it starts reads NO configuration file: a developer's `default-terminal`
# or status-line settings would change what the app paints, and the suite
# states its environment rather than borrowing it (docs/design.md).
tmux() { command tmux -S "$SMOKE_TMUX_SOCKET" -f /dev/null "$@"; }

# ---------------------------------------------------------------------------
# Fail accounting and log structure.
# ---------------------------------------------------------------------------
# fail MESSAGE — record one failed check. The phase keeps going: a phase is a
# scenario, and the later checks usually say more about what broke.
fail() {
	SMOKE_FAILS=$((SMOKE_FAILS + 1))
	echo "FAIL: Phase $SMOKE_PHASE — $*" >&2
}
# note TITLE — a section marker in the phase's log.
note() { echo "==== Phase $SMOKE_PHASE: $* ===="; }
# dump TITLE CONTENT — a section marker over a captured pane.
dump() {
	note "$1"
	printf '%s\n' "$2"
}

# ---------------------------------------------------------------------------
# Driving the app.
# ---------------------------------------------------------------------------
# launch [-c DIR] [-w PATTERN | -n] SESSION COLS ROWS [COMMAND]
#   Open a tmux session of the given size running COMMAND (default: $APP) and
#   wait for the app to come up — its composer prompt `❯` at the start of a
#   row — instead of sleeping a fixed 0.4s and hoping. -w waits for another
#   pattern (grep -E), -n does not wait at all (a shell pane, a --help run).
launch() {
	local dir="" wait='^❯' opt
	while getopts ':c:w:n' opt; do
		case "$opt" in
		c) dir="$OPTARG" ;;
		w) wait="$OPTARG" ;;
		n) wait="" ;;
		*) ;;
		esac
	done
	shift $((OPTIND - 1))
	OPTIND=1
	local sess="$1" cols="$2" rows="$3" cmd="${4:-$APP}"
	if [ -n "$dir" ]; then
		tmux new-session -d -s "$sess" -x "$cols" -y "$rows" -c "$dir" "$cmd"
	else
		tmux new-session -d -s "$sess" -x "$cols" -y "$rows" "$cmd"
	fi
	if [ -n "$wait" ]; then
		if ! wait_for 10 "$sess" -E "$wait"; then
			fail "the app never came up in session $sess (no '$wait' within 10s)"
			return 1
		fi
	fi
	return 0
}
# work_dir [NAME] — create and echo a short working directory for the app to run
# in, under the phase's isolated HOME so the footer renders it as `~/{NAME}`.
#
# A phase whose assertions read the footer's TAIL — the `· N shells` count, the
# context gauge — must launch in one (and so with `$APP_ABS`/`$BIN_ABS`, since
# `$BIN` may be relative to the repo). The footer lays the cwd out ahead of
# those segments and truncates the row from the right, and the phase's HOME is
# its own temp tree, so the repo cwd can no longer abbreviate to `~/…`: it
# renders absolute and the tail's fate depends on how deep the developer
# happened to clone. A 43-character checkout path already cuts `· 1 shell` to
# `· 1 sh…` at 80 columns — Phase 42 failed and Phase 43's wait for the same
# text timed out silently, both passing on a short path and neither having
# anything to do with what they test. `~/work` is six columns wherever the
# repo lives.
work_dir() {
	local dir="$SMOKE_HOME/${1:-work}"
	mkdir -p "$dir"
	printf '%s' "$dir"
}

# keys SESSION KEY… — send tmux key names (Enter, Escape, C-o, M-Up, BSpace…).
keys() { tmux send-keys -t "$@"; }
# type_text SESSION TEXT — type literal text (no key-name interpretation).
type_text() { tmux send-keys -t "$1" -l "$2"; }
# submit SESSION TEXT — type a message and press Enter, with the settle pause
# that keeps the Enter out of the paste-burst window.
submit() {
	tmux send-keys -t "$1" -l "$2"
	sleep "$SMOKE_TYPE_SETTLE"
	tmux send-keys -t "$1" Enter
}
# pane SESSION [capture-pane options] — the pane's text. `-S -N` reaches N
# rows of scrollback; `-e` keeps the SGR escapes.
pane() {
	local sess="$1"
	shift
	tmux capture-pane -t "$sess" -p "$@"
}

# ---------------------------------------------------------------------------
# Polling. The suite's own rule: poll for the state, never sleep a fixed
# amount and hope — a fixed sleep is a race against the 45ms-per-chunk stream
# on a fast machine and a hang on a slow one. Every timeout below is a CAP:
# the wait returns the moment the condition holds.
# ---------------------------------------------------------------------------
smoke_ticks() { # SECONDS → number of poll iterations
	awk -v s="$1" -v i="$SMOKE_POLL_INTERVAL" 'BEGIN { n = int(s / i); if (n * i < s) n++; if (n < 1) n = 1; print n }'
}
# Split "capture opts -- grep opts" (the -- optional: no capture opts without it).
# Sets the arrays SMOKE_CAP and SMOKE_GREP.
smoke_split_args() {
	SMOKE_CAP=()
	SMOKE_GREP=()
	local seen=0 a
	for a in "$@"; do
		if [ "$a" = "--" ] && [ "$seen" -eq 0 ]; then
			seen=1
			continue
		fi
		if [ "$seen" -eq 1 ]; then SMOKE_GREP+=("$a"); else SMOKE_CAP+=("$a"); fi
	done
	if [ "$seen" -eq 0 ]; then
		SMOKE_GREP=(${SMOKE_CAP[@]+"${SMOKE_CAP[@]}"})
		SMOKE_CAP=()
	fi
}
# has CONTENT [grep options] PATTERN — does the captured text match?
has() {
	local content="$1"
	shift
	printf '%s\n' "$content" | grep -q "$@"
}
lacks() { ! has "$@"; }
# pane_has SESSION [capture opts --] [grep opts] PATTERN
pane_has() {
	local sess="$1"
	shift
	smoke_split_args "$@"
	tmux capture-pane -t "$sess" -p ${SMOKE_CAP[@]+"${SMOKE_CAP[@]}"} | grep -q "${SMOKE_GREP[@]}"
}
# poll SECONDS COMMAND [ARGS…] — run COMMAND until it succeeds (0) or the cap
# passes (1).
poll() {
	local secs="$1" n i
	shift
	n="$(smoke_ticks "$secs")"
	for ((i = 0; i < n; i++)); do
		if "$@"; then return 0; fi
		sleep "$SMOKE_POLL_INTERVAL"
	done
	return 1
}
# wait_for SECONDS SESSION [capture opts --] [grep opts] PATTERN — wait until
# the pane matches. Returns 1 on timeout.
wait_for() {
	local secs="$1" sess="$2"
	shift 2
	poll "$secs" pane_has "$sess" "$@"
}
# wait_pane SECONDS SESSION [capture opts --] [grep opts] PATTERN — the same
# wait, printing the last capture taken (matched or not) so the phase can
# assert on — and log — the frame that satisfied it.
wait_pane() {
	local secs="$1" sess="$2" last="" n i
	shift 2
	smoke_split_args "$@"
	n="$(smoke_ticks "$secs")"
	for ((i = 0; i < n; i++)); do
		last="$(tmux capture-pane -t "$sess" -p ${SMOKE_CAP[@]+"${SMOKE_CAP[@]}"})"
		if printf '%s\n' "$last" | grep -q "${SMOKE_GREP[@]}"; then
			printf '%s\n' "$last"
			return 0
		fi
		sleep "$SMOKE_POLL_INTERVAL"
	done
	printf '%s\n' "$last"
	return 1
}
# wait_settled SECONDS SESSION [capture opts --] [grep opts] PATTERN — wait
# until the pane matches AND has stopped changing (two identical samples 0.2s
# apart), printing it: the settled layout after a reply, not a mid-stream
# frame.
wait_settled() {
	local secs="$1" sess="$2" prev="" cur="" n i
	shift 2
	smoke_split_args "$@"
	n="$(awk -v s="$secs" 'BEGIN { n = int(s / 0.2); if (n * 0.2 < s) n++; print n }')"
	for ((i = 0; i < n; i++)); do
		cur="$(tmux capture-pane -t "$sess" -p ${SMOKE_CAP[@]+"${SMOKE_CAP[@]}"})"
		if printf '%s\n' "$cur" | grep -q "${SMOKE_GREP[@]}" && [ "$cur" = "$prev" ]; then
			printf '%s\n' "$cur"
			return 0
		fi
		prev="$cur"
		sleep 0.2
	done
	printf '%s\n' "$cur"
	return 1
}
# wait_file SECONDS FILE [grep opts] PATTERN — wait until a file exists and
# matches.
wait_file() {
	local secs="$1" file="$2" n i
	shift 2
	n="$(smoke_ticks "$secs")"
	for ((i = 0; i < n; i++)); do
		if [ -f "$file" ] && grep -q "$@" "$file"; then return 0; fi
		sleep "$SMOKE_POLL_INTERVAL"
	done
	return 1
}
# wait_gone SECONDS SESSION — wait for the session (its app) to exit.
wait_gone() {
	local secs="$1" sess="$2" n i
	n="$(smoke_ticks "$secs")"
	for ((i = 0; i < n; i++)); do
		if ! tmux has-session -t "$sess" 2>/dev/null; then return 0; fi
		sleep "$SMOKE_POLL_INTERVAL"
	done
	return 1
}

# ---------------------------------------------------------------------------
# Assertions. Each records a failure (and keeps going) rather than aborting:
# the message is the diagnosis, the pane dump above it the evidence.
# ---------------------------------------------------------------------------
# expect_has CONTENT [grep opts] PATTERN MESSAGE
expect_has() {
	local content="$1" msg="${*: -1}"
	shift
	set -- "${@:1:$#-1}"
	printf '%s\n' "$content" | grep -q "$@" || fail "$msg"
}
# expect_lacks CONTENT [grep opts] PATTERN MESSAGE
expect_lacks() {
	local content="$1" msg="${*: -1}"
	shift
	set -- "${@:1:$#-1}"
	printf '%s\n' "$content" | grep -q "$@" && fail "$msg"
	return 0
}
# expect_eq ACTUAL EXPECTED MESSAGE
expect_eq() { [ "$1" = "$2" ] || fail "$3 (got '$1', expected '$2')"; }
# expect_ne ACTUAL UNWANTED MESSAGE
expect_ne() { [ "$1" != "$2" ] || fail "$3 (got '$1')"; }
# expect_file_has FILE [grep opts] PATTERN MESSAGE
expect_file_has() {
	local file="$1" msg="${*: -1}"
	shift
	set -- "${@:1:$#-1}"
	grep -q "$@" "$file" 2>/dev/null || fail "$msg"
}

# Exactly one input box on a captured screen: one bare prompt row (the
# composer's `❯` — trailing blanks are trimmed by capture-pane; echoed messages
# are `❯ text`), two horizontal rules (the box's frame), one session footer.
# Phantom stale boxes add extras of each. (`(─)+` groups the multibyte rule
# char so the repeat applies to the whole UTF-8 sequence even under a
# byte-wise C locale.)
count_bare_prompts() { printf '%s\n' "$1" | grep -cE '^❯[[:space:]]*$'; }
count_rules() { printf '%s\n' "$1" | grep -cE '^(─)+$'; }
count_footers() { printf '%s\n' "$1" | grep -cF 'dummy_model_name ·'; }
count_msg_lines() { printf '%s\n' "$1" | grep -cF "❯ $2"; }
