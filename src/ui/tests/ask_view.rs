//! The `AskUserQuestion` modal's renderer (`docs/ask.md`): the chip strip,
//! the option pages, the preview panel, the Submit review page, and the
//! resolved cell's headline header.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::super::theme::ask_chip_current_bg;
use super::*;
use crate::ask::{AskOption, AskQuestion, AskRequest};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn option(label: &str, desc: &str) -> AskOption {
    AskOption {
        label: label.to_string(),
        description: desc.to_string(),
        preview: None,
    }
}

fn coffee_question() -> AskQuestion {
    AskQuestion {
        question: "What's your favorite way to drink coffee?".to_string(),
        header: "Coffee style".to_string(),
        options: vec![
            option("Black", "No milk, no sugar — just coffee"),
            option("Latte", "Espresso with steamed milk"),
            option("Cold brew", "Slow-steeped, served cold"),
        ],
        multi_select: false,
    }
}

fn topics_question() -> AskQuestion {
    AskQuestion {
        question: "Which of these tool features would you like to see demoed next?".to_string(),
        header: "Demo topics".to_string(),
        options: vec![
            option("Preview panel", "Side-by-side layout"),
            option("Custom 'Other' input", "Free-text answers"),
        ],
        multi_select: true,
    }
}

fn preview_question() -> AskQuestion {
    AskQuestion {
        question: "Which code style do you prefer?".to_string(),
        header: "Code style".to_string(),
        options: vec![
            AskOption {
                label: "Arrow function".to_string(),
                description: "Modern".to_string(),
                preview: Some("const greet = (name) => {\n  return `Hello`;\n};".to_string()),
            },
            AskOption {
                label: "One-liner".to_string(),
                description: "Terse".to_string(),
                preview: Some("const greet = (n) => `Hello, ${n}!`;".to_string()),
            },
        ],
        multi_select: false,
    }
}

fn open(app: &mut App, questions: Vec<AskQuestion>) {
    app.open_ask(AskRequest {
        id: "ask_0".to_string(),
        questions,
    });
}

/// A terminal tall enough for the whole page.
const TALL: u16 = 200;

fn all_text(lines: &[Line]) -> String {
    lines.iter().map(plain).collect::<Vec<_>>().join("\n")
}

#[test]
fn the_modal_frames_the_page_with_rules_chips_question_options_and_hints() {
    let mut app = App::new();
    open(&mut app, vec![coffee_question(), topics_question()]);
    let lines = ask_lines(&app, 80, TALL);
    let text = all_text(&lines);
    // The frame: full-width rules top and bottom.
    assert!(plain(&lines[0]).starts_with("──"), "a top rule");
    assert!(
        plain(lines.last().unwrap()).starts_with("──"),
        "a bottom rule"
    );
    // The chip strip: both questions unanswered (☐), the Submit tab, the
    // ←/→ bookends.
    assert!(text.contains("☐ Coffee style"), "got:\n{text}");
    assert!(text.contains("☐ Demo topics"), "got:\n{text}");
    assert!(text.contains("✔ Submit"), "got:\n{text}");
    assert!(text.contains('←') && text.contains('→'));
    // The question, the numbered options with their descriptions, the Other
    // row, and Chat about this.
    assert!(text.contains("What's your favorite way to drink coffee?"));
    assert!(text.contains("1. Black"));
    assert!(text.contains("No milk, no sugar — just coffee"));
    assert!(text.contains("4. Type something."));
    assert!(text.contains("5. Chat about this"));
    // The hint row.
    assert!(text.contains("Enter"), "got:\n{text}");
    assert!(text.contains("Esc"), "got:\n{text}");
    // The `❯` marks the highlighted first row.
    assert!(text.contains("❯ 1. Black"), "got:\n{text}");
}

#[test]
fn every_page_hugs_its_frame() {
    // The chip strip straight under the opening rule, the hint row straight
    // over the closing one — a lone question, a set of them, the preview
    // layout, an open entry field, the review page, and with the idle
    // clock's chip riding the closing rule.
    let with = |questions: Vec<AskQuestion>| {
        let mut app = App::new();
        open(&mut app, questions);
        app
    };
    let mut entry = with(vec![coffee_question()]);
    entry.on_key(key(KeyCode::Char('4'))); // the Other row
    let mut notes = with(vec![preview_question()]);
    notes.on_key(key(KeyCode::Char('n')));
    let mut review = with(vec![coffee_question(), topics_question()]);
    review.on_key(key(KeyCode::Char('2'))); // Latte → advance
    review.on_key(key(KeyCode::Right)); // → Submit page
    let mut timed = with(vec![coffee_question()]);
    timed.set_ask_remaining(Some(std::time::Duration::from_secs(581)));
    for (what, app) in [
        ("a lone question", with(vec![coffee_question()])),
        (
            "a set of questions",
            with(vec![coffee_question(), topics_question()]),
        ),
        ("the preview layout", with(vec![preview_question()])),
        ("an open Other entry", entry),
        ("an open notes field", notes),
        ("the review page", review),
        ("a counting-down question", timed),
    ] {
        // Every rung a short terminal walks down hugs its rules too.
        for height in (1..=40).chain([TALL]) {
            assert_frame_hugs_content(
                &format!("{what} on {height} rows"),
                &ask_lines(&app, 80, height),
            );
        }
    }
}

