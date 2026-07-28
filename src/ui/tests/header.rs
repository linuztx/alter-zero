//! The startup banner (`docs/header.md`).

use super::*;
use crate::ui::theme::HEADER_LOGO_FULL;
use crate::ui::wrap::cols;

#[test]
fn transcript_opens_with_the_header_banner() {
    // The Ctrl+O overlay shows the same conversation the inline view
    // holds, and that conversation opens with the startup banner
    // (docs/header.md): the transcript's first rows are the banner plus a
    // blank spacer, then the history walk.
    let app = transcript_fixture();
    let lines = transcript_lines(&app, 80);
    let banner = header_lines(&app, 80);
    assert!(!banner.is_empty());
    let head: Vec<String> = lines.iter().take(banner.len()).map(plain).collect();
    let want: Vec<String> = banner.iter().map(plain).collect();
    assert_eq!(head, want, "the banner tops the transcript");
    assert_eq!(
        plain(&lines[banner.len()]).trim(),
        "",
        "a spacer divides the banner from the conversation"
    );
    let texts: Vec<String> = lines
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert!(texts.iter().any(|t| t == "❯ hello"), "{texts:?}");
}

#[test]
fn header_full_shows_logo_version_cwd_tagline_and_hint() {
    let text = header_text(&with_session(), 90);
    assert!(text.contains('█'), "block wordmark art: {text:?}");
    assert!(
        text.contains(env!("CARGO_PKG_VERSION")),
        "version: {text:?}"
    );
    assert!(
        text.contains("~/alter-zero"),
        "cwd from the session: {text:?}"
    );
    assert!(text.contains("autonomous ai agent"), "tagline: {text:?}");
    assert!(text.contains("terminal ui"), "tagline: {text:?}");
    for token in ["/help", "/model", "/resume"] {
        assert!(text.contains(token), "hint token {token}: {text:?}");
    }
}

#[test]
fn header_falls_back_to_a_text_badge_when_very_narrow() {
    let width = 24;
    let lines = header_lines(&with_session(), width);
    let text = lines.iter().map(plain).collect::<Vec<_>>().join("\n");
    // Neither wordmark spells the name in literal letters — a literal
    // "ALTER ZERO" can only be the one-line text badge.
    assert!(text.contains("ALTER ZERO"), "text badge: {text:?}");
    assert!(
        text.contains(env!("CARGO_PKG_VERSION")),
        "version: {text:?}"
    );
    for line in &lines {
        assert!(
            cols(&plain(line)) <= width as usize,
            "fits {width}: {line:?}"
        );
    }
}

#[test]
fn header_logo_carries_the_cyan_to_blue_gradient() {
    let lines = header_lines(&with_session(), 90);
    // Row 0 starts at column 0 (gradient t=0) → the exact cyan endpoint.
    let first = lines[0].spans.first().expect("a logo span");
    assert_eq!(
        first.style.fg,
        Some(Color::Rgb(0x56, 0xB6, 0xC2)),
        "logo starts cyan"
    );
    // Some cell reaches the far edge (t=1) → the exact blue endpoint.
    let has_blue = lines.iter().take(6).any(|l| {
        l.spans
            .iter()
            .any(|s| s.style.fg == Some(Color::Rgb(0x61, 0xAF, 0xEF)))
    });
    assert!(has_blue, "logo ends blue");
}

#[test]
fn header_logo_rows_match_the_wordmark_art_verbatim() {
    // The `A`'s crown row leads with a space; a `\`-continued string literal
    // strips it and shifts the glyph a column left — regression guard.
    let lines = header_lines(&with_session(), 90);
    for (i, art) in HEADER_LOGO_FULL.iter().enumerate() {
        assert_eq!(&plain(&lines[i]), art, "logo row {i} rendered verbatim");
    }
    assert!(
        plain(&lines[0]).starts_with(' '),
        "the A's crown keeps its leading indent"
    );
}

