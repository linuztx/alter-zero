#!/usr/bin/env bash
# scripts/smoke.sh — drive the TUI inside a real terminal (tmux) and ASSERT on
# what it painted. This is the only automated coverage of the terminal I/O
# boundary (src/main.rs, src/tui/, term.rs), so it asserts rather than
# eyeballs: every phase polls for the state it expects and exits non-zero on a
# mismatch, so the suite can gate in CI or a pre-commit hook.
#
# The suite is a RUNNER over PHASES. Each phase is one file under
# scripts/smoke/phases/ — `NNN-slug.sh`, a self-contained scenario that sources
# scripts/smoke/lib.sh, gets its own tmux server, config home and temp tree,
# and can be run on its own (`bash scripts/smoke/phases/055-permissions.sh`).
# This runner discovers them, runs them on a pool of parallel workers, keeps a
# log per phase, and prints one line per result plus a summary. Phases tagged
# `serial` (a timing measurement a loaded machine would skew) run after the
# pool drains, one at a time. See docs/smoke.md.
#
#   scripts/smoke.sh                      # everything, in parallel
#   scripts/smoke.sh -j 2                 # two workers
#   scripts/smoke.sh --serial             # one at a time, in phase order
#   scripts/smoke.sh 55 66 107b           # only these phases
#   scripts/smoke.sh 50-60 permission     # a range, a name substring
#   scripts/smoke.sh --skip 115           # everything but
#   scripts/smoke.sh --failed             # re-run what failed last time
#   scripts/smoke.sh --sweep-leaked       # list (then --yes: delete) rollouts old runs leaked into ~/.alter-zero
#   scripts/smoke.sh --list               # the phases, their tags, last timings
#   scripts/smoke.sh -v                   # print every phase's log, not just failures
#   scripts/smoke.sh target/release/alter-zero   # another binary (default: target/debug)
set -uo pipefail

SMOKE_HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=smoke/lib.sh
. "$SMOKE_HERE/smoke/lib.sh"

usage() {
	cat <<'EOF'
usage: scripts/smoke.sh [BIN] [options] [SELECTOR…]

  BIN                 the binary to drive (default: target/debug/alter-zero)
  SELECTOR            a phase id (55, 107b), a range (50-60) or a name
                      substring (permission); several may be given
  -j, --jobs N        parallel workers (default: 2× the CPU count, at most 8; SMOKE_JOBS)
      --serial        one phase at a time, in phase order (= -j 1)
  -o, --only SPEC     select phases (same grammar as SELECTOR; comma-separated)
  -s, --skip SPEC     deselect phases
      --failed        run only the phases that failed in the previous run
      --sweep-leaked  list the rollouts older runs of this suite left in your own
                      sessions dir (they show up in /resume); --yes deletes them
  -l, --list          list the phases and exit
  -v, --verbose       print every phase's whole log as it finishes
  -k, --keep          keep each phase's temp tree (SMOKE_KEEP=1)
      --timeout SECS  kill a phase that runs longer than this (default 600)
  -h, --help          this text

Logs land in target/smoke/logs/NNN-slug.log (SMOKE_OUT overrides the dir);
target/smoke/times remembers each phase's duration to schedule the longest
first next time, and target/smoke/failed feeds --failed.
EOF
}

JOBS="${SMOKE_JOBS:-}"
ONLY=()
SKIP=()
LIST=0
VERBOSE=0
KEEP="${SMOKE_KEEP:-}"
TIMEOUT="${SMOKE_TIMEOUT:-600}"
FAILED_ONLY=0
SWEEP=0
YES=0
BIN_ARG=""
while [ $# -gt 0 ]; do
	case "$1" in
	-j | --jobs)
		JOBS="$2"
		shift 2
		;;
	-j*)
		JOBS="${1#-j}"
		shift
		;;
	--serial)
		JOBS=1
		shift
		;;
	-o | --only)
		ONLY+=("$2")
		shift 2
		;;
	-s | --skip)
		SKIP+=("$2")
		shift 2
		;;
	--failed)
		FAILED_ONLY=1
		shift
		;;
	--sweep-leaked)
		SWEEP=1
		shift
		;;
	--yes)
		YES=1
		shift
		;;
	-l | --list)
		LIST=1
		shift
		;;
	-v | --verbose)
		VERBOSE=1
		shift
		;;
	-k | --keep)
		KEEP=1
		shift
		;;
	--timeout)
		TIMEOUT="$2"
		shift 2
		;;
	-h | --help)
		usage
		exit 0
		;;
	--)
		shift
		while [ $# -gt 0 ]; do
			ONLY+=("$1")
			shift
		done
		;;
	-*)
		echo "smoke: unknown option '$1'" >&2
		usage >&2
		exit 2
		;;
	*)
		# The first positional that is an executable file is the binary (the
		# original contract); anything else is a phase selector.
		if [ -z "$BIN_ARG" ] && [ -f "$1" ] && [ -x "$1" ]; then
			BIN_ARG="$1"
		else
			ONLY+=("$1")
		fi
		shift
		;;
	esac
