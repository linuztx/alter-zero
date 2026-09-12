#!/usr/bin/env bash
# scripts/release/build.sh — build one target's release binary and package
# it as an asset.
#
#   scripts/release.sh build                              # the host target
#   scripts/release.sh build aarch64-unknown-linux-gnu    # a cross build
#   scripts/release.sh build x86_64-apple-darwin --dist out/
#
# The build is the one Cargo.toml documents for a packaged release —
# `cargo build --release --locked --target T --config
# profile.release.incremental=false` (the incremental release profile is a
# developer's rebuild speed, not a shipped layout; docs/build-time.md) — and
# the binary lands in dist/ as {name}-v{version}-{target}.tar.gz beside its
# .sha256 (lib.sh's package_dist). Cross-compiling a Linux GNU target from
# another Linux CPU needs that target's gcc (for oniguruma's and ring's C
# sources as much as for linking): with `aarch64-linux-gnu-gcc` on PATH the
# linker and `cc`-crate variables are set here, honouring any the caller
# already exported. The macOS targets cross-compile between each other on
# any Mac (Apple's clang is a universal cross compiler); they do not build
# on Linux.
set -euo pipefail
# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

target=""
dist="$RELEASE_ROOT/dist"
while [ $# -gt 0 ]; do
	case "$1" in
	--dist)
		dist="$2"
		shift 2
		;;
	-h | --help)
		sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d' | sed 's/^# \{0,1\}//'
		exit 0
		;;
	-*) die "build: unknown option $1" ;;
	*)
		[ -z "$target" ] || die "build: one target at a time (got '$target' and '$1')"
		target="$1"
		shift
		;;
	esac
done
[ -n "$target" ] || target="$(host_target)"
host="$(host_target)"
version="$(manifest_version)"
name="$(manifest_name)"
cargo_target_dir="${CARGO_TARGET_DIR:-$RELEASE_ROOT/target}"

# Set one variable unless the caller already did.
default_env() {
	local var="$1" value="$2"
	if [ -z "${!var:-}" ]; then
		export "$var=$value"
		info "$var=$value"
	fi
}

if [ "$target" != "$host" ]; then
	case "$target" in
	*-unknown-linux-gnu)
		case "$host" in *-unknown-linux-*) ;; *) die "build: $target cross-compiles from Linux only (host: $host)" ;; esac
		arch="${target%%-*}"
		prefix="$arch-linux-gnu"
		command -v "$prefix-gcc" >/dev/null 2>&1 || die "build: $target needs $prefix-gcc on PATH — on Debian/Ubuntu: apt-get install gcc-$(printf '%s' "$arch" | tr _ -)-linux-gnu libc6-dev-$(case "$arch" in aarch64) printf arm64 ;; x86_64) printf amd64 ;; *) printf '%s' "$arch" ;; esac)-cross"
		upper="$(printf '%s' "$target" | tr 'a-z-' 'A-Z_')"
		under="$(printf '%s' "$target" | tr - _)"
		default_env "CARGO_TARGET_${upper}_LINKER" "$prefix-gcc"
		default_env "CC_$under" "$prefix-gcc"
		default_env "AR_$under" "$prefix-ar"
		;;
	*-apple-darwin)
		case "$host" in *-apple-darwin) ;; *) die "build: $target builds on macOS only (host: $host)" ;; esac
		;;
	*)
		warn "build: no cross-compilation rule for $target — trusting the environment"
		;;
	esac
fi

if command -v rustup >/dev/null 2>&1; then
	if ! rustup target list --installed 2>/dev/null | grep -qx "$target"; then
		info "rustup target add $target"
		rustup target add "$target"
	fi
fi

info "cargo build --release --locked --target $target (profile.release.incremental=false)"
(cd "$RELEASE_ROOT" && cargo build --release --locked --target "$target" --config profile.release.incremental=false)

bin="$cargo_target_dir/$target/release/$name"
[ -x "$bin" ] || die "build: expected the binary at $bin"
archive="$(package_dist "$bin" "$version" "$target" "$dist")"
ok "built $archive ($(du -h "$archive" | awk '{ print $1 }'))"
