# docker/lib.sh — what docker/build.sh and docker/run.sh share (docs/docker.md).
# Sourced, never run. POSIX sh, like install.sh: no arrays, no `local`, no `[[`.
#
# The names live here, once, so the command build.sh prints at the end is the
# command run.sh's defaults actually answer to.

# The defaults below are read by the scripts that source this file, which a
# lint of this file alone cannot see.
# shellcheck shell=sh disable=SC2034

DEFAULT_IMAGE="alter-zero:kali"
DEFAULT_NAME="alter-zero-kali"
DEFAULT_HOME_VOLUME="alter-zero-home"
DEFAULT_WORKSPACE_VOLUME="alter-zero-workspace"
# Published to the host by default; the Dockerfile EXPOSEs the same two.
DEFAULT_PORTS="8080 8888"
DEFAULT_BIND="127.0.0.1"

# fail WHAT [HINT] — the one exit that is not success. `prog` is the caller's.
fail() {
	printf '%s: %s\n' "${prog:-alter-zero}" "$1" >&2
	if [ -n "${2:-}" ]; then printf '  %s\n' "$2" >&2; fi
	exit 1
}

# need_value OPTION COUNT — an option that takes a value was given one.
need_value() {
	[ "$2" -ge 2 ] || fail "$1 needs a value" "Try --help."
}

# pick_engine [NAME] — sets `engine`. An explicit NAME wins, then
# $CONTAINER_ENGINE, then whichever is installed — Docker first.
pick_engine() {
	engine=${1:-${CONTAINER_ENGINE:-}}
	if [ -z "$engine" ]; then
		if command -v docker >/dev/null 2>&1; then
			engine=docker
		elif command -v podman >/dev/null 2>&1; then
			engine=podman
		else
			fail "neither docker nor podman is installed" "Install one, then run this again."
		fi
	fi
	case "$engine" in
	docker | podman) ;;
	*) fail "unknown engine: $engine" "Use --engine docker or --engine podman." ;;
	esac
	command -v "$engine" >/dev/null 2>&1 || fail "$engine is not installed" "Install it, or pick the other with --engine."
}

# The command that opens Alter Zero in a running container — the one line the
# README, build.txt and both scripts all print. Each bare `-e NAME` forwards
# that variable's value from YOUR terminal, which is what lets the app pick
# kitty / iTerm2 / sixel graphics instead of falling back to half-blocks.
enter_command() {
	printf '%s exec -it \\\n' "$engine"
	printf '    -e TERM -e COLORTERM -e TERM_PROGRAM \\\n'
	printf '    -e KITTY_WINDOW_ID -e TMUX \\\n'
	printf '    %s alter-zero\n' "$1"
}