done

# --- --sweep-leaked: the rollouts the old suite left in the real sessions dir ---
# Before the suite pointed ALTER_ZERO_SESSIONS_DIR into its temp tree (and
# before the app kept sessions under the config home), every phase's turns were
# recorded into the developer's own `~/.alter-zero/sessions` — a few hundred
# dummy-backend conversations per run, listed by /resume in this checkout. They
# are recognisable by the offline backends' model names in their `session_meta`
# line (`stream::dummy` and `stream::stall`), which no real provider ever uses.
# The listing is the default; deleting takes --yes.
if [ "$SWEEP" -eq 1 ]; then
	sweep_root="${ALTER_ZERO_SESSIONS_DIR:-${ALTER_ZERO_CONFIG_DIR:-$HOME/.alter-zero}/sessions}"
	if [ ! -d "$sweep_root" ]; then
		echo "smoke: no sessions dir at $sweep_root — nothing to sweep"
		exit 0
	fi
	leaked=()
	while IFS= read -r f; do
		if head -c 800 "$f" | grep -qE '"model":"(dummy_model_name|stall_model)"'; then
			leaked+=("$f")
		fi
	done < <(find "$sweep_root" -type f -name 'rollout-*.jsonl' | sort)
	if [ "${#leaked[@]}" -eq 0 ]; then
		echo "smoke: no leaked smoke rollouts under $sweep_root"
		exit 0
	fi
	printf '%s\n' "${leaked[@]}"
	if [ "$YES" -eq 1 ]; then
		rm -f -- "${leaked[@]}"
		find "$sweep_root" -mindepth 1 -type d -empty -delete 2>/dev/null
		echo "smoke: deleted ${#leaked[@]} leaked smoke rollout(s) from $sweep_root"
	else
		echo "smoke: ${#leaked[@]} leaked smoke rollout(s) under $sweep_root — re-run with --yes to delete them"
	fi
	exit 0
fi

SMOKE_OUT="${SMOKE_OUT:-$SMOKE_ROOT/target/smoke}"
SMOKE_LOGS="$SMOKE_OUT/logs"
SMOKE_TIMES="$SMOKE_OUT/times"
SMOKE_FAILED="$SMOKE_OUT/failed"

if [ -z "$JOBS" ]; then
	# The suite is wait-bound (an app streaming at 45ms a chunk, a shell
	# sampling it every 100ms), so twice the CPU count packs well; past eight
	# the longest phases bound the wall time and more workers only add load.
	JOBS="$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 2)"
	JOBS=$((JOBS * 2))
	[ "$JOBS" -gt 8 ] && JOBS=8
fi
[ "$JOBS" -ge 1 ] 2>/dev/null || JOBS=1

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
	C_OK=$'\033[32m' C_FAIL=$'\033[31m' C_DIM=$'\033[2m' C_BOLD=$'\033[1m' C_OFF=$'\033[0m'
else
	C_OK="" C_FAIL="" C_DIM="" C_BOLD="" C_OFF=""
fi

# --- phase discovery + selection --------------------------------------------
ALL_PHASES=()
while IFS= read -r f; do ALL_PHASES+=("$f"); done < <(find "$SMOKE_PHASES_DIR" -maxdepth 1 -name '[0-9]*.sh' | sort)
if [ "${#ALL_PHASES[@]}" -eq 0 ]; then
	echo "smoke: no phases found under $SMOKE_PHASES_DIR" >&2
	exit 2
