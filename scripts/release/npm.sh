#!/usr/bin/env bash
# scripts/release/npm.sh — publish a release to npm: one package per release
# target holding its binary, then the launcher that depends on them
# (docs/npm.md).
#
#   scripts/release.sh npm 0.12.0                    # dist/ → the registry
#   scripts/release.sh npm 0.12.0 dist --dry-run     # stage and validate, publish nothing
#   scripts/release.sh npm 0.12.0 dist --out DIR     # stage into DIR (default: target/npm)
#
# The assets are proven first (`verify`); a version already on GitHub is
# fetched back for it with `scripts/release.sh download`, so it can be
# published to npm by hand (docs/npm.md). Each target's archive becomes a
# platform package, `{launcher}-{os}-{cpu}`: its binary at bin/alter-zero
# (mode 755), the LICENSE, a README pointing at the launcher, and `os`/`cpu`
# — plus `libc: glibc` on Linux — so npm installs only the one that runs. The
# launcher is npm/ staged with the LICENSE and without its `scripts` (one of
# which refuses any publish made from npm/ itself). The launcher goes last,
# so its optional dependencies are on the registry before anything can
# install it, and every package must be there: a missing platform is a
# missing binary for every install on it, so only a dry run may skip one. A
# version already on the registry is left as is — npm versions are immutable
# — so a run that died halfway is simply run again. A pre-release goes to the
# `next` dist-tag rather than `latest`.
#
# The registry and its credentials are npm's own configuration: `npm login`
# on a laptop, setup-node's NODE_AUTH_TOKEN or trusted publishing in the
# release workflow. A dry run needs neither and never touches the network.
set -euo pipefail
# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

version=""
dist=""
out=""
dry_run=0
while [ $# -gt 0 ]; do
	case "$1" in
	--out)
		[ $# -ge 2 ] || die "npm: --out needs a directory"
		out="$2"
		shift 2
		;;
	--dry-run)
		dry_run=1
		shift
		;;
	-h | --help)
		sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d' | sed 's/^# \{0,1\}//'
		exit 0
		;;
	-*) die "npm: unknown option $1" ;;
	*)
		if [ -z "$version" ]; then
			version="$1"
		elif [ -z "$dist" ]; then
			dist="$1"
		else
			die "npm: unexpected argument $1"
		fi
		shift
		;;
	esac
done
[ -n "$version" ] || die "usage: scripts/release.sh npm VERSION [DIST] [--out DIR] [--dry-run]"
is_semver "$version" || die "npm: '$version' is not a semver version"
[ -n "$dist" ] || dist="$RELEASE_ROOT/dist"
[ -d "$dist" ] || die "npm: no directory at $dist"
[ -n "$out" ] || out="$RELEASE_ROOT/target/npm"
command -v node >/dev/null 2>&1 || die "npm: node is not installed"
command -v npm >/dev/null 2>&1 || die "npm: npm is not installed"

manifest="$(npm_manifest_path)"
[ -f "$manifest" ] || die "npm: no $manifest — the launcher package (docs/npm.md)"
launcher="$(npm_name)"
[ "$(npm_version)" = "$version" ] || die "npm: npm/package.json is at $(npm_version), not $version — scripts/release.sh prepare keeps it in step"
for target in $NPM_TARGETS; do
	pkg="$(npm_package_of "$target")"
	pin="$(npm_pins | awk -v p="$pkg" '$1 == p { print $2; exit }')"
	[ "$pin" = "$version" ] || die "npm: npm/package.json pins $pkg at ${pin:-nothing}, not $version — scripts/release.sh check names every mismatch"
done

# The assets are proven before anything is staged from them.
bash "$RELEASE_LIB_DIR/verify.sh" "$dist" --version "$version"

bin="$(manifest_name)"
repo="$(manifest_repository)"
tag="$(npm_dist_tag "$version")"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

