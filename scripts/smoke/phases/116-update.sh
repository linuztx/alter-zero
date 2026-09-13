#!/usr/bin/env bash
# Phase 116 — update

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# update — the once-a-day update check, its card under the banner, the
# /settings row, and `alter-zero update` (docs/update.md). The release
# tooling's stand-in github.com (scripts/release/release_server.py) serves a
# fake release — the binary under test, packaged as v9.9.9 for this machine
# by the release tooling's own package_dist, so the archive's layout and
# checksum are exactly a real release's — plus install.sh, and redirects
# /releases/latest to it. The launches below UNSET the suite-wide
# ALTER_ZERO_UPDATE_CHECK=0 and point the app at the stub, each against a
# fresh config home. NO_PROXY keeps a developer's HTTP proxy off loopback.
S116="${S}_update"
UP_TMP="$(mktemp -d "$SMOKE_TMP/update.XXXXXX")"
UP_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
UP_DIST="$UP_TMP/dist"
UP_CARD="Update available"
if ! (
	set -e
	# shellcheck source=../../release/lib.sh
	. "$SMOKE_ROOT/scripts/release/lib.sh"
	package_dist "$BIN_ABS" 9.9.9 "$(host_target)" "$UP_DIST" >/dev/null
	# A published release carries SHA256SUMS beside its archives, and that is
	# the file install.sh verifies against — package_dist writes only the
	# per-asset ones.
	write_sha256sums "$UP_DIST" >/dev/null
); then
	fail "could not package the fake v9.9.9 release with the release tooling"
fi
UP_SERVER=""
UP_PORT=""
# up_start TAG — the stand-in over the fake dist, redirecting latest → TAG.
up_start() {
	: >"$UP_TMP/port"
	python3 "$SMOKE_ROOT/scripts/release/release_server.py" "$UP_DIST" "$1" "$SMOKE_ROOT/install.sh" >"$UP_TMP/port" 2>/dev/null &
	UP_SERVER=$!
	for _ in $(seq 1 50); do
		[ -s "$UP_TMP/port" ] && break
		sleep 0.1
	done
	UP_PORT="$(cat "$UP_TMP/port")"
}
up_stop() {
	kill "$UP_SERVER" 2>/dev/null
	wait "$UP_SERVER" 2>/dev/null
	UP_SERVER=""
}
UP_BASE="env -u ALTER_ZERO_UPDATE_CHECK NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_SKILLS_DIR=$SMOKE_SKILLS ALTER_ZERO_AGENTS_DIR=$SMOKE_AGENTS ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS"
up_quit() {
	submit "$S116" "/quit"
	sleep 0.6
	tmux kill-session -t "$S116" 2>/dev/null
}
# A top-level field of update.json (pretty JSON, one field per line), quotes
# stripped: `up_field FILE KEY`.
up_field() {
	sed -n -E "s/^ *\"$2\": *(.*)$/\1/p" "$1" 2>/dev/null | sed -E 's/,$//; s/^"//; s/"$//' | head -1
}
# Open /settings filtered to the Update check row.
up_open_row() {
	tmux send-keys -t "$S116" -l "/settings"
	sleep 0.3
	tmux send-keys -t "$S116" Enter
	sleep 0.5
	tmux send-keys -t "$S116" -l "update"
	sleep 0.4
}
TODAY="$(date -u +%Y-%m-%d)"

# (a) A fresh config home against a newer release: the check runs after the
# first frame, the card lands under the banner naming both versions and the
# command, and update.json records the check, what it found, and the day
# the card was shown.
up_start v9.9.9
tmux new-session -d -s "$S116" -x 100 -y 34 "$UP_BASE ALTER_ZERO_UPDATE_URL=http://127.0.0.1:$UP_PORT ALTER_ZERO_CONFIG_DIR=$UP_TMP/home $BIN"
up_first="$(wait_pane 10 "$S116" -S -40 -- -F "$UP_CARD")" || fail "the card never appeared under the banner"
dump "the first launch against v9.9.9" "$up_first"
if ! printf '%s' "$up_first" | grep -qF "v9.9.9"; then
	fail "the card does not name the newer version"
fi
if ! printf '%s' "$up_first" | grep -qF "v$UP_VERSION"; then
	fail "the card does not name the running version v$UP_VERSION"
fi
if ! printf '%s' "$up_first" | grep -qF "alter-zero update"; then
	fail "the card does not name alter-zero update"
fi
if ! printf '%s' "$up_first" | grep -qF "ALTER_ZERO_UPDATE_CHECK=0"; then
	fail "the card lost its opt-out — clamped instead of wrapped?"
