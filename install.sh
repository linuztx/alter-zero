#!/bin/sh
# install.sh — the one-line installer for Alter Zero (docs/release.md).
#
#   curl -fsSL https://raw.githubusercontent.com/linuztx/alter-zero/main/install.sh | sh
#
# Picks the release archive built for this machine (Linux x86_64 or arm64,
# macOS Intel or Apple silicon), verifies its SHA-256 against the checksum
# published beside it, and installs the binary to ~/.local/bin. Nothing is
# installed unless the checksum matches and the binary runs.
#
#   ALTER_ZERO_VERSION=v0.1.0 … | sh              # pin a release (default: the latest)
#   ALTER_ZERO_INSTALL_DIR=/usr/local/bin … | sh   # install somewhere else
#   … | sh -s -- --version v0.1.0 --dir ~/bin      # the same two, as flags
#   … | sh -s -- --help
#
# POSIX sh only — no arrays, no `local`, no `[[` — so `sh` on macOS (bash 3.2
# in POSIX mode), dash and busybox all run it. Everything is a function and
# the last line calls `main`, so a download cut short executes nothing.
# ALTER_ZERO_INSTALL_BASE_URL points it at a stand-in for github.com; the
# release tooling's selftest uses that to install from a local server.
set -eu

REPO="linuztx/alter-zero"
BIN="alter-zero"
BASE_URL="${ALTER_ZERO_INSTALL_BASE_URL:-https://github.com/$REPO}"
GLIBC_FLOOR_MINOR=35

# ---------------------------------------------------------------------------
# Looks: colour on a terminal (truecolor for the banner gradient when the
# terminal says it has it), the app's own glyphs in a UTF-8 locale, plain
# ASCII otherwise, and nothing at all under NO_COLOR or in a pipe.
# ---------------------------------------------------------------------------
setup_looks() {
	esc=$(printf '\033')
	bold='' dim='' reset='' red='' green='' yellow='' cyan='' g1='' g2='' g3=''
	if [ -t 1 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-}" != dumb ]; then
		bold="${esc}[1m" dim="${esc}[2m" reset="${esc}[0m"
		red="${esc}[31m" green="${esc}[32m" yellow="${esc}[33m" cyan="${esc}[36m"
		case "${COLORTERM:-}" in
		truecolor | 24bit)
			g1="${esc}[38;2;137;220;235m" # #89DCEB, the banner gradient's near end
			g2="${esc}[38;2;137;200;242m"
			g3="${esc}[38;2;137;180;250m" # #89B4FA, its far end
			;;
		*) g1="$cyan" g2="$cyan" g3="${esc}[34m" ;;
		esac
	fi
	case "${LC_ALL:-${LC_CTYPE:-${LANG:-}}}" in
	*UTF-8* | *utf-8* | *UTF8* | *utf8*) utf8=1 ;;
	*) utf8=0 ;;
	esac
	if [ "$utf8" -eq 1 ]; then
		arrow='→' tick='✔' cross='✘' bang='!'
	else
		arrow='>' tick='+' cross='x' bang='!'
	fi
}

banner() {
	printf '\n'
	if [ "$utf8" -eq 1 ]; then
		printf '   %s▙▄▙▄▟▄▟%s     %sAlter Zero%s\n' "$g1" "$reset" "$bold" "$reset"
		printf '  %s▝▜▄███▄▛▘%s    %sthe one-line installer%s\n' "$g2" "$reset" "$dim" "$reset"
		printf '    %s▘▘ ▝▝%s\n' "$g3" "$reset"
	else
		printf '  %sAlter Zero%s  %sthe one-line installer%s\n' "$bold" "$reset" "$dim" "$reset"
	fi
	printf '\n'
}

