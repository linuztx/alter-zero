//! The startup review's state and key map (`docs/state/config-review.md`).

use super::*;

use crate::trust::HookIdentity;

fn hook_item(command: &str) -> ReviewItem {
    ReviewItem {
        key: ReviewKey::Hook {
            source: "/home/u/.alter-zero/hooks.json".to_string(),
            identity: HookIdentity {
                event: "Stop".to_string(),
                matcher: String::new(),
                command: command.to_string(),
            },
        },
        title: "Stop".to_string(),
        detail: vec![command.to_string()],
        change: None,
        checked: true,
    }
}

fn server_item(name: &str) -> ReviewItem {
    ReviewItem {
        key: ReviewKey::Server {
            source: "/repo/.mcp.json".to_string(),
            name: name.to_string(),
        },
        title: format!("{name} · stdio"),
        detail: vec![format!("npx -y {name}")],
        change: None,
        checked: true,
    }
}

fn hooks_page(commands: &[&str]) -> ReviewPage {
    ReviewPage::new(
        ReviewKind::Hooks,
        vec![ReviewSection {
            scope: Some("User".to_string()),
            path: "~/.alter-zero/hooks.json".to_string(),
            items: commands.iter().map(|c| hook_item(c)).collect(),
        }],
    )
}

fn servers_page(names: &[&str]) -> ReviewPage {
    ReviewPage::new(
        ReviewKind::McpServers,
        vec![ReviewSection {
            scope: None,
            path: "~/repo/.mcp.json".to_string(),
            items: names.iter().map(|n| server_item(n)).collect(),
        }],
    )
}

fn page(app: &App) -> &ReviewPage {
    app.config_review
        .as_ref()
        .and_then(|review| review.pages.first())
        .expect("a review page is open")
}

fn checked(app: &App) -> Vec<bool> {
    page(app).items().map(|item| item.checked).collect()
}

#[test]
fn a_review_opens_on_its_first_item_with_everything_selected() {
    let mut app = App::new();
    app.shortcuts_open = true;
    type_chars(&mut app, "/he");
    assert!(app.command_menu.is_some());
    app.open_config_review(vec![
        hooks_page(&["./a.sh", "./b.sh"]),
        servers_page(&["gh"]),
    ]);
    // The review replaces the composer, so the bands sharing it go.
    assert!(app.command_menu.is_none());
    assert!(!app.shortcuts_open);
    let review = app.config_review.as_ref().expect("the review is open");
    assert_eq!(review.total, 2);
    assert_eq!(review.pages.len(), 2);
    assert_eq!(page(&app).kind, ReviewKind::Hooks);
    assert_eq!(page(&app).selected, 0);
    // Everything starts selected (Claude Code's default).
    assert_eq!(checked(&app), vec![true, true]);
}

#[test]
fn a_review_with_nothing_to_decide_opens_nothing() {
    let mut app = App::new();
    app.open_config_review(Vec::new());
    assert!(app.config_review.is_none());
    // A page with no items is nothing to decide either.
    app.open_config_review(vec![hooks_page(&[])]);
    assert!(app.config_review.is_none());
}

#[test]
fn space_toggles_the_highlighted_item_only() {
    let mut app = App::new();
    app.open_config_review(vec![hooks_page(&["./a.sh", "./b.sh"])]);
    assert_eq!(app.on_key(key(KeyCode::Char(' '))), Action::None);
    assert_eq!(checked(&app), vec![false, true]);
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Char(' ')));
    assert_eq!(checked(&app), vec![false, false]);
    app.on_key(key(KeyCode::Char(' ')));
    assert_eq!(checked(&app), vec![false, true]);
    // On an action row Space selects nothing.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Char(' ')));
    assert_eq!(checked(&app), vec![false, true]);
    assert!(app.config_review.is_some(), "Space never answers the page");
}

#[test]
fn the_arrows_walk_the_items_then_the_two_actions_wrapping_at_the_ends() {
    let mut app = App::new();
    app.open_config_review(vec![hooks_page(&["./a.sh", "./b.sh"])]);
    let at = |app: &App| page(app).selected;
    for expected in [1, 2, 3, 0] {
        app.on_key(key(KeyCode::Down));
        assert_eq!(at(&app), expected);
    }
    app.on_key(key(KeyCode::Up));
    assert_eq!(
        at(&app),
        3,
        "↑ from the first item wraps to the last action"
    );
    app.on_key(key(KeyCode::Home));
    assert_eq!(at(&app), 0);
    app.on_key(key(KeyCode::End));
    assert_eq!(at(&app), 3);
    assert_eq!(page(&app).action_at(3), Some(ReviewChoice::Decline));
    assert_eq!(page(&app).action_at(2), Some(ReviewChoice::Confirm));
    assert_eq!(page(&app).action_at(1), None, "an item row is no action");
}

#[test]
fn enter_on_an_item_confirms_the_selection() {
    let mut app = App::new();
    app.open_config_review(vec![hooks_page(&["./a.sh", "./b.sh"])]);
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Char(' ')));
    let Action::ReviewAnswered(answer) = app.on_key(key(KeyCode::Enter)) else {
        panic!("Enter answers the page");
    };
    assert_eq!(answer.kind, ReviewKind::Hooks);
    assert!(answer.confirmed);
    let choices: Vec<bool> = answer.choices.iter().map(|(_, on)| *on).collect();
    assert_eq!(choices, vec![true, false]);
    assert_eq!(answer.choices[1].0, hook_item("./b.sh").key);
    assert!(
        app.config_review.is_none(),
        "the last page closes the review"
    );
}