fi

# smoke_matches FILE SPEC — does one selector item name this phase? Items:
# an id (55, 107b), a numeric range (50-60, matched on the numeric part), or a
# case-insensitive substring of the slug or title.
smoke_matches() {
	local file="$1" spec="$2" id num slug title lo hi
	id="$(smoke_phase_id "$file")"
	num="${id%%[a-z]*}"
	slug="$(smoke_phase_slug "$file")"
	case "$spec" in
	"$id") return 0 ;;
	*[!0-9a-z-]*) ;; # a name substring, matched below
	[0-9]*-[0-9]*)
		lo="${spec%-*}"
		hi="${spec#*-}"
		[ "$num" -ge "$lo" ] && [ "$num" -le "$hi" ] && return 0
		return 1
		;;
	[0-9]*)
		[ "$spec" = "$num" ] && return 0
		return 1
		;;
	esac
	title="$(smoke_phase_title "$file")"
	printf '%s %s' "$slug" "$title" | grep -qiF -- "$spec"
}
smoke_selected() { # FILE → 0 if the selectors keep it
	local file="$1" spec item keep=1
	if [ "${#ONLY[@]}" -gt 0 ]; then
		keep=0
		for spec in "${ONLY[@]}"; do
			for item in ${spec//,/ }; do
				smoke_matches "$file" "$item" && keep=1
			done
		done
	fi
	[ "$keep" -eq 1 ] || return 1
	for spec in ${SKIP[@]+"${SKIP[@]}"}; do
		for item in ${spec//,/ }; do
			smoke_matches "$file" "$item" && return 1
		done
	done
	return 0
}

if [ "$FAILED_ONLY" -eq 1 ]; then
	if [ ! -s "$SMOKE_FAILED" ]; then
		echo "smoke: nothing failed in the previous run" >&2
		exit 0
	fi
	while IFS= read -r id; do [ -n "$id" ] && ONLY+=("$id"); done <"$SMOKE_FAILED"
fi

PHASES=()
for f in "${ALL_PHASES[@]}"; do
	smoke_selected "$f" && PHASES+=("$f")
done
if [ "${#PHASES[@]}" -eq 0 ]; then
	echo "smoke: no phase matches the selection" >&2
	exit 2
fi

# smoke_last_time ID → the phase's duration in the previous run, or "" —
# used to schedule the longest phases first (the pool packs better) and to
# annotate --list.
smoke_last_time() {
	[ -f "$SMOKE_TIMES" ] || return 0
	awk -v id="$1" '$1 == id { t = $2 } END { if (t != "") print t }' "$SMOKE_TIMES"
}

if [ "$LIST" -eq 1 ]; then
	printf '%s%-6s %-30s %-8s %6s%s  %s\n' "$C_BOLD" "id" "slug" "tags" "last" "$C_OFF" "title"
	for f in "${PHASES[@]}"; do
		id="$(smoke_phase_id "$f")"
		t="$(smoke_last_time "$id")"
		printf '%-6s %-30s %-8s %6s  %s\n' "$id" "$(smoke_phase_slug "$f")" "$(smoke_phase_tags "$f")" "${t:+${t}s}" "$(smoke_phase_title "$f")"
	done
	exit 0
fi

# --- the binary ---------------------------------------------------------------
BIN="${BIN_ARG:-${SMOKE_BIN:-$SMOKE_ROOT/target/debug/alter-zero}}"
if [ ! -x "$BIN" ]; then
	echo "FAIL: binary not found at $BIN (run: cargo build)" >&2
	exit 1
fi
export SMOKE_BIN
SMOKE_BIN="$(readlink -f "$BIN" 2>/dev/null || realpath "$BIN")"
[ -n "$KEEP" ] && export SMOKE_KEEP=1
if ! command -v tmux >/dev/null 2>&1; then
	echo "FAIL: tmux is required to drive the TUI" >&2
	exit 1
fi

mkdir -p "$SMOKE_LOGS"

# --- scheduling ---------------------------------------------------------------
# Two queues: the parallel pool, longest-known-first, then the serial phases
# in phase order.
PARALLEL=()
SERIAL=()
for f in "${PHASES[@]}"; do
	case " $(smoke_phase_tags "$f") " in
	*" serial "*) SERIAL+=("$f") ;;
	*) PARALLEL+=("$f") ;;
	esac
