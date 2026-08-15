//! The read-only `/hooks` menu's state and key map (`docs/hooks-menu.md`).

use super::*;

use crate::hooks::{HookEvent, HooksFile, HooksOverview};

/// PreToolUse with two matcher rows (`bash` holding two handlers, a
/// match-everything group holding one) and Stop with one handler — enough to
/// walk every level and both descent shapes.
const SAMPLE: &str = r#"{
  "hooks": {
    "PreToolUse": [
      { "matcher": "bash",
        "hooks": [
          { "type": "command", "command": "./guard.sh" },
          { "type": "command", "command": "./lint.sh" }
        ] },
      { "hooks": [ { "type": "command", "command": "./always.sh" } ] }
    ],
    "Stop": [
      { "hooks": [ { "type": "command", "command": "./done.sh" } ] }
    ]
  }
}"#;

fn sample_overview() -> HooksOverview {
    HooksOverview::from_file(&HooksFile::parse(SAMPLE).expect("fixture parses"))
}

/// An app with the menu open over the sample config — the ordinary shape the
/// loop produces after `Action::OpenHooksMenu`.
fn hooks_app() -> App {
    let mut app = App::new();
    app.open_hooks_menu(
        sample_overview(),
        Some("~/.alter-zero/hooks.json".into()),
        true,
    );
    app
}

fn level(app: &App) -> HooksLevel {
    app.hooks_menu.as_ref().expect("the menu is open").level
}

/// The index of Stop in the events list — a matcher-less event with a hook.
const STOP: usize = 6;

#[test]
fn stop_is_where_this_suite_thinks_it_is() {
    assert_eq!(HookEvent::ALL[STOP], HookEvent::Stop);
}

#[test]
fn slash_hooks_dispatches_the_open_action_mid_turn_too() {
    let mut app = App::new();
    type_chars(&mut app, "/hooks");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenHooksMenu);
    assert!(
        app.input.is_empty(),
        "running the command consumes the token"
    );
    assert!(app.command_menu.is_none());

    // Mid-turn too — the menu only replaces the composer (the /model rule);
    // browsing a config file touches nothing running.
    let mut busy = App::new();
    busy.begin_stream();
    type_chars(&mut busy, "/hooks");
    assert_eq!(busy.on_key(key(KeyCode::Enter)), Action::OpenHooksMenu);
}

#[test]
fn opening_lands_on_the_events_level_and_abandons_the_composer_bands() {
    let mut app = App::new();
    app.shortcuts_open = true;
    type_chars(&mut app, "/hoo");
    assert!(app.command_menu.is_some());
    app.open_hooks_menu(sample_overview(), None, true);
    assert!(app.command_menu.is_none());
    assert!(!app.shortcuts_open);
    assert!(app.file_search.is_none());
    assert_eq!(level(&app), HooksLevel::Events { selected: 0 });
}

#[test]
fn arrows_move_within_the_events_and_wrap_at_both_ends() {
    let mut app = hooks_app();
    let last = HookEvent::ALL.len() - 1;
    app.on_key(key(KeyCode::Up));
    assert_eq!(
        level(&app),
        HooksLevel::Events { selected: last },
        "Up from the first event wraps to the last"
    );
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        level(&app),
        HooksLevel::Events { selected: 0 },
        "Down from the last event wraps back to the first"
    );
    app.on_key(key(KeyCode::Down));
    assert_eq!(level(&app), HooksLevel::Events { selected: 1 });
    app.on_key(key(KeyCode::End));
    assert_eq!(level(&app), HooksLevel::Events { selected: last });
    app.on_key(key(KeyCode::Home));
    assert_eq!(level(&app), HooksLevel::Events { selected: 0 });
}

#[test]
fn enter_descends_events_matchers_hooks_detail_and_esc_walks_back() {
    let mut app = hooks_app();
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        level(&app),
        HooksLevel::Matchers {
            event: 0,
            selected: 0
        },
        "PreToolUse matches on the tool name, so Enter lands on its matchers"
    );
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        level(&app),
        HooksLevel::Hooks {
            event: 0,
            matcher: Some(0),
            selected: 0
        }
    );
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        level(&app),
        HooksLevel::Detail {
            event: 0,
            matcher: Some(0),
            hook: 0
        }
    );
    // Enter at the bottom of the stack does nothing.
    app.on_key(key(KeyCode::Enter));
    assert!(matches!(level(&app), HooksLevel::Detail { .. }));
    // Esc pops one frame at a time, restoring the parent's selection…
    app.on_key(key(KeyCode::Esc));
    assert_eq!(
        level(&app),
        HooksLevel::Hooks {
            event: 0,
            matcher: Some(0),
            selected: 0
        }
    );
    app.on_key(key(KeyCode::Esc));
    assert_eq!(
        level(&app),
        HooksLevel::Matchers {
            event: 0,
            selected: 0
        }
    );
    app.on_key(key(KeyCode::Esc));
    assert_eq!(level(&app), HooksLevel::Events { selected: 0 });
    // …and closes from the top.
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CloseHooksMenu);
    assert!(app.hooks_menu.is_none());
}