#[test]
fn the_current_chip_lights_on_the_selection_background() {
    let mut app = App::new();
    open(&mut app, vec![coffee_question(), topics_question()]);
    let lines = ask_lines(&app, 80, TALL);
    // The chip strip opens the page, straight under the rule.
    let chip_line = &lines[1];
    let current = chip_line
        .spans
        .iter()
        .find(|s| s.content.contains("Coffee style"))
        .expect("the current chip");
    assert_eq!(
        current.style.bg,
        Some(ask_chip_current_bg()),
        "the current section is highlighted cyan"
    );
    let other = chip_line
        .spans
        .iter()
        .find(|s| s.content.contains("Demo topics"))
        .expect("the other chip");
    assert_eq!(other.style.bg, None, "only the current chip is lit");
}

#[test]
fn an_answered_question_checks_its_chip() {
    let mut app = App::new();
    open(&mut app, vec![coffee_question(), topics_question()]);
    // Answer the first question (advances to the second tab).
    app.on_key(key(KeyCode::Char('1')));
    let lines = ask_lines(&app, 80, TALL);
    let text = all_text(&lines);
    assert!(text.contains("☒ Coffee style"), "answered → ☒:\n{text}");
    assert!(text.contains("☐ Demo topics"), "unanswered stays ☐");
}

#[test]
fn a_single_select_answer_wears_the_check_and_multi_wears_checkboxes() {
    let mut app = App::new();
    open(&mut app, vec![coffee_question(), topics_question()]);
    app.on_key(key(KeyCode::Char('1'))); // Black ✔, advance
    // Back to the first page to see the check.
    app.on_key(key(KeyCode::Left));
    let text = all_text(&ask_lines(&app, 80, TALL));
    assert!(text.contains("1. Black ✔"), "got:\n{text}");
    // Forward to the multi page: checkboxes, one checked.
    app.on_key(key(KeyCode::Right));
    app.on_key(key(KeyCode::Char('1')));
    let text = all_text(&ask_lines(&app, 80, TALL));
    assert!(text.contains("[✔] Preview panel"), "got:\n{text}");
    assert!(text.contains("[ ] Custom 'Other' input"), "got:\n{text}");
    // The multi page carries its unnumbered Submit row.
    assert!(
        text.contains("Submit\n") || text.contains(" Submit"),
        "got:\n{text}"
    );
}

#[test]
fn a_preview_question_renders_the_side_panel_and_notes_line() {
    let mut app = App::new();
    open(&mut app, vec![preview_question()]);
    let lines = ask_lines(&app, 90, TALL);
    let text = all_text(&lines);
    // The focused option's preview rides the bordered panel.
    assert!(text.contains("┌"), "the panel's border:\n{text}");
    assert!(text.contains("const greet = (name) => {"), "got:\n{text}");
    assert!(text.contains("└"));
    // The options sit left of the panel on the same rows.
    let arrow_row = lines
        .iter()
        .map(plain)
        .find(|l| l.contains("Arrow function"))
        .expect("the option row");
    assert!(
        arrow_row.contains('┌') || arrow_row.contains('│'),
        "options and panel share rows: {arrow_row}"
    );
    // The notes line under the panel, with its placeholder + the n hint.
    assert!(text.contains("Notes: press n to add notes"), "got:\n{text}");
    assert!(text.contains("n"), "the hint offers n");
    // Chat about this below.
    assert!(text.contains("Chat about this"));
    // Focusing the second option swaps the panel content.
    app.on_key(key(KeyCode::Down));
    let text = all_text(&ask_lines(&app, 90, TALL));
    assert!(text.contains("`Hello, ${n}!`"), "got:\n{text}");
}

#[test]
fn typed_notes_replace_the_placeholder() {
    let mut app = App::new();
    open(&mut app, vec![preview_question()]);
    app.on_key(key(KeyCode::Char('n')));
    for c in "keep it short".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    // While editing, the typed text shows at the notes line.
    let text = all_text(&ask_lines(&app, 90, TALL));
    assert!(text.contains("Notes: keep it short"), "got:\n{text}");
    app.on_key(key(KeyCode::Esc));
    let text = all_text(&ask_lines(&app, 90, TALL));
    assert!(
        text.contains("Notes: keep it short"),
        "the note stays after Esc:\n{text}"
    );
}

#[test]
fn a_partial_review_warns_and_lists_only_the_answered_questions() {
    let mut app = App::new();
    open(&mut app, vec![coffee_question(), topics_question()]);
    app.on_key(key(KeyCode::Char('2'))); // Latte → advance
    app.on_key(key(KeyCode::Right)); // → Submit page
    assert!(app.ask().unwrap().on_submit_tab());
    let lines = ask_lines(&app, 80, TALL);
    let text = all_text(&lines);
    assert!(text.contains("Review your answers"), "got:\n{text}");
    // The partial submission leads with the amber warning…
    assert!(
        text.contains("⚠ You have not answered all questions"),
        "got:\n{text}"
    );
    let warning = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.contains("⚠"))
        .expect("the warning span");
    assert_eq!(
        warning.style.fg,
        Some(super::super::theme::ask_warning_color())
    );
    // …lists only the ANSWERED question — the open one shows nothing here
    // (no placeholder row, and its question text stays off the page)…
    assert!(text.contains("● What's your favorite way to drink coffee?"));
    assert!(text.contains("→ Latte"), "got:\n{text}");
    assert!(!text.contains("(not answered)"), "got:\n{text}");
    assert!(
        !text.contains("Which of these tool features"),
        "the unanswered question is omitted:\n{text}"
    );
    // …and the recorded answer is green.
    let answer = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.contains("Latte"))
        .expect("the answer span");
    assert_eq!(
        answer.style.fg,
        Some(super::super::theme::ask_answer_color())
    );
    assert!(text.contains("Ready to submit your answers?"));
    assert!(text.contains("❯ 1. Submit answers"), "got:\n{text}");
    assert!(text.contains("2. Cancel"));
}