done
if [ "${#PARALLEL[@]}" -gt 1 ] && [ -f "$SMOKE_TIMES" ]; then
	ordered=()
	while IFS= read -r f; do ordered+=("$f"); done < <(
		for f in "${PARALLEL[@]}"; do
			t="$(smoke_last_time "$(smoke_phase_id "$f")")"
			printf '%s\t%s\n' "${t:-99999}" "$f"
		done | sort -t "$(printf '\t')" -k1,1nr -k2,2 | cut -f2-
	)
	PARALLEL=("${ordered[@]}")
fi

TOTAL=$(("${#PARALLEL[@]}" + "${#SERIAL[@]}"))
DONE=0
PASSED=0
FAILED=()
SUITE_T0="$(date +%s)"
NEW_TIMES="$(mktemp)"
: >"$NEW_TIMES"
# (Initialised, not just declared: under `set -u` an empty declared array is
# "unbound" to `${#a[@]}` on the bash versions that predate 5.1.)
declare -A PID_FILE=() PID_T0=() PID_LOG=()
HAVE_SETSID=0
command -v setsid >/dev/null 2>&1 && HAVE_SETSID=1

smoke_start() { # FILE — launch one phase on its own process group
	local file="$1" id log pid
	id="$(smoke_phase_id "$file")"
	log="$SMOKE_LOGS/$(basename "$file" .sh).log"
	if [ "$HAVE_SETSID" -eq 1 ]; then
		setsid bash "$file" >"$log" 2>&1 &
	else
		bash "$file" >"$log" 2>&1 &
	fi
	pid=$!
	PID_FILE[$pid]="$file"
	PID_T0[$pid]="$(date +%s)"
	PID_LOG[$pid]="$log"
}

smoke_report() { # PID RC — one result line (+ the FAIL lines / the log)
	local pid="$1" rc="$2" file id slug secs tag colour
	file="${PID_FILE[$pid]}"
	id="$(smoke_phase_id "$file")"
	slug="$(smoke_phase_slug "$file")"
	secs=$(($(date +%s) - PID_T0[$pid]))
	DONE=$((DONE + 1))
	# A bash abort inside the phase (an unbound variable, a syntax error) is a
	# failure whatever the exit status says: the checks after it never ran.
	if [ "$rc" -eq 0 ] && grep -qE '^[^ ]*: line [0-9]+: ' "${PID_LOG[$pid]}"; then
		rc=3
	fi
	printf '%s\t%s\n' "$id" "$secs" >>"$NEW_TIMES"
	if [ "$rc" -eq 0 ]; then
		PASSED=$((PASSED + 1))
		tag="ok  "
		colour="$C_OK"
	else
		FAILED+=("$id")
		tag="FAIL"
		colour="$C_FAIL"
	fi
	printf '%s%s%s [%3d/%3d] %-5s %-30s %s%4ds%s\n' "$colour" "$tag" "$C_OFF" "$DONE" "$TOTAL" "$id" "$slug" "$C_DIM" "$secs" "$C_OFF"
	if [ "$VERBOSE" -eq 1 ]; then
		sed 's/^/    │ /' "${PID_LOG[$pid]}"
	elif [ "$rc" -ne 0 ]; then
		grep -E '^(FAIL|ABORT):|^[^ ]*: line [0-9]+: ' "${PID_LOG[$pid]}" | sed "s/^/    ${C_FAIL}│${C_OFF} /"
		printf '    %s→ %s%s\n' "$C_DIM" "${PID_LOG[$pid]}" "$C_OFF"
	fi
	unset "PID_FILE[$pid]" "PID_T0[$pid]" "PID_LOG[$pid]"
}

