//! Composer editing: pastes, image attachments, and the `!` shell mode
//! (`docs/paste.md`, `docs/image-paste.md`, `docs/shell-command.md`).

use super::*;

#[test]
fn typing_appends_characters_to_input() {
    let mut app = App::new();
    app.on_key(key(KeyCode::Char('h')));
    app.on_key(key(KeyCode::Char('i')));
    assert_eq!(app.input.text(), "hi");
}

#[test]
fn backspace_removes_last_character() {
    let mut app = App::new();
    app.input = TextArea::from_text("hi");
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(app.input.text(), "h");
}

#[test]
fn backspace_on_empty_input_is_harmless() {
    let mut app = App::new();
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(app.input.text(), "");
}

// ===== large-paste placeholders (docs/paste.md) =====

#[test]
fn large_paste_shows_a_placeholder_not_the_raw_text() {
    let mut app = App::new();
    let big = "x".repeat(crate::paste::LARGE_PASTE_CHAR_THRESHOLD + 1);
    app.on_paste(&big);
    assert_eq!(app.input.text(), "[Pasted Content 1001 chars]");
    assert_eq!(app.pasted.len(), 1, "the real text is remembered");
    assert_eq!(app.pasted[0].1, big);
}

#[test]
fn small_paste_inserts_inline() {
    let mut app = App::new();
    app.on_paste("just a little");
    assert_eq!(app.input.text(), "just a little");
    assert!(app.pasted.is_empty());
}

#[test]
fn paste_inserts_at_the_cursor() {
    let mut app = App::new();
    app.input = TextArea::from_text("ab");
    app.input.move_left(); // cursor between a and b
    app.on_paste("XY");
    assert_eq!(app.input.text(), "aXYb");
}

#[test]
fn paste_normalises_crlf_newlines() {
    let mut app = App::new();
    app.on_paste("a\r\nb");
    assert_eq!(app.input.text(), "a\nb", "CRLF becomes LF");
}

#[test]
fn paste_sanitises_tabs_to_spaces_in_the_composer() {
    // unicode-width counts '\t' as one column but ratatui renders zero
    // cells, so a tab in the composer drifts the hardware cursor one
    // column right of the text. Pastes are the only way a tab can get in
    // (the Tab key is intercepted); sanitise them to a space.
    let mut app = App::new();
    app.on_paste("ab\tcd");
    assert_eq!(app.input.text(), "ab cd");
}

#[test]
fn paste_sanitises_other_control_characters_too() {
    let mut app = App::new();
    app.on_paste("a\u{7f}b\u{1b}c");
    assert_eq!(app.input.text(), "a b c", "DEL and ESC become spaces");
}

#[test]
fn paste_keeps_newlines_while_sanitising() {
    let mut app = App::new();
    app.on_paste("a\t\nb");
    assert_eq!(app.input.text(), "a \nb", "'\\n' is the one control kept");
}

#[test]
fn large_paste_stores_the_raw_text_but_sends_it_verbatim() {
    // The placeholder path never renders the payload in the composer, so
    // the stored text keeps its tabs for send fidelity.
    let mut app = App::new();
    let big = format!(
        "x\t{}",
        "y".repeat(crate::paste::LARGE_PASTE_CHAR_THRESHOLD)
    );
    app.on_paste(&big);
    assert_eq!(app.pasted[0].1, big, "the remembered text is untouched");
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(action, Action::Submit(big), "expanded with the tab intact");
}

#[test]
fn large_paste_expands_to_full_text_on_submit() {
    let mut app = App::new();
    let big = "y".repeat(crate::paste::LARGE_PASTE_CHAR_THRESHOLD + 5);
    app.on_paste(&big);
    assert!(
        app.input.text().starts_with("[Pasted Content"),
        "the composer shows the placeholder"
    );
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(action, Action::Submit(big), "but the full text is sent");
    assert!(app.pasted.is_empty(), "consumed on submit");
}

#[test]
fn two_large_pastes_both_expand_on_submit() {
    let mut app = App::new();
    let a = "a".repeat(crate::paste::LARGE_PASTE_CHAR_THRESHOLD + 1);
    let b = "b".repeat(crate::paste::LARGE_PASTE_CHAR_THRESHOLD + 2);
    app.on_paste(&a);
    app.on_key(key(KeyCode::Char(' ')));
    app.on_paste(&b);
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(action, Action::Submit(format!("{a} {b}")));
}

