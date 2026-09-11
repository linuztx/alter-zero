#!/usr/bin/env bash
# Phase 113 — the FILE TOOL HEADER'S PATH IS A file:// LINK

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the FILE TOOL HEADER'S PATH IS A file:// LINK (docs/links.md
# *The file tool header*). `● Read(about.py)` reads exactly as before, but the
# path's cells ride an OSC 8 hyperlink whose target is the file's ABSOLUTE
# `file://` URI — the whole file, wherever the row showed it from — so a click
# opens it; the `(`/`)` and the `⎿ Read N lines` corner row carry none. Like
# Phase 87 the assertion reads the RAW byte stream (pipe-pane): the escape
# paints as nothing, and the carrier must never leak as an SGR 58 colour.
S111="${S}_filelink"
FILELINK_RAW="$(mktemp "$SMOKE_TMP/filelink.XXXXXX")"
tmux new-session -d -s "$S111" -x 80 -y 40 "$APP"
tmux pipe-pane -t "$S111" -o "cat >> $FILELINK_RAW"
sleep 0.4
submit "$S111" "$USER_MSG"
wait_for 15 "$S111" -S -200 -- -F "$SETTLED_REPLY"
sleep 0.5
filelink_pane="$(tmux capture-pane -t "$S111" -p -S -200)"
echo "==== Phase 113: the default turn's Read/Edit cells (headers unchanged) ===="
printf '%s\n' "$filelink_pane" | grep -F "about.py" | head -6
# The visible rows are what they always were — the link is invisible.
for expect in "● Read(about.py)" "● Edit(about.py)"; do
	expect_has "$filelink_pane" -F "$expect" "'$expect' is missing from the pane"
done
# The raw stream carries the file's absolute URI. The dummy's `about.py` is a
# relative argument, so the target is the launch directory's own about.py —
# spelled exactly when the directory needs no percent-encoding, by shape
# otherwise.
filelink_url="file://$PWD/about.py"
case "$PWD" in
*[!A-Za-z0-9/._~-]*)
	if ! grep -aqE $']8;id=az[0-9]+;file:///[^\x1b]*/about\\.py\x1b' "$FILELINK_RAW"; then
		fail "no OSC 8 open carries a file:// URI ending in /about.py"
	fi
	;;
*)
	if ! grep -aqF ";${filelink_url}"$'\x1b' "$FILELINK_RAW"; then
		fail "no OSC 8 open carries ${filelink_url} in the raw stream"
	fi
	;;
esac
if ! grep -aqF $'\x1b]8;;\x1b' "$FILELINK_RAW"; then
	fail "the OSC 8 close is missing from the raw stream"
fi
if grep -aqF $'\x1b[58;' "$FILELINK_RAW"; then
	fail "the link-id carrier leaked as an SGR 58 underline colour"
fi
# What sits INSIDE a file:// link is the path and nothing else: the text run
# right after each open (past any cursor/style escapes) is `about.py`, never
# a `⎿` corner row or the `Read N lines` head.
filelink_runs="$(tr -d '\n' <"$FILELINK_RAW" | grep -aoE $']8;id=az[0-9]+;file://[^\x1b]*\x1b\\\\(\x1b\\[[0-9;?]*[A-Za-z])*[^\x1b]+' | sed -E $'s/.*\x1b\\\\//; s/\x1b\\[[0-9;?]*[A-Za-z]//g')"
echo "==== Phase 113: the text runs inside file:// links ===="
printf '%s\n' "$filelink_runs" | sort | uniq -c | head -6
expect_has "$filelink_runs" -F "about.py" "the path text is not what the file:// link wraps"
expect_lacks "$filelink_runs" -E '⎿|Read [0-9]+ lines|Updated' "a corner row rode inside the file:// link"
tmux kill-session -t "$S111" 2>/dev/null
rm -f "$FILELINK_RAW"
