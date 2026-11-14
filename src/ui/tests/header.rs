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
    assert!(
        text.contains("/ commands  @ files  ? shortcuts"),
        "the hint names the composer's three keys: {text:?}"
    );
    // The retired wordmark tiers, tagline and command-list hint are gone.
    assert!(!text.contains("autonomous ai agent"), "tagline: {text:?}");
    assert!(!text.contains("╗"), "ANSI-Shadow art: {text:?}");
    for retired in ["/login", "/model", "/resume"] {
        assert!(
            !text.contains(retired),
            "the old hint's {retired}: {text:?}"
        );
    }
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
        format!("{}/ commands  @ files  ? shortcuts", pad(art[2]))
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
            plain(&lines[2]).contains("? shortcuts"),
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
fn header_cwd_wears_the_footers_path_colour() {
    // The path is one colour wherever the app names it — the banner's cwd
    // row and the footer's cwd segment (docs/header.md, docs/footer.md).
    let lines = header_lines(&with_session(), 90);
    let cwd = lines[1]
        .spans
        .iter()
        .find(|s| s.content.contains("~/alter-zero"))
        .expect("the cwd span");
    assert_eq!(cwd.style.fg, Some(crate::ui::theme::header_cwd_color()));
    assert_eq!(
        crate::ui::theme::header_cwd_color(),
        crate::ui::theme::footer_cwd_color()
    );
    assert_ne!(
        crate::ui::theme::header_cwd_color(),
        crate::ui::theme::header_meta_color(),
        "the path stands out from the dim version beside the title"
    );
}

#[test]
fn header_hint_dresses_its_keys_like_the_shortcuts_band() {
    // Each pair is a key in the accent beside its label in the dim — the
    // `?` band's dress for its own entries (docs/shortcuts.md) — so the
    // keys are what the eye finds, and the hint reads as a preview of that
    // band under every theme.
    use crate::ui::theme::{
        HEADER_HINT, header_accent_color, shortcuts_key_color, shortcuts_text_color,
    };
    for theme in crate::app::Theme::ALL {
        with_theme(theme, || {
            let lines = header_lines(&with_session(), 90);
            let hint = &lines[2];
            let span_for = |text: &str| {
                hint.spans
                    .iter()
                    .find(|s| s.content.trim() == text)
                    .unwrap_or_else(|| panic!("{theme:?}: no {text:?} span in {hint:?}"))
            };
            for (key, label) in HEADER_HINT {
                let key = span_for(key);
                assert_eq!(key.style.fg, Some(header_accent_color()), "{theme:?}");
                assert_eq!(key.style.fg, Some(shortcuts_key_color()), "{theme:?}");
                let label = span_for(label);
                assert_eq!(label.style.fg, Some(header_meta_color()), "{theme:?}");
                assert_eq!(label.style.fg, Some(shortcuts_text_color()), "{theme:?}");
            }
        });
    }
}

#[test]
fn header_hint_spaces_its_pairs_like_the_shortcuts_band() {
    // One space joins a key to its label, and the pairs sit the `?` band's
    // narrowest gutter apart (two columns), so spacing alone groups each key
    // with its label and the keys stay the only symbols on the row — no
    // separator glyph between them (docs/header.md).
    use crate::ui::theme::{HEADER_HINT, SHORTCUTS_MIN_GUTTER};
    const {
        assert!(
            SHORTCUTS_MIN_GUTTER > 1,
            "the pairs sit wider apart than a key and its label"
        );
    }
    let pairs: Vec<String> = HEADER_HINT
        .iter()
        .map(|(key, label)| format!("{key} {label}"))
        .collect();
    let want = pairs.join(&" ".repeat(SHORTCUTS_MIN_GUTTER));
    let row = plain(&header_lines(&with_session(), 90)[2]);
    assert!(row.ends_with(&want), "{row:?} ends with {want:?}");
}

