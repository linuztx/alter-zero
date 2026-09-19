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
#       --net-raw                grant NET_RAW, including on replacement
#       --clipboard              forward this desktop's Wayland and/or X11
#                                socket so Ctrl+V can read a copied image
#       --no-clipboard           disable inherited clipboard forwarding
#       --home-volume NAME       the volume kept at /root: sign-ins, settings,
#                                sessions (default: alter-zero-home)
#       --workspace-volume NAME  use this named volume instead of a folder
#       --replace                inherit the existing launch settings, then
#                                recreate it; explicit options override them
#       --reset-config           with --replace, use defaults and supplied
#                                options instead of inheriting old settings
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

valid_port_range() {
	case "$1" in
	*-*)
		port_first=${1%%-*}; port_last=${1#*-}
		valid_port "$port_first" && valid_port "$port_last" && [ "$port_first" -le "$port_last" ]
		;;
	*) valid_port "$1" ;;
	esac
}

decimal_port() {
	decimal=$1
	while [ "${decimal#0}" != "$decimal" ]; do decimal=${decimal#0}; done
	printf '%s' "${decimal:-0}"
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
	full_form=0
	case "$spec" in
	\[*\]:*:*)
		address=${spec%%]*}; address=${address#\[}
		pair=${spec#*\]:}; host_port=${pair%%:*}; container_port=${pair#*:}
		case "$address" in *:*) ;; *) fail "not an IPv6 address: $address" ;; esac
		full_form=1
		;;
	*:*:*)
		address=${spec%%:*}; pair=${spec#*:}
		host_port=${pair%%:*}; container_port=${pair#*:}
		full_form=1
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
	if [ "$full_form" -eq 1 ]; then
		case "$address" in *[!0-9A-Fa-f.:]*) fail "not a published address: $address" ;; esac
		case "$host_port" in '' | 0) ;; *) valid_port_range "$host_port" || fail "not a port: $1" ;; esac
	elif ! valid_port_range "$host_port"; then
		fail "not a port: $1" "Use ports 1-65535, or an ascending port range."
	fi
	if ! valid_port_range "$container_port"; then
		fail "not a port: $1" "Use PORT, HOST:CONTAINER, or IP:HOST:CONTAINER — each port 1-65535."
	fi
	# A range-to-range mapping must have equally many host and container ports.
	case "$host_port:$container_port" in
	*-*:*-*)
		host_first=$(decimal_port "${host_port%%-*}"); host_last=$(decimal_port "${host_port#*-}")
		container_first=$(decimal_port "${container_port%%-*}"); container_last=$(decimal_port "${container_port#*-}")
		[ "$((host_last - host_first))" -eq "$((container_last - container_first))" ] || fail "port ranges must have the same length: $1"
		;;
	esac
	if [ "$full_form" -eq 1 ]; then printf '%s%s\n' "$spec" "$proto"; return; fi
	case "$bind" in
	*:*) printf '[%s]:%s:%s%s\n' "$bind" "$host_port" "$container_port" "$proto" ;;
	*) printf '%s:%s:%s%s\n' "$bind" "$host_port" "$container_port" "$proto" ;;
	esac
}