#[test]
fn enter_on_the_first_action_confirms_and_on_the_second_declines() {
    let mut app = App::new();
    app.open_config_review(vec![servers_page(&["gh"])]);
    app.on_key(key(KeyCode::Down));
    let Action::ReviewAnswered(answer) = app.on_key(key(KeyCode::Enter)) else {
        panic!("Enter on the first action answers the page");
    };
    assert!(answer.confirmed);
    assert_eq!(answer.kind, ReviewKind::McpServers);

    let mut app = App::new();
    app.open_config_review(vec![servers_page(&["gh"])]);
    app.on_key(key(KeyCode::End));
    let Action::ReviewAnswered(answer) = app.on_key(key(KeyCode::Enter)) else {
        panic!("Enter on the second action answers the page");
    };
    assert!(!answer.confirmed);
    // The choices still report each item, so the boundary knows what the
    // decline covered.
    assert_eq!(answer.choices.len(), 1);
}

#[test]
fn esc_declines_the_page() {
    let mut app = App::new();
    app.open_config_review(vec![hooks_page(&["./a.sh"])]);
    let Action::ReviewAnswered(answer) = app.on_key(key(KeyCode::Esc)) else {
        panic!("Esc answers the page");
    };
    assert!(!answer.confirmed);
    assert!(app.config_review.is_none());
}

#[test]
fn ctrl_c_quits_without_answering_anything() {
    // The way out of a review the user did not mean to face: nothing is
    // decided — Esc on the servers page would record every server disabled
    // — so nothing it asked about runs, and the next launch asks again.
    let mut app = App::new();
    app.open_config_review(vec![hooks_page(&["./a.sh"]), servers_page(&["gh"])]);
    assert_eq!(
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Action::Quit
    );
}

#[test]
fn ctrl_c_closes_a_review_the_user_opened_like_a_picker() {
    // Opened on purpose — **Hooks** turned on in `/settings` — the review is
    // a picker the user asked for, and its Ctrl+C closes it the pickers'
    // way: nothing answered, nothing recorded, the session goes on.
    let mut app = App::new();
    app.open_requested_review(vec![hooks_page(&["./a.sh"])]);
    assert!(app.config_review.as_ref().is_some_and(|r| r.requested));
    assert_eq!(
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Action::None
    );
    assert!(app.config_review.is_none());
    // The startup review is not one the user asked for.
    app.open_config_review(vec![hooks_page(&["./a.sh"])]);
    assert!(app.config_review.as_ref().is_some_and(|r| !r.requested));
}

#[test]
fn answering_a_page_moves_on_to_the_next() {
    let mut app = App::new();
    app.open_config_review(vec![hooks_page(&["./a.sh"]), servers_page(&["gh", "docs"])]);
    let Action::ReviewAnswered(first) = app.on_key(key(KeyCode::Enter)) else {
        panic!("Enter answers the hooks page");
    };
    assert_eq!(first.kind, ReviewKind::Hooks);
    let review = app
        .config_review
        .as_ref()
        .expect("the servers page follows");
    assert_eq!(review.total, 2, "the counter keeps the review's whole size");
    assert_eq!(review.pages.len(), 1);
    assert_eq!(page(&app).kind, ReviewKind::McpServers);
    assert_eq!(page(&app).selected, 0);
    let Action::ReviewAnswered(second) = app.on_key(key(KeyCode::Esc)) else {
        panic!("Esc answers the servers page");
    };
    assert_eq!(second.kind, ReviewKind::McpServers);
    assert!(app.config_review.is_none());
}

#[test]
fn the_review_owns_every_other_key() {
    let mut app = App::new();
    app.open_config_review(vec![hooks_page(&["./a.sh"])]);
    for code in [
        KeyCode::Char('x'),
        KeyCode::Char('/'),
        KeyCode::Tab,
        KeyCode::Left,
    ] {
        assert_eq!(app.on_key(key(code)), Action::None, "{code:?}");
    }
    assert!(app.input.is_empty(), "nothing reached the composer");
    assert!(app.command_menu.is_none());
    assert!(app.config_review.is_some());
}

#[test]
fn a_page_opened_over_an_open_review_queues_behind_it_once() {
    let mut app = App::new();
    app.open_config_review(vec![servers_page(&["gh"])]);
    app.open_config_review(vec![hooks_page(&["./a.sh"])]);
    let review = app.config_review.as_ref().expect("still open");
    assert_eq!(review.total, 2);
    assert_eq!(
        page(&app).kind,
        ReviewKind::McpServers,
        "the page on screen stays"
    );
    // A page of a kind already queued is not asked twice.
    app.open_config_review(vec![hooks_page(&["./b.sh"])]);
    assert_eq!(app.config_review.as_ref().map(|r| r.pages.len()), Some(2));
}

#[test]
fn a_page_counts_its_new_and_changed_items() {
    let mut changed = hook_item("./b.sh");
    changed.change = Some(ItemChange::Was("./old.sh".to_string()));
    let page = ReviewPage::new(
        ReviewKind::Hooks,
        vec![ReviewSection {
            scope: None,
            path: "~/x".to_string(),
            items: vec![hook_item("./a.sh"), changed],
        }],
    );
    assert_eq!(page.counts(), (1, 1));
    assert_eq!(page.item_count(), 2);
}
