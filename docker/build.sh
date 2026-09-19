#!/bin/sh
# docker/build.sh — build the headless Kali image for Alter Zero (docs/docker.md).
#
#   docker/build.sh                            # build with Docker (Podman if Docker is absent)
#   docker/build.sh --engine podman            # build with Podman
#   docker/build.sh ~/projects/site            # build, then start a container on that folder
#   docker/build.sh --version v0.4.0           # pin a release (default: the latest)
#   docker/build.sh --with "binutils tcpdump"  # bake extra apt packages into the image
#   docker/build.sh --help
#
# Nothing is compiled. The image downloads the published Alter Zero release —
# checksum-verified by the repository's own install.sh — so a build is an apt
# install and one download. "Latest" is resolved HERE, before the engine looks
# at its layer cache, and handed over as a build argument: that is what makes
# a rebuild pick up a new release, where a `latest` resolved inside a cached
# layer would keep installing the release it first saw.
#
# Options:
#   -e, --engine docker|podman   the container engine (default: $CONTAINER_ENGINE,
#                                else docker when installed, else podman)
#   -v, --version vX.Y.Z         the Alter Zero release to install (default: latest)
#   -t, --image NAME             the image to build (default: alter-zero:kali)
#   -w, --with "PKG ..."         extra apt packages to install, space-separated
#       --no-pull                do not refresh the Kali base image first
#       --no-cache               rebuild every layer
#   -h, --help
#
# With a WORKSPACE_DIR, the build is followed by `docker/run.sh WORKSPACE_DIR`,
# which creates the container with that folder mounted at /workspace:
#   -n, --name NAME              the container to create (default: alter-zero-kali)
#       --replace                replace a container of that name (its data is kept)
# Everything else about the container — ports, the clipboard — is run.sh's own.
#
# Anything after `--` goes to `docker build` as is: -- --platform linux/arm64
#
# POSIX sh only, like install.sh. ALTER_ZERO_INSTALL_BASE_URL points the
# release lookup, and the download inside the build, at a fork's repository.
set -eu

prog="build.sh"
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH='' cd -- "$here/.." && pwd)
# shellcheck source=docker/lib.sh
. "$here/lib.sh"

usage() { sed -n '2,/^set -eu/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; }

# The final URL of the repository's "latest release" redirect.
latest_url() {
	if command -v curl >/dev/null 2>&1; then
		curl -fsSLI --retry 2 --connect-timeout 15 --max-time 60 \
			-o /dev/null -w '%{url_effective}' "$1"
	elif command -v wget >/dev/null 2>&1; then
		# -S prints each hop's headers; the last Location is where it ended.
		wget -q -S --spider --max-redirect=10 "$1" 2>&1 |
			awk 'tolower($1) == "location:" { url = $2 } END { if (url == "") exit 1; print url }'
	else
		return 1
	fi
}

want_engine="" want_version="" image="$DEFAULT_IMAGE" extra="" pull=1 no_cache=0
workspace="" name="" replace=0
while [ $# -gt 0 ]; do
	case "$1" in
	-e | --engine) need_value "$1" $#; want_engine=$2; shift 2 ;;
	--engine=*) want_engine=${1#*=}; shift ;;
	-v | --version) need_value "$1" $#; want_version=$2; shift 2 ;;
	--version=*) want_version=${1#*=}; shift ;;
	-t | --image) need_value "$1" $#; image=$2; shift 2 ;;
	--image=*) image=${1#*=}; shift ;;
	-w | --with) need_value "$1" $#; extra="$extra $2"; shift 2 ;;
	--with=*) extra="$extra ${1#*=}"; shift ;;
	-n | --name) need_value "$1" $#; name=$2; shift 2 ;;
	--name=*) name=${1#*=}; shift ;;
	--replace) replace=1; shift ;;
	--no-pull) pull=0; shift ;;
	--no-cache) no_cache=1; shift ;;
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
[ -n "$image" ] || fail "--image needs a name"
if [ -z "$workspace" ]; then
	[ -z "$name" ] || fail "--name only applies with a workspace folder" "It names the container that run.sh creates: docker/build.sh --name NAME DIR"
	[ "$replace" -eq 0 ] || fail "--replace only applies with a workspace folder" "It replaces the container that run.sh creates: docker/build.sh --replace DIR"