#[test]
fn header_without_a_session_still_shows_logo_and_version() {
    let text = header_text(&App::new(), 90);
    assert!(text.contains('█'), "logo still drawn: {text:?}");
    assert!(
        text.contains(env!("CARGO_PKG_VERSION")),
        "version: {text:?}"
    );
}

#[test]
fn header_avoids_the_smoke_reserved_strings() {
    // The banner shares the screen with the smoke suite's structural
    // counters (docs/header.md, smoke Phases 11/16/17): it must never carry
    // these markers, nor a full `─` rule / bare `❯` row.
    for width in [24u16, 50, 90] {
        let lines = header_lines(&with_session(), width);
        let text = lines.iter().map(plain).collect::<Vec<_>>().join("\n");
        for banned in [
            "for commands",
            "dummy_model_name",
            "Happy",
            "Done for",
            "esc to interrupt",
            "Conversation interrupted",
        ] {
            assert!(
                !text.contains(banned),
                "width {width} leaks {banned:?}: {text:?}"
            );
        }
        for line in &lines {
            let row = plain(line);
            let trimmed = row.trim();
            let is_rule = !trimmed.is_empty() && trimmed.chars().all(|c| c == '─');
            assert!(!is_rule, "width {width} drew a rule row: {row:?}");
            assert_ne!(trimmed, "❯", "width {width} drew a bare prompt row");
        }
    }
}

// --- the banner-topped repaint tail (docs/header.md) ---

#[test]
fn banner_tail_restores_the_banner_over_a_short_tail() {
    // The InPlace overlay return on a short conversation: the banner the
    // window still showed comes back — banner, spacer, then the tail.
    let out = banner_tail(
        vec![Line::raw("LOGO"), Line::raw("meta")],
        vec![Line::raw("❯ hi"), Line::raw("ok")],
        10,
    );
    let texts: Vec<String> = out.iter().map(plain).collect();
    assert_eq!(texts, ["LOGO", "meta", "", "❯ hi", "ok"]);
}

#[test]
fn banner_tail_drops_the_banner_once_the_window_is_full() {
    // A conversation that already fills the repaint window: the recap
    // drops the banner — it scrolled into the terminal's kept scrollback,
    // and re-adding it on screen would duplicate it.
    let tail: Vec<Line<'static>> = (0..4).map(|i| Line::raw(format!("r{i}"))).collect();
    let out = banner_tail(vec![Line::raw("LOGO")], tail, 4);
    let texts: Vec<String> = out.iter().map(plain).collect();
    assert_eq!(texts, ["r0", "r1", "r2", "r3"]);
}

#[test]
fn banner_tail_keeps_the_banner_bottom_when_it_half_fits() {
    // Mid-scroll: the window held only the banner's bottom rows, so only
    // those come back — the top rows stay in the kept scrollback above.
    let out = banner_tail(
        vec![Line::raw("top"), Line::raw("bottom")],
        vec![Line::raw("❯ hi")],
        3,
    );
    let texts: Vec<String> = out.iter().map(plain).collect();
    assert_eq!(texts, ["bottom", "", "❯ hi"]);
}

#[test]
fn banner_tail_uncapped_never_clips() {
    // The Purge rebuild passes usize::MAX: the banner tops the fresh
    // scrollback whatever the conversation's length.
    let tail: Vec<Line<'static>> = (0..100).map(|i| Line::raw(format!("r{i}"))).collect();
    let out = banner_tail(vec![Line::raw("LOGO")], tail, usize::MAX);
    assert_eq!(out.len(), 102, "banner + spacer + every tail row");
    assert_eq!(plain(&out[0]), "LOGO");
}

// --- the startup header banner (docs/header.md) ---

/// The whole banner as one plain string (rows joined by newlines).
fn header_text(app: &App, width: u16) -> String {
    header_lines(app, width)
        .iter()
        .map(plain)
        .collect::<Vec<_>>()
        .join("\n")
}
