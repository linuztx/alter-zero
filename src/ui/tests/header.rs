//! The startup banner (`docs/header.md`, `docs/mascot.md`).

use super::*;
use crate::app::Mascot;
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
fn startup_notice_is_one_dim_indented_row() {
    // The checkpoint pre-flight's `Snapshotting …` line
    // (docs/checkpoint.md): scrollback chrome committed above the banner,
    // wearing the banner's indent and dim meta colour so the two read as one
    // block; a narrow width truncates rather than wrapping, because the row
    // is a status, not prose.
    let notice = "Snapshotting 326 files (7.9 MB) for checkpoints…";
    let lines = startup_notice_lines(notice, 90);
    assert_eq!(lines.len(), 1);
    assert_eq!(plain(&lines[0]), format!("  {notice}"));
    let narrow = startup_notice_lines(notice, 24);
    assert_eq!(narrow.len(), 1, "truncated, never wrapped");
    assert!(cols(&plain(&narrow[0])) <= 24, "{:?}", plain(&narrow[0]));
    assert!(plain(&narrow[0]).ends_with('…'), "{:?}", plain(&narrow[0]));
}

#[test]
fn header_shows_mascot_title_cwd_and_hint() {
    let text = header_text(&with_session(), 90);
    // The default mascot (crest) draws flush-left.
    assert!(text.contains("▙▄▙▄▟▄▟"), "crest art: {text:?}");
    assert!(text.contains("Alter Zero"), "name: {text:?}");
    assert!(
        text.contains(&format!("(v{})", env!("CARGO_PKG_VERSION"))),
        "version: {text:?}"
    );
    assert!(
        text.contains("~/alter-zero"),
        "cwd from the session: {text:?}"
    );
    for token in ["/login", "/model", "/resume"] {
        assert!(text.contains(token), "hint token {token}: {text:?}");
    }
    // The retired wordmark tiers and tagline are gone.
    assert!(!text.contains("autonomous ai agent"), "tagline: {text:?}");
    assert!(!text.contains("╗"), "ANSI-Shadow art: {text:?}");
}