# stage_manifest MODE DIR [ARGS…] — DIR/package.json, written by node so the
# JSON is node's own: `platform KEY LABEL` derives a platform package from
# the launcher's manifest, `launcher` is that manifest minus its scripts.
stage_manifest() {
	node - "$manifest" "$version" "$@" <<'EOF'
const fs = require('fs');
const path = require('path');
const [manifestPath, version, mode, dir, key, label] = process.argv.slice(2);
const launcher = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
let out;
if (mode === 'platform') {
  const [os, cpu] = key.split('-');
  const { directory, ...repository } = launcher.repository || {};
  out = {
    name: `${launcher.name}-${key}`,
    version,
    description: `The ${label} binary of ${launcher.name}.`,
    homepage: launcher.homepage,
    bugs: launcher.bugs,
    repository,
    license: launcher.license,
    author: launcher.author,
    os: [os],
    cpu: [cpu],
    ...(os === 'linux' ? { libc: ['glibc'] } : {}),
    files: ['bin/alter-zero'],
    preferUnplugged: true,
    publishConfig: launcher.publishConfig,
  };
} else {
  out = { ...launcher };
  delete out.scripts;
}
fs.writeFileSync(path.join(dir, 'package.json'), `${JSON.stringify(out, null, 2)}\n`);
EOF
}

# Stage every package, in publishing order: the platforms, then the launcher.
mkdir -p "$out"
dirs=()
names=()
for target in $NPM_TARGETS; do
	key="$(npm_platform "$target")"
	pkg="$(npm_package_of "$target")"
	archive="$dist/$(asset_archive "$version" "$target")"
	if [ ! -f "$archive" ]; then
		if [ "$dry_run" -eq 0 ]; then
			die "npm: no $(basename "$archive") in $dist — without it $pkg is missing, and so is the binary of every $key install"
		fi
		warn "no $(basename "$archive") in $dist — skipping $pkg (dry run)"
		continue
	fi
	dir="$out/${pkg#*/}"
	rm -rf "$dir"
	mkdir -p "$dir/bin"
	stem="$(asset_stem "$version" "$target")"
	tar -xzf "$archive" -C "$scratch"
	cp "$scratch/$stem/$bin" "$dir/bin/$bin"
	chmod 755 "$dir/bin/$bin"
	cp "$scratch/$stem/LICENSE" "$dir/LICENSE"
	stage_manifest platform "$dir" "$key" "$(platform_label "$target")"
	cat >"$dir/README.md" <<EOF
# $pkg

The $(platform_label "$target") build of [Alter Zero]($repo), packaged on its
own so npm downloads only the binary a machine can run. Install
[\`$launcher\`](https://www.npmjs.com/package/$launcher), which depends on it:

\`\`\`bash
npm install -g $launcher
\`\`\`
EOF
	dirs+=("$dir")
	names+=("$pkg")
done
dir="$out/${launcher#*/}"
rm -rf "$dir"
mkdir -p "$dir/bin"
cp "$RELEASE_ROOT/npm/bin/alter-zero.js" "$dir/bin/alter-zero.js"
chmod 755 "$dir/bin/alter-zero.js"
cp "$RELEASE_ROOT/npm/README.md" "$RELEASE_ROOT/LICENSE" "$dir/"
stage_manifest launcher "$dir"
dirs+=("$dir")
names+=("$launcher")
ok "staged ${#dirs[@]} package(s) for $launcher $version in $out"

published=0
present=0
for i in "${!dirs[@]}"; do
	dir="${dirs[$i]}"
	name="${names[$i]}"
	publish=(npm publish "$dir" --tag "$tag")
	if [ "$dry_run" -eq 1 ]; then
		info "would run: ${publish[*]}"
		"${publish[@]}" --dry-run
		continue
	fi
	if [ "$(npm view "$name@$version" version 2>/dev/null || true)" = "$version" ]; then
		warn "$name@$version is already on the registry — left as is (npm versions are immutable)"
		present=$((present + 1))
		continue
	fi
	info "publishing $name@$version ($tag)"
	"${publish[@]}"
	published=$((published + 1))
done

if [ "$dry_run" -eq 1 ]; then
	ok "dry run complete: ${#dirs[@]} package(s) staged in $out (nothing published)"
else
	ok "npm: published $published package(s) of $launcher $version, $present already on the registry"
fi