fi
if ! printf '%s' "$up_first" | grep -qF "releases/tag/v9.9.9"; then
	fail "the card does not link the release page"
fi
note "update.json after the first launch"
cat "$UP_TMP/home/update.json" 2>/dev/null
for _ in $(seq 1 30); do
	[ "$(up_field "$UP_TMP/home/update.json" notice_day)" = "$TODAY" ] && break
	sleep 0.1
done
if [ "$(up_field "$UP_TMP/home/update.json" last_check_day)" != "$TODAY" ]; then
	fail "last_check_day was not recorded as today"
fi
if [ "$(up_field "$UP_TMP/home/update.json" latest)" != "9.9.9" ]; then
	fail "latest was not recorded as 9.9.9 (got '$(up_field "$UP_TMP/home/update.json" latest)')"
fi
if [ "$(up_field "$UP_TMP/home/update.json" notice_day)" != "$TODAY" ]; then
	fail "notice_day was not recorded"
fi
if [ "$(up_field "$UP_TMP/home/update.json" enabled)" != "true" ]; then
	fail "enabled should read true after a default first launch"
fi
up_quit

# (b) The relaunch on the same config home the same day: no card a second
# time, and the /settings row toggles the switch into update.json — never
# settings.json.
tmux new-session -d -s "$S116" -x 100 -y 34 "$UP_BASE ALTER_ZERO_UPDATE_URL=http://127.0.0.1:$UP_PORT ALTER_ZERO_CONFIG_DIR=$UP_TMP/home $BIN"
sleep 1.5
up_second="$(tmux capture-pane -t "$S116" -p -S -40)"
dump "the relaunch (no second card today)" "$up_second"
if printf '%s' "$up_second" | grep -qF "$UP_CARD"; then
	fail "the relaunch repeated the card on the same day"
fi
up_open_row
up_menu="$(tmux capture-pane -t "$S116" -p)"
dump "/settings filtered to the Update check row" "$up_menu"
if ! printf '%s' "$up_menu" | grep -qE "Update check +true"; then
	fail "the Update check row did not read true"
fi
tmux send-keys -t "$S116" Space
sleep 0.5
up_toggled="$(tmux capture-pane -t "$S116" -p)"
dump "after Space on the Update check row" "$up_toggled"
if ! printf '%s' "$up_toggled" | grep -qF "Update check: false"; then
	fail "no 'Update check: false' toast after Space"
fi
if [ "$(up_field "$UP_TMP/home/update.json" enabled)" != "false" ]; then
	fail "update.json did not record enabled: false"
fi
if grep -q "update" "$UP_TMP/home/settings.json" 2>/dev/null; then
	fail "the toggle leaked into settings.json (it is a per-user knob)"
fi
tmux send-keys -t "$S116" Space
sleep 0.5
if [ "$(up_field "$UP_TMP/home/update.json" enabled)" != "true" ]; then
	fail "a second Space did not record enabled: true"
fi
tmux send-keys -t "$S116" Escape
sleep 0.3
up_quit

# (c) ALTER_ZERO_UPDATE_CHECK=0 on a fresh config home: nothing is checked,
# shown or written, and the row is UNAVAILABLE — a "send nothing" stated in
# the environment cannot be cycled around from inside the app.
tmux new-session -d -s "$S116" -x 100 -y 34 "$UP_BASE ALTER_ZERO_UPDATE_CHECK=0 ALTER_ZERO_UPDATE_URL=http://127.0.0.1:$UP_PORT ALTER_ZERO_CONFIG_DIR=$UP_TMP/off $BIN"
sleep 1.5
up_off="$(tmux capture-pane -t "$S116" -p -S -40)"
if printf '%s' "$up_off" | grep -qF "$UP_CARD"; then
	fail "ALTER_ZERO_UPDATE_CHECK=0 still showed the card"
fi
if [ -e "$UP_TMP/off/update.json" ]; then
	fail "ALTER_ZERO_UPDATE_CHECK=0 still wrote update.json"
fi
up_open_row
up_off_row="$(tmux capture-pane -t "$S116" -p)"
dump "the row under ALTER_ZERO_UPDATE_CHECK=0" "$up_off_row"
if ! printf '%s' "$up_off_row" | grep -qE "Update check +false \\(unavailable\\)"; then
	fail "the Update check row is not unavailable under ALTER_ZERO_UPDATE_CHECK=0"
fi
tmux send-keys -t "$S116" Escape
sleep 0.3
up_quit
up_stop