#[test]
fn header_hint_keys_open_what_they_name() {
    // The hint is a promise about the composer: from a fresh one, each key
    // it shows opens the band its label names — `/` the command palette,
    // `@` the file picker, `?` the shortcuts band. A key rebound, or a band
    // that stopped opening on it, fails here rather than on the user who
    // pressed what the banner said.
    use crate::ui::theme::HEADER_HINT;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    for (key, label) in HEADER_HINT {
        let opened = |app: &App| match *label {
            "commands" => app.command_menu.is_some(),
            "files" => app.file_search.is_some(),
            "shortcuts" => app.shortcuts_open,
            other => panic!("the banner names {other:?}; teach this test what opens it"),
        };
        let mut app = with_session();
        assert!(!opened(&app), "{label} open before {key} was pressed");
        let mut chars = key.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            panic!("the hint key {key:?} is not one keystroke");
        };
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        assert!(opened(&app), "{key} did not open the {label}");
    }
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
    assert!(
        text.contains("/ commands  @ files  ? shortcuts"),
        "the hint still shows: {text:?}"
    );
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
    use crate::update::{DEFAULT_REPO_URL, PackageManager, notice, update_command};

    let compact = |text: &str| {
        text.chars()
            .filter(|c| !c.is_whitespace() && !"╭╮╰╯─│*`".contains(*c))
            .collect::<String>()
    };
    // The card's own command, and the longest a package manager names.
    for command in [
        update_command(None),
        update_command(Some(PackageManager::Npm)),
    ] {
        let expected = compact(&format!(
            "Update available{}",
            notice("0.1.0", "0.2.0", DEFAULT_REPO_URL, &command)
        ));
        assert!(update_notice_lines(0, "0.1.0", "0.2.0", DEFAULT_REPO_URL, &command).is_empty());
        for width in 1..=160 {
            let lines = update_notice_lines(width, "0.1.0", "0.2.0", DEFAULT_REPO_URL, &command);
            let rendered: String = lines.iter().map(plain).collect();
            assert_eq!(
                compact(&rendered),
                expected,
                "lost content at width {width} for {command}"
            );
            for line in &lines {
                assert!(
                    line.width() <= usize::from(width),
                    "overflow at {width}: {line:?}"
                );
            }
        }
    }
    let wide = update_notice_lines(
        120,
        "0.1.0",
        "0.2.0",
        DEFAULT_REPO_URL,
        &update_command(None),
    );
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
            let lines = update_notice_lines(
                80,
                "0.1.0",
                "0.2.0",
                DEFAULT_REPO_URL,
                &crate::update::update_command(None),
            );
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

// ===== the mascot in motion (docs/mascot-motion.md) =====

/// The foreground colour of each display column of `line`.
fn column_colors(line: &Line) -> Vec<Option<Color>> {
    line.spans
        .iter()
        .flat_map(|span| std::iter::repeat_n(span.style.fg, cols(&span.content)))
        .collect()
}

#[test]
fn the_resting_pose_is_the_banner_as_committed() {
    use crate::app::MascotPose;
    use crate::ui::header::banner_lines;
    for mascot in Mascot::ALL {
        let mut app = with_session();
        app.set_mascot(mascot);
        for width in [90, 40, 24, 12] {
            assert_eq!(
                banner_lines(&app, mascot, &MascotPose::rest(mascot), width),
                header_lines(&app, width),
                "{} at {width}",
                mascot.name()
            );
        }
    }
}

#[test]
fn a_pose_is_drawn_in_the_resting_arts_place() {
    // Every still of every gesture swaps the art and nothing else: the same
    // rows, and the metadata column exactly where the resting banner has it.
    use crate::app::Gesture;
    use crate::ui::header::banner_lines;
    for mascot in Mascot::ALL {
        let mut app = with_session();
        app.set_mascot(mascot);
        let rest = header_lines(&app, 90);
        let w = mascot.art_width();
        for gesture in Gesture::ALL {
            for still in mascot.stills(gesture) {
                let lines = banner_lines(&app, mascot, &still.pose, 90);
                assert_eq!(lines.len(), rest.len(), "{} {gesture:?}", mascot.name());
                for (row, (line, rest_line)) in lines.iter().zip(&rest).enumerate() {
                    let text = plain(line);
                    let art: String = text.chars().take(w).collect();
                    assert_eq!(
                        art.trim_end(),
                        still.pose.rows()[row],
                        "{} {gesture:?} row {row}: the still's art",
                        mascot.name()
                    );
                    let beside: String = text.chars().skip(w).collect();
                    let rest_beside: String = plain(rest_line).chars().skip(w).collect();
                    assert_eq!(
                        beside,
                        rest_beside,
                        "{} {gesture:?} row {row}: the metadata moved",
                        mascot.name()
                    );
                }
            }
        }
    }
}