want_engine="" name="$DEFAULT_NAME" image="$DEFAULT_IMAGE" home_volume="$DEFAULT_HOME_VOLUME"
bind="$DEFAULT_BIND" ports="" no_ports=0 net_raw=1 clipboard=0 replace=0 workspace=""
workspace_volume="$DEFAULT_WORKSPACE_VOLUME" reset_config=0
image_set=0 home_set=0 workspace_set=0 ports_set=0 bind_set=0 clipboard_set=0 net_raw_set=0
while [ $# -gt 0 ]; do
	case "$1" in
	-e | --engine) need_value "$1" $#; want_engine=$2; shift 2 ;;
	--engine=*) want_engine=${1#*=}; shift ;;
	-n | --name) need_value "$1" $#; name=$2; shift 2 ;;
	--name=*) name=${1#*=}; shift ;;
	-t | --image) need_value "$1" $#; image=$2; image_set=1; shift 2 ;;
	--image=*) image=${1#*=}; image_set=1; shift ;;
	-p | --port) need_value "$1" $#; [ -n "$2" ] || fail "--port needs a port"; ports="$ports $2"; ports_set=1; shift 2 ;;
	--port=*) [ -n "${1#*=}" ] || fail "--port needs a port"; ports="$ports ${1#*=}"; ports_set=1; shift ;;
	-b | --bind) need_value "$1" $#; bind=$2; bind_set=1; shift 2 ;;
	--bind=*) bind=${1#*=}; bind_set=1; shift ;;
	--home-volume) need_value "$1" $#; home_volume=$2; home_set=1; shift 2 ;;
	--home-volume=*) home_volume=${1#*=}; home_set=1; shift ;;
	--workspace-volume) need_value "$1" $#; [ "$workspace_set" -eq 0 ] || fail "only one workspace folder or volume can be given"; workspace_volume=$2; workspace_set=1; shift 2 ;;
	--workspace-volume=*) [ "$workspace_set" -eq 0 ] || fail "only one workspace folder or volume can be given"; workspace_volume=${1#*=}; workspace_set=1; shift ;;
	--no-ports) no_ports=1; ports_set=1; shift ;;
	--no-net-raw) net_raw=0; net_raw_set=1; shift ;;
	--net-raw) net_raw=1; net_raw_set=1; shift ;;
	--clipboard) clipboard=1; clipboard_set=1; shift ;;
	--no-clipboard) clipboard=0; clipboard_set=1; shift ;;
	--replace) replace=1; shift ;;
	--reset-config) reset_config=1; shift ;;
	-h | --help) usage; exit 0 ;;
	--) shift; break ;;
	-*) fail "unknown option: $1" "Try --help." ;;
	*)
		[ "$workspace_set" -eq 0 ] || fail "only one workspace folder or volume can be given" "Quote a path that contains spaces."
		workspace=$1
		workspace_set=1
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
[ "$reset_config" -eq 0 ] || [ "$replace" -eq 1 ] || fail "--reset-config requires --replace"

existing=0
if "$engine" container inspect "$name" >/dev/null 2>&1; then
	[ "$replace" -eq 1 ] ||
		fail "a container called $name already exists" "Enter it with '$engine exec -it $name alter-zero', recreate it with --replace (volumes and mounted folders are kept), or pick another --name."
	existing=1
fi