#[test]
fn a_complete_review_shows_no_warning() {
    let mut app = App::new();
    open(&mut app, vec![coffee_question(), topics_question()]);
    app.on_key(key(KeyCode::Char('2'))); // Latte → advance
    app.on_key(key(KeyCode::Char('1'))); // toggle Preview panel
    app.on_key(key(KeyCode::Right)); // → Submit page
    let text = all_text(&ask_lines(&app, 80, TALL));
    assert!(
        !text.contains("⚠"),
        "every question answered — no warning:\n{text}"
    );
    assert!(text.contains("→ Latte"));
    assert!(text.contains("→ Preview panel"));
}

#[test]
fn a_huge_review_answer_previews_capped_with_an_ellipsis() {
    // An expanded 2k+ paste as the Other answer: the review shows a few rows
    // and a dim `…`, not the whole payload (the submission still carries it).
    let mut app = App::new();
    open(&mut app, vec![coffee_question(), topics_question()]);
    app.on_key(key(KeyCode::Char('4'))); // the Other entry
    app.paste_into_ask(&"w".repeat(1400));
    app.on_key(key(KeyCode::Enter)); // accept → advances to topics
    app.on_key(key(KeyCode::Right)); // → Submit page
    assert!(app.ask().unwrap().on_submit_tab());
    let lines = ask_lines(&app, 80, TALL);
    let answer_rows = lines
        .iter()
        .map(plain)
        .filter(|l| l.trim_start().starts_with('w') || l.contains("→ w"))
        .count();
    assert!(
        answer_rows <= super::super::theme::ASK_REVIEW_ANSWER_MAX_ROWS,
        "the preview caps at the ceiling, got {answer_rows} rows"
    );
    let text = all_text(&lines);
    assert!(text.contains('…'), "the cap is marked:\n{text}");
}

#[test]
fn a_review_with_nothing_answered_is_just_the_warning() {
    let mut app = App::new();
    open(&mut app, vec![coffee_question(), topics_question()]);
    app.on_key(key(KeyCode::Right));
    app.on_key(key(KeyCode::Right)); // straight to Submit, nothing answered
    let text = all_text(&ask_lines(&app, 80, TALL));
    assert!(text.contains("⚠ You have not answered all questions"));
    assert!(!text.contains("●"), "no review rows at all:\n{text}");
    assert!(text.contains("Ready to submit your answers?"));
}

#[test]
fn the_page_builds_whole_and_the_paint_bottom_anchors() {
    // A short terminal used to cut the page by dropping its top rows into no
    // buffer at all — the chip strip, the question and the first options were
    // simply gone (the reported "small terminal hides the texts"). The page
    // now sheds what the choice doesn't need first (`docs/ask.md`), and on a
    // terminal shorter than even its smallest page the region clamps to the
    // terminal, the paint bottom-anchors so the interactive tail stays on
    // screen, and the skipped top flows into real scrollback
    // (`ui::view_flow`, `docs/view-flow.md`).
    let mut app = App::new();
    open(&mut app, vec![coffee_question(), topics_question()]);
    let full = ask_lines(&app, 80, TALL);
    assert!(all_text(&full).contains("☐ Coffee style"), "nothing is cut");
    let small = ask_lines(&app, 80, 8);
    assert!(small.len() > 8, "even the smallest page overflows 8 rows");
    // The region reserves the terminal when the page overflows, the page's
    // own height when it fits.
    assert_eq!(ask_height(&app, 80, 8), Some(8));
    assert_eq!(ask_height(&app, 80, 60), Some(full.len() as u16));
    // The paint shows exactly the page's last rows — the window's options,
    // the hints and the closing rule stay reachable.
    let area = Rect::new(0, 0, 80, 8);
    let mut buf = Buffer::empty(area);
    render_ask(area, &mut buf, &app);
    let painted: Vec<String> = (0..8)
        .map(|y| row(&buf, y, 80).trim_end().to_string())
        .collect();
    let expected: Vec<String> = small[small.len() - 8..]
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert_eq!(painted, expected, "the paint bottom-anchors the page");
    assert!(
        painted.iter().any(|r| r.contains("❯ 1. Black")),
        "the highlighted row paints: {painted:?}"
    );
    assert!(
        painted.last().unwrap().starts_with("──"),
        "the closing rule stays on screen"
    );
}