#[test]
fn a_pose_keeps_a_lone_art_row_unpadded() {
    // A session-less banner's first row is art alone; a still of it is too.
    use crate::app::Gesture;
    use crate::ui::header::banner_lines;
    let app = App::new();
    for still in Mascot::Crest.stills(Gesture::Signature) {
        let lines = banner_lines(&app, Mascot::Crest, &still.pose, 90);
        assert_eq!(plain(&lines[0]), still.pose.rows()[0]);
    }
}

#[test]
fn a_pose_changes_nothing_in_the_badge_tier() {
    // Too narrow for the art: there is nothing to move.
    use crate::app::Gesture;
    use crate::ui::header::banner_lines;
    let app = with_session();
    for gesture in Gesture::ALL {
        for still in Mascot::Crest.stills(gesture) {
            assert_eq!(
                banner_lines(&app, Mascot::Crest, &still.pose, 14),
                header_lines(&app, 14)
            );
        }
    }
}

/// The gestures of `mascot` that move no glyph: light on the resting art —
/// a glint either way, a glow, and the gem's signature, which is a flash.
fn light_gestures(mascot: Mascot) -> Vec<crate::app::Gesture> {
    use crate::app::{Gesture, Side};
    let mut gestures = vec![
        Gesture::Glint(Side::Left),
        Gesture::Glint(Side::Right),
        Gesture::Glow,
    ];
    if mascot == Mascot::Gem {
        gestures.push(Gesture::Signature);
    }
    gestures
}