#[test]
fn a_matcherless_event_skips_the_matcher_level_both_ways() {
    let mut app = hooks_app();
    // Digit keys jump-activate their absolute row (the ask modal's rule).
    app.on_key(key(KeyCode::Char('7')));
    assert_eq!(
        level(&app),
        HooksLevel::Hooks {
            event: STOP,
            matcher: None,
            selected: 0
        },
        "Stop matches on nothing — no matcher level to show"
    );
    app.on_key(key(KeyCode::Esc));
    assert_eq!(
        level(&app),
        HooksLevel::Events { selected: STOP },
        "Esc returns to the event row it came from"
    );
    // Its detail page carries the matcher-less path.
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        level(&app),
        HooksLevel::Detail {
            event: STOP,
            matcher: None,
            hook: 0
        }
    );
}

#[test]
fn an_event_with_nothing_configured_still_descends_to_its_empty_state() {
    let mut app = hooks_app();
    app.on_key(key(KeyCode::Down)); // PostToolUse — nothing configured.
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        level(&app),
        HooksLevel::Matchers {
            event: 1,
            selected: 0
        },
        "the empty state keeps the event's description reachable"
    );
    app.on_key(key(KeyCode::Enter));
    assert!(
        matches!(level(&app), HooksLevel::Matchers { .. }),
        "Enter with no rows is a no-op"
    );
    app.on_key(key(KeyCode::Esc));
    assert_eq!(level(&app), HooksLevel::Events { selected: 1 });
}

#[test]
fn a_digit_past_the_rows_is_ignored() {
    let mut app = hooks_app();
    app.on_key(key(KeyCode::Char('7'))); // Stop → its one-hook list.
    app.on_key(key(KeyCode::Char('5')));
    assert_eq!(
        level(&app),
        HooksLevel::Hooks {
            event: STOP,
            matcher: None,
            selected: 0
        },
        "digit 5 names no row here — no jump, no descent"
    );
}

#[test]
fn the_menu_owns_every_key_and_ctrl_c_closes_it() {
    let mut app = hooks_app();
    assert_eq!(app.on_key(key(KeyCode::Char('x'))), Action::None);
    assert!(app.input.is_empty(), "typing never reaches the composer");
    assert_eq!(app.on_key(ctrl('o')), Action::None);
    assert_eq!(
        app.view,
        View::Conversation,
        "Ctrl+O is swallowed while the menu owns the keys"
    );
    assert_eq!(app.on_key(ctrl('c')), Action::CloseHooksMenu);
    assert!(app.hooks_menu.is_none());
}

#[test]
fn navigation_walks_the_matcher_and_hook_rows_too() {
    let mut app = hooks_app();
    app.on_key(key(KeyCode::Enter)); // PreToolUse → matchers (2 rows).
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        level(&app),
        HooksLevel::Matchers {
            event: 0,
            selected: 1
        }
    );
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        level(&app),
        HooksLevel::Matchers {
            event: 0,
            selected: 0
        },
        "Down past the last matcher row wraps to the first"
    );
    app.on_key(key(KeyCode::Up));
    assert_eq!(
        level(&app),
        HooksLevel::Matchers {
            event: 0,
            selected: 1
        },
        "Up from the first wraps back to the last"
    );
    app.on_key(key(KeyCode::Enter)); // the (all) row → its one hook.
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        level(&app),
        HooksLevel::Hooks {
            event: 0,
            matcher: Some(1),
            selected: 0
        },
        "one row — ↓ wraps onto itself"
    );
    // Digit 1 on the hooks level opens the detail page.
    app.on_key(key(KeyCode::Char('1')));
    assert_eq!(
        level(&app),
        HooksLevel::Detail {
            event: 0,
            matcher: Some(1),
            hook: 0
        }
    );
    // Esc from the detail restores the hook row, then the matcher row.
    app.on_key(key(KeyCode::Esc));
    assert_eq!(
        level(&app),
        HooksLevel::Hooks {
            event: 0,
            matcher: Some(1),
            selected: 0
        }
    );
    app.on_key(key(KeyCode::Esc));
    assert_eq!(
        level(&app),
        HooksLevel::Matchers {
            event: 0,
            selected: 1
        },
        "the matcher selection survives the round trip"
    );
}