#[test]
fn editing_the_other_row_shows_the_entry_and_seats_the_cursor() {
    let mut app = App::new();
    open(&mut app, vec![coffee_question()]);
    app.on_key(key(KeyCode::Char('4'))); // the Other row
    for c in "My an".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    let lines = ask_lines(&app, 80, TALL);
    let text = all_text(&lines);
    assert!(
        text.contains("❯ 4. My an"),
        "the entry renders in place:\n{text}"
    );
    // The cursor sits right after the typed text on that row.
    let (x, y) = super::super::ask_view::ask_cursor(&app, 80, 40).expect("editing shows a caret");
    let entry_row = lines
        .iter()
        .position(|l| plain(l).contains("4. My an"))
        .unwrap();
    assert_eq!(usize::from(y), entry_row);
    let expected_x = " ".len() + "❯ ".chars().count() + "4. ".len() + "My an".len();
    assert_eq!(usize::from(x), expected_x);
    // Outside an entry the caret hides — while its *seat* moves back to the
    // highlighted `❯` row (the permission prompt's rule), so a terminal's
    // cursor animation lands on the option and not the bottom rule.
    app.on_key(key(KeyCode::Esc));
    assert!(!cursor_visible(&app), "the option menu hides the cursor");
    let (x, y) = super::super::ask_view::ask_cursor(&app, 80, 40).expect("a seat on the ❯ row");
    let marker_row = ask_lines(&app, 80, TALL)
        .iter()
        .position(|l| plain(l).contains("❯ "))
        .expect("the highlighted row");
    assert_eq!(usize::from(y), marker_row);
    assert_eq!(x, 3);
}

#[test]
fn a_shift_enter_entry_renders_multi_line_with_the_cursor_on_the_last_row() {
    // The Other entry is the composer's field: Shift+Enter (here the textarea
    // newline it maps to) breaks the line, both rows render — the first
    // behind `❯ 4. `, the continuation aligned under the text — and the
    // cursor seats on the continuation row (docs/ask.md).
    let mut app = App::new();
    open(&mut app, vec![coffee_question()]);
    app.on_key(key(KeyCode::Char('4')));
    for c in "line one".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    for c in "line two".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    let lines = ask_lines(&app, 80, TALL);
    let text = all_text(&lines);
    assert!(text.contains("❯ 4. line one"), "got:\n{text}");
    let first = lines
        .iter()
        .position(|l| plain(l).contains("4. line one"))
        .unwrap();
    assert_eq!(
        plain(&lines[first + 1]),
        format!(
            "{}line two",
            " ".repeat(" ".len() + "❯ ".chars().count() + "4. ".len())
        ),
        "the continuation row aligns under the text"
    );
    let (x, y) = super::super::ask_view::ask_cursor(&app, 80, 40).expect("editing shows a caret");
    assert_eq!(usize::from(y), first + 1, "the cursor follows to row two");
    let content_col = " ".len() + "❯ ".chars().count() + "4. ".len();
    assert_eq!(usize::from(x), content_col + "line two".len());
    // The hint offers the newline key while the entry is live.
    assert!(text.contains("Shift+Enter"), "got:\n{text}");
}

#[test]
fn a_pasted_placeholder_renders_in_the_entry_field() {
    let mut app = App::new();
    open(&mut app, vec![coffee_question()]);
    app.on_key(key(KeyCode::Char('4')));
    app.paste_into_ask(&"x".repeat(2253));
    let text = all_text(&ask_lines(&app, 80, TALL));
    assert!(
        text.contains("[Pasted Content 2253 chars]"),
        "the entry shows the compact placeholder:\n{text}"
    );
}

#[test]
fn a_multi_line_note_renders_wrapped_under_the_label() {
    let mut app = App::new();
    open(&mut app, vec![preview_question()]);
    app.on_key(key(KeyCode::Char('n')));
    for c in "top".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    for c in "bottom".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    let lines = ask_lines(&app, 90, TALL);
    let notes_at = lines
        .iter()
        .position(|l| plain(l).contains("Notes: top"))
        .expect("the notes entry's first row");
    assert!(
        plain(&lines[notes_at + 1])
            .trim_start()
            .starts_with("bottom"),
        "the second note row renders: {:?}",
        plain(&lines[notes_at + 1])
    );
    let (_, y) = super::super::ask_view::ask_cursor(&app, 90, 40).expect("a caret while editing");
    assert_eq!(usize::from(y), notes_at + 1);
}

/// The **painted** row the builder puts the `❯` selection marker on, and the
/// hardware cursor's seat — the permission-view pair (`marker_row_and_cursor`'s
/// sibling): the two must be the same row for every highlight. The paint
/// bottom-anchors, so the marker's page row maps through the same skip.
fn marker_row_and_cursor(app: &App, width: u16, term_height: u16) -> (u16, (u16, u16)) {
    let lines = ask_lines(app, width, term_height);
    let marker = lines
        .iter()
        .position(|l| plain(l).contains("❯ "))
        .expect("the highlighted row");
    let height = ask_height(app, width, term_height).unwrap();
    let skip = lines.len().saturating_sub(usize::from(height));
    let marker = marker
        .checked_sub(skip)
        .expect("the caller's highlight is inside the painted tail") as u16;
    (marker, cursor_position(Rect::new(0, 0, width, height), app))
}