#[test]
fn light_lifts_the_glyphs_it_falls_on_and_no_others() {
    // Where the light is and how strong is the pose's (`MascotPose::light`):
    // a glint's leaning band, a glow over everything. The banner colours
    // exactly the cells it lights, the strongest brightest.
    use crate::ui::header::banner_lines;
    use crate::ui::theme::{MASCOT_LIGHT_MAX_BLEND, shimmer_highlight};
    use crate::ui::wrap::blend_color;
    for mascot in Mascot::ALL {
        let mut app = with_session();
        app.set_mascot(mascot);
        let rest = header_lines(&app, 90);
        for gesture in light_gestures(mascot) {
            for still in mascot.stills(gesture) {
                let pose = still.pose;
                let lines = banner_lines(&app, mascot, &pose, 90);
                for (row, (line, rest_line)) in lines.iter().zip(&rest).enumerate() {
                    assert_eq!(plain(line), plain(rest_line), "light moves no glyph");
                    let (lit, resting) = (column_colors(line), column_colors(rest_line));
                    assert_eq!(lit.len(), resting.len());
                    for col in 0..lit.len() {
                        let light = pose.light(col, row);
                        assert_eq!(
                            lit[col] != resting[col],
                            light > 0.0,
                            "{} {gesture:?}: row {row} col {col} (light {light})",
                            mascot.name()
                        );
                        if light > 0.0 {
                            assert_eq!(
                                lit[col],
                                Some(blend_color(
                                    shimmer_highlight(),
                                    resting[col].expect("the art is coloured"),
                                    light * MASCOT_LIGHT_MAX_BLEND
                                )),
                                "{} {gesture:?}: row {row} col {col}",
                                mascot.name()
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn every_lit_still_redraws_the_banner() {
    // A still that lit nothing would be a frame drawn for nothing. (The one
    // dark still is the gem's, between the two flashes of its signature —
    // and that one is the resting banner, cell for cell.)
    use crate::app::MascotPose;
    use crate::ui::header::banner_lines;
    for mascot in Mascot::ALL {
        let mut app = with_session();
        app.set_mascot(mascot);
        let rest = header_lines(&app, 90);
        for gesture in light_gestures(mascot) {
            for still in mascot.stills(gesture) {
                let lines = banner_lines(&app, mascot, &still.pose, 90);
                if still.pose == MascotPose::rest(mascot) {
                    assert_eq!(lines, rest);
                } else {
                    assert_ne!(
                        lines,
                        rest,
                        "{} {gesture:?}: the light at {:?}/{} lights nothing",
                        mascot.name(),
                        still.pose.glint,
                        still.pose.glow
                    );
                }
            }
        }
    }
}

#[test]
fn a_glow_lights_every_glyph_alike() {
    // Not a band: the whole mascot at one strength, so the gradient under it
    // still runs cyan to blue, only paler.
    use crate::app::Gesture;
    use crate::ui::header::banner_lines;
    let app = with_session();
    let rest = header_lines(&app, 90);
    let w = Mascot::Crest.art_width();
    for still in Mascot::Crest.stills(Gesture::Glow) {
        let lines = banner_lines(&app, Mascot::Crest, &still.pose, 90);
        for (line, rest_line) in lines.iter().zip(&rest) {
            let text: Vec<char> = plain(line).chars().collect();
            let (lit, resting) = (column_colors(line), column_colors(rest_line));
            for col in 0..w.min(text.len()) {
                assert_eq!(
                    lit[col] != resting[col],
                    text[col] != ' ',
                    "col {col} of {:?}",
                    plain(line)
                );
            }
            // The metadata beside the art keeps its colours.
            assert_eq!(lit[w..], resting[w..]);
        }
    }
}

#[test]
fn light_keeps_to_the_terminals_own_colours_under_the_ansi_theme() {
    // The terminal owns a named colour's value, so light cannot mix one:
    // every cell is its resting colour or the highlight, whole. A glint
    // still crosses, and a glow still shows at its peak.
    use crate::app::Theme;
    use crate::ui::header::banner_lines;
    use crate::ui::theme::shimmer_highlight;
    with_theme(Theme::Ansi, || {
        let mut app = with_session();
        app.set_mascot(Mascot::Gem);
        let rest = header_lines(&app, 90);
        for gesture in light_gestures(Mascot::Gem) {
            let mut lit_somewhere = false;
            for still in Mascot::Gem.stills(gesture) {
                let lines = banner_lines(&app, Mascot::Gem, &still.pose, 90);
                for (line, rest_line) in lines.iter().zip(&rest) {
                    for (colour, resting) in
                        column_colors(line).iter().zip(column_colors(rest_line))
                    {
                        assert!(
                            *colour == resting || *colour == Some(shimmer_highlight()),
                            "{gesture:?}: an invented colour: {colour:?}"
                        );
                        lit_somewhere |= *colour != resting;
                    }
                }
            }
            assert!(lit_somewhere, "{gesture:?} still shows");
        }
    });
}

#[test]
fn the_banner_in_motion_is_the_resting_banner_until_its_greeting() {
    use crate::app::MOTION_GREETING_DELAY;
    let mut app = with_session();
    app.set_pulse(Duration::ZERO);
    assert_eq!(banner_motion_lines(&app, 90), Some(header_lines(&app, 90)));
    app.set_pulse(MOTION_GREETING_DELAY);
    let lit = banner_motion_lines(&app, 90).expect("in motion");
    assert_ne!(lit, header_lines(&app, 90), "the greeting's glint is on it");
    let texts = |lines: &[Line]| lines.iter().map(plain).collect::<Vec<_>>();
    assert_eq!(texts(&lit), texts(&header_lines(&app, 90)));
}

#[test]
fn a_banner_that_does_not_move_has_no_motion_lines() {
    // The first cell stills it for good…
    let mut app = with_session();
    app.record_user_message("hello");
    assert_eq!(banner_motion_lines(&app, 90), None);
    // …the setting stills it…
    let mut app = with_session();
    app.settings_mut().mascot_animation = false;
    assert_eq!(banner_motion_lines(&app, 90), None);
    // …and the badge tier has no art to move.
    let app = with_session();
    assert!(banner_motion_lines(&app, 90).is_some());
    assert_eq!(banner_motion_lines(&app, 14), None);
}

#[test]
fn the_committed_banner_and_the_transcript_never_move() {
    // `header_lines` is what reaches scrollback, the Ctrl+O transcript and
    // /export — records, and a record does not blink.
    use crate::app::{MOTION_GREETING_DELAY, MascotPose};
    let mut app = with_session();
    let rest = header_lines(&app, 90);
    app.set_pulse(Duration::ZERO);
    app.set_pulse(MOTION_GREETING_DELAY + Duration::from_millis(100));
    assert_ne!(
        app.banner_motion().expect("in motion").pose,
        MascotPose::rest(Mascot::Crest),
        "the mascot is mid-greeting"
    );
    assert_eq!(header_lines(&app, 90), rest);
    let transcript = transcript_lines(&app, 90);
    assert_eq!(&transcript[..rest.len()], &rest[..]);
}

#[test]
fn the_next_motion_frame_is_due_when_the_banners_pose_changes() {
    use crate::app::{MOTION_GREETING_DELAY, View};
    let mut app = with_session();
    app.set_pulse(Duration::ZERO);
    assert_eq!(
        mascot_motion_due(&app, 90, true),
        Some(MOTION_GREETING_DELAY)
    );
    app.set_pulse(Duration::from_millis(200));
    assert_eq!(
        mascot_motion_due(&app, 90, true),
        Some(MOTION_GREETING_DELAY - Duration::from_millis(200)),
        "what is left of the hold"
    );
    assert_eq!(
        mascot_motion_due(&app, 90, false),
        None,
        "the banner has left the screen"
    );
    assert_eq!(mascot_motion_due(&app, 14, true), None, "the badge tier");
    // Under an overlay nothing inline is on screen: no frame is asked for.
    app.view = View::ToolOutput;
    assert_eq!(mascot_motion_due(&app, 90, true), None);
}

#[test]
fn a_still_banner_asks_for_no_frames() {
    let mut app = with_session();
    app.set_pulse(Duration::ZERO);
    app.record_user_message("hello");
    assert_eq!(mascot_motion_due(&app, 90, true), None);
    let mut app = with_session();
    app.settings_mut().mascot_animation = false;
    app.open_mascot_picker();
    app.set_pulse(Duration::ZERO);
    assert_eq!(mascot_motion_due(&app, 90, true), None);
}

#[test]
fn an_open_mascot_picker_keeps_its_previews_frames_due() {
    use crate::app::{MOTION_GREETING_DELAY, mascot_motion};
    // Over a conversation the banner is still, but the preview moves.
    let mut app = with_session();
    app.record_user_message("hello");
    app.open_mascot_picker();
    app.set_pulse(Duration::from_secs(9));
    assert_eq!(
        mascot_motion_due(&app, 90, false),
        Some(MOTION_GREETING_DELAY)
    );
    // With both moving, the frame is due at the sooner of the two.
    let mut app = with_session();
    app.set_pulse(Duration::ZERO);
    app.set_pulse(Duration::from_millis(1_000));
    app.open_mascot_picker();
    app.set_pulse(Duration::from_millis(1_001));
    let banner = mascot_motion(Mascot::Crest, 0, Duration::from_millis(1_001)).holds;
    assert!(banner < MOTION_GREETING_DELAY, "mid-glint: {banner:?}");
    assert_eq!(mascot_motion_due(&app, 90, true), Some(banner));
    assert_eq!(
        mascot_motion_due(&app, 90, false),
        Some(MOTION_GREETING_DELAY),
        "only the preview is on screen"
    );
}
