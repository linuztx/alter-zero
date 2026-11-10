#!/usr/bin/env bash
# scripts/preflight.sh — is this machine able to run the gate and the smoke
# suite? One line per tool, the macOS-specific traps named, and a non-zero exit
# when something REQUIRED is missing.
#
#   scripts/preflight.sh              # what the default loop needs
#   scripts/preflight.sh --smoke      # add the smoke suite's requirements
#   scripts/preflight.sh --all        # add the optional halves (node, python3…)
#
# Why it exists: on macOS the two things that make a run look broken are not
# the code. `setsid` is absent (46 lib tests fail with "no way to give the
# command a terminal of its own here"), and `/bin/bash` is 3.2 while
# `scripts/smoke.sh` needs 4+ for `declare -A` (it dies with
# `declare: -A: invalid option`, which reads like a phase failure). Neither
# says what is wrong; this does. AGENTS.md, *Linux vs macOS*, has the full
# table this checks against.
#
# POSIX-ish on purpose where it can be: bash 3.2 exists on a stock macOS, and
# the point is to run BEFORE one has installed a newer bash, so nothing here
# needs bash 4 (no associative arrays, no `${var,,}`).
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WANT_SMOKE=0
WANT_ALL=0
for arg in "$@"; do
	case "$arg" in
	--smoke) WANT_SMOKE=1 ;;
	--all) WANT_SMOKE=1 WANT_ALL=1 ;;
	-h | --help)
		sed -n '2,17p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
		exit 0
		;;
	*)
		echo "preflight: unknown argument '$arg' (try --help)" >&2
		exit 2
		;;
	esac
done

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
	c_ok=$'\033[32m' c_bad=$'\033[31m' c_warn=$'\033[33m' c_dim=$'\033[2m' c_off=$'\033[0m'
else
	c_ok="" c_bad="" c_warn="" c_dim="" c_off=""
fi

FAILS=0
WARNS=0
ok() { printf '  %sok%s    %-12s %s\n' "$c_ok" "$c_off" "$1" "$2"; }
warn() {
	WARNS=$((WARNS + 1))
	printf '  %swarn%s  %-12s %s\n' "$c_warn" "$c_off" "$1" "$2"
}
miss() {
	FAILS=$((FAILS + 1))
	printf '  %smiss%s  %-12s %s\n' "$c_bad" "$c_off" "$1" "$2"
}
note() { printf '%s%s%s\n' "$c_dim" "$*" "$c_off"; }

OS="$(uname -s)"
case "$OS" in
Darwin) MAC=1 ;;
*) MAC=0 ;;
esac
note "preflight · $OS $(uname -m) · $(uname -r) · bash $BASH_VERSION"

# ---------------------------------------------------------------------------
# The toolchain (required everywhere): the pinned rustc, rustfmt and clippy.
# ---------------------------------------------------------------------------
note "toolchain"
PIN="$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$ROOT/rust-toolchain.toml" 2>/dev/null | head -1)"
if command -v cargo >/dev/null 2>&1 && command -v rustc >/dev/null 2>&1; then
	have="$(rustc --version | awk '{ print $2 }')"
	if [ -n "$PIN" ] && [ "$have" != "$PIN" ]; then
		# rustup materializes the pinned toolchain from rust-toolchain.toml, so
		# a mismatch inside the checkout usually means the shim/channel needs a
		# sync rather than that the pin is wrong.
		warn rustc "cargo/rustc on PATH is $have, the pin is $PIN (run rustup toolchain install $PIN)"
	else
		ok rustc "cargo $(cargo --version | awk '{ print $2 }') · rustc $have"
	fi
else
	miss rustc "cargo/rustc not on PATH — install rustup (https://rustup.rs)"
fi
# `rustfmt-aarch64-apple-darwin` — the name carries the target triple, so
# match on the prefix rather than the whole line.
if rustup component list --installed 2>/dev/null | grep -q '^rustfmt'; then
	ok rustfmt "$(rustfmt --version 2>/dev/null | head -1)"
else
	warn rustfmt "not installed for the active toolchain (rustup component add rustfmt — the gate needs it)"
fi
if rustup component list --installed 2>/dev/null | grep -q '^clippy'; then
	ok clippy "$(cargo clippy --version 2>/dev/null | awk '{ print $2 }')"
else
	warn clippy "not installed for the active toolchain (rustup component add clippy — the gate needs it)"
fi
if command -v git >/dev/null 2>&1; then
	ok git "$(git --version)"