#[test]
fn the_cursor_seat_follows_the_highlighted_row() {
    // Hidden, but not homeless — the permission prompt's rule
    // (`the_cursor_seat_follows_the_highlighted_option`): the seat tracks
    // the row being chosen, so a terminal with a cursor animation lands on
    // `❯ 1. Black` instead of the far end of the bottom rule.
    let mut app = App::new();
    open(&mut app, vec![coffee_question()]);
    let mut seats = Vec::new();
    for step in 0..5 {
        if step > 0 {
            app.on_key(key(KeyCode::Down));
        }
        assert!(!cursor_visible(&app), "the option menu hides the cursor");
        let (marker, (x, y)) = marker_row_and_cursor(&app, 80, 40);
        assert_eq!(y, marker, "the cursor sits on the highlighted ❯ row");
        // One inset column + the two-column `❯ ` — the permission prompt's
        // column, so the two modals' seats agree.
        assert_eq!(x, 3, "the column is the option text's first");
        seats.push(y);
    }
    // The descriptions sit between the option rows, so ↓ steps *past* them —
    // the seat rides the rendered row, not a fixed stride.
    assert!(
        seats.windows(2).all(|w| w[1] > w[0]),
        "↓ walks the seat down the rows: {seats:?}"
    );
}

#[test]
fn the_cursor_seat_follows_the_review_page_rows() {
    let mut app = App::new();
    open(&mut app, vec![coffee_question(), topics_question()]);
    app.on_key(key(KeyCode::Right));
    app.on_key(key(KeyCode::Right)); // to the Submit page
    let lines = ask_lines(&app, 80, TALL);
    let submit = lines
        .iter()
        .position(|l| plain(l).contains("❯ 1. Submit answers"))
        .expect("the Submit row highlighted") as u16;
    let area = Rect::new(0, 0, 80, ask_height(&app, 80, 40).unwrap());
    let (x, y) = cursor_position(area, &app);
    assert_eq!((x, y), (3, submit), "the seat lands on Submit answers");
    app.on_key(key(KeyCode::Down));
    let (_, (x, y)) = marker_row_and_cursor(&app, 80, 40);
    assert_eq!((x, y), (3, submit + 1), "…and follows onto Cancel");
}

#[test]
fn the_cursor_seat_follows_the_preview_page_rows() {
    // The side-by-side layout zips the left column with the panel; the seat
    // still lands on the highlighted left row, and on the full-width Chat
    // row below the notes line when the walk reaches it.
    let mut app = App::new();
    open(&mut app, vec![preview_question()]);
    let mut seats = Vec::new();
    for step in 0..4 {
        if step > 0 {
            app.on_key(key(KeyCode::Down));
        }
        let (marker, (x, y)) = marker_row_and_cursor(&app, 90, 40);
        assert_eq!(y, marker, "the cursor sits on the highlighted ❯ row");
        assert_eq!(x, 3);
        seats.push(y);
    }
    let lines = ask_lines(&app, 90, TALL);
    assert!(
        plain(&lines[usize::from(seats[3])]).contains("Chat about this"),
        "the walk ends on the Chat row"
    );
}

#[test]
fn a_flowed_off_highlight_parks_the_seat_in_the_corner() {
    // The bottom anchor paints the page's tail; a highlighted row that sits
    // in the flowed top has no on-screen row, so the cursor falls back to
    // the far corner (the menus' rule for a marker-less page).
    let mut app = App::new();
    open(&mut app, vec![coffee_question()]);
    // At 5 rows only the smallest page's tail (the window's last option, its
    // overflow slot, hints, rule) paints — the highlighted first option sits
    // in the flowed top.
    let lines = ask_lines(&app, 80, 5);
    let skip = lines.len() - 5;
    let marker = lines
        .iter()
        .position(|l| plain(l).contains("❯ "))
        .expect("the highlighted row");
    assert!(marker < skip, "the highlighted row is in the flowed top");
    let area = Rect::new(0, 0, 80, ask_height(&app, 80, 5).unwrap());
    let (x, y) = cursor_position(area, &app);
    assert_eq!((x, y), (79, 4), "the corner fallback");
    // ↑ wraps the highlight to the last row (Chat about this), inside the
    // painted tail — the seat comes back to it.
    app.on_key(key(KeyCode::Up));
    let (marker, (x, y)) = marker_row_and_cursor(&app, 80, 5);
    assert_eq!(y, marker, "a visible highlight seats normally");
    assert_eq!(x, 3);
}

#[test]
fn region_is_modal_covers_the_ask_prompt() {
    let mut app = App::new();
    assert!(!region_is_modal(&app));
    open(&mut app, vec![coffee_question()]);
    assert!(region_is_modal(&app), "the close needs the purge rebuild");
}

#[test]
fn render_ask_paints_the_region() {
    let mut app = App::new();
    open(&mut app, vec![coffee_question()]);
    let height = ask_height(&app, 60, 40).unwrap();
    let area = Rect::new(0, 0, 60, height);
    let mut buf = Buffer::empty(area);
    render_ask(area, &mut buf, &app);
    let top = row(&buf, 0, 60);
    assert!(top.starts_with("──"), "got {top:?}");
    let all: Vec<String> = (0..height).map(|y| row(&buf, y, 60)).collect();
    assert!(
        all.iter().any(|r| r.contains("1. Black")),
        "the options painted: {all:?}"
    );
}