# step LABEL DETAIL — a row in progress; done_ LABEL DETAIL — a row finished.
step() { printf '  %s%s%s  %-9s %s\n' "$cyan" "$arrow" "$reset" "$1" "$2"; }
done_() { printf '  %s%s%s  %-9s %s\n' "$green" "$tick" "$reset" "$1" "$2"; }
note() { printf '  %s%s%s  %s\n' "$yellow" "$bang" "$reset" "$1"; }
# fail WHAT [WHY] — the one exit that is not success; nothing has been
# installed when it fires, and it says so.
fail() {
	printf '\n  %s%s  %s%s\n' "$red" "$cross" "$1" "$reset" >&2
	[ -n "${2:-}" ] && printf '     %s%s%s\n' "$dim" "$2" "$reset" >&2
	printf '     %sNothing was installed.%s\n\n' "$dim" "$reset" >&2
	exit 1
}

usage() {
	if [ -f "$0" ]; then
		sed -n '2,/^set -eu/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
	else
		printf 'usage: curl -fsSL https://raw.githubusercontent.com/%s/main/install.sh | sh -s -- [--version vX.Y.Z] [--dir DIR]\n' "$REPO"
	fi
}

# ---------------------------------------------------------------------------
# The machine: OS and CPU to a Rust target triple, plus the two things that
# make a Linux download useless — musl, and a glibc older than the build's.
# ---------------------------------------------------------------------------
detect_target() {
	os=$(uname -s 2>/dev/null || echo unknown)
	cpu=$(uname -m 2>/dev/null || echo unknown)
	case "$cpu" in
	x86_64 | amd64) cpu=x86_64 ;;
	aarch64 | arm64) cpu=aarch64 ;;
	*) fail "unsupported CPU: $cpu" "Alter Zero ships for x86_64 and arm64." ;;
	esac
	case "$os" in
	Linux)
		target="$cpu-unknown-linux-gnu"
		platform="Linux $cpu"
		if command -v ldd >/dev/null 2>&1; then
			ldd_out=$(ldd --version 2>&1 || true)
			case "$ldd_out" in
			*musl*) fail "this Linux uses musl, and the release build needs glibc" "Build from source instead: https://github.com/$REPO#build-from-source" ;;
			esac
			glibc=$(printf '%s\n' "$ldd_out" | sed -n '1s/.*[^0-9.]\([0-9][0-9]*\.[0-9][0-9]*\).*/\1/p')
			if [ -n "$glibc" ]; then
				major=${glibc%%.*}
				minor=${glibc#*.}
				minor=${minor%%.*}
				if [ "$major" -eq 2 ] && [ "$minor" -lt "$GLIBC_FLOOR_MINOR" ]; then
					fail "glibc $glibc is older than the 2.$GLIBC_FLOOR_MINOR the release build needs" "Build from source instead: https://github.com/$REPO#build-from-source"
				fi
				platform="$platform · glibc $glibc"
			fi
		fi
		;;
	Darwin)
		target="$cpu-apple-darwin"
		if [ "$cpu" = aarch64 ]; then platform="macOS Apple silicon"; else platform="macOS Intel"; fi
		;;
	MINGW* | MSYS* | CYGWIN* | Windows_NT)
		fail "Windows is not supported" "Alter Zero runs on Linux and macOS; on Windows, use it inside WSL."
		;;
	*) fail "unsupported operating system: $os" "Alter Zero ships for Linux and macOS." ;;
	esac
}

# ---------------------------------------------------------------------------
# The network: curl, or wget where curl is missing.
# ---------------------------------------------------------------------------
need_tools() {
	if command -v curl >/dev/null 2>&1; then
		fetcher=curl
	elif command -v wget >/dev/null 2>&1; then
		fetcher=wget
	else
		fail "neither curl nor wget is installed" "Install one and run this again."
	fi
	command -v tar >/dev/null 2>&1 || fail "tar is not installed"
	if command -v sha256sum >/dev/null 2>&1; then
		hasher=sha256sum
	elif command -v shasum >/dev/null 2>&1; then
		hasher=shasum
	elif command -v openssl >/dev/null 2>&1; then
		hasher=openssl
	else
		fail "no SHA-256 tool found" "Install sha256sum, shasum or openssl."
	fi
}

