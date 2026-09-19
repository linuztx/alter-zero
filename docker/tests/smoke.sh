#!/bin/sh
# docker/tests/smoke.sh — the built image and docker/run.sh, against a real
# engine (docs/docker.md). Build first: docker/build.sh [--engine podman]
#
#   docker/tests/smoke.sh                     # Docker (Podman if Docker is absent)
#   docker/tests/smoke.sh --engine podman
#   docker/tests/smoke.sh --image me/az:dev
#
# Two halves. smoke_image.py runs INSIDE a throwaway container with no network
# — tools, root, the scanner, and the real CLI pinging a local stub collector,
# which is how "telemetry is still on" is proved without touching production.
# Then run.sh creates a disposable container on a temporary folder: files must
# cross the mount both ways, and a server started inside must answer on the
# published port. Everything it creates has a unique name and is removed.
set -eu

prog="smoke.sh"
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
# shellcheck source=docker/lib.sh
. "$here/../lib.sh"

want_engine="" image="$DEFAULT_IMAGE"
while [ $# -gt 0 ]; do
	case "$1" in
	-e | --engine) need_value "$1" $#; want_engine=$2; shift 2 ;;
	--engine=*) want_engine=${1#*=}; shift ;;
	-t | --image) need_value "$1" $#; image=$2; shift 2 ;;
	--image=*) image=${1#*=}; shift ;;
	-h | --help) sed -n '2,/^set -eu/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;;
	*) fail "unknown option: $1" ;;
	esac
done
pick_engine "$want_engine"
"$engine" image inspect "$image" >/dev/null 2>&1 || fail "there is no image called $image" "Build it first: docker/build.sh --engine $engine"

step() { printf '\n== %s\n' "$1"; }

step "inside the image, with no network"
"$engine" run --rm -i --network=none "$image" python3 - <"$here/smoke_image.py"

name="alter-zero-smoke-$$"
work=$(mktemp -d 2>/dev/null || mktemp -d -t alter-zero-smoke)
cleanup() {
	# From inside first: under rootful Docker the container's files are
	# root's, and this script is not.
	"$engine" exec "$name" sh -c 'rm -rf /workspace/* /workspace/.[!.]*' >/dev/null 2>&1 || true
	"$engine" rm -f "$name" >/dev/null 2>&1 || true
	"$engine" volume rm "$name-home" >/dev/null 2>&1 || true
	rm -rf "$work"
}
trap cleanup EXIT INT TERM HUP

step "run.sh: a folder of yours at /workspace, a port chosen by the engine"
printf 'from the host\n' >"$work/from-host.txt"
# The engine's full -p form, passed through as is: an empty host port is "any free one".
"$here/../run.sh" --engine "$engine" --image "$image" --name "$name" --home-volume "$name-home" \
	--port 127.0.0.1::8080 "$work" >/dev/null

[ "$("$engine" exec "$name" id -u)" = 0 ] || fail "the container is not running as root"
[ "$("$engine" exec "$name" cat /workspace/from-host.txt)" = "from the host" ] || fail "the host's file is not visible at /workspace"
"$engine" exec "$name" sh -c 'printf "from the container\n" > /workspace/from-container.txt'
[ "$(cat "$work/from-container.txt")" = "from the container" ] || fail "the container's file did not reach the host folder"
printf 'files cross the mount both ways\n'

mapped=$("$engine" port "$name" 8080/tcp | head -n 1)
case "$mapped" in
127.0.0.1:*) ;;
*) fail "port 8080 is published on '$mapped', not on loopback" ;;
esac
"$engine" exec -d "$name" python3 -m http.server 8080 --bind 0.0.0.0 --directory /workspace >/dev/null
tries=0
until answer=$(curl -fsS --noproxy '*' --max-time 2 "http://$mapped/from-container.txt" 2>/dev/null); do
	tries=$((tries + 1))
	[ "$tries" -lt 40 ] || fail "nothing answered on http://$mapped"
	sleep 0.25
done
[ "$answer" = "from the container" ] || fail "http://$mapped answered '$answer'"
printf 'a server inside answers on http://%s\n' "$mapped"

step "the scanner can open a raw socket"
# `nmap localhost` as root is a SYN scan. Docker grants NET_RAW by default and
# Podman does not, so this is the check that keeps the two engines answering
# the same way (docs/docker.md *Privileges*).
scan=$("$engine" exec "$name" nmap -sS -Pn -n -p 8080 127.0.0.1 2>&1) ||
	fail "a SYN scan failed inside the container" "$scan"
case "$scan" in
*"raw socket"*) fail "a SYN scan could not open a raw socket — NET_RAW is missing" "$scan" ;;
esac
printf '%s\n' "$scan" | grep -Eq '^8080/tcp +open' || fail "a SYN scan did not see the open port" "$scan"
printf 'a SYN scan opens a raw socket and sees the port\n'

printf '\nsmoke: passed (%s, %s)\n' "$engine" "$image"