#[test]
fn a_full_entry_row_keeps_the_caret_inside_the_width() {
    // The Other entry keeps one column for the caret like the composer
    // (docs/textarea.md): however long the text, the caret's column is
    // inside the 80 the prompt was laid out in — never one past its edge.
    let mut app = App::new();
    open(&mut app, vec![coffee_question()]);
    app.on_key(key(KeyCode::Char('4'))); // the Other row
    for n in 1..=200 {
        app.on_key(key(KeyCode::Char('x')));
        let (x, _) =
            super::super::ask_view::ask_cursor(&app, 80, 40).expect("a caret while editing");
        assert!(x < 80, "{n} chars: the caret sits at column {x}");
    }
}

// ===== The idle timeout's countdown (docs/ask.md) =====

/// The closing rule's countdown chip — the span carrying the label text.
fn countdown_span(app: &App, width: u16) -> Option<Span<'static>> {
    let lines = ask_lines(app, width, TALL);
    lines
        .last()?
        .spans
        .iter()
        .find(|s| s.content.contains("Timeout:"))
        .cloned()
}

#[test]
fn the_closing_rule_counts_down_while_the_idle_clock_runs() {
    let mut app = App::new();
    open(&mut app, vec![coffee_question()]);
    // No clock (the timeout is off, or no draw has injected one): plain.
    let bare = plain(ask_lines(&app, 80, TALL).last().unwrap());
    assert_eq!(bare, "─".repeat(80));
    app.set_ask_remaining(Some(std::time::Duration::from_secs(581)));
    let rule = plain(ask_lines(&app, 80, TALL).last().unwrap());
    assert!(rule.ends_with("── Timeout: 9:41 ─"), "got {rule:?}");
    assert_eq!(rule.chars().count(), 80, "the rule keeps its width");
}

#[test]
fn the_countdown_rounds_up_so_it_never_reads_zero_while_open() {
    let mut app = App::new();
    open(&mut app, vec![coffee_question()]);
    for (left_ms, shown) in [(600_000, "10:00"), (599_200, "10:00"), (400, "0:01")] {
        app.set_ask_remaining(Some(std::time::Duration::from_millis(left_ms)));
        let rule = plain(ask_lines(&app, 80, TALL).last().unwrap());
        assert!(
            rule.ends_with(&format!(" Timeout: {shown} ─")),
            "{left_ms} ms left: {rule:?}"
        );
    }
}

#[test]
fn the_countdown_is_a_chip_in_the_theme_accent_set_into_the_rule() {
    // The user-requested fill: the countdown rides the closing rule as the
    // agent session view's label does — a chip of the theme's accent under
    // its on-accent ink, the label's padding inside the fill, the rule before
    // it and the one tail cell after it bare and border-coloured.
    use crate::app::Theme;
    use crate::ui::palette::{palette_of, with_theme};
    use crate::ui::theme::{agent_view_label_bg, agent_view_label_fg, border_color};
    let mut app = App::new();
    open(&mut app, vec![coffee_question()]);
    app.set_ask_remaining(Some(std::time::Duration::from_secs(581)));
    let lines = ask_lines(&app, 80, TALL);
    let spans = &lines.last().unwrap().spans;
    let chip = countdown_span(&app, 80).expect("the countdown");
    assert_eq!(
        chip.content, " Timeout: 9:41 ",
        "the padding rides the fill"
    );
    assert_eq!(chip.style.bg, Some(agent_view_label_bg()));
    assert_eq!(chip.style.fg, Some(agent_view_label_fg()));
    for span in spans.iter().filter(|s| !s.content.contains("Timeout:")) {
        assert!(
            span.content.chars().all(|c| c == '─'),
            "only rule cells beside the chip: {span:?}"
        );
        assert_eq!(span.style.fg, Some(border_color()), "{span:?}");
        assert_eq!(span.style.bg, None, "no fill outside the chip: {span:?}");
    }
    // Every theme paints it from its own table, the ink never the fill.
    for theme in Theme::ALL {
        with_theme(theme, || {
            let p = palette_of(theme);
            let chip = countdown_span(&app, 80).expect("the countdown");
            assert_eq!(chip.style.bg, Some(p.accent), "{theme:?}: the fill");
            assert_eq!(chip.style.fg, Some(p.on_accent), "{theme:?}: the ink");
            assert_ne!(p.on_accent, p.accent, "{theme:?}: the ink must show");
        });
    }
}

#[test]
fn the_countdown_turns_amber_in_its_last_minute() {
    use crate::ui::theme::{agent_view_label_bg, agent_view_label_fg, ask_warning_color};
    let mut app = App::new();
    open(&mut app, vec![coffee_question()]);
    app.set_ask_remaining(Some(std::time::Duration::from_secs(60)));
    let calm = countdown_span(&app, 80).expect("the countdown");
    assert_eq!(
        calm.style.bg,
        Some(agent_view_label_bg()),
        "1:00 left is calm"
    );
    app.set_ask_remaining(Some(std::time::Duration::from_secs(59)));
    let late = countdown_span(&app, 80).expect("the countdown");
    assert_eq!(
        late.style.bg,
        Some(ask_warning_color()),
        "0:59 left lights the chip amber"
    );
    assert_eq!(late.style.fg, Some(agent_view_label_fg()), "the same ink");
}

