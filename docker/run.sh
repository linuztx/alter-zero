#!/bin/sh
# docker/run.sh — create and start the Alter Zero Kali container (docs/docker.md).
#
#   docker/run.sh                          # /workspace is a named volume
#   docker/run.sh ~/projects/site          # /workspace is that folder on your machine
#   docker/run.sh --engine podman ~/projects/site
#   docker/run.sh --port 3000              # publish 3000 instead of 8080 and 8888
#   docker/run.sh --bind 0.0.0.0           # reachable from your network, not just this machine
#   docker/run.sh --clipboard              # forward the desktop clipboard: Ctrl+V pastes images
#   docker/run.sh --replace ~/other/dir    # recreate it on another folder; data is kept
#   docker/run.sh --help
#
# The container idles; you work in it with `docker exec` (the command is
# printed at the end). It runs as root — under a rootless engine (Podman's
# default) that root is your own user on the host; under the usual rootful
# Docker it is the machine's root, who then owns the files it creates in a
# mounted folder.
#
# Options:
#   -e, --engine docker|podman   the container engine (default: $CONTAINER_ENGINE,
#                                else docker when installed, else podman)
#   -n, --name NAME              the container's name (default: alter-zero-kali)
#   -t, --image NAME             the image to start (default: alter-zero:kali)
#   -p, --port SPEC              publish a port; repeat for more. Replaces the
#                                default pair, 8080 and 8888. SPEC is PORT,
#                                HOST:CONTAINER, either with /udp, or the
#                                engine's own full IP:HOST:CONTAINER form.
#   -b, --bind ADDRESS           the host address the ports listen on
#                                (default: 127.0.0.1 — this machine only)
#       --no-ports               publish nothing
#       --no-net-raw             drop NET_RAW, which is granted so that the
#                                scanner works (`nmap -sS`, traceroute, tcpdump)
#       --clipboard              forward this desktop's Wayland and/or X11
#                                socket so Ctrl+V can read a copied image
#       --home-volume NAME       the volume kept at /root: sign-ins, settings,
#                                sessions (default: alter-zero-home)
#       --replace                remove an existing container of this name
#                                first. Volumes and mounted folders are kept.
#   -h, --help
#
# Anything after `--` goes to `docker run` as is: -- --cap-add NET_ADMIN
# (To drop NET_RAW use --no-net-raw, not `-- --cap-drop NET_RAW`: beside the
# --cap-add this script passes, Docker ignores the drop and Podman refuses
# the container.)
#
# POSIX sh only, like install.sh.
set -eu

prog="run.sh"
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
# shellcheck source=docker/lib.sh
. "$here/lib.sh"

usage() { sed -n '2,/^set -eu/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; }

valid_port() {
	case "$1" in '' | *[!0-9]*) return 1 ;; esac
	[ "${#1}" -le 5 ] && [ "$1" -ge 1 ] && [ "$1" -le 65535 ]
}