#[test]
fn backspace_removes_a_whole_placeholder_atomically() {
    let mut app = App::new();
    let big = "z".repeat(crate::paste::LARGE_PASTE_CHAR_THRESHOLD + 1);
    app.on_paste(&big);
    assert_eq!(app.input.text(), "[Pasted Content 1001 chars]");
    // A single Backspace removes the entire placeholder, not one character.
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(app.input.text(), "");
    assert!(app.pasted.is_empty(), "the remembered paste is dropped too");
}

#[test]
fn delete_removes_a_whole_placeholder_atomically() {
    let mut app = App::new();
    let big = "z".repeat(crate::paste::LARGE_PASTE_CHAR_THRESHOLD + 1);
    app.on_paste(&big);
    app.on_key(key(KeyCode::Home)); // cursor to the placeholder's start
    app.on_key(key(KeyCode::Delete));
    assert_eq!(app.input.text(), "");
    assert!(app.pasted.is_empty());
}

#[test]
fn backspace_before_a_placeholder_deletes_only_the_preceding_char() {
    let mut app = App::new();
    app.on_key(key(KeyCode::Char('x')));
    let big = "z".repeat(crate::paste::LARGE_PASTE_CHAR_THRESHOLD + 1);
    app.on_paste(&big); // input: "x[Pasted Content 1001 chars]", cursor at end
    app.on_key(key(KeyCode::Home));
    app.on_key(key(KeyCode::Right)); // between 'x' and the placeholder
    app.on_key(key(KeyCode::Backspace)); // deletes 'x', placeholder intact
    assert_eq!(app.input.text(), "[Pasted Content 1001 chars]");
    assert_eq!(app.pasted.len(), 1, "the placeholder and its paste survive");
}

// ===== the kill keys stay placeholder-atomic (docs/textarea.md) =====

#[test]
fn ctrl_w_swallows_a_placeholder_whole() {
    // The placeholder text contains spaces, so a naive unix-word rubout from
    // its end would kill only "chars]" and leave half a placeholder backed by
    // a live pair. The kill widens over the occurrence instead.
    let mut app = App::new();
    let big = "z".repeat(crate::paste::LARGE_PASTE_CHAR_THRESHOLD + 1);
    app.on_paste(&big);
    assert_eq!(app.input.text(), "[Pasted Content 1001 chars]");
    app.on_key(ctrl('w'));
    assert_eq!(app.input.text(), "");
    assert!(app.pasted.is_empty(), "the remembered paste is dropped too");
}

#[test]
fn ctrl_u_swallows_every_placeholder_it_crosses() {
    let mut app = App::new();
    type_chars(&mut app, "see ");
    let big = "z".repeat(crate::paste::LARGE_PASTE_CHAR_THRESHOLD + 1);
    app.on_paste(&big);
    type_chars(&mut app, " ok");
    app.on_key(ctrl('u'));
    assert_eq!(app.input.text(), "");
    assert!(app.pasted.is_empty());
}

#[test]
fn a_kill_over_an_image_placeholder_discards_the_attachment() {
    // The killed image's pair goes with its placeholder, and the orphaned
    // temp file is handed to the boundary for removal — exactly what an
    // atomic Backspace does (docs/image-paste.md).
    let mut app = App::new();
    app.attach_image(PathBuf::from("/tmp/kill.png"));
    assert_eq!(app.input.text(), "[Image #1]");
    app.on_key(ctrl('u'));
    assert_eq!(app.input.text(), "");
    assert!(app.images.is_empty(), "the pair is dropped");
    assert_eq!(
        app.take_discarded_images(),
        vec![PathBuf::from("/tmp/kill.png")],
        "the temp file is queued for removal"
    );
}

#[test]
fn a_kill_leaves_placeholders_outside_its_span_backed() {
    // Two images; a ctrl+w from the end reaches only the second — the first
    // keeps its pair (and its temp file).
    let mut app = App::new();
    app.attach_image(PathBuf::from("/tmp/a.png"));
    type_chars(&mut app, " and ");
    app.attach_image(PathBuf::from("/tmp/b.png"));
    assert_eq!(app.input.text(), "[Image #1] and [Image #2]");
    app.on_key(ctrl('w'));
    assert_eq!(app.input.text(), "[Image #1] and ");
    assert_eq!(app.images.len(), 1, "only the killed pair went");
    assert_eq!(
        app.take_discarded_images(),
        vec![PathBuf::from("/tmp/b.png")]
    );
}