#[test]
fn a_rule_too_narrow_for_the_countdown_stays_plain_and_the_height_holds() {
    let mut app = App::new();
    open(&mut app, vec![coffee_question()]);
    // ` Timeout: 9:41 ` is 15 columns, plus the tail cell and a two-cell
    // lead: 17 columns are too few, 18 are enough.
    let before = ask_lines(&app, 17, TALL).len();
    app.set_ask_remaining(Some(std::time::Duration::from_secs(581)));
    let lines = ask_lines(&app, 17, TALL);
    assert_eq!(plain(lines.last().unwrap()), "─".repeat(17));
    assert_eq!(lines.len(), before, "the countdown never costs a row");
    let fits = plain(ask_lines(&app, 18, TALL).last().unwrap());
    assert_eq!(fits, "── Timeout: 9:41 ─");
    let wide_before = {
        app.set_ask_remaining(None);
        ask_lines(&app, 80, TALL).len()
    };
    app.set_ask_remaining(Some(std::time::Duration::from_secs(581)));
    assert_eq!(ask_lines(&app, 80, TALL).len(), wide_before);
}

// ===== A terminal too short for the whole page (docs/ask.md) =====

/// The page on a terminal `height` rows tall, as trimmed plain rows.
fn page_at(app: &App, width: u16, height: u16) -> Vec<String> {
    ask_lines(app, width, height)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

/// The height one row short of `texts`.
fn short_of(texts: &[String]) -> u16 {
    u16::try_from(texts.len()).expect("rows") - 1
}

/// Every distinct page the builder can build at `width`, most complete
/// first: each one the page one row short of the one before builds, until
/// the pages stop shrinking.
fn levels(app: &App, width: u16) -> Vec<Vec<String>> {
    let mut pages = vec![page_at(app, width, TALL)];
    loop {
        let last = pages.last().expect("a page");
        let next = page_at(app, width, short_of(last));
        if next.len() >= last.len() {
            return pages;
        }
        pages.push(next);
    }
}

fn has(texts: &[String], needle: &str) -> bool {
    texts.iter().any(|t| t.contains(needle))
}

/// How many of the question's numbered rows the page lists.
fn numbered(texts: &[String]) -> usize {
    texts
        .iter()
        .filter(|t| {
            let t = t.trim_start().trim_start_matches('❯').trim_start();
            t.split_once(". ")
                .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        })
        .count()
}

/// Four options whose descriptions wrap at 60 columns.
fn wordy_question() -> AskQuestion {
    AskQuestion {
        question: "Which deployment target should the release go to first?".to_string(),
        header: "Target".to_string(),
        options: vec![
            option(
                "Staging",
                "The shared staging cluster, which mirrors production's configuration closely",
            ),
            option(
                "Canary",
                "A single production node taking one percent of traffic for an hour",
            ),
            option(
                "Production",
                "Every node at once, after the on-call has signed the release checklist",
            ),
            option(
                "Nowhere",
                "Hold the release until the flaky suite is green three runs in a row",
            ),
        ],
        multi_select: false,
    }
}

#[test]
fn a_short_terminal_cuts_the_descriptions_to_one_row_first() {
    // The whole page wraps every description; on a terminal shorter than
    // it, the paint bottom-anchored the page and flowed the chip strip, the
    // question and the first options off the top — the highlighted one
    // among them. The descriptions yield first, cut to one row each.
    let mut app = App::new();
    open(&mut app, vec![wordy_question(), coffee_question()]);
    let levels = levels(&app, 60);
    let (whole, cut) = (&levels[0], &levels[1]);
    assert!(
        has(whole, "configuration closely"),
        "wrapped whole: {whole:#?}"
    );
    assert!(cut.len() < whole.len(), "{cut:#?}");
    assert!(has(cut, "The shared staging cluster"), "{cut:#?}");
    assert!(!has(cut, "configuration closely"), "{cut:#?}");
    assert!(cut.iter().any(|t| t.ends_with('…')), "cut with …: {cut:#?}");
    assert!(has(cut, "☐ Target"), "the chips stay: {cut:#?}");
    assert!(has(cut, "Which deployment target"), "{cut:#?}");
    assert_eq!(numbered(cut), 6, "every row stays: {cut:#?}");
}

#[test]
fn then_the_descriptions_go_and_every_row_stays() {
    let mut app = App::new();
    open(&mut app, vec![wordy_question(), coffee_question()]);
    let levels = levels(&app, 60);
    let bare = &levels[2];
    assert!(!has(bare, "The shared staging cluster"), "{bare:#?}");
    assert!(has(bare, "☐ Target"), "{bare:#?}");
    assert!(has(bare, "Which deployment target"), "{bare:#?}");
    assert_eq!(numbered(bare), 6, "{bare:#?}");
    assert!(has(bare, "❯ 1. Staging"), "{bare:#?}");
}

#[test]
fn then_the_rows_window_around_the_selection_and_last_the_chips_go() {
    // codex's order: the tabs stay while the list shrinks to a window
    // between the ↑/↓ overflow slots; the chip strip goes last.
    let mut app = App::new();
    open(&mut app, vec![wordy_question(), coffee_question()]);
    let levels = levels(&app, 60);
    assert_eq!(levels.len(), 5, "{levels:#?}");
    let windowed = &levels[3];
    assert!(has(windowed, "☐ Target"), "{windowed:#?}");
    assert!(has(windowed, "Which deployment target"), "{windowed:#?}");
    assert_eq!(numbered(windowed), 3, "{windowed:#?}");
    assert!(has(windowed, "❯ 1. Staging"), "{windowed:#?}");
    assert!(
        windowed.iter().any(|t| t.trim() == "↓ 3 more below"),
        "{windowed:#?}"
    );
    let smallest = &levels[4];
    assert!(!has(smallest, "☐ Target"), "{smallest:#?}");
    assert!(has(smallest, "Which deployment target"), "{smallest:#?}");
    assert_eq!(numbered(smallest), 3, "{smallest:#?}");
    assert!(has(smallest, "❯ 1. Staging"), "{smallest:#?}");
    assert!(has(smallest, "Esc"), "the hint stays: {smallest:#?}");
    assert!(
        smallest[0].starts_with("──") && smallest.last().unwrap().starts_with("──"),
        "{smallest:#?}"
    );
}

#[test]
fn the_window_follows_the_selection() {
    let mut app = App::new();
    open(&mut app, vec![wordy_question(), coffee_question()]);
    let height = short_of(&levels(&app, 60)[3]);
    for _ in 0..4 {
        app.on_key(key(KeyCode::Down));
    }
    let texts = page_at(&app, 60, height);
    assert!(has(&texts, "❯ 5. Type something."), "{texts:#?}");
    assert!(
        texts.iter().any(|t| t.trim() == "↑ 3 more above"),
        "{texts:#?}"
    );
    assert!(!has(&texts, " more below"), "nothing below: {texts:#?}");
    assert!(has(&texts, "6. Chat about this"), "{texts:#?}");
}

#[test]
fn a_short_page_fits_and_holds_one_height_at_every_selection() {
    for width in [50_u16, 60, 80] {
        let mut app = App::new();
        open(&mut app, vec![wordy_question(), coffee_question()]);
        let smallest = page_at(&app, width, 1).len();
        for height in [8_u16, 10, 12, 14, 16, 20, 24] {
            let mut heights = Vec::new();
            for _ in 0..6 {
                let texts = page_at(&app, width, height);
                if usize::from(height) >= smallest {
                    assert!(
                        texts.len() <= usize::from(height),
                        "{width}x{height}: {texts:#?}"
                    );
                }
                assert!(
                    texts
                        .windows(2)
                        .all(|pair| !(pair[0].is_empty() && pair[1].is_empty())),
                    "{width}x{height}: two blank rows stack: {texts:#?}"
                );
                heights.push(texts.len());
                app.on_key(key(KeyCode::Down));
            }
            assert!(
                heights.windows(2).all(|pair| pair[0] == pair[1]),
                "{width}x{height}: one height at every selection: {heights:?}"
            );
        }
    }
}

#[test]
fn a_short_page_seats_the_cursor_on_the_highlighted_row() {
    let mut app = App::new();
    open(&mut app, vec![wordy_question(), coffee_question()]);
    for step in 0..6 {
        if step > 0 {
            app.on_key(key(KeyCode::Down));
        }
        let (marker, (x, y)) = marker_row_and_cursor(&app, 60, 12);
        assert_eq!((x, y), (3, marker), "step {step}");
    }
}

#[test]
fn a_short_preview_page_drops_the_notes_hint_then_caps_the_panel_then_the_chips() {
    // The side-by-side page keeps its options and its panel: the dim notes
    // line yields first (the hint already offers `n`), then the panel is
    // cut to the option column's height, then the chip strip goes.
    let mut app = App::new();
    open(&mut app, vec![preview_question(), coffee_question()]);
    let levels = levels(&app, 90);
    assert_eq!(levels.len(), 4, "{levels:#?}");
    assert!(has(&levels[0], "press n to add notes"), "{:#?}", levels[0]);
    let quiet = &levels[1];
    assert!(!has(quiet, "Notes:"), "{quiet:#?}");
    assert!(
        has(quiet, "return `Hello`"),
        "the whole preview: {quiet:#?}"
    );
    let capped = &levels[2];
    assert!(has(capped, "const greet = (name) => {"), "{capped:#?}");
    assert!(
        !has(capped, "return `Hello`"),
        "cut to the column: {capped:#?}"
    );
    assert!(has(capped, "☐ Code style"), "{capped:#?}");
    let smallest = &levels[3];
    assert!(!has(smallest, "☐ Code style"), "{smallest:#?}");
    assert!(has(smallest, "Which code style"), "{smallest:#?}");
    assert!(has(smallest, "❯ 1. Arrow function"), "{smallest:#?}");
    assert!(has(smallest, "Chat about this"), "{smallest:#?}");
    assert!(
        has(smallest, "n to add notes"),
        "the hint still offers notes"
    );
}

#[test]
fn typed_notes_stay_on_every_short_preview_page() {
    let mut app = App::new();
    open(&mut app, vec![preview_question(), coffee_question()]);
    app.on_key(key(KeyCode::Char('n')));
    for c in "tabs please".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    // While typing, the field is on every page and the caret in it.
    for texts in levels(&app, 90) {
        assert!(has(&texts, "Notes: tabs please"), "{texts:#?}");
    }
    app.on_key(key(KeyCode::Enter));
    for texts in levels(&app, 90) {
        assert!(has(&texts, "Notes: tabs please"), "{texts:#?}");
    }
}
