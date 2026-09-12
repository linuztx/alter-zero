//! The startup banner (`docs/header.md`, `docs/mascot.md`).

use super::*;
use crate::app::Mascot;
use crate::ui::theme::header_meta_color;
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
        Some(crate::ui::theme::header_meta_color()),
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
    assert_eq!(cwd.style.fg, Some(crate::ui::theme::header_meta_color()));
    let hint = lines[2]
        .spans
        .iter()
        .find(|s| s.content.contains("/login"))
        .expect("a hint token span");
    assert_eq!(hint.style.fg, Some(crate::ui::theme::header_accent_color()));
}

#[test]
fn header_mascot_carries_the_banner_gradient() {
    let lines = header_lines(&with_session(), 90);
    // Row 0 starts at column 0 (gradient t=0) → the exact cyan endpoint.
    let first = lines[0].spans.first().expect("an art span");
    assert_eq!(
        first.style.fg,
        Some(crate::ui::theme::header_gradient_start()),
        "the art starts at the accent (Mocha's sky)"
    );
    // Some art cell reaches the block's far edge (t=1) → the exact blue
    // endpoint.
    let has_blue = lines.iter().any(|l| {
        l.spans
            .iter()
            .any(|s| s.style.fg == Some(crate::ui::theme::header_gradient_end()))
    });
    assert!(has_blue, "the art ends at the link blue");
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

#[test]
fn startup_paragraph_wraps_under_the_banner_indent() {
    // Startup prose is meant to be read in full, so it wraps where the
    // status notice clamps: every row
    // wears the banner's indent and dim meta colour, no row is wider than the
    // terminal, and no word is lost at any width.
    let text = "Alter Zero sends one anonymous ping a day so its users can be counted, never your prompts.";
    let wide = startup_paragraph_lines(text, 120);
    assert_eq!(wide.len(), 1);
    assert_eq!(plain(&wide[0]), format!("  {text}"));
    let narrow = startup_paragraph_lines(text, 40);
    assert!(narrow.len() > 1, "wrapped, never clamped: {narrow:?}");
    for line in &narrow {
        let row = plain(line);
        assert!(row.starts_with("  "), "{row:?}");
        assert!(cols(&row) <= 40, "{row:?}");
        for span in line.spans.iter().filter(|s| !s.content.trim().is_empty()) {
            assert_eq!(span.style.fg, Some(header_meta_color()), "{row:?}");
        }
    }
    let joined = narrow
        .iter()
        .map(|l| plain(l).trim().to_string())
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(joined, text, "every word survives");
    // Even a width too narrow for the indent yields rows, never a panic or
    // an empty result (the render pipeline's extreme-size rule).
    assert!(!startup_paragraph_lines(text, 1).is_empty());
    assert!(!startup_paragraph_lines(text, 0).is_empty());
}

#[test]
fn telemetry_card_keeps_every_disclosure_and_opt_out_character_at_all_widths() {
    let compact = |text: &str| {
        text.chars()
            .filter(|c| !c.is_whitespace() && !"╭╮╰╯─│*`".contains(*c))
            .collect::<String>()
    };
    let expected = compact(&format!("Telemetry{}", crate::telemetry::notice()));
    assert!(telemetry_notice_lines(0).is_empty());
    for width in 1..=160 {
        let lines = telemetry_notice_lines(width);
        let rendered: String = lines.iter().map(plain).collect();
        assert_eq!(
            compact(&rendered),
            expected,
            "lost content at width {width}"
        );
        for line in &lines {
            assert!(
                line.width() <= usize::from(width),
                "overflow at {width}: {line:?}"
            );
        }
    }
    let wide = telemetry_notice_lines(120);
    assert!(
        wide.iter().all(|line| line.width() <= 78),
        "bounded reading width"
    );
    assert_eq!(wide.len(), 7, "the card stays compact on a normal terminal");
}

#[test]
fn telemetry_card_uses_the_active_theme_for_title_border_and_commands() {
    use crate::ui::theme::{border_color, header_accent_color, telemetry_text_color};

    for theme in crate::app::Theme::ALL {
        with_theme(theme, || {
            let lines = telemetry_notice_lines(80);
            let span_for = |needle: &str| {
                lines
                    .iter()
                    .flat_map(|line| &line.spans)
                    .find(|span| span.content.contains(needle))
                    .unwrap_or_else(|| panic!("missing {needle}"))
            };
            let title = span_for("Telemetry");
            assert_eq!(title.style.fg, Some(header_accent_color()));
            assert!(title.style.add_modifier.contains(Modifier::BOLD));
            assert_eq!(span_for("╭").style.fg, Some(border_color()));
            assert_eq!(span_for("Shares").style.fg, Some(telemetry_text_color()));
            assert!(
                span_for("Never")
                    .style
                    .add_modifier
                    .contains(Modifier::BOLD)
            );
            for command in ["/settings", "ALTER_ZERO_TELEMETRY=0"] {
                assert_eq!(span_for(command).style.fg, Some(header_accent_color()));
            }
        });
    }
}

#[test]
fn update_card_keeps_every_character_at_all_widths_and_shares_the_telemetry_chrome() {
    use crate::update::{DEFAULT_REPO_URL, notice};

    let compact = |text: &str| {
        text.chars()
            .filter(|c| !c.is_whitespace() && !"╭╮╰╯─│*`".contains(*c))
            .collect::<String>()
    };
    let expected = compact(&format!(
        "Update available{}",
        notice("0.1.0", "0.2.0", DEFAULT_REPO_URL)
    ));
    assert!(update_notice_lines(0, "0.1.0", "0.2.0", DEFAULT_REPO_URL).is_empty());
    for width in 1..=160 {
        let lines = update_notice_lines(width, "0.1.0", "0.2.0", DEFAULT_REPO_URL);
        let rendered: String = lines.iter().map(plain).collect();
        assert_eq!(
            compact(&rendered),
            expected,
            "lost content at width {width}"
        );
        for line in &lines {
            assert!(
                line.width() <= usize::from(width),
                "overflow at {width}: {line:?}"
            );
        }
    }
    let wide = update_notice_lines(120, "0.1.0", "0.2.0", DEFAULT_REPO_URL);
    assert!(
        wide.iter().all(|line| line.width() <= 78),
        "bounded reading width"
    );
    assert_eq!(wide.len(), 7, "the card stays compact on a normal terminal");
    // The same chrome as the telemetry card: one builder, two texts.
    let telemetry = telemetry_notice_lines(120);
    assert_eq!(
        plain(&wide[0]).chars().next(),
        plain(&telemetry[0]).chars().next()
    );
    assert_eq!(
        plain(&wide[wide.len() - 1]),
        plain(&telemetry[telemetry.len() - 1])
    );
}

#[test]
fn update_card_uses_the_active_theme_for_title_border_and_commands() {
    use crate::ui::theme::{border_color, header_accent_color, telemetry_text_color};
    use crate::update::DEFAULT_REPO_URL;

    for theme in crate::app::Theme::ALL {
        with_theme(theme, || {
            let lines = update_notice_lines(80, "0.1.0", "0.2.0", DEFAULT_REPO_URL);
            let span_for = |needle: &str| {
                lines
                    .iter()
                    .flat_map(|line| &line.spans)
                    .find(|span| span.content.contains(needle))
                    .unwrap_or_else(|| panic!("missing {needle}"))
            };
            let title = span_for("Update available");
            assert_eq!(title.style.fg, Some(header_accent_color()));
            assert!(title.style.add_modifier.contains(Modifier::BOLD));
            assert_eq!(span_for("╭").style.fg, Some(border_color()));
            assert_eq!(span_for("running").style.fg, Some(telemetry_text_color()));
            assert!(
                span_for("v0.2.0")
                    .style
                    .add_modifier
                    .contains(Modifier::BOLD),
                "the new version is the one thing to see"
            );
            // The wrapper splits a code span at its space, so the command's
            // second word stands for it; the title's `Update` is capitalised.
            for command in ["update", "/settings", "ALTER_ZERO_UPDATE_CHECK=0"] {
                assert_eq!(span_for(command).style.fg, Some(header_accent_color()));
            }
        });
    }
}
