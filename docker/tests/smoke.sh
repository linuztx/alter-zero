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
custom_name="$name-custom"
work=$(mktemp -d 2>/dev/null || mktemp -d -t alter-zero-smoke)
cleanup() {
	"$engine" rm -f "$custom_name" >/dev/null 2>&1 || true
	# From inside first: under rootful Docker the container's files are
	# root's, and this script is not.
	"$engine" exec "$name" sh -c 'rm -rf /workspace/* /workspace/.[!.]*' >/dev/null 2>&1 || true
	"$engine" rm -f "$name" >/dev/null 2>&1 || true
	"$engine" volume rm "$name-home" >/dev/null 2>&1 || true
	"$engine" volume rm "$custom_name-home" >/dev/null 2>&1 || true
	rm -rf "$work"
}
trap cleanup EXIT INT TERM HUP

step "run.sh: a folder of yours at /workspace, a port chosen by the engine"
printf 'from the host\n' >"$work/from-host.txt"
# A zero host port must request automatic allocation on both engines.
# The launcher translates it to the empty host-port form Podman accepts.
"$here/../run.sh" --engine "$engine" --image "$image" --name "$name" --home-volume "$name-home" \
	--port 127.0.0.1:0:8080 "$work" -- --pids-limit 42 >/dev/null

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

step "invalid replacement options leave the working container alone"
before=$("$engine" container inspect --format '{{.Id}}' "$name")
check_untouched() {
	[ "$("$engine" container inspect --format '{{.Id}}' "$1")" = "$2" ] ||
		fail "a refused replacement removed the original container"
	[ "$("$engine" container inspect --format '{{.State.Running}}' "$1")" = true ] ||
		fail "a refused replacement stopped the original container"
}
reject_invalid_replacement() {
	if "$here/../run.sh" --engine "$engine" --image "$image" --name "$name" \
		--replace "$@" >"$work/rejected-replacement.log" 2>&1; then
		fail "invalid replacement options were accepted: $*"
	fi
	check_untouched "$name" "$before"
}
reject_invalid_replacement --port 70000
# A full publication must not bypass validation of an explicit --bind.
reject_invalid_replacement --bind 127.0.0.999 --port 127.0.0.1::8080
reject_invalid_replacement --port 127.0.0.999::8080
reject_invalid_replacement --port 18080:8080-8082
reject_invalid_replacement --port 127.0.0.1:18080:8080-8082
reject_invalid_replacement --port '[::1]:18080:8080-8082/udp'
# Docker accepts a host range for one container port as dynamic allocation;
# Podman requires matching lengths even when just one side is a range.
if [ "$engine" = podman ]; then
	reject_invalid_replacement --port 18080-18082:8080
	reject_invalid_replacement --port 127.0.0.1:18080-18082:8080
	reject_invalid_replacement --port '[::1]:18080-18082:8080/udp'
fi
printf 'invalid options leave the original container running\n'

step "replacement keeps mounts and ports, and explicitly drops NET_RAW"
"$engine" exec "$name" sh -c 'printf "keep this home\n" > /root/.smoke-home-keep'
# Only the capability changes. The image, custom home volume, workspace
# folder, loopback port mapping and PID limit must be inherited.
"$here/../run.sh" --engine "$engine" --name "$name" --replace --no-net-raw >/dev/null
check_replacement() {
	[ "$("$engine" exec "$name" cat /workspace/from-host.txt)" = "from the host" ] || fail "replacement lost the workspace mount"
	[ "$("$engine" exec "$name" cat /root/.smoke-home-keep)" = "keep this home" ] || fail "replacement lost the home volume"
	[ "$("$engine" exec "$name" hostname)" = "$DEFAULT_HOSTNAME" ] || fail "replacement lost the hostname"
	[ "$("$engine" container inspect --format '{{.HostConfig.PidsLimit}}' "$name")" = 42 ] || fail "replacement lost the PID limit"
	case "$("$engine" port "$name" 8080/tcp)" in
	127.0.0.1:*) ;;
	*) fail "replacement lost the loopback port mapping" ;;
	esac
	"$engine" exec -i "$name" python3 - <<'PY'
import socket
from pathlib import Path

status = dict(line.split(":", 1) for line in Path("/proc/self/status").read_text().splitlines())
for field in ("CapEff", "CapBnd"):
    assert not int(status[field].strip(), 16) & (1 << 13), f"NET_RAW remains in {field}"
try:
    raw = socket.socket(socket.AF_INET, socket.SOCK_RAW, socket.IPPROTO_ICMP)
except PermissionError:
    pass
else:
    raw.close()
    raise AssertionError("--no-net-raw still permits raw sockets")
PY
}
check_replacement

# A later bare replacement must also remember the disabled capability.
"$here/../run.sh" --engine "$engine" --name "$name" --replace >/dev/null
check_replacement
printf 'replacement preserves the workspace, home, ports, hostname, PID limit and disabled NET_RAW\n'

step "replacement accepts an explicit zero host port"
"$here/../run.sh" --engine "$engine" --name "$name" --replace --port 127.0.0.1:0:8080 >/dev/null
check_replacement
printf 'zero host port requests automatic allocation on creation and replacement\n'

step "unsupported environment and CPU settings require an explicit reset"
for custom_setting in environment cpu; do
	case "$custom_setting" in
	environment) set -- --env ALTER_ZERO_SMOKE_SETTING=kept ;;
	cpu) set -- --cpus 0.5 ;;
	esac
	"$here/../run.sh" --engine "$engine" --image "$image" --name "$custom_name" \
		--home-volume "$custom_name-home" --no-ports "$work/custom" -- "$@" >/dev/null
	custom_before=$("$engine" container inspect --format '{{.Id}}' "$custom_name")
	cpu_before=$("$engine" container inspect --format '{{.HostConfig.CpuPeriod}}:{{.HostConfig.CpuQuota}}:{{.HostConfig.NanoCpus}}' "$custom_name")
	if "$here/../run.sh" --engine "$engine" --name "$custom_name" --replace \
		>"$work/rejected-custom.log" 2>&1; then
		fail "replacement silently discarded custom $custom_setting settings"
	fi
	check_untouched "$custom_name" "$custom_before"
	grep -q -- '--reset-config' "$work/rejected-custom.log" || fail "refusal did not explain the explicit reset option"
	# An explicit reset is valid when the caller repeats all required options.
	"$here/../run.sh" --engine "$engine" --image "$image" --name "$custom_name" \
		--replace --reset-config --home-volume "$custom_name-home" --no-ports "$work/custom" -- "$@" >/dev/null
	[ "$("$engine" container inspect --format '{{.Id}}' "$custom_name")" != "$custom_before" ] || fail "explicit reset did not replace the container"
	case "$custom_setting" in
	environment)
		[ "$("$engine" exec "$custom_name" printenv ALTER_ZERO_SMOKE_SETTING)" = kept ] || fail "explicit reset lost the repeated environment option"
		;;
	cpu)
		[ "$("$engine" container inspect --format '{{.HostConfig.CpuPeriod}}:{{.HostConfig.CpuQuota}}:{{.HostConfig.NanoCpus}}' "$custom_name")" = "$cpu_before" ] || fail "explicit reset lost the repeated CPU limit"
		;;
	esac
	"$engine" rm -f "$custom_name" >/dev/null
done
printf 'custom environment and CPU limits are refused safely or preserved by explicit reset\n'

printf '\nsmoke: passed (%s, %s)\n' "$engine" "$image"