# Read actual engine state, including containers created before inheritance
# existed. Go templates keep this POSIX-shell launcher independent of jq/Python.
# Records have a strict shape; unusual mounts/configurations require an explicit
# reset instead of guessing and detaching a user's data.
cannot_inherit() {
	fail "cannot preserve $name: $1" "Use --replace --reset-config and repeat all required launch options. The existing container has not been removed."
}
inspect_path() {
	case "$1" in \"*\") decoded=${1#\"}; decoded=${decoded%\"} ;; *) cannot_inherit "an invalid quoted inspect value" ;; esac
	case "$decoded" in *\\* | *\"* | *'|'*) cannot_inherit "a path containing quotes, backslashes, control characters or '|'" ;; esac
	printf '%s' "$decoded"
}
if [ "$existing" -eq 1 ] && [ "$reset_config" -eq 0 ]; then
	mount_options_template=''
	host_ip_field=HostIp
	if [ "$engine" = podman ]; then
		host_ip_field=HostIP
		# shellcheck disable=SC2016 # Podman-only Go-template mount fields.
		mount_options_template='{{range $option := .Options}}{{printf "mount-option|%q|%q\n" $mount.Destination $option}}{{end}}'
	fi
	# shellcheck disable=SC2016 # Go-template variables, not shell expansion.
	settings=$("$engine" container inspect --format '
{{printf "format|1\nimage|%s\n" .Config.Image}}
{{range $mount := .Mounts}}{{printf "mount|%s|" .Type}}{{if eq .Type "volume"}}{{printf "%q" .Name}}{{else}}{{printf "%q" .Source}}{{end}}{{printf "|%q|%t\n" .Destination .RW}}{{printf "mount-mode|%q|%q|%q\n" .Destination .Mode .Propagation}}'"$mount_options_template"'{{end}}
{{range $port, $bindings := .HostConfig.PortBindings}}{{range $bindings}}{{printf "port|%s|%s|%s\n" .'"$host_ip_field"' .HostPort $port}}{{end}}{{end}}
{{range .HostConfig.CapAdd}}{{printf "capadd|%s\n" .}}{{end}}{{range .HostConfig.CapDrop}}{{printf "capdrop|%s\n" .}}{{end}}
{{printf "restart|%s\nnetwork|%s\n" .HostConfig.RestartPolicy.Name .HostConfig.NetworkMode}}
{{if .HostConfig.Privileged}}unsupported|privileged mode{{"\n"}}{{end}}
{{if .HostConfig.ReadonlyRootfs}}unsupported|read-only root filesystem{{"\n"}}{{end}}
{{if .HostConfig.PublishAllPorts}}unsupported|automatic port publication{{"\n"}}{{end}}
{{if .HostConfig.Devices}}unsupported|custom devices{{"\n"}}{{end}}
{{if and .HostConfig.Memory (ne (printf "%v" .HostConfig.Memory) "0")}}unsupported|memory limit{{"\n"}}{{end}}
{{if and .HostConfig.CpuShares (ne (printf "%v" .HostConfig.CpuShares) "0")}}unsupported|CPU shares{{"\n"}}{{end}}
{{if .HostConfig.ExtraHosts}}unsupported|custom host entries{{"\n"}}{{end}}
{{if .HostConfig.Dns}}unsupported|custom DNS{{"\n"}}{{end}}
{{if .HostConfig.UsernsMode}}unsupported|custom user namespace{{"\n"}}{{end}}
{{if and .Config.User (ne .Config.User "root") (ne .Config.User "0")}}unsupported|custom user{{"\n"}}{{end}}
{{if ne .Config.WorkingDir "/workspace"}}unsupported|custom working directory{{"\n"}}{{end}}
{{if ne .Config.Hostname "az-kali"}}unsupported|custom hostname{{"\n"}}{{end}}
{{if ne (printf "%v" .Config.Entrypoint) "[/usr/bin/tini --]"}}unsupported|custom entrypoint{{"\n"}}{{end}}
{{if ne (printf "%v" .Config.Cmd) "[sleep infinity]"}}unsupported|custom command{{"\n"}}{{end}}
{{range .HostConfig.SecurityOpt}}{{printf "security|%s\n" .}}{{end}}' "$name") || cannot_inherit "container inspection failed"
	old_home="" old_workspace="" old_workspace_type="" old_ports="" old_clipboard=0 old_restart=""
	old_raw=0 old_raw_add=0 old_raw_drop=0 format_seen=0
	# Docker grants NET_RAW by default; Podman requires an explicit grant.
	if [ "$engine" = docker ]; then old_raw=1; fi
	while IFS='|' read -r kind first second third fourth extra; do
		[ -z "$extra" ] || cannot_inherit "an inspect value contains an unsupported delimiter"
		case "$kind" in mount | mount-mode | mount-option | port) ;; *) [ -z "$second$third$fourth" ] || cannot_inherit "an inspect value contains an unsupported delimiter" ;; esac
		case "$kind" in
		'') ;;
		format) [ "$first" = 1 ] || cannot_inherit "unknown inspect format"; format_seen=1 ;;
		image) if [ "$image_set" -eq 0 ]; then image=$first; fi ;;
		mount)
			second=$(inspect_path "$second") || exit 1
			third=$(inspect_path "$third") || exit 1
			[ -n "$second" ] || cannot_inherit "an empty mount source"
			case "$third" in
			/root)
				[ -z "$old_home" ] || cannot_inherit "multiple home mounts"
				if [ "$home_set" -eq 0 ]; then
					if [ "$first" != volume ] || [ "$fourth" != true ]; then cannot_inherit "the home is not a writable named volume"; fi
				fi
				old_home=$second
				;;
			/workspace)
				[ -z "$old_workspace" ] || cannot_inherit "multiple workspace mounts"
				if [ "$workspace_set" -eq 0 ]; then
					case "$first:$fourth" in bind:true | volume:true) ;; *) cannot_inherit "the workspace is not a writable folder or volume" ;; esac
				fi
				old_workspace=$second; old_workspace_type=$first
				;;
			/run/alter-zero/wayland-0 | /tmp/.X11-unix | /run/alter-zero/x11)
				[ "$first:$fourth" = bind:false ] || cannot_inherit "an unsupported clipboard mount"
				old_clipboard=1
				;;
			*) cannot_inherit "an extra mount at $third" ;;
			esac
			;;
		mount-mode | mount-option)
			first=$(inspect_path "$first") || exit 1
			second=$(inspect_path "$second") || exit 1
			case "$first" in /workspace) [ "$workspace_set" -eq 0 ] || continue ;; /root) [ "$home_set" -eq 0 ] || continue ;; esac
			if [ "$kind" = mount-mode ]; then
				third=$(inspect_path "$third") || exit 1
				case "$second" in '' | rw | ro | Z | rw,Z | Z,rw) ;; *) cannot_inherit "mount mode $second at $first" ;; esac
				case "$third" in '' | private | rprivate) ;; *) cannot_inherit "mount propagation $third at $first" ;; esac
			else
				case "$second" in rw | ro | bind | rbind | nosuid | nodev | private | rprivate | Z) ;; *) cannot_inherit "mount option $second at $first" ;; esac
			fi
			;;
		port)
			if [ "$ports_set" -eq 0 ]; then
				case "$first" in *[!0-9A-Fa-f.:]*) cannot_inherit "an invalid published address" ;; esac
				case "$third" in */tcp | */udp | */sctp) ;; *) cannot_inherit "an unsupported published protocol" ;; esac
				valid_port "${third%/*}" || cannot_inherit "an invalid published container port"
				[ -z "$second" ] || valid_port "$second" || cannot_inherit "an invalid published host port"
				if [ "$bind_set" -eq 1 ]; then first=$bind; fi
				case "$first" in *:*) first="[$first]" ;; '') first=0.0.0.0 ;; esac
				old_ports="$old_ports $first:$second:$third"
			fi
			;;
		capadd) case "$first" in NET_RAW | CAP_NET_RAW) old_raw=1; old_raw_add=1 ;; *) cannot_inherit "additional capability $first" ;; esac ;;
		capdrop) case "$first" in NET_RAW | CAP_NET_RAW) old_raw_drop=1 ;; *) cannot_inherit "dropped capability $first" ;; esac ;;
		restart) case "$first" in '' | no | always | unless-stopped) old_restart=$first ;; *) cannot_inherit "restart policy $first" ;; esac ;;
		network) case "$first" in bridge | default | pasta | slirp4netns) ;; *) cannot_inherit "network mode $first" ;; esac ;;
		security) case "$first" in no-new-privileges | no-new-privileges:true) ;; *) cannot_inherit "security option $first" ;; esac ;;
		unsupported) cannot_inherit "$first" ;;
		*) cannot_inherit "an unsupported inspect record" ;;
		esac
	done <<EOF
