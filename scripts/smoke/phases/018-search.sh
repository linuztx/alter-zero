#!/usr/bin/env bash
# Phase 18 — Ctrl+R reverse history search

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Ctrl+R reverse history search (docs/history-search.md). Build
# two history entries — one submitted turn plus one Ctrl+C-cleared draft (the
# clear records it, no second slow turn needed) — then: Ctrl+R opens the
# reverse-i-search line in the footer slot; typing a query previews the newest
# matching entry in the composer; Ctrl+R again steps to the older match; Enter
# accepts it (the search line closes, the session footer returns, the text
# stays as an editable draft); a hopeless query shows "no match" with the
# draft restored; and Esc closes the search WITHOUT quitting the app.
S15="${S}_search"
launch "$S15" 80 24
submit "$S15" "alpha bravo"
wait_for 20 "$S15" -E "^$SUMMARY_RE" # up to ~20s: the turn must finish first
tmux send-keys -t "$S15" -l "charlie alpha"
sleep 0.2
tmux send-keys -t "$S15" C-c
sleep 0.2
tmux send-keys -t "$S15" C-r
sleep 0.3
search_open="$(tmux capture-pane -t "$S15" -p)"
echo "==== captured visible screen (Ctrl+R pressed — search open, idle) ===="
printf '%s\n' "$search_open"
tmux send-keys -t "$S15" -l "alpha"
sleep 0.3
search_match="$(tmux capture-pane -t "$S15" -p -S -60)"
echo "==== captured pane (query 'alpha' typed — newest match previews) ===="
printf '%s\n' "$search_match"
tmux send-keys -t "$S15" C-r
sleep 0.3
search_older="$(tmux capture-pane -t "$S15" -p -S -60)"
echo "==== captured pane (Ctrl+R again — older match) ===="
printf '%s\n' "$search_older"
tmux send-keys -t "$S15" Enter
sleep 0.3
search_accept="$(tmux capture-pane -t "$S15" -p -S -60)"
echo "==== captured pane (Enter — match accepted as a draft) ===="
printf '%s\n' "$search_accept"
tmux send-keys -t "$S15" C-r
sleep 0.2
tmux send-keys -t "$S15" -l "zzz"
sleep 0.3
search_nomatch="$(tmux capture-pane -t "$S15" -p -S -60)"
echo "==== captured pane (query 'zzz' — no match) ===="
printf '%s\n' "$search_nomatch"
tmux send-keys -t "$S15" Escape
sleep 0.3
search_cancel="$(tmux capture-pane -t "$S15" -p -S -60)"
echo "==== captured visible screen (Esc — search cancelled, app still alive) ===="
printf '%s\n' "$search_cancel"
tmux kill-session -t "$S15" 2>/dev/null

expect_has "$search_open" -F "reverse-i-search:" "Ctrl+R did not open the reverse-i-search line"
expect_has "$search_match" -F "reverse-i-search: alpha" "the search line does not show the typed query"
if [ "$(count_msg_lines "$search_match" "charlie alpha")" != "1" ]; then
	fail "the newest match (the Ctrl+C-cleared draft) did not preview in the composer"
fi
if [ "$(count_msg_lines "$search_older" "alpha bravo")" != "2" ]; then
	fail "Ctrl+R again did not step the preview to the older match"
fi
expect_lacks "$search_accept" -F "reverse-i-search:" "accepting with Enter did not close the search line"
if [ "$(count_msg_lines "$search_accept" "alpha bravo")" != "2" ]; then
	fail "the accepted match did not stay in the composer as a draft"
fi
expect_has "$search_accept" -F "dummy_model_name ·" "the session footer did not return once the search closed"
expect_has "$search_nomatch" -F "no match" "a hopeless query does not show 'no match'"
if [ "$(count_msg_lines "$search_nomatch" "alpha bravo")" != "2" ]; then
	fail "the draft was not restored while the query has no match"
fi
expect_lacks "$search_cancel" -F "reverse-i-search:" "Esc did not close the search"
expect_has "$search_cancel" -F "dummy_model_name ·" "the app quit on Esc instead of only closing the search"
if [ "$(count_msg_lines "$search_cancel" "alpha bravo")" != "2" ]; then
	fail "Esc-cancel did not keep the restored draft"
fi