else
	miss git "not on PATH"
fi
# A C compiler: some dependencies build C (`ring`, the image codecs).
if command -v cc >/dev/null 2>&1 || command -v gcc >/dev/null 2>&1 || command -v clang >/dev/null 2>&1; then
	ok cc "$(cc --version 2>/dev/null | head -1)"
else
	miss cc "no C compiler — a dependency builds C (xcode-select --install on macOS, build-essential on Linux)"
fi

# A build cache is optional. `mbx` is the one this checkout is developed on:
# `mbx <cargo command>` runs any cargo command through it and `mbx clean`
# clears the workspace's managed target, so a cargo that bypasses it only
# costs a rebuild, never correctness.
if command -v mbx >/dev/null 2>&1; then
	ok mbx "$(mbx --version 2>/dev/null | head -1) · ${MBX_HINT:-wrapped cargo: see 'mbx doctor'}"
else
	warn mbx "not installed — plain cargo works, a shared build cache just saves rebuilds"
fi

# ---------------------------------------------------------------------------
# The smoke suite (`--smoke`): tmux, bash 4+, and the macOS `setsid`.
# ---------------------------------------------------------------------------
if [ "$WANT_SMOKE" -eq 1 ]; then
	note "smoke suite"
	if command -v tmux >/dev/null 2>&1; then
		ok tmux "$(tmux -V)"
	else
		miss tmux "required to drive the TUI (brew install tmux / apt install tmux)"
	fi
	# scripts/smoke.sh declares an associative array at its worker table; bash
	# 3.2 (every macOS /bin/bash) rejects it and the run dies with
	# `declare: -A: invalid option` — which reads like a phase failure.
	bash_major="${BASH_VERSINFO[0]}"
	if [ "$bash_major" -ge 4 ]; then
		ok bash "$BASH_VERSION"
	else
		miss bash "this shell is $BASH_VERSION; scripts/smoke.sh needs 4+ (brew install bash, then run it with that one)"
	fi
	if command -v setsid >/dev/null 2>&1; then
		ok setsid "$(setsid --version 2>&1 | head -1)"
	else
		# Not fatal: the six baseline failures are the ones the suite documents,
		# and the 46 tty-spawning ones only need a terminal of their own.
		if [ "$MAC" -eq 1 ]; then
			miss setsid "absent — util-linux has it: brew install util-linux, then PATH=\"/opt/homebrew/opt/util-linux/bin:\$PATH\" (46 lib tests fail without it)"
		else
			warn setsid "not on PATH (util-linux)"
		fi
	fi
	# The suite's own readers were BSD-`sed`-broken once (docs/smoke.md), so it
	# is worth knowing which pair this run has; `selftest.sh` pins them either
	# way, so this line is information, not a check.
	if command -v sed >/dev/null 2>&1 && command -v grep >/dev/null 2>&1; then
		sed_kind="$(sed --version >/dev/null 2>&1 && echo GNU || echo BSD)"
		ok sedgrep "$sed_kind sed · BSD/BSD grep are what macOS ships; run 'bash scripts/smoke/selftest.sh' after touching the readers"
	fi
fi

# ---------------------------------------------------------------------------
# The optional halves (`--all`): what a few specific areas need.
# ---------------------------------------------------------------------------
if [ "$WANT_ALL" -eq 1 ]; then
	note "optional"
	for tool in python3 node shellcheck sha256sum shasum; do
		if command -v "$tool" >/dev/null 2>&1; then
			ok "$tool" "$(command -v "$tool")"
		else
			warn "$tool" "absent — only some areas need it (see CONTRIBUTING.md, *Getting set up*)"
		fi
	done
	if command -v nproc >/dev/null 2>&1; then
		ok nproc "$(nproc) CPUs"
	elif command -v sysctl >/dev/null 2>&1; then
		ok sysctl "$(sysctl -n hw.ncpu) CPUs (scripts/smoke.sh falls back to this)"
	fi
fi

if [ "$FAILS" -gt 0 ]; then
	printf '\n%s%d missing, %d to look at.%s\n' "$c_bad" "$FAILS" "$WARNS" "$c_off" >&2
	exit 1
fi
printf '\n%severything this needs is here%s' "$c_ok" "$c_off"
[ "$WARNS" -gt 0 ] && printf ' (%d warning%s — not fatal)' "$WARNS" "$([ "$WARNS" -eq 1 ] || echo s)"
printf '.\n'
exit 0