elif [ -e "$workspace" ] && [ ! -d "$workspace" ]; then
	# Found now rather than after the build: run.sh would refuse it anyway.
	fail "not a folder: $workspace"
fi

# Package names only. The list is word-split into an `apt-get install` inside
# the build, so an option-shaped word would be apt's to interpret.
for package in $extra; do
	case "$package" in
	[a-z0-9]*) ;;
	*) fail "not a package name: $package" ;;
	esac
	case "$package" in
	*[!a-z0-9+.-]*) fail "not a package name: $package" ;;
	esac
done
extra=$(printf '%s' "$extra" | sed 's/^ *//; s/ *$//')

base_url=${ALTER_ZERO_INSTALL_BASE_URL:-https://github.com/linuztx/alter-zero}
if [ -n "$want_version" ]; then
	case "$want_version" in v*) tag=$want_version ;; *) tag="v$want_version" ;; esac
	how="pinned"
else
	resolved=$(latest_url "$base_url/releases/latest") ||
		fail "could not resolve the latest release from $base_url" "Check your connection (this needs curl or wget), or pin one: --version vX.Y.Z"
	case "$resolved" in
	"$base_url"/releases/tag/*) tag=${resolved##*/} ;;
	*) fail "the latest-release redirect ended somewhere unexpected: $resolved" "Pin a release instead: --version vX.Y.Z" ;;
	esac
	how="latest"
fi
# The tag becomes a build argument and part of a download URL: it is a
# version, or the build does not start.
printf '%s\n' "$tag" | LC_ALL=C grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$' ||
	fail "not a release tag: $tag" "Expected vX.Y.Z."

printf 'Building %s — Alter Zero %s (%s) on Kali Rolling, with %s.\n\n' "$image" "$tag" "$how" "$engine"

# Assemble `build …` in front of whatever followed `--`, which stays last so
# it can override anything here.
passthrough=$#
set -- "$@" build
if [ "$pull" -eq 1 ]; then
	# Kali is a rolling distribution: an old base can carry an expired archive
	# key, and then `apt-get update` fails on a perfectly good Dockerfile.
	if [ "$engine" = podman ]; then set -- "$@" --pull=always; else set -- "$@" --pull; fi
fi
if [ "$no_cache" -eq 1 ]; then set -- "$@" --no-cache; fi
set -- "$@" --build-arg "ALTER_ZERO_VERSION=$tag"
if [ -n "$extra" ]; then set -- "$@" --build-arg "EXTRA_PACKAGES=$extra"; fi
if [ -n "${ALTER_ZERO_INSTALL_BASE_URL:-}" ]; then
	set -- "$@" --build-arg "ALTER_ZERO_INSTALL_BASE_URL=$ALTER_ZERO_INSTALL_BASE_URL"
fi
set -- "$@" -t "$image" -f "$here/Dockerfile"
while [ "$passthrough" -gt 0 ]; do
	moved=$1
	shift
	set -- "$@" "$moved"
	passthrough=$((passthrough - 1))
done
"$engine" "$@" "$root"

printf '\nBuilt %s — Alter Zero %s.\n' "$image" "$tag"

if [ -n "$workspace" ]; then
	printf '\n'
	# run.sh reads a leading dash as an option, so a folder named that way
	# goes over as a path.
	case "$workspace" in -*) workspace="./$workspace" ;; esac
	set -- --engine "$engine" --image "$image"
	if [ -n "$name" ]; then set -- "$@" --name "$name"; fi
	if [ "$replace" -eq 1 ]; then set -- "$@" --replace; fi
	exec "$here/run.sh" "$@" "$workspace"
fi

# Name the engine in the hints only when it was chosen: run.sh picks its own
# the same way, so an unqualified command lands on the same one.
flag=""
if [ -n "$want_engine" ] || [ -n "${CONTAINER_ENGINE:-}" ]; then flag=" --engine $engine"; fi
cat <<EOF

Next, create the container (its data outlives it, in named volumes):

  docker/run.sh$flag                      # /workspace is a named volume
  docker/run.sh$flag ~/projects/site      # /workspace is that folder

Already have one from an older build? Recreate it to move onto this image;
sign-ins, settings and your files are kept:

  docker/run.sh$flag --replace
EOF