#[test]
fn header_rows_compose_art_beside_the_metadata() {
    // Row for row: `{art padded to the art width}{2-space gap}{meta}` — the
    // metadata column aligned down the block, two columns off the art's
    // right edge (the user's spec). Crest (the default) is 3 rows beside 3
    // metadata rows, so every row pairs.
    let lines = header_lines(&with_session(), 90);
    let crest = Mascot::Crest;
    let art = crest.art();
    let w = crest.art_width();
    let pad = |row: &str| format!("{row}{}", " ".repeat(w - cols(row) + 2));
    assert_eq!(
        plain(&lines[0]),
        format!("{}Alter Zero (v{})", pad(art[0]), env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(plain(&lines[1]), format!("{}~/alter-zero", pad(art[1])));
    assert_eq!(
        plain(&lines[2]),
        format!("{}/login   /model   /resume", pad(art[2]))
    );
    assert_eq!(lines.len(), art.len());
}

#[test]
fn header_meta_block_seats_lower_beside_a_taller_mascot() {
    // A mascot with more rows than the metadata seats the block **centered,
    // ties resolving downward** — the text sits low rather than hanging off
    // the art's top. A session-less banner is the live case: it carries only
    // the title and the hint (no cwd row), so every 3-row mascot puts them on
    // rows 1–2 with the art's first row standing alone above.
    for mascot in Mascot::ALL {
        let mut app = App::new();
        app.set_mascot(mascot);
        let lines = header_lines(&app, 90);
        let art = mascot.art();
        assert_eq!(
            plain(&lines[0]),
            art[0],
            "{}: row 0 is art alone, unpadded",
            mascot.name()
        );
        assert!(
            plain(&lines[1]).contains("Alter Zero"),
            "{}: title on row 1",
            mascot.name()
        );
        assert!(
            plain(&lines[2]).contains("/login"),
            "{}: hint on row 2",
            mascot.name()
        );
        assert_eq!(lines.len(), art.len(), "{}", mascot.name());
    }
}

#[test]
fn header_uses_the_selected_mascot() {
    let mut app = with_session();
    app.set_mascot(Mascot::Sprout);
    let text = header_text(&app, 90);
    assert!(text.contains("▝▛▛▀▜▜▘"), "sprout art: {text:?}");
    assert!(!text.contains("▙▄▙▄▟▄▟"), "crest art gone: {text:?}");
    // A three-row mascot beside three metadata rows: no fourth row.
    assert_eq!(header_lines(&app, 90).len(), 3);
}

#[test]
fn header_title_is_bold_name_over_dim_version() {
    let lines = header_lines(&with_session(), 90);
    let name = lines[0]
        .spans
        .iter()
        .find(|s| s.content.contains("Alter Zero"))
        .expect("the name span");
    assert!(
        name.style.add_modifier.contains(Modifier::BOLD),
        "the name is bold"
    );
    let version = lines[0]
        .spans
        .iter()
        .find(|s| s.content.contains("(v"))
        .expect("the version span");
    assert_eq!(
        version.style.fg,
        Some(crate::ui::theme::HEADER_META_COLOR),
        "the version is dim"
    );
}

#[test]
fn header_cwd_is_dim_and_hint_is_cyan() {
    let lines = header_lines(&with_session(), 90);
    let cwd = lines[1]
        .spans
        .iter()
        .find(|s| s.content.contains("~/alter-zero"))
        .expect("the cwd span");
    assert_eq!(cwd.style.fg, Some(crate::ui::theme::HEADER_META_COLOR));
    let hint = lines[2]
        .spans
        .iter()
        .find(|s| s.content.contains("/login"))
        .expect("a hint token span");
    assert_eq!(hint.style.fg, Some(crate::ui::theme::HEADER_ACCENT_COLOR));
}

#[test]
fn header_mascot_carries_the_banner_gradient() {
    let lines = header_lines(&with_session(), 90);
    // Row 0 starts at column 0 (gradient t=0) → the exact cyan endpoint.
    let first = lines[0].spans.first().expect("an art span");
    assert_eq!(
        first.style.fg,
        Some(Color::Rgb(0x56, 0xB6, 0xC2)),
        "the art starts cyan"
    );
    // Some art cell reaches the block's far edge (t=1) → the exact blue
    // endpoint.
    let has_blue = lines.iter().any(|l| {
        l.spans
            .iter()
            .any(|s| s.style.fg == Some(Color::Rgb(0x61, 0xAF, 0xEF)))
    });
    assert!(has_blue, "the art ends blue");
}

#[test]
fn header_art_rows_render_verbatim() {
    // Leading spaces survive (a `\`-continued literal would strip them and
    // shift the glyphs left) — each line's art prefix is the row exactly,
    // whatever rows the centered metadata block lands beside.
    for mascot in Mascot::ALL {
        let mut app = with_session();
        app.set_mascot(mascot);
        let lines = header_lines(&app, 90);
        for (i, art) in mascot.art().iter().enumerate() {
            assert!(
                plain(&lines[i]).starts_with(art),
                "{} art row {i} rendered verbatim: {:?}",
                mascot.name(),
                plain(&lines[i])
            );
        }
    }
}

#[test]
fn header_falls_back_to_a_text_badge_when_very_narrow() {
    let width = 20;
    let lines = header_lines(&with_session(), width);
    let text = lines.iter().map(plain).collect::<Vec<_>>().join("\n");
    assert!(
        !text.contains('█'),
        "no room for the mascot at {width}: {text:?}"
    );
    assert!(text.contains("Alter Zero"), "text badge: {text:?}");
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
fn header_without_a_session_still_shows_mascot_and_version() {
    let text = header_text(&App::new(), 90);
    assert!(text.contains('█'), "art still drawn: {text:?}");
    assert!(
        text.contains(env!("CARGO_PKG_VERSION")),
        "version: {text:?}"
    );
    assert!(
        !text.contains("~/"),
        "no cwd row without a session: {text:?}"
    );
    assert!(text.contains("/login"), "the hint still shows: {text:?}");
}

#[test]
fn header_avoids_the_smoke_reserved_strings() {
    // The banner shares the screen with the smoke suite's structural
    // counters (docs/header.md, smoke Phases 11/16/17): it must never carry
    // these markers, nor a full `─` rule / bare `❯` row.
    for width in [16u16, 24, 50, 90] {
        for mascot in Mascot::ALL {
            let mut app = with_session();
            app.set_mascot(mascot);
            let lines = header_lines(&app, width);
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
                    "width {width} {} leaks {banned:?}: {text:?}",
                    mascot.name()
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
}

// --- the banner-topped repaint tail (docs/header.md) ---

#[test]
fn banner_tail_restores_the_banner_over_the_rebuilt_tail() {
    // Every purge rebuild reconstructs scrollback from nothing, so the
    // banner unconditionally tops it — banner, spacer, then the tail.
    let out = banner_tail(
        vec![Line::raw("LOGO"), Line::raw("meta")],
        vec![Line::raw("❯ hi"), Line::raw("ok")],
    );
    let texts: Vec<String> = out.iter().map(plain).collect();
    assert_eq!(texts, ["LOGO", "meta", "", "❯ hi", "ok"]);
}

#[test]
fn banner_tail_never_clips() {
    // The banner tops the fresh scrollback whatever the conversation's
    // length (the tail itself is already capped by the rebuild's budget).
    let tail: Vec<Line<'static>> = (0..100).map(|i| Line::raw(format!("r{i}"))).collect();
    let out = banner_tail(vec![Line::raw("LOGO")], tail);
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
