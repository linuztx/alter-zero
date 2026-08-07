//! The `AskUserQuestion` modal's renderer (`docs/ask.md`): the chip strip,
//! the option pages, the preview panel, the Submit review page, and the
//! resolved cell's headline header.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::super::theme::ASK_CHIP_CURRENT_BG;
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

fn all_text(lines: &[Line]) -> String {
    lines.iter().map(plain).collect::<Vec<_>>().join("\n")
}

#[test]
fn the_modal_frames_the_page_with_rules_chips_question_options_and_hints() {
    let mut app = App::new();
    open(&mut app, vec![coffee_question(), topics_question()]);
    let lines = ask_lines(&app, 80, 40);
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
fn the_current_chip_lights_on_the_selection_background() {
    let mut app = App::new();
    open(&mut app, vec![coffee_question(), topics_question()]);
    let lines = ask_lines(&app, 80, 40);
    let chip_line = &lines[2];
    let current = chip_line
        .spans
        .iter()
        .find(|s| s.content.contains("Coffee style"))
        .expect("the current chip");
    assert_eq!(
        current.style.bg,
        Some(ASK_CHIP_CURRENT_BG),
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
    let lines = ask_lines(&app, 80, 40);
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
    let text = all_text(&ask_lines(&app, 80, 40));
    assert!(text.contains("1. Black ✔"), "got:\n{text}");
    // Forward to the multi page: checkboxes, one checked.
    app.on_key(key(KeyCode::Right));
    app.on_key(key(KeyCode::Char('1')));
    let text = all_text(&ask_lines(&app, 80, 40));
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
    let lines = ask_lines(&app, 90, 40);
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
    let text = all_text(&ask_lines(&app, 90, 40));
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
    let text = all_text(&ask_lines(&app, 90, 40));
    assert!(text.contains("Notes: keep it short"), "got:\n{text}");
    app.on_key(key(KeyCode::Esc));
    let text = all_text(&ask_lines(&app, 90, 40));
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
    let lines = ask_lines(&app, 80, 40);
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
        Some(super::super::theme::ASK_WARNING_COLOR)
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
    assert_eq!(answer.style.fg, Some(super::super::theme::ASK_ANSWER_COLOR));
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
    let text = all_text(&ask_lines(&app, 80, 40));
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
    let lines = ask_lines(&app, 80, 60);
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
    let text = all_text(&ask_lines(&app, 80, 40));
    assert!(text.contains("⚠ You have not answered all questions"));
    assert!(!text.contains("●"), "no review rows at all:\n{text}");
    assert!(text.contains("Ready to submit your answers?"));
}

#[test]
fn the_modal_clamps_to_the_terminal_by_dropping_top_rows() {
    let mut app = App::new();
    open(&mut app, vec![coffee_question(), topics_question()]);
    let short = ask_lines(&app, 80, 8);
    assert_eq!(short.len(), 8, "clamped to the terminal");
    let text = all_text(&short);
    // The tail (options + hints) survives; the chip strip scrolls off first.
    assert!(text.contains("Chat about this"), "got:\n{text}");
    assert_eq!(
        ask_height(&app, 80, 8),
        Some(8),
        "the reserved height equals the built rows"
    );
    // …and the un-clamped height equals the natural row count.
    let tall = ask_lines(&app, 80, 60);
    assert_eq!(ask_height(&app, 80, 60), Some(tall.len() as u16));
}

#[test]
fn editing_the_other_row_shows_the_entry_and_seats_the_cursor() {
    let mut app = App::new();
    open(&mut app, vec![coffee_question()]);
    app.on_key(key(KeyCode::Char('4'))); // the Other row
    for c in "My an".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    let lines = ask_lines(&app, 80, 40);
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
    // Outside an entry there is no caret at all (the menu hides it).
    app.on_key(key(KeyCode::Esc));
    assert!(super::super::ask_view::ask_cursor(&app, 80, 40).is_none());
    assert!(!cursor_visible(&app), "the option menu hides the cursor");
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
    let lines = ask_lines(&app, 80, 40);
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
    let text = all_text(&ask_lines(&app, 80, 40));
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
    let lines = ask_lines(&app, 90, 40);
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