# (d) A repository whose newest release is this very version: the check
# runs and records it, and no card is shown — a build that is up to date
# (or ahead of the last release) is never nagged.
up_start "v$UP_VERSION"
tmux new-session -d -s "$S116" -x 100 -y 34 "$UP_BASE ALTER_ZERO_UPDATE_URL=http://127.0.0.1:$UP_PORT ALTER_ZERO_CONFIG_DIR=$UP_TMP/current $BIN"
for _ in $(seq 1 50); do
	[ "$(up_field "$UP_TMP/current/update.json" latest)" = "$UP_VERSION" ] && break
	sleep 0.1
done
sleep 0.5
up_current="$(tmux capture-pane -t "$S116" -p -S -40)"
dump "a launch against the current version" "$up_current"
if printf '%s' "$up_current" | grep -qF "$UP_CARD"; then
	fail "an up-to-date build was told to update"
fi
if [ "$(up_field "$UP_TMP/current/update.json" latest)" != "$UP_VERSION" ]; then
	fail "the check did not record the current version as latest"
fi
if [ "$(up_field "$UP_TMP/current/update.json" notice_day)" != "null" ]; then
	fail "notice_day was written although no card was due"
fi
up_quit

# (e) `alter-zero update` against the same up-to-date repository: says so
# and exits 0. Run from a COPY in its own bin dir — the checkout's own
# target/debug binary is refused below, as a cargo build directory.
UP_BIN="$UP_TMP/bin"
mkdir -p "$UP_BIN"
cp "$BIN_ABS" "$UP_BIN/alter-zero"
up_out="$(env $UP_BASE NO_PROXY=127.0.0.1 ALTER_ZERO_UPDATE_URL="http://127.0.0.1:$UP_PORT" ALTER_ZERO_CONFIG_DIR="$UP_TMP/current" "$UP_BIN/alter-zero" update 2>&1)"
up_rc=$?
note "alter-zero update when already newest (exit $up_rc)"
printf '%s\n' "$up_out"
if [ "$up_rc" != "0" ] || ! printf '%s' "$up_out" | grep -qF "is the newest release"; then
	fail "alter-zero update did not report the current version as newest (exit $up_rc)"
fi
up_stop

# (f) `alter-zero update` from a cargo build directory is refused with the
# checkout's own advice, whatever the repository says.
up_start v9.9.9
up_dev="$(env $UP_BASE ALTER_ZERO_UPDATE_URL="http://127.0.0.1:$UP_PORT" "$BIN_ABS" update 2>&1)"
up_dev_rc=$?
note "alter-zero update from target/debug (exit $up_dev_rc)"
printf '%s\n' "$up_dev"
if [ "$up_dev_rc" = "0" ] || ! printf '%s' "$up_dev" | grep -qF "cargo build directory"; then
	fail "a cargo build directory was not refused (exit $up_dev_rc)"
fi

# (g) `alter-zero update` from the copy, against v9.9.9: the installer runs
# over the copy's own directory, checksum verified, and the binary there is
# a new file that still runs. The fake release IS this binary, so the
# installer notes the version it reports rather than failing.
up_inode_before="$(ls -i "$UP_BIN/alter-zero" | awk '{ print $1 }')"
up_upd="$(env $UP_BASE NO_PROXY=127.0.0.1 ALTER_ZERO_UPDATE_URL="http://127.0.0.1:$UP_PORT" ALTER_ZERO_CONFIG_DIR="$UP_TMP/home" "$UP_BIN/alter-zero" update 2>&1)"
up_upd_rc=$?
note "alter-zero update to v9.9.9 (exit $up_upd_rc)"
printf '%s\n' "$up_upd"
if [ "$up_upd_rc" != "0" ]; then
	fail "alter-zero update exited $up_upd_rc"
fi
if ! printf '%s' "$up_upd" | grep -qF "v9.9.9 is out"; then
	fail "alter-zero update did not announce v9.9.9"
fi
if ! printf '%s' "$up_upd" | grep -qF "matches the published value"; then
	fail "the installer did not verify the checksum"
fi
if ! printf '%s' "$up_upd" | grep -qF "Installed"; then
	fail "the installer did not report the install"
fi
up_inode_after="$(ls -i "$UP_BIN/alter-zero" | awk '{ print $1 }')"
if [ "$up_inode_before" = "$up_inode_after" ]; then
	fail "the binary was not replaced (same inode $up_inode_after)"
fi
up_version_after="$("$UP_BIN/alter-zero" --version 2>&1)"
if [ "$up_version_after" != "alter-zero $UP_VERSION" ]; then
	fail "the updated binary does not run: '$up_version_after'"
fi
up_stop
rm -rf "$UP_TMP"