#[test]
fn a_kill_over_duplicate_placeholders_drops_the_right_pairs() {
    // A merged queue can leave two occurrences of the SAME placeholder text,
    // each backed by its own pair (docs/image-paste.md). Killing the second
    // occurrence must drop the second pair — and killing both must not trip
    // over its own ordinal bookkeeping (pairs are removed highest ordinal
    // first).
    let mut app = App::new();
    app.input = TextArea::from_text("[Image #1] x [Image #1]");
    app.images = vec![
        ("[Image #1]".to_string(), PathBuf::from("/tmp/first.png")),
        ("[Image #1]".to_string(), PathBuf::from("/tmp/second.png")),
    ];
    app.on_key(ctrl('w')); // kills the trailing occurrence
    assert_eq!(app.input.text(), "[Image #1] x ");
    assert_eq!(
        app.take_discarded_images(),
        vec![PathBuf::from("/tmp/second.png")],
        "the SECOND pair went, not the first"
    );
    app.on_key(ctrl('u')); // kills the rest, first occurrence included
    assert_eq!(app.input.text(), "");
    assert!(app.images.is_empty());
    assert_eq!(
        app.take_discarded_images(),
        vec![PathBuf::from("/tmp/first.png")]
    );
}

// ===== Ctrl+V image paste (docs/image-paste.md) =====

#[test]
fn record_user_message_with_images_attaches_the_paths() {
    // The submit path records the turn's attachments onto the user
    // message so the conversation context re-sends them (docs/context.md).
    let mut app = App::new();
    app.record_user_message_with_images("[Image #1] look", vec![PathBuf::from("/tmp/a.png")]);
    let Some(HistoryItem::Message(message)) = app.history.last() else {
        panic!("a user message was recorded");
    };
    assert_eq!(message.role, Role::User);
    assert_eq!(message.text, "[Image #1] look");
    assert_eq!(message.images, vec![PathBuf::from("/tmp/a.png")]);
}

#[test]
fn record_user_message_records_no_images() {
    let mut app = App::new();
    app.record_user_message("plain");
    let Some(HistoryItem::Message(message)) = app.history.last() else {
        panic!("a user message was recorded");
    };
    assert!(message.images.is_empty());
}

#[test]
fn ctrl_v_requests_an_image_paste() {
    let mut app = App::new();
    assert_eq!(app.on_key(ctrl('v')), Action::PasteImage);
}

