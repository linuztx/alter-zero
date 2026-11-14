#!/usr/bin/env bash
# Measure what an idle, just-started interface holds in resident memory
# (docs/memory.md, "Startup").
#
# Launches the REAL binary in a private tmux server, over a throwaway config
# home and the offline backend, waits for the banner and for the startup
# workers to go quiet, then reads /proc/PID/status and /proc/PID/smaps and
# splits the resident set by kind:
#
#   anon    what the process wrote: the heap, the stacks, the pages of the
#           binary the loader relocated (`data`)
#   file    mapped files: the binary's code (`text`) and read-only data
#           (`rodata`), and the shared libraries (`libs`)
#
#   cargo build --release
#   scripts/startup_mem.sh [bin] [launches]
#
#   bin       target/release/alter-zero
#   launches  10
#
# The file-backed half moves by megabytes for the SAME bytes, with how the
# binary's file sits in the page cache: the kernel maps the cached pages
# around each page the code runs through (64 KiB of them by default), and
# only pages already cached. A binary its own launches faulted in back after
# an eviction measured 11.4 MB where the same bytes freshly written measured
# 13.0 and read in whole 14.8. So every launch runs a copy written just
# before it, beside the binary — the state an install or `alter-zero update`
# leaves, whatever the machine has since done with its cache — and two
# binaries measured this way compare. STARTUP_MEM_CACHE=keep measures the
# binary where it is, as it is.
#
# The launch is hermetic (`env -i`): no provider key reaches it, so the
# startup probe never opens a connection, and nothing of the developer's
# home is read. Prints one row per launch, then the mean, median and spread
# of each column. Linux only (/proc).
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="${1:-$ROOT/target/release/alter-zero}"
RUNS="${2:-10}"
CACHE="${STARTUP_MEM_CACHE:-written}"
SETTLE="${STARTUP_MEM_SETTLE:-2.5}"

if [ ! -x "$SRC" ]; then
	echo "startup_mem: $SRC not found — cargo build --release" >&2
	exit 2
fi
if ! command -v tmux >/dev/null; then
	echo "startup_mem: tmux is required" >&2
	exit 2
fi
if [ ! -r /proc/self/smaps ]; then
	echo "startup_mem: /proc/PID/smaps is required (Linux)" >&2
	exit 2
fi
SRC="$(readlink -f "$SRC")"

case "$CACHE" in
written | keep) ;;
*)
	echo "startup_mem: STARTUP_MEM_CACHE is written or keep, not $CACHE" >&2
	exit 2
	;;
esac

TMPROOT="$(mktemp -d "${TMPDIR:-/tmp}/startup-mem.XXXXXX")"
SOCK="$TMPROOT/tmux.sock"
# One home, config home and working directory for every launch, so the
# warm-up below is the only first launch.
HOME_DIR="$TMPROOT/home"
WORK="$TMPROOT/work"
mkdir -p "$HOME_DIR" "$WORK"
# The copies sit beside the binary, not in $TMPDIR: on a tmpfs their pages
# would count as shared memory, not as a mapped file.
COPIES="$(dirname "$SRC")/.startup-mem-$$"
cleanup() {
	tmux -S "$SOCK" kill-server 2>/dev/null
	rm -f "$COPIES".*
	rm -rf "$TMPROOT"
}
trap cleanup EXIT

row() { # pid bin → one row of MB columns
	local pid="$1" bin="$2"
	awk -v bin="$bin" '
	function flush() {
		if (name == "") return
		if (name == bin) {
			if (perms ~ /x/) kb["text"] += rss - anon
			else kb["rodata"] += rss - anon
			kb["data"] += anon
		} else if (name == "[heap]") kb["heap"] += rss
		else if (name == "[stack]") kb["stack"] += rss
		else if (name ~ /\.so/) kb["libs"] += rss - anon
	}
	FILENAME ~ /smaps$/ && /^[0-9a-f]+-[0-9a-f]+ / {
		flush()
		perms = $2
		name = (NF >= 6) ? $6 : ""
		rss = 0; anon = 0
		next
	}
	FILENAME ~ /smaps$/ && /^Rss:/ { rss = $2 }
	FILENAME ~ /smaps$/ && /^Anonymous:/ { anon = $2 }
	FILENAME ~ /status$/ && /^VmRSS/ { total = $2 }
	FILENAME ~ /status$/ && /^RssAnon/ { a = $2 }
	FILENAME ~ /status$/ && /^RssFile/ { f = $2 }
	FILENAME ~ /status$/ && /^Threads/ { t = $2 }
	END {
		flush()
		printf "RSS %6.2f  anon %5.2f  file %5.2f  |  text %5.2f  rodata %5.2f  data %5.2f  heap %5.2f  stack %5.2f  libs %5.2f  |  threads %d\n",
			total/1024, a/1024, f/1024, kb["text"]/1024, kb["rodata"]/1024,
			kb["data"]/1024, kb["heap"]/1024, kb["stack"]/1024, kb["libs"]/1024, t
	}' "/proc/$pid/smaps" "/proc/$pid/status"
}