smoke_reap() { # collect every finished worker; enforce the timeout
	local pid now
	now="$(date +%s)"
	for pid in "${!PID_FILE[@]}"; do
		if ! kill -0 "$pid" 2>/dev/null; then
			wait "$pid"
			smoke_report "$pid" "$?"
		elif [ $((now - PID_T0[$pid])) -gt "$TIMEOUT" ]; then
			echo "smoke: Phase $(smoke_phase_id "${PID_FILE[$pid]}") exceeded ${TIMEOUT}s — killing it" >&2
			if [ "$HAVE_SETSID" -eq 1 ]; then kill -TERM -- "-$pid" 2>/dev/null; else kill -TERM "$pid" 2>/dev/null; fi
			sleep 3
			if [ "$HAVE_SETSID" -eq 1 ]; then kill -KILL -- "-$pid" 2>/dev/null; else kill -KILL "$pid" 2>/dev/null; fi
			wait "$pid"
			smoke_report "$pid" 124
		fi
	done
}

smoke_abort() { # Ctrl+C: stop the workers (each tears down its own server)
	local pid
	trap - INT TERM
	echo >&2
	echo "smoke: interrupted — stopping ${#PID_FILE[@]} running phase(s)" >&2
	for pid in "${!PID_FILE[@]}"; do
		if [ "$HAVE_SETSID" -eq 1 ]; then kill -TERM -- "-$pid" 2>/dev/null; else kill -TERM "$pid" 2>/dev/null; fi
	done
	wait 2>/dev/null
	rm -f "$NEW_TIMES"
	exit 130
}
trap smoke_abort INT TERM

smoke_run_queue() { # JOBS FILE… — run the files on at most JOBS workers
	local jobs="$1" f
	shift
	for f in "$@"; do
		while [ "${#PID_FILE[@]}" -ge "$jobs" ]; do
			sleep 0.2
			smoke_reap
		done
		smoke_start "$f"
	done
	while [ "${#PID_FILE[@]}" -gt 0 ]; do
		sleep 0.2
		smoke_reap
	done
}

printf '%ssmoke:%s %d phase(s), %d worker(s), binary %s\n' "$C_BOLD" "$C_OFF" "$TOTAL" "$JOBS" "$SMOKE_BIN"
[ "${#PARALLEL[@]}" -gt 0 ] && smoke_run_queue "$JOBS" "${PARALLEL[@]}"
[ "${#SERIAL[@]}" -gt 0 ] && smoke_run_queue 1 "${SERIAL[@]}"

# --- bookkeeping + summary ------------------------------------------------------
# Merge this run's timings over the remembered ones (a phase not run keeps
# its old entry) and record the failures for --failed.
{
	[ -f "$SMOKE_TIMES" ] && cat "$SMOKE_TIMES"
	cat "$NEW_TIMES"
} | awk -F '\t' '{ t[$1] = $2 } END { for (k in t) printf "%s\t%s\n", k, t[k] }' | sort >"$SMOKE_TIMES.tmp" && mv "$SMOKE_TIMES.tmp" "$SMOKE_TIMES"
if [ "${#FAILED[@]}" -gt 0 ]; then
	printf '%s\n' "${FAILED[@]}" >"$SMOKE_FAILED"
else
	: >"$SMOKE_FAILED"
fi
rm -f "$NEW_TIMES"

elapsed=$(($(date +%s) - SUITE_T0))
printf '\n%s==== smoke: %d passed, %d failed of %d in %dm%02ds (-j %d) ====%s\n' \
	"$C_BOLD" "$PASSED" "${#FAILED[@]}" "$TOTAL" $((elapsed / 60)) $((elapsed % 60)) "$JOBS" "$C_OFF"
if [ "$TOTAL" -gt 3 ]; then
	printf '%sslowest:%s ' "$C_DIM" "$C_OFF"
	sort -t "$(printf '\t')" -k2,2nr "$SMOKE_TIMES" | head -5 | awk -F '\t' '{ printf "%s (%ss) ", $1, $2 } END { print "" }'
fi
if [ "${#FAILED[@]}" -gt 0 ]; then
	printf '%sfailed:%s %s\n' "$C_FAIL" "$C_OFF" "${FAILED[*]}"
	printf '%slogs:%s   %s/\n' "$C_DIM" "$C_OFF" "$SMOKE_LOGS"
	printf '%sre-run:%s scripts/smoke.sh --failed\n' "$C_DIM" "$C_OFF"
	exit 1
fi
if [ "$PASSED" -ne "$TOTAL" ]; then
	echo "FAIL: only $PASSED of $TOTAL phases reported a pass" >&2
	exit 1
fi
echo "PASS: every phase drove the real binary in tmux and its assertions held"
exit 0