#[test]
fn ctrl_alt_v_also_requests_an_image_paste() {
    // The WSL-friendly alias — codex binds both Ctrl+V and Ctrl+Alt+V.
    let mut app = App::new();
    let key = KeyEvent::new(
        KeyCode::Char('v'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    );
    assert_eq!(app.on_key(key), Action::PasteImage);
}

#[test]
fn attach_image_inserts_a_placeholder_and_records_the_path() {
    let mut app = App::new();
    app.attach_image(PathBuf::from("/tmp/a.png"));
    assert_eq!(app.input.text(), "[Image #1]");
    assert_eq!(app.images.len(), 1);
    assert_eq!(app.images[0].1, PathBuf::from("/tmp/a.png"));
}

#[test]
fn attach_image_numbers_each_attachment() {
    let mut app = App::new();
    app.attach_image(PathBuf::from("/tmp/a.png"));
    app.attach_image(PathBuf::from("/tmp/b.png"));
    assert_eq!(app.input.text(), "[Image #1][Image #2]");
    assert_eq!(app.images.len(), 2);
}

#[test]
fn attach_image_inserts_at_the_cursor() {
    let mut app = App::new();
    app.input = TextArea::from_text("ab");
    app.input.move_left(); // cursor between a and b
    app.attach_image(PathBuf::from("/tmp/a.png"));
    assert_eq!(app.input.text(), "a[Image #1]b");
}

#[test]
fn submitting_keeps_the_image_placeholder_and_surfaces_the_path() {
    // Unlike a text paste (expanded on send), an image placeholder STAYS in
    // the message text; the path travels the separate submission channel.
    let mut app = App::new();
    app.attach_image(PathBuf::from("/tmp/a.png"));
    for c in " describe".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(action, Action::Submit("[Image #1] describe".to_string()));
    assert_eq!(
        app.take_submission_images(),
        vec![("[Image #1]".to_string(), PathBuf::from("/tmp/a.png"))]
    );
}

#[test]
fn one_backspace_removes_the_whole_image_placeholder_and_drops_the_path() {
    let mut app = App::new();
    app.attach_image(PathBuf::from("/tmp/a.png")); // cursor at the end of it
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(app.input.text(), "");
    assert!(
        app.images.is_empty(),
        "the path is dropped with the placeholder"
    );
}

#[test]
fn clearing_the_draft_with_ctrl_c_drops_attached_images() {
    // Ctrl+C empties the composer; its attachments go too, so they can't leak
    // onto a later submit.
    let mut app = App::new();
    app.attach_image(PathBuf::from("/tmp/a.png"));
    app.on_key(ctrl('c'));
    assert!(app.images.is_empty());
    assert!(app.take_submission_images().is_empty());
}

// ===== discarded temp-PNG bookkeeping (docs/image-paste.md: the boundary
// deletes the files of attachments that will never be submitted) =====

#[test]
fn deleting_an_image_placeholder_discards_its_temp_path() {
    let mut app = App::new();
    app.attach_image(PathBuf::from("/tmp/a.png"));
    app.on_key(key(KeyCode::Backspace)); // atomic placeholder delete
    assert_eq!(
        app.take_discarded_images(),
        vec![PathBuf::from("/tmp/a.png")],
        "the orphaned temp file is handed to the boundary to remove"
    );
    assert!(app.take_discarded_images().is_empty(), "drained once");
}

#[test]
fn ctrl_c_clearing_a_draft_discards_its_image_paths() {
    let mut app = App::new();
    app.attach_image(PathBuf::from("/tmp/a.png"));
    app.on_key(ctrl('c'));
    assert_eq!(
        app.take_discarded_images(),
        vec![PathBuf::from("/tmp/a.png")]
    );
}

#[test]
fn clear_command_discards_the_queued_batches_image_paths() {
    // /clear wipes the queued backlog; the images riding those batches
    // will never dispatch, so their temp files must not leak.
    let mut app = App::new();
    app.begin_stream();
    app.attach_image(PathBuf::from("/tmp/q.png"));
    app.on_key(key(KeyCode::Enter)); // queue the draft mid-turn
    type_str(&mut app, "/clear");
    app.on_key(key(KeyCode::Enter)); // run the highlighted /clear
    assert!(app.queued.is_empty());
    assert_eq!(
        app.take_discarded_images(),
        vec![PathBuf::from("/tmp/q.png")]
    );
}

#[test]
fn submitted_and_queued_images_are_never_discarded() {
    // The idle submit stages its paths; a queued batch carries its own —
    // neither is a drop, so no file may be deleted underneath them.
    let mut app = App::new();
    app.attach_image(PathBuf::from("/tmp/sent.png"));
    app.on_key(key(KeyCode::Enter)); // idle submit
    assert!(app.take_discarded_images().is_empty());
    app.begin_stream();
    app.attach_image(PathBuf::from("/tmp/queued.png"));
    app.on_key(key(KeyCode::Enter)); // queue mid-turn
    assert!(app.drain_next_batch().is_some());
    assert!(app.take_discarded_images().is_empty());
}

#[test]
fn count_input_images_adds_to_the_up_tally() {
    let mut app = App::new();
    app.begin_stream();
    app.count_input_images(2);
    let status = app.status().unwrap();
    assert!(status.tokens > 0, "attached images are counted as input");
    assert_eq!(status.arrow, TokenArrow::Up, "uploaded input → ↑");
}

#[test]
fn record_error_message_appends_a_red_error_to_history() {
    // The loop records a Ctrl+V clipboard failure here (so it repaints on a
    // resize) before committing the red notice to scrollback.
    let mut app = App::new();
    app.record_error_message("Failed to paste image: no image on the clipboard");
    match app.history.last() {
        Some(HistoryItem::Message(m)) => {
            assert_eq!(m.role, Role::Error);
            assert!(m.text.starts_with("Failed to paste image"));
        }
        other => panic!("expected an error message, got {other:?}"),
    }
}

#[test]
fn interrupt_turn_undo_restores_the_submissions_image_attachments() {
    // An undone submission's Ctrl+V attachments come back with it: the
    // placeholders in the restored draft are backed again, so resubmitting
    // sends the images (docs/context.md).
    let mut app = App::new();
    app.attach_image(PathBuf::from("/tmp/a.png"));
    for c in " look".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(action, Action::Submit("[Image #1] look".to_string()));
    let paths = app
        .take_submission_images()
        .into_iter()
        .map(|(_, path)| path)
        .collect();
    app.record_user_message_with_images("[Image #1] look", paths);
    app.begin_stream();
    assert_eq!(app.interrupt_turn(), Some(InterruptedTurn::Undone));
    assert_eq!(app.input.text(), "[Image #1] look");
    assert_eq!(
        app.images,
        vec![("[Image #1]".to_string(), PathBuf::from("/tmp/a.png"))],
        "the placeholder is backed by its path again"
    );
}

#[test]
fn interrupt_turn_undo_rekeys_a_batchs_duplicate_placeholders_per_message() {
    // Placeholder numbering restarts per draft, so a merged batch can hold
    // two "[Image #1]"s with different paths. Recording stores each path
    // on its own message; the undo re-key zips per message, so neither
    // path is dropped or swapped (the review's duplicate-placeholder bug).
    let mut app = App::new();
    app.record_user_message_with_images("[Image #1] first", vec![PathBuf::from("/a.png")]);
    app.record_user_message_with_images("[Image #1] second", vec![PathBuf::from("/b.png")]);
    app.begin_stream();
    assert_eq!(app.interrupt_turn(), Some(InterruptedTurn::Undone));
    assert_eq!(app.input.text(), "[Image #1] first\n[Image #1] second");
    assert_eq!(
        app.images,
        vec![
            ("[Image #1]".to_string(), PathBuf::from("/a.png")),
            ("[Image #1]".to_string(), PathBuf::from("/b.png")),
        ],
        "both duplicate-named pairs survive, in message order"
    );
    assert!(
        app.take_discarded_images().is_empty(),
        "nothing leaks to the discard list"
    );
}

#[test]
fn queueing_a_draft_mid_turn_carries_its_image_attachments() {
    // A mid-turn Enter must not silently drop a Ctrl+V attachment: the
    // (placeholder, path) pairs ride with the batch and dispatch when its
    // turn comes (docs/image-paste.md).
    let mut app = App::new();
    app.begin_stream();
    app.attach_image(PathBuf::from("/tmp/img1.png"));
    for c in " describe".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.on_key(key(KeyCode::Enter));
    assert!(app.images.is_empty(), "the attachment left the composer");
    assert_eq!(
        app.drain_next_batch(),
        Some(QueuedTurn::Messages {
            texts: vec!["[Image #1] describe".to_string()],
            images: vec![("[Image #1]".to_string(), PathBuf::from("/tmp/img1.png"))],
        })
    );
}

#[test]
fn a_tab_follow_up_batch_carries_its_images_too() {
    let mut app = App::new();
    app.begin_stream();
    app.attach_image(PathBuf::from("/tmp/pic.png"));
    app.on_key(key(KeyCode::Tab));
    assert_eq!(
        app.drain_next_batch(),
        Some(QueuedTurn::Messages {
            texts: vec!["[Image #1]".to_string()],
            images: vec![("[Image #1]".to_string(), PathBuf::from("/tmp/pic.png"))],
        })
    );
}

#[test]
fn merging_enters_merges_their_images_in_order() {
    let mut app = App::new();
    app.begin_stream();
    app.attach_image(PathBuf::from("/a.png"));
    app.on_key(key(KeyCode::Enter)); // batch 1, first message
    app.attach_image(PathBuf::from("/b.png"));
    app.on_key(key(KeyCode::Enter)); // appends to batch 1
    match app.drain_next_batch() {
        Some(QueuedTurn::Messages { texts, images }) => {
            assert_eq!(texts.len(), 2, "one merged batch");
            let paths: Vec<_> = images.into_iter().map(|(_, p)| p).collect();
            assert_eq!(
                paths,
                vec![PathBuf::from("/a.png"), PathBuf::from("/b.png")]
            );
        }
        other => panic!("expected the merged batch, got {other:?}"),
    }
}

#[test]
fn alt_up_restores_a_queued_batchs_images_to_the_composer() {
    // The pull-back re-attaches the batch's images so the placeholders in
    // the restored draft are backed again — an idle re-submit stages them.
    let mut app = App::new();
    app.begin_stream();
    app.attach_image(PathBuf::from("/tmp/pic.png"));
    app.on_key(key(KeyCode::Enter)); // queue it
    app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT));
    assert_eq!(app.input.text(), "[Image #1]");
    assert_eq!(
        app.images,
        vec![("[Image #1]".to_string(), PathBuf::from("/tmp/pic.png"))]
    );
    app.finish_stream();
    app.end_turn(1); // the turn ends; the composer is idle again
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Submit("[Image #1]".to_string())
    );
    assert_eq!(
        app.take_submission_images(),
        vec![("[Image #1]".to_string(), PathBuf::from("/tmp/pic.png"))]
    );
}

