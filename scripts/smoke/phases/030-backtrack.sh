#!/usr/bin/env bash
# Phase 30 — Esc-Esc BACKTRACK

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Esc-Esc BACKTRACK (docs/backtrack.md). After two finished
# exchanges, the first idle Esc ARMS the gesture (the footer slot shows the
# "esc again to edit previous message" hint instead of quitting), the second
# opens the transcript overlay as a preview (backtrack key hints replace the
# quit hint), a further Esc steps the highlight to the OLDER user message, and
# Enter REWINDS: back inline, the conversation truncated from that message on
# (here: everything — it was the first), its text back in the composer to
# edit. Resubmitting it must stream a fresh turn to its summary
# ($SUMMARY_TURN3 — turn 3's, each turn staying under 30s and so ending on the
# verb it opened on), proving the loop survived the rewind.
S30="${S}_backtrack"
launch "$S30" 80 24
submit "$S30" "alpha question"
wait_for 20.1 "$S30" -S -40 -- -F "$SUMMARY_TURN1" # up to ~20s: turn 1 runs to its summary
submit "$S30" "beta question"
wait_for 20.1 "$S30" -S -40 -- -F "$SUMMARY_TURN2" # turn 2 → its summary
tmux send-keys -t "$S30" Escape # arm the gesture
sleep 0.3
backtrack_armed="$(tmux capture-pane -t "$S30" -p)"
echo "==== captured pane (first Esc — backtrack armed, hint in the footer slot) ===="
printf '%s\n' "$backtrack_armed"
tmux send-keys -t "$S30" Escape # open the transcript preview
sleep 0.4
backtrack_preview="$(tmux capture-pane -t "$S30" -p)"
echo "==== captured pane (second Esc — transcript preview with backtrack hints) ===="
printf '%s\n' "$backtrack_preview"
tmux send-keys -t "$S30" Escape # step older: "beta question" → "alpha question"
sleep 0.3
tmux send-keys -t "$S30" Enter # confirm the rewind
sleep 0.6
backtrack_rewound="$(tmux capture-pane -t "$S30" -p)"
echo "==== captured pane (Enter — rewound, the first message back in the composer) ===="
printf '%s\n' "$backtrack_rewound"
# The rewind PURGES scrollback (docs/backtrack.md — like /resume/resize): the
# dropped exchange must not linger even in the terminal's scrollback (the
# duplication bug where it only cleared on the next resize). Capture WITH
# scrollback and assert "beta question" is gone entirely — the composer holds
# "alpha question", so "beta question" must appear zero times anywhere.
backtrack_rewound_scroll="$(tmux capture-pane -t "$S30" -p -S -120)"
tmux send-keys -t "$S30" Enter # resubmit the recalled draft
backtrack_resent="$(wait_pane 30 "$S30" -S -40 -- -F "$SUMMARY_TURN3")" # up to ~30s: turn 3 → its summary
echo "==== captured pane (rewound message resubmitted — a fresh turn streamed) ===="
printf '%s\n' "$backtrack_resent"
tmux kill-session -t "$S30" 2>/dev/null

# Phase 30: Esc-Esc backtrack — arm, preview, step, rewind, resubmit.
expect_has "$backtrack_armed" -F "esc again to edit previous message" "the first idle Esc did not show the backtrack hint in the footer slot (did the app quit?)"
expect_has "$backtrack_preview" -F "T R A N S C R I P T" "the second Esc did not open the transcript overlay as the backtrack preview"
expect_has "$backtrack_preview" -F "enter to edit message" "the preview's key-hint row does not show the backtrack hints"
expect_has "$backtrack_rewound" -F "❯ alpha question" "after Enter the composer does not hold the rewound first message"
expect_lacks "$backtrack_rewound" -F "beta question" "the second exchange survived the rewind on the repainted screen"
# The dropped exchange must be gone from SCROLLBACK too, not just the visible
# screen — an in-place overwrite left it lingering above the fold until the next
# resize (the duplication bug). A Purge-rebuild clears scrollback, so nothing
# should scroll back to "beta question" (the composer holds "alpha question").
expect_lacks "$backtrack_rewound_scroll" -F "beta question" "the rewound exchange lingered in scrollback (backtrack must Purge-rebuild, not overwrite in place)"
expect_has "$backtrack_resent" -F "$SUMMARY_TURN3" "resubmitting the rewound message never streamed to turn 3's '$SUMMARY_TURN3 …' summary"