launch() { # name → the row, once the app is up and quiet
	local s="$1" bin="$SRC" pid tries
	if [ "$CACHE" = written ]; then
		# A new file per launch: rewriting one a process just ran from can
		# meet ETXTBSY, and a new file starts with nothing else cached.
		bin="$COPIES.$1"
		cat "$SRC" >"$bin" && chmod +x "$bin" || return 1
	fi
	tmux -S "$SOCK" new-session -d -s "$s" -x 140 -y 45 -c "$WORK" \
		"env -i PATH=\"$PATH\" HOME=$HOME_DIR TERM=xterm-256color LANG=C.UTF-8 \
		ALTER_ZERO_CONFIG_DIR=$HOME_DIR/cfg ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_UPDATE_CHECK=0 \
		ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_HISTORY_FILE=/dev/null $bin"
	tries=0
	until tmux -S "$SOCK" capture-pane -t "$s" -p 2>/dev/null | grep -q "Alter Zero"; do
		tries=$((tries + 1))
		if [ "$tries" -gt 100 ]; then
			echo "startup_mem: no banner from $SRC" >&2
			tmux -S "$SOCK" capture-pane -t "$s" -p >&2
			return 1
		fi
		sleep 0.1
	done
	sleep "$SETTLE"
	# `env` execs the binary, so the app is the pane's process — or, under
	# a shell that forks its last command, that process's child.
	pid="$(tmux -S "$SOCK" list-panes -t "$s" -F '#{pane_pid}')"
	if [ "$(readlink -f "/proc/$pid/exe")" != "$bin" ]; then
		for child in $(pgrep -P "$pid"); do
			[ "$(readlink -f "/proc/$child/exe")" = "$bin" ] && pid="$child"
		done
	fi
	if [ "$(readlink -f "/proc/$pid/exe")" != "$bin" ]; then
		echo "startup_mem: no $bin process in the pane" >&2
		return 1
	fi
	row "$pid" "$bin"
	tmux -S "$SOCK" send-keys -t "$s" C-c
	sleep 0.3
	tmux -S "$SOCK" kill-session -t "$s" 2>/dev/null
	[ "$bin" != "$SRC" ] && rm -f "$bin"
	return 0
}

echo "$SRC ($(stat -c %s "$SRC") bytes), cache: $CACHE, $RUNS launches"
# The first launch of a config home seeds its skills and agent types, which
# no later launch does: it warms that home up and is not counted.
launch warm >/dev/null || exit 1
rows="$TMPROOT/rows"
for i in $(seq 1 "$RUNS"); do
	launch "run$i" >>"$rows" || exit 1
	echo "  $(tail -1 "$rows")"
done
awk '
{
	for (i = 1; i < NF; i++)
		if ($(i + 1) ~ /^[0-9.]+$/ && $i ~ /^[a-zA-Z]+$/) {
			if (!($i in seen)) { seen[$i] = 1; order[++n] = $i }
			v[$i, ++c[$i]] = $(i + 1)
		}
}
END {
	for (k = 1; k <= n; k++) {
		name = order[k]
		if (name == "threads") continue
		m = c[name]; sum = 0
		for (j = 1; j <= m; j++) { x[j] = v[name, j]; sum += x[j] }
		# insertion sort for the median
		for (j = 2; j <= m; j++) { y = x[j]; i = j - 1; while (i > 0 && x[i] > y) { x[i + 1] = x[i]; i-- } x[i + 1] = y }
		mean = sum / m
		med = (m % 2) ? x[(m + 1) / 2] : (x[m / 2] + x[m / 2 + 1]) / 2
		ss = 0
		for (j = 1; j <= m; j++) ss += (x[j] - mean) ^ 2
		sd = (m > 1) ? sqrt(ss / (m - 1)) : 0
		printf "%-7s mean %6.2f  median %6.2f  stdev %5.2f  min %6.2f  max %6.2f  (MB)\n", name, mean, med, sd, x[1], x[m]
	}
}' "$rows"