#[test]
fn typing_filters_and_clamps_the_selection() {
    // Seat the highlight on a NON-zero row first, then narrow the filter
    // past it — the refresh must pull the selection back in bounds, or
    // the highlight (and Enter) lands on nothing.
    let mut app = App::new();
    type_str(&mut app, "/c");
    assert_eq!(matching_commands("c").len(), 3, "/clear, /copy, /compact");
    app.on_key(key(KeyCode::Down)); // highlight /copy (index 1)
    assert_eq!(app.command_menu.as_ref().unwrap().selected, 1);
    type_str(&mut app, "l"); // "/cl" — only /clear matches now
    assert_eq!(matching_commands("cl").len(), 1, "only /clear matches");
    assert_eq!(app.command_menu.as_ref().unwrap().selected, 0, "clamped");
    assert_eq!(
        app.highlighted_command().map(|c| c.name),
        Some("clear"),
        "the highlight lands on a real row, so Enter still runs something"
    );
}

#[test]
fn typing_builds_the_query_and_previews_the_newest_match() {
    let mut app = searchable_app(&["git status", "cargo build", "git push"]);
    app.on_key(ctrl('r'));
    type_query(&mut app, "git");
    let search = app.history_search.as_ref().expect("search open");
    assert_eq!(search.query, "git");
    assert_eq!(search.state, SearchState::Match { selected: 0 });
    assert_eq!(app.input.text(), "git push", "newest match previews");
}