# fetch URL DEST [progress] — a file; `progress` shows curl's bar on a terminal.
fetch() {
	if [ "$fetcher" = curl ]; then
		if [ "${3:-}" = progress ] && [ -t 2 ]; then
			curl -fL --progress-bar -o "$2" "$1"
		else
			curl -fsSL -o "$2" "$1"
		fi
	else
		wget -q -O "$2" "$1"
	fi
}

# The tag the repository's "latest release" redirect points at.
latest_tag() {
	if [ "$fetcher" = curl ]; then
		final=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$BASE_URL/releases/latest") || return 1
		printf '%s\n' "${final##*/}"
	else
		wget -qO- "https://api.github.com/repos/$REPO/releases/latest" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p'
	fi
}

sha256_of() {
	case "$hasher" in
	sha256sum) sha256sum "$1" | awk '{ print $1 }' ;;
	shasum) shasum -a 256 "$1" | awk '{ print $1 }' ;;
	openssl) openssl dgst -sha256 "$1" | awk '{ print $NF }' ;;
	esac
}

human_size() {
	awk -v b="$1" 'BEGIN { if (b >= 1048576) printf "%.1f MB", b / 1048576; else if (b >= 1024) printf "%d KB", b / 1024; else printf "%d B", b }'
}

# ~-shorten a path for display.
pretty() {
	case "$1" in
	"$HOME"/*) printf '~%s\n' "${1#"$HOME"}" ;;
	*) printf '%s\n' "$1" ;;
	esac
}

# ---------------------------------------------------------------------------
main() {
	want="${ALTER_ZERO_VERSION:-}"
	dir="${ALTER_ZERO_INSTALL_DIR:-$HOME/.local/bin}"
	while [ $# -gt 0 ]; do
		case "$1" in
		--version | -v)
			[ $# -ge 2 ] || fail "--version needs a value, like v0.1.0"
			want="$2"
			shift 2
			;;
		--version=*) want="${1#--version=}"; shift ;;
		--dir | -d)
			[ $# -ge 2 ] || fail "--dir needs a directory"
			dir="$2"
			shift 2
			;;
		--dir=*) dir="${1#--dir=}"; shift ;;
		-h | --help)
			usage
			exit 0
			;;
		*) fail "unknown option: $1" "Try --help." ;;
		esac
	done

	setup_looks
	banner
	need_tools
	detect_target
	step "Platform" "$platform $dim($target)$reset"

	if [ -n "$want" ]; then
		case "$want" in v*) tag="$want" ;; *) tag="v$want" ;; esac
		how="pinned"
	else
		tag=$(latest_tag) || fail "could not reach $BASE_URL/releases/latest" "Check your connection, or pin a release with ALTER_ZERO_VERSION=vX.Y.Z."
		case "$tag" in v[0-9]*) ;; *) fail "could not work out the latest release" "The redirect ended at '$tag'. Pin one with ALTER_ZERO_VERSION=vX.Y.Z." ;; esac
		how="latest"
	fi
	version="${tag#v}"
	step "Release" "$tag $dim· $how$reset"

	asset="$BIN-$tag-$target"
	tmp=$(mktemp -d 2>/dev/null || mktemp -d -t alter-zero)
	trap 'rm -rf "$tmp"' EXIT INT TERM HUP
	archive="$tmp/$asset.tar.gz"
	url="$BASE_URL/releases/download/$tag/$asset.tar.gz"

	step "Download" "$asset.tar.gz"
	fetch "$url" "$archive" progress || fail "no release asset for $target at $tag" "Looked for $url"
	# SHA256SUMS covers every archive in the release — one published file
	# rather than one beside each asset, which is what the release page
	# lists and `sha256sum -c` reads.
	sums="$tmp/SHA256SUMS"
	sums_url="$BASE_URL/releases/download/$tag/SHA256SUMS"
	fetch "$sums_url" "$sums" || fail "the checksum file for this release is missing" "Refusing to install an unverified download. Looked for $sums_url"
	size=$(human_size "$(wc -c <"$archive" | tr -d ' ')")

	expected=$(awk -v name="$asset.tar.gz" '$2 == name { print $1; exit }' "$sums")
	[ -n "$expected" ] || fail "SHA256SUMS does not list $asset.tar.gz" "Refusing to install an unverified download. It lists: $(awk '{ print $2 }' "$sums" | paste -sd ' ' - | cut -c1-160)"
	case "$expected" in
	[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]*) ;;
	*) fail "the published checksum file is malformed" "SHA256SUMS reads: $(head -c 80 "$sums")" ;;
	esac
	[ "${#expected}" -eq 64 ] || fail "the published checksum file is malformed" "Expected 64 hex digits, got ${#expected}."
	actual=$(sha256_of "$archive")
	if [ "$actual" != "$expected" ]; then
		fail "checksum mismatch for $asset.tar.gz" "published $expected
     computed  $actual
     The download is corrupt or was tampered with."
	fi
	done_ "Checksum" "sha256 $dim$(printf '%s' "$expected" | cut -c1-12)…$(printf '%s' "$expected" | cut -c57-64)$reset matches the published value $dim($size)$reset"

	tar -xzf "$archive" -C "$tmp" || fail "could not extract $asset.tar.gz"
	src="$tmp/$asset/$BIN"
	[ -f "$src" ] || fail "the archive does not contain $asset/$BIN"

	previous=""
	if command -v "$BIN" >/dev/null 2>&1; then
		previous=$("$BIN" --version 2>/dev/null | awk '{ print $2 }') || previous=""
	fi

	mkdir -p "$dir" 2>/dev/null || fail "cannot create $(pretty "$dir")" "Pick another place: ALTER_ZERO_INSTALL_DIR=\$HOME/bin"
	[ -w "$dir" ] || fail "$(pretty "$dir") is not writable" "Pick a directory you own: ALTER_ZERO_INSTALL_DIR=\$HOME/bin"
	# Land beside the old binary, then move over it: a running alter-zero
	# keeps its mapped file and the new one appears whole, never half-written.
	staged="$dir/.$BIN.$$"
	if ! { cp "$src" "$staged" && chmod 755 "$staged" && mv -f "$staged" "$dir/$BIN"; }; then
		rm -f "$staged"
		fail "could not write $(pretty "$dir/$BIN")"
	fi

	reported=$("$dir/$BIN" --version 2>&1) || fail "the installed binary did not run" "$reported"
	case "$reported" in
	"$BIN $version") ;;
	*) note "the binary reports '$reported', expected '$BIN $version'" ;;
	esac
	if [ -n "$previous" ] && [ "$previous" != "$version" ]; then
		done_ "Installed" "$(pretty "$dir/$BIN") $dim· upgraded from $previous$reset"
	else
		done_ "Installed" "$(pretty "$dir/$BIN") $dim· $reported$reset"
	fi

	printf '\n'
	printf '  %sAlter Zero %s is ready.%s Start it with\n\n' "$bold" "$tag" "$reset"
	printf '      %s%s%s\n\n' "$cyan" "$BIN" "$reset"
	printf '  then %s/login%s to connect a provider and %s/model%s to choose a model.\n' "$bold" "$reset" "$bold" "$reset"
	case ":$PATH:" in
	*":$dir:"*) ;;
	*)
		printf '\n'
		note "$(pretty "$dir") is not on your PATH. Add it to your shell's startup file:"
		case "$(basename "${SHELL:-sh}")" in
		zsh) rc=".zshrc" ;;
		bash) if [ "$os" = Darwin ]; then rc=".bash_profile"; else rc=".bashrc"; fi ;;
		fish) rc="" ;;
		*) rc=".profile" ;;
		esac
		if [ -n "$rc" ]; then
			printf "     %secho 'export PATH=\"%s:\$PATH\"' >> ~/%s%s\n" "$cyan" "$(pretty "$dir" | sed "s|^~|\\\$HOME|")" "$rc" "$reset"
		else
			printf '     %sfish_add_path %s%s\n' "$cyan" "$dir" "$reset"
		fi
		;;
	esac
	printf '\n  %sUninstall any time with: rm %s%s\n\n' "$dim" "$(pretty "$dir/$BIN")" "$reset"
}

main "$@"