# publish_spec SPEC — SPEC as the engine's -p takes it, the short forms
# completed with the bind address.
publish_spec() {
	spec=$1
	proto=""
	case "$spec" in
	*/tcp | */udp | */sctp)
		proto="/${spec##*/}"
		spec=${spec%/*}
		;;
	esac
	case "$spec" in
	*:*:* | \[*)
		# The engine's full form: an address of the caller's choosing.
		printf '%s%s\n' "$spec" "$proto"
		return
		;;
	*:*)
		host_port=${spec%%:*}
		container_port=${spec#*:}
		;;
	*)
		host_port=$spec
		container_port=$spec
		;;
	esac
	if ! valid_port "$host_port" || ! valid_port "$container_port"; then
		fail "not a port: $1" "Use PORT, HOST:CONTAINER, or IP:HOST:CONTAINER — each port 1-65535."
	fi
	case "$bind" in
	*:*) printf '[%s]:%s:%s%s\n' "$bind" "$host_port" "$container_port" "$proto" ;;
	*) printf '%s:%s:%s%s\n' "$bind" "$host_port" "$container_port" "$proto" ;;
	esac
}

want_engine="" name="$DEFAULT_NAME" image="$DEFAULT_IMAGE" home_volume="$DEFAULT_HOME_VOLUME"
bind="$DEFAULT_BIND" ports="" no_ports=0 net_raw=1 clipboard=0 replace=0 workspace=""
while [ $# -gt 0 ]; do
	case "$1" in
	-e | --engine) need_value "$1" $#; want_engine=$2; shift 2 ;;
	--engine=*) want_engine=${1#*=}; shift ;;
	-n | --name) need_value "$1" $#; name=$2; shift 2 ;;
	--name=*) name=${1#*=}; shift ;;
	-t | --image) need_value "$1" $#; image=$2; shift 2 ;;
	--image=*) image=${1#*=}; shift ;;
	-p | --port) need_value "$1" $#; [ -n "$2" ] || fail "--port needs a port"; ports="$ports $2"; shift 2 ;;
	--port=*) [ -n "${1#*=}" ] || fail "--port needs a port"; ports="$ports ${1#*=}"; shift ;;
	-b | --bind) need_value "$1" $#; bind=$2; shift 2 ;;
	--bind=*) bind=${1#*=}; shift ;;
	--home-volume) need_value "$1" $#; home_volume=$2; shift 2 ;;
	--home-volume=*) home_volume=${1#*=}; shift ;;
	--no-ports) no_ports=1; shift ;;
	--no-net-raw) net_raw=0; shift ;;
	--clipboard) clipboard=1; shift ;;
	--replace) replace=1; shift ;;
	-h | --help) usage; exit 0 ;;
	--) shift; break ;;
	-*) fail "unknown option: $1" "Try --help." ;;
	*)
		[ -z "$workspace" ] || fail "only one workspace folder can be given" "Got '$workspace' and '$1'. Quote a path that contains spaces."
		workspace=$1
		shift
		;;
	esac
done
# What is left in "$@" is the engine's own: everything after `--`.

pick_engine "$want_engine"
# A name is what the engine accepts as one — and nothing it would read as an
# option, or that would break the mount strings built from it below.
case "$name" in
'' | [!A-Za-z0-9]* | *[!A-Za-z0-9_.-]*) fail "not a container name: $name" "Letters, digits, '_', '.' and '-', starting with a letter or digit." ;;
esac
case "$home_volume" in
'' | [!A-Za-z0-9]* | *[!A-Za-z0-9_.-]*) fail "not a volume name: $home_volume" ;;
esac
# The engine would refuse an empty image itself, but with its own message
# about a name it never received; the sibling options all answer for
# themselves here.
case "$image" in
'' | -*) fail "not an image name: $image" ;;
esac
case "$bind" in
'' | *[!0-9A-Fa-f.:]*) fail "not an address: $bind" "Use an IP of this machine, like 127.0.0.1 or 0.0.0.0." ;;
esac
if [ "$no_ports" -eq 1 ] && [ -n "$ports" ]; then
	fail "--no-ports and --port contradict each other"
fi

"$engine" image inspect "$image" >/dev/null 2>&1 ||
	fail "there is no image called $image" "Build it first: docker/build.sh --engine $engine"

if "$engine" container inspect "$name" >/dev/null 2>&1; then
	[ "$replace" -eq 1 ] ||
		fail "a container called $name already exists" "Enter it with '$engine exec -it $name alter-zero', recreate it with --replace (volumes and mounted folders are kept), or pick another --name."
	"$engine" rm -f "$name" >/dev/null
	printf 'Removed the old %s. Its volumes and folders are untouched.\n' "$name"
fi

# ---------------------------------------------------------------------------
# /workspace: a folder of yours, or a named volume.
# ---------------------------------------------------------------------------
if [ -n "$workspace" ]; then
	if [ -e "$workspace" ] && [ ! -d "$workspace" ]; then fail "not a folder: $workspace"; fi
	# Created here, as you: an engine handed a path that does not exist makes
	# the folder itself, owned by root.
	mkdir -p -- "$workspace" 2>/dev/null || fail "cannot create $workspace"
	workspace=$(CDPATH='' cd -- "$workspace" && pwd -P) || fail "cannot open $workspace"
	case "$workspace" in
	/) fail "refusing to mount / as the workspace" "Pick the folder you want to work in." ;;
	*:*) fail "the engine cannot mount a path containing ':': $workspace" ;;
	esac
	mount="$workspace:/workspace"
	# SELinux: without a label the container is denied its own workspace. :Z
	# relabels the folder for this container alone — which is not something
	# to do to a whole home directory.
	if command -v selinuxenabled >/dev/null 2>&1 && selinuxenabled; then
		[ "$workspace" != "${HOME:-}" ] ||
			fail "refusing to relabel your home directory for SELinux" "Mount a folder inside it instead."
		mount="$mount:Z"
	fi
	where="$workspace"
else
	mount="$DEFAULT_WORKSPACE_VOLUME:/workspace"
	where="the $DEFAULT_WORKSPACE_VOLUME volume"
fi

# Assemble `run …` in front of whatever followed `--`, which stays last so it
# can override what is here — except the capability below, which is a list
# and not a last-one-wins flag; --no-net-raw is its off switch.
# Keep the Linux hostname short and independent of the engine's container name.
passthrough=$#
set -- "$@" run --detach --name "$name" --hostname "$DEFAULT_HOSTNAME" \
	--security-opt no-new-privileges
# NET_RAW, because the image ships a scanner and running as root makes `nmap
# localhost` a SYN scan, which opens a raw socket. Docker grants this
# capability by default; Podman 4.x dropped it — so without this line the same
# image answers "Couldn't open a raw socket" on one engine and scans on the
# other, which is exactly what "works the same with Docker and Podman" must
# not mean. It is one named capability, scoped to the container's own network
# namespace: no --privileged, no host networking, no_new_privileges still on.
# --no-net-raw drops it, and traceroute, tcpdump and `nmap -sS` go with it.
if [ "$net_raw" -eq 1 ]; then set -- "$@" --cap-add NET_RAW; fi
set -- "$@" --volume "$home_volume:/root" --volume "$mount"

# ---------------------------------------------------------------------------
# Ports. Nothing in the image listens; these are for what you start.
# ---------------------------------------------------------------------------
published=""
if [ "$no_ports" -eq 0 ]; then
	for spec in ${ports:-$DEFAULT_PORTS}; do
		full=$(publish_spec "$spec") || exit 1
		set -- "$@" --publish "$full"
		published="$published $full"
	done
fi

# ---------------------------------------------------------------------------
# The clipboard. `exec -it` carries keystrokes, never the desktop's selection,
# so Ctrl+V needs the display server's own socket. Sources are --mount, which
# fails on a missing path where -v would create a root-owned folder in its
# place — in your runtime dir, where the compositor's socket belongs.
# ---------------------------------------------------------------------------
forwarded=""
caveat=""
if [ "$clipboard" -eq 1 ]; then
	if [ -n "${WAYLAND_DISPLAY:-}" ]; then
		case "$WAYLAND_DISPLAY" in
		/*) socket=$WAYLAND_DISPLAY ;;
		*) socket="${XDG_RUNTIME_DIR:-}/$WAYLAND_DISPLAY" ;;
		esac
		# ',' ends the --mount source. Say so, rather than blank the path and
		# report "found no desktop session" — the X11 branch below fails here.
		case "$socket" in *,*) fail "the engine cannot mount a path containing ',': $socket" ;; esac
		if [ -S "$socket" ]; then
			# The one socket, never the runtime dir around it (D-Bus, PipeWire,
			# your keyring's agent all live there).
			set -- "$@" --mount "type=bind,src=$socket,dst=/run/alter-zero/wayland-0,ro" \
				--env XDG_RUNTIME_DIR=/run/alter-zero --env WAYLAND_DISPLAY=wayland-0
			forwarded="Wayland"
		fi
	fi
	# X11 too, not instead: a Wayland compositor without a data-control
	# protocol (GNOME's) serves its clipboard through XWayland, and the app
	# falls back to it on its own.
	# (ALTER_ZERO_X11_DIR is the tests' seam: where this machine keeps its X
	# sockets is fixed, so a sandbox has to be able to say otherwise.)
	x11_dir=${ALTER_ZERO_X11_DIR:-/tmp/.X11-unix}
	case "${DISPLAY:-}" in
	:*)
		number=${DISPLAY#:}
		number=${number%%.*}
		case "$x11_dir" in *,*) fail "the engine cannot mount a path containing ',': $x11_dir" ;; esac
		if [ -n "$number" ] && [ -S "$x11_dir/X$number" ]; then
			set -- "$@" --mount "type=bind,src=$x11_dir,dst=/tmp/.X11-unix,ro" --env "DISPLAY=:$number"
			# The server checks a cookie filed under this machine's hostname,
			# which the container does not share — so hand it a copy filed
			# under the wildcard family instead. The folder is mounted, not
			# the file, so a refreshed cookie is seen without a new container.
			if command -v xauth >/dev/null 2>&1; then
				state="${XDG_STATE_HOME:-${HOME:?}/.local/state}/alter-zero/docker/$name"
				case "$state" in *,*) fail "the engine cannot mount a path containing ',': $state" ;; esac
				mkdir -p -- "$state" && chmod 700 "$state"
				# chmod, not umask: a folder with a default ACL ignores the
				# umask, and this file is about to hold the display's cookie.
				rm -f -- "$state/Xauthority"
				: >"$state/Xauthority"
				chmod 600 "$state/Xauthority"
				xauth nlist "$DISPLAY" 2>/dev/null | sed 's/^..../ffff/' |
					xauth -f "$state/Xauthority" nmerge - 2>/dev/null || true
				set -- "$@" --mount "type=bind,src=$state,dst=/run/alter-zero/x11,ro" \
					--env XAUTHORITY=/run/alter-zero/x11/Xauthority
			else
				caveat="xauth is not installed, so the X11 cookie was not handed over. If a paste is refused, install xauth and recreate the container with --replace."
			fi
			forwarded="${forwarded:+$forwarded and }X11"
		fi
		;;
	esac
	[ -n "$forwarded" ] ||
		fail "found no desktop session to forward" "Run this from a terminal on your own desktop: it needs WAYLAND_DISPLAY or a local DISPLAY. Over SSH, copy the picture into the workspace and ask Alter Zero to read it."
else
	# A desktop login's sockets do not exist at boot, so only a container
	# without them is safe to bring back up by itself.
	set -- "$@" --restart unless-stopped
fi

while [ "$passthrough" -gt 0 ]; do
	moved=$1
	shift
	set -- "$@" "$moved"
	passthrough=$((passthrough - 1))
done

if ! "$engine" "$@" "$image" >/dev/null; then
	# A port already taken fails AFTER the container is created, and the
	# leftover would block the retry.
	"$engine" rm -f "$name" >/dev/null 2>&1 || true
	fail "$engine could not start $name" "If a port is already in use, choose others with --port, or publish none with --no-ports."
fi

printf 'Started %s from %s.\n\n' "$name" "$image"
printf '  /workspace   %s\n' "$where"
printf '  /root        the %s volume — sign-ins, settings, sessions\n' "$home_volume"
if [ -n "$published" ]; then
	for full in $published; do
		printf '  port         %s\n' "$full"
	done
else
	printf '  ports        none published\n'
fi
if [ -n "$forwarded" ]; then printf '  clipboard    %s forwarded — Ctrl+V pastes a copied image\n' "$forwarded"; fi
if [ -n "$caveat" ]; then printf '\n  Note: %s\n' "$caveat"; fi

printf '\nOpen Alter Zero from your own terminal:\n\n  '
enter_command "$name"
printf '\nA shell instead: %s exec -it %s bash\n' "$engine" "$name"
printf 'Stop / start:    %s stop %s · %s start %s\n' "$engine" "$name" "$engine" "$name"