#[test]
fn a_paste_mid_search_extends_the_query_not_the_preview() {
    // The search owns *every* key — a bracketed paste is input too, so it
    // extends the query (readline's paste-into-isearch) instead of editing
    // the previewed match, whose text the next rerun/cancel would discard.
    let mut app = searchable_app(&["git status", "git push"]);
    app.on_key(ctrl('r'));
    app.on_paste("git st");
    let search = app.history_search.as_ref().expect("search stays open");
    assert_eq!(search.query, "git st");
    assert_eq!(search.state, SearchState::Match { selected: 0 });
    assert_eq!(app.input.text(), "git status", "the match previews");
}

#[test]
fn a_paste_mid_search_never_flips_shell_mode() {
    // begin_history_search suspends shell mode; a pasted `!` must not
    // re-enter it through on_paste's sync_shell_mode while the search owns
    // the composer.
    let mut app = searchable_app(&["!ls"]);
    app.on_key(ctrl('r'));
    app.on_paste("!ls");
    assert!(!app.shell_mode, "the search owns the composer");
    let search = app.history_search.as_ref().expect("search open");
    assert_eq!(search.query, "!ls");
    assert_eq!(app.input.text(), "!ls", "previewed raw, bang and all");
}

#[test]
fn a_paste_mid_search_never_opens_the_file_picker() {
    let mut app = searchable_app(&["look at @src/app.rs please"]);
    app.on_key(ctrl('r'));
    app.on_paste("@src");
    assert!(app.file_search.is_none(), "the search owns the band slot");
}

#[test]
fn a_large_paste_mid_search_leaves_no_orphaned_placeholder() {
    let mut app = searchable_app(&["hello"]);
    app.on_key(ctrl('r'));
    let big = "z".repeat(crate::paste::LARGE_PASTE_CHAR_THRESHOLD + 1);
    app.on_paste(&big);
    assert!(app.pasted.is_empty(), "no placeholder pair is recorded");
    assert!(
        !app.input.text().starts_with("[Pasted Content"),
        "no placeholder lands in the preview"
    );
}

#[test]
fn a_paste_mid_search_flattens_control_characters_into_the_query() {
    // The query renders on a single footer row — a pasted newline or tab
    // would corrupt it (and '\t' breaks the cursor math, like the composer).
    let mut app = searchable_app(&["a b"]);
    app.on_key(ctrl('r'));
    app.on_paste("a\tb\nc");
    let search = app.history_search.as_ref().expect("search open");
    assert_eq!(search.query, "a b c");
}

#[test]
fn typing_a_bang_into_an_empty_composer_enters_shell_mode() {
    // codex's absorbed prefix: the `!` flips the mode flag and is *not*
    // inserted — the prompt renders it instead (`! pwd`, not `❯ !pwd`).
    let mut app = App::new();
    type_query(&mut app, "!pwd");
    assert!(app.shell_mode);
    assert_eq!(app.input.text(), "pwd", "the bang is absorbed, not typed");
}

#[test]
fn a_bang_typed_mid_text_is_just_a_character() {
    let mut app = App::new();
    type_query(&mut app, "hi!");
    assert!(!app.shell_mode);
    assert_eq!(app.input.text(), "hi!");
}

