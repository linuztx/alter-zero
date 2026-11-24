#!/usr/bin/env bash
# Phase 169 — /copy and /export reach the user's terminal from tmux on a remote host
#
# The reported setup: alter-zero inside tmux on a VPS, the user's own
# terminal at the far end of an SSH login. tmux's defaults
# (`set-clipboard external`, `allow-passthrough off`) drop an OSC 52 that an
# application writes, so a copy that leans on one never leaves the server:
# `/copy` and `/export`'s clipboard row toasted "Copied" while the user's
# clipboard stayed as it was. Inside tmux the copy now hands the text to tmux
# itself (`tmux load-buffer -w -`), which keeps it in its own buffer and
# sends it on to its client's terminal (docs/copy.md).
#
# Two tmux servers play it. The phase's own server is the user's terminal,
# with `set-clipboard on`, so whatever OSC 52 reaches it lands in its buffer,
# where `show-buffer` reads it back. A second server, on a socket of its own,
# is the VPS's tmux, left at tmux's defaults but for its status line (a row
# the app would otherwise lose). Its client is attached inside the outer pane
# as an xterm, the terminal type tmux's default `terminal-features` grants
# the clipboard, and the app runs in its pane with an SSH_CONNECTION, as an
# SSH login gives it. The check that matters is the OUTER buffer: the text
# made it out of the remote tmux to the terminal the user pastes from.

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

S169="${S}_vps"
VPS_SOCK="$SMOKE_TMP/vps.sock"
VPS_CONF="$SMOKE_TMP/vps.conf"
printf 'set -g status off\n' >"$VPS_CONF"
# The remote host's tmux server.
vps() { command tmux -S "$VPS_SOCK" "$@"; }
smoke_on_exit vps kill-server
# What reached the user's terminal as an OSC 52.
outer_holds() { tmux show-buffer 2>/dev/null | grep -qF -- "$1"; }
# What the remote tmux keeps in its own paste buffer.
vps_holds() { vps show-buffer 2>/dev/null | grep -qF -- "$1"; }
clear_buffers() {
	while tmux delete-buffer 2>/dev/null; do :; done
	while vps delete-buffer 2>/dev/null; do :; done
}
# Whether the tmux on PATH is MAJOR.MINOR or later. A build that names no
# number (one from master) counts as new.
tmux_at_least() {
	[[ "$(command tmux -V)" =~ ([0-9]+)\.([0-9]+) ]] || return 0
	((BASH_REMATCH[1] > $1 || (BASH_REMATCH[1] == $1 && BASH_REMATCH[2] >= $2)))
}

VPS_APP="env SSH_CONNECTION='198.51.100.7 52144 203.0.113.10 22' $APP_ABS"
launch "$S169" 100 30 "env -u TMUX -u TMUX_PANE TERM=xterm-256color tmux -u -S $VPS_SOCK -f $VPS_CONF new-session -s vps \"$VPS_APP\""
tmux set-option -g set-clipboard on
# The remote tmux is at tmux's own defaults, the ones the report was made
# under — neither setting lets an application's OSC 52 through.
# (`allow-passthrough` arrived in tmux 3.3; an older tmux has no such option.)
expect_eq "$(vps show-options -gv set-clipboard)" "external" "the remote tmux is not at its default set-clipboard"
passthrough="$(vps show-options -gv allow-passthrough 2>/dev/null)"
[ -z "$passthrough" ] || expect_eq "$passthrough" "off" "the remote tmux is not at its default allow-passthrough"

submit "$S169" "hello there"
wait_settled 20 "$S169" -F "$SETTLED_REPLY" >/dev/null || fail "the reply never settled inside the remote tmux"

# /copy: the toast, and the reply in the user's terminal's clipboard.
clear_buffers
type_text "$S169" "/copy"
sleep 0.3
keys "$S169" Enter
copied="$(wait_pane 3 "$S169" -F "Copied last message to clipboard")"
dump "after /copy, inside the remote tmux" "$copied"
expect_has "$copied" -F "Copied last message to clipboard" "/copy did not confirm the copy"
expect_lacks "$copied" -F "52;c;" "the clipboard escape was painted on screen"
if ! poll 5 outer_holds "$SETTLED_REPLY"; then
	fail "/copy never reached the user's terminal: the remote tmux kept it to itself"
fi
dump "the user's terminal's clipboard after /copy" "$(tmux show-buffer 2>/dev/null)"
# `load-buffer -w` arrived in tmux 3.2. An older tmux takes the copy through
# the escape fallback, which reaches the terminal but leaves no buffer behind.
if tmux_at_least 3 2; then
	poll 3 vps_holds "$SETTLED_REPLY" || fail "/copy did not leave the reply in the remote tmux's own buffer"
else
	note "$(command tmux -V) has no load-buffer -w: the escape fallback carried the copy"
fi

# /export's clipboard row: the whole conversation makes the same trip.
clear_buffers
type_text "$S169" "/export"
sleep 0.3
keys "$S169" Enter
wait_pane 3 "$S169" -F "Export conversation" >/dev/null || fail "/export did not open its page"
keys "$S169" Enter
exported="$(wait_pane 3 "$S169" -F "Copied the conversation to clipboard")"
dump "after /export's clipboard row" "$exported"
expect_has "$exported" -F "Copied the conversation to clipboard" "/export's clipboard row did not confirm the copy"
if ! poll 5 outer_holds "❯ hello there"; then
	fail "/export's clipboard row never reached the user's terminal"
fi
outer="$(tmux show-buffer 2>/dev/null)"
dump "the user's terminal's clipboard after /export" "$outer"
expect_has "$outer" -F "$SETTLED_REPLY" "the exported conversation in the user's clipboard lacks the reply"
expect_has "$outer" -F "$HEADER_MARK (v" "the exported conversation in the user's clipboard lacks the startup banner"