$settings
EOF
	[ "$format_seen" -eq 1 ] || cannot_inherit "empty container inspection"
	if [ "$old_raw_add" -eq 1 ] && [ "$old_raw_drop" -eq 1 ] && [ "$net_raw_set" -eq 0 ]; then cannot_inherit "conflicting NET_RAW grants and drops"; fi
	if [ "$home_set" -eq 0 ]; then [ -n "$old_home" ] || cannot_inherit "no persistent home mount"; home_volume=$old_home; fi
	if [ "$workspace_set" -eq 0 ]; then
		[ -n "$old_workspace" ] || cannot_inherit "no persistent workspace mount"
		if [ "$old_workspace_type" = bind ]; then workspace=$old_workspace; else workspace_volume=$old_workspace; fi
	fi
	if [ "$ports_set" -eq 0 ]; then ports=$old_ports; if [ -z "$ports" ]; then no_ports=1; fi; fi
	if [ "$clipboard_set" -eq 0 ]; then clipboard=$old_clipboard; fi
	if [ "$net_raw_set" -eq 0 ]; then net_raw=$old_raw; if [ "$old_raw_drop" -eq 1 ]; then net_raw=0; fi; fi
fi

for volume_name in "$home_volume" "$workspace_volume"; do
	case "$volume_name" in '' | [!A-Za-z0-9]* | *[!A-Za-z0-9_.-]*) fail "not a volume name: $volume_name" ;; esac
done
case "$image" in '' | -*) fail "not an image name: $image" ;; esac
"$engine" image inspect "$image" >/dev/null 2>&1 ||
	fail "there is no image called $image" "Build it first: docker/build.sh --engine $engine"

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
		resolved_home=$(CDPATH='' cd -- "${HOME:?}" && pwd -P) || fail "cannot resolve your home directory"
		[ "$workspace" != "$resolved_home" ] ||
			fail "refusing to relabel your home directory for SELinux" "Mount a folder inside it instead."
		mount="$mount:Z"
	fi
	where="$workspace"
else
	mount="$workspace_volume:/workspace"
	where="the $workspace_volume volume"
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
if [ "$net_raw" -eq 1 ]; then
	set -- "$@" --cap-add NET_RAW
else
	set -- "$@" --cap-drop NET_RAW
fi
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
	set -- "$@" --restart "${old_restart:-unless-stopped}"
fi

while [ "$passthrough" -gt 0 ]; do
	moved=$1
	shift
	set -- "$@" "$moved"
	passthrough=$((passthrough - 1))
done

# Validate launcher options before removing a working container. The engine
# still validates its own passthrough options when creating the replacement.
if [ "$existing" -eq 1 ]; then
	"$engine" rm -f "$name" >/dev/null
	printf 'Removed the old %s. Its volumes and folders are untouched.\n' "$name"
fi

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