#[test]
fn backspace_on_an_empty_shell_composer_exits_shell_mode() {
    let mut app = App::new();
    type_query(&mut app, "!");
    app.on_key(key(KeyCode::Backspace));
    assert!(!app.shell_mode, "backspace deletes the absorbed bang");
    assert_eq!(app.input.text(), "");
}

#[test]
fn an_empty_bang_posts_the_help_notice_and_stays_in_the_mode() {
    let mut app = App::new();
    type_query(&mut app, "!");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Notice(SHELL_EMPTY_NOTICE.to_string())
    );
    assert!(
        app.shell_mode,
        "codex keeps the mode open on the help notice"
    );
    type_query(&mut app, "   ");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Notice(SHELL_EMPTY_NOTICE.to_string())
    );
}

#[test]
fn enter_restores_the_rewound_messages_image_attachments() {
    // The rewound message's Ctrl+V attachments come back with its text —
    // the interrupt-undo dance — so resubmitting still sends the image;
    // and the dropped *later* user message's attachment is orphaned, so
    // its temp file is queued for deletion.
    let mut app = App::new();
    app.record_user_message_with_images("[Image #1] look", vec![PathBuf::from("/a.png")]);
    app.begin_stream();
    app.push_chunk("a reply");
    app.finish_stream();
    app.end_turn(1);
    app.record_user_message_with_images("[Image #1] later", vec![PathBuf::from("/b.png")]);
    app.begin_stream();
    app.push_chunk("b reply");
    app.finish_stream();
    app.end_turn(1);
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Esc)); // preview on the later message
    app.on_key(key(KeyCode::Esc)); // step to the first
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.input.text(), "[Image #1] look");
    assert_eq!(
        app.images,
        vec![("[Image #1]".to_string(), PathBuf::from("/a.png"))],
        "the placeholder is backed by its path again"
    );
    assert_eq!(
        app.take_discarded_images(),
        vec![PathBuf::from("/b.png")],
        "the dropped later message's temp file is queued for deletion"
    );
}

#[test]
fn typing_filters_the_rows_and_reseats_the_selection() {
    let mut app = picker_app(&[("a", "wrap bug"), ("b", "shell fun"), ("c", "WRAP fix")]);
    app.on_key(key(KeyCode::Down)); // move off the top first
    type_chars(&mut app, "wrap");
    let picker = app.resume_picker.as_ref().unwrap();
    assert_eq!(picker.query, "wrap");
    assert_eq!(picker.selected, 0, "a query edit reseats the selection");
    // Case-insensitive substring over the preview (codex's matches_query).
    let matches = picker.matches();
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0].preview, "wrap bug");
    assert_eq!(matches[1].preview, "WRAP fix");
}

#[test]
fn backspace_pops_the_query() {
    let mut app = picker_app(&[("a", "wrap bug")]);
    type_chars(&mut app, "wx");
    app.on_key(key(KeyCode::Backspace));
    let picker = app.resume_picker.as_ref().unwrap();
    assert_eq!(picker.query, "w");
    assert_eq!(picker.matches().len(), 1, "the widened query matches again");
}

#[test]
fn a_paste_joins_the_picker_search_query_flattened() {
    // Codex normalizes a pasted search query: whitespace runs collapse to
    // single spaces, a non-empty query gains a separating space, and a
    // whitespace-only paste is ignored.
    let mut app = picker_app(&[("a", "wrap bug"), ("b", "other")]);
    app.on_key(key(KeyCode::Down));
    app.paste_into_resume_search("wrap\n   bug");
    let picker = app.resume_picker.as_ref().unwrap();
    assert_eq!(picker.query, "wrap bug");
    assert_eq!(picker.selected, 0, "a query edit reseats the selection");
    app.paste_into_resume_search("  \n ");
    assert_eq!(app.resume_picker.as_ref().unwrap().query, "wrap bug");
    app.paste_into_resume_search("fix");
    assert_eq!(app.resume_picker.as_ref().unwrap().query, "wrap bug fix");
}

#[test]
fn typing_reseats_the_selection_to_the_top() {
    let mut app = model_app(&sample_models());
    app.model_picker.as_mut().unwrap().selected = 2;
    type_chars(&mut app, "anthropic");
    assert_eq!(app.model_picker.as_ref().unwrap().selected, 0);
}

#[test]
fn typing_filters_providers_case_insensitively() {
    let mut app = login_app();
    type_chars(&mut app, "OPEN");
    let onboarding = app.key_onboarding.as_ref().unwrap();
    assert_eq!(onboarding.matches().len(), 1);
    assert_eq!(onboarding.matches()[0].id, "openrouter");
}

#[test]
fn key_step_types_and_backspaces_the_key() {
    let mut app = key_app("openrouter");
    type_chars(&mut app, "sk-abc");
    assert_eq!(app.key_onboarding.as_ref().unwrap().key_input, "sk-abc");
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(app.key_onboarding.as_ref().unwrap().key_input, "sk-ab");
}

#[test]
fn paste_into_the_key_step_strips_whitespace_and_newlines() {
    let mut app = key_app("openrouter");
    app.paste_into_key_onboarding("sk-abc\n  def\t");
    assert_eq!(app.key_onboarding.as_ref().unwrap().key_input, "sk-abcdef");
}

#[test]
fn paste_into_the_provider_step_extends_the_filter() {
    let mut app = login_app();
    app.paste_into_key_onboarding("open router");
    assert_eq!(app.key_onboarding.as_ref().unwrap().query, "open router");
}

#[test]
fn deleting_one_duplicate_named_placeholder_keeps_the_other_pair() {
    // An undone batch can hold two "[Image #1]"s with different paths
    // (numbering restarts per draft). Backspacing over ONE occurrence
    // must drop only its own pair — not every pair sharing the name,
    // which deleted the temp file backing the occurrence still in the
    // composer (docs/image-paste.md).
    let mut app = App::new();
    app.record_user_message_with_images("[Image #1] first", vec![PathBuf::from("/a.png")]);
    app.record_user_message_with_images("[Image #1] second", vec![PathBuf::from("/b.png")]);
    app.begin_stream();
    assert_eq!(app.interrupt_turn(), Some(InterruptedTurn::Undone));
    // Seat the cursor right after the SECOND [Image #1] and Backspace it.
    let text = app.input.text().to_string();
    let second_end = text.rfind("[Image #1]").unwrap() + "[Image #1]".len();
    app.input.set_text_with_cursor(&text, second_end);
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(app.input.text(), "[Image #1] first\n second");
    assert_eq!(
        app.images,
        vec![("[Image #1]".to_string(), PathBuf::from("/a.png"))],
        "the first occurrence's pair survives"
    );
    assert_eq!(
        app.take_discarded_images(),
        vec![PathBuf::from("/b.png")],
        "only the deleted occurrence's temp file is discarded"
    );
}

#[test]
fn accepting_a_search_match_discards_the_replaced_drafts_attachments() {
    // Enter on a Ctrl+R match replaces the pre-search draft; the pairs
    // that backed it are unanchored and must be discarded — otherwise the
    // stale attachment silently rides the next submission
    // (paste::distribute_images parks unclaimed pairs on the first text).
    let mut app = App::new();
    submit(&mut app, "old entry");
    app.attach_image(PathBuf::from("/tmp/img.png"));
    app.on_key(ctrl('r'));
    type_query(&mut app, "old");
    app.on_key(key(KeyCode::Enter)); // accept the previewed match
    assert_eq!(app.input.text(), "old entry");
    assert!(app.images.is_empty(), "no unanchored pairs survive");
    assert_eq!(
        app.take_discarded_images(),
        vec![PathBuf::from("/tmp/img.png")]
    );
}

#[test]
fn cancelling_a_search_discards_an_image_attached_while_it_was_open() {
    // A Ctrl+V decode finishing while the search owns the composer lands
    // its placeholder in the preview text; the snapshot restore drops the
    // text, so the pair must go too — not linger invisibly and ride the
    // next submission.
    let mut app = App::new();
    submit(&mut app, "old entry");
    app.on_key(ctrl('r'));
    app.attach_image(PathBuf::from("/tmp/late.png"));
    app.on_key(key(KeyCode::Esc)); // cancel → snapshot (empty draft) restored
    assert_eq!(app.input.text(), "");
    assert!(app.images.is_empty());
    assert_eq!(
        app.take_discarded_images(),
        vec![PathBuf::from("/tmp/late.png")]
    );
}

#[test]
fn cancelling_a_search_keeps_the_snapshot_drafts_attachments() {
    let mut app = App::new();
    submit(&mut app, "old entry");
    app.attach_image(PathBuf::from("/tmp/keep.png"));
    app.on_key(ctrl('r'));
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.input.text(), "[Image #1]");
    assert_eq!(
        app.images,
        vec![("[Image #1]".to_string(), PathBuf::from("/tmp/keep.png"))],
        "the restored draft's own pair stays backed"
    );
    assert!(app.take_discarded_images().is_empty());
}
