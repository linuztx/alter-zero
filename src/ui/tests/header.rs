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

// --- the live banner: the mascot in motion (docs/mascot-animation.md) ---

use crate::app::{MASCOT_GREETING_MS, MascotPose};
use crate::ui::header::header_lines_posed;

/// The seed the animated fixtures run on.
const MOTION_SEED: u64 = 0x5EED;

/// An app whose mascot animates, `at` milliseconds into its timeline.
fn animated(mascot: Mascot, at: u64) -> App {
    let mut app = with_session();
    app.set_mascot(mascot);
    app.seed_mascot_motion(MOTION_SEED);
    app.set_mascot_clock(Duration::from_millis(at));
    app
}

/// The first millisecond of `mascot`'s greeting at which its pose satisfies
/// `wanted` — found on the timeline, so no test spells the script's times.
fn pose_time(mascot: Mascot, wanted: impl Fn(&MascotPose) -> bool) -> u64 {
    (0..MASCOT_GREETING_MS)
        .find(|&at| wanted(&mascot.pose(MOTION_SEED, Duration::from_millis(at))))
        .unwrap_or_else(|| panic!("{}: no such pose in the greeting", mascot.name()))
}

/// When the crest's eyes are shut.
fn crest_blinks_at() -> u64 {
    pose_time(Mascot::Crest, |pose| pose.art[1] == "▝▜█████▛▘")
}

/// A line's cells: each glyph with the colour it is drawn in.
fn cells(line: &Line) -> Vec<(String, Option<Color>)> {
    line.spans
        .iter()
        .flat_map(|span| {
            span.content
                .graphemes(true)
                .map(|g| (g.to_string(), span.style.fg))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// How far `color` sits from `other`, summed over the three channels.
fn distance(color: Option<Color>, other: Color) -> u32 {
    let (r, g, b) = rgb_of(color.expect("a coloured cell"));
    let (or, og, ob) = rgb_of(other);
    u32::from(r.abs_diff(or)) + u32::from(g.abs_diff(og)) + u32::from(b.abs_diff(ob))
}

#[test]
fn the_committed_banner_wears_the_rest_pose_whatever_the_clock_says() {
    // `header_lines` is what reaches scrollback, the Ctrl+O transcript and
    // an export. Scrollback cannot be repainted, so a blink committed there
    // would be shut eyes for the rest of the session.
    let app = animated(Mascot::Crest, crest_blinks_at());
    assert_ne!(
        app.banner_pose(),
        MascotPose::rest(Mascot::Crest),
        "the fixture is mid-blink"
    );
    let still = with_session();
    assert_eq!(header_lines(&app, 90), header_lines(&still, 90));
    let mut block = header_lines(&still, 90);
    block.push(Line::default());
    assert_eq!(
        banner_block_lines(&app, 90),
        block,
        "the banner over its spacer"
    );
    assert_eq!(
        banner_tail(header_lines(&still, 90), Vec::new()),
        block,
        "exactly what a rebuild of an empty conversation writes"
    );
}

#[test]
fn the_live_banner_draws_the_pose_beside_unmoved_metadata() {
    let app = animated(Mascot::Crest, crest_blinks_at());
    let frame = banner_frame_lines(&app, 90);
    let rest = banner_block_lines(&app, 90);
    assert_eq!(frame.len(), rest.len(), "a pose never changes the height");
    assert!(
        plain(&frame[1]).starts_with("▝▜█████▛▘  ~/alter-zero"),
        "{:?}",
        plain(&frame[1])
    );
    // Only the eyes' two cells differ, on the one row they sit in.
    for (row, (now, was)) in frame.iter().zip(&rest).enumerate() {
        let changed: Vec<usize> = cells(now)
            .iter()
            .zip(cells(was))
            .enumerate()
            .filter(|(_, (a, b))| **a != *b)
            .map(|(col, _)| col)
            .collect();
        let want: &[usize] = if row == 1 { &[2, 6] } else { &[] };
        assert_eq!(changed, want, "row {row}");
    }
    assert_eq!(frame.last(), Some(&Line::default()), "the spacer row");
}

#[test]
fn no_pose_moves_the_metadata_or_the_banners_shape() {
    // Whatever a frame draws, the title, the cwd and the hint stay on their
    // rows at their column, and the banner keeps its height: the composer
    // under it never shifts for a blink.
    for mascot in Mascot::ALL {
        let mut app = with_session();
        app.set_mascot(mascot);
        let rest = header_lines(&app, 90);
        let column = mascot.art_width();
        for art in mascot.motion_frames() {
            let pose = MascotPose { art, shine: None };
            let lines = header_lines_posed(&app, mascot, &pose, 90);
            assert_eq!(lines.len(), rest.len(), "{}: {art:?}", mascot.name());
            for (i, (line, was)) in lines.iter().zip(&rest).enumerate() {
                let (now, was) = (cells(line), cells(was));
                assert_eq!(
                    &now[column.min(now.len())..],
                    &was[column.min(was.len())..],
                    "{}: {art:?} row {i}",
                    mascot.name()
                );
                assert!(
                    plain(line).starts_with(art[i]),
                    "{}: {art:?} row {i} drawn verbatim",
                    mascot.name()
                );
            }
        }
    }
}

#[test]
fn a_banner_with_nothing_moving_is_the_committed_banner() {
    // Standing still, the live block is the block scrollback holds — so a
    // frame at rest repaints nothing at all.
    let moving = crest_blinks_at();
    // Not seeded.
    let mut unseeded = with_session();
    unseeded.set_mascot_clock(Duration::from_millis(moving));
    // The row off.
    let mut off = animated(Mascot::Crest, moving);
    off.settings_mut().animate_mascot = false;
    // A conversation under it.
    let mut busy = animated(Mascot::Crest, moving);
    busy.record_user_message("hi");
    // Between clips.
    let rested = animated(Mascot::Crest, 0);
    for (what, app) in [
        ("unseeded", unseeded),
        ("row off", off),
        ("a conversation", busy),
        ("between clips", rested),
    ] {
        assert_eq!(
            banner_frame_lines(&app, 90),
            banner_block_lines(&app, 90),
            "{what}"
        );
    }
}

#[test]
fn a_sweep_lights_the_art_under_its_band_and_nothing_else() {
    use crate::ui::theme::{header_gradient_at, shimmer_highlight};
    let mascot = Mascot::Crest;
    let middle = pose_time(mascot, |pose| pose.shine.is_some_and(|p| p >= 0.5));
    let app = animated(mascot, middle);
    let frame = banner_frame_lines(&app, 90);
    let rest = banner_block_lines(&app, 90);
    // Light only: not a glyph changes.
    let text = |lines: &[Line]| lines.iter().map(plain).collect::<Vec<_>>();
    assert_eq!(text(&frame), text(&rest));
    let width = mascot.art_width();
    let (lit, unlit) = (cells(&frame[1]), cells(&rest[1]));
    // The band is over the art's middle: those cells sit nearer the
    // highlight than their own gradient colour, the far edges do not move…
    let highlight = shimmer_highlight();
    assert!(
        distance(lit[width / 2].1, highlight) < distance(unlit[width / 2].1, highlight) / 2,
        "{lit:?}"
    );
    assert_eq!(lit[0].1, Some(header_gradient_at(0, width)), "{lit:?}");
    // …and the light falls off smoothly to either side of its crest.
    let glow: Vec<u32> = (0..width)
        .map(|col| distance(lit[col].1, header_gradient_at(col, width)))
        .collect();
    let crest = glow.iter().copied().max().unwrap();
    let peak = glow.iter().position(|&g| g == crest).unwrap();
    assert!(glow[..=peak].windows(2).all(|p| p[0] <= p[1]), "{glow:?}");
    assert!(glow[peak..].windows(2).all(|p| p[0] >= p[1]), "{glow:?}");
    // The metadata beside it is untouched.
    assert_eq!(&lit[width..], &unlit[width..]);
}

#[test]
fn a_sweep_leans_like_a_glint() {
    // The band crosses the lower rows a little ahead of the upper ones — a
    // slanted streak, the way light catches a surface, rather than a bar.
    use crate::ui::theme::header_gradient_at;
    let mascot = Mascot::Bloom;
    let middle = pose_time(mascot, |pose| pose.shine.is_some_and(|p| p >= 0.5));
    let frame = banner_frame_lines(&animated(mascot, middle), 90);
    let width = mascot.art_width();
    let brightest = |row: usize| {
        let lit = cells(&frame[row]);
        (0..width.min(lit.len()))
            .filter(|&col| lit[col].0 != " ")
            .max_by_key(|&col| distance(lit[col].1, header_gradient_at(col, width)))
            .unwrap()
    };
    assert!(brightest(1) <= brightest(0), "row 1 trails row 0");
    assert!(brightest(1) < width - 1 && brightest(1) > 0);
}

#[test]
fn a_sweep_enters_and_leaves_the_art_without_a_jump() {
    // The band starts and ends off the art, so the first and last frames of
    // a sweep are the rest colours — no cell pops as the light arrives.
    for mascot in Mascot::ALL {
        let mut app = with_session();
        app.set_mascot(mascot);
        let rest = header_lines(&app, 90);
        let entering = MascotPose {
            art: mascot.art(),
            shine: Some(0.0),
        };
        assert_eq!(
            header_lines_posed(&app, mascot, &entering, 90),
            rest,
            "{}",
            mascot.name()
        );
        let leaving = MascotPose {
            art: mascot.art(),
            shine: Some(0.999),
        };
        let lines = header_lines_posed(&app, mascot, &leaving, 90);
        for (line, was) in lines.iter().zip(&rest) {
            for (cell, rested) in cells(line).iter().zip(cells(was)) {
                let (Some(_), Some(rested)) = (cell.1, rested.1) else {
                    continue;
                };
                assert!(distance(cell.1, rested) <= 6, "{}: {cell:?}", mascot.name());
            }
        }
    }
}

#[test]
fn a_spark_is_drawn_in_the_sweeps_light() {
    // The gem's twinkle is light, not gem: its spark wears the highlight the
    // sweep lifts the art toward, whatever column it opens in.
    use crate::ui::theme::shimmer_highlight;
    let mascot = Mascot::Gem;
    let at = pose_time(mascot, |pose| pose.art[2].contains('✶'));
    let frame = banner_frame_lines(&animated(mascot, at), 90);
    let spark = cells(&frame[2])
        .into_iter()
        .find(|(glyph, _)| glyph == "✶")
        .expect("the open spark");
    assert_eq!(spark.1, Some(shimmer_highlight()));
    // The stone beside it keeps the gradient.
    let stone = cells(&frame[2])
        .into_iter()
        .find(|(glyph, _)| glyph == "▀")
        .expect("the gem's tip");
    assert_ne!(stone.1, Some(shimmer_highlight()));
}

#[test]
fn the_narrow_badge_has_no_art_to_move() {
    // Too narrow for the mascot, the banner is the one-line text badge:
    // nothing on it animates, and nothing is scheduled for it.
    let app = animated(Mascot::Crest, crest_blinks_at());
    assert!(banner_shows_art(&app, 90));
    assert!(!banner_shows_art(&app, 20));
    assert_eq!(banner_frame_lines(&app, 20), banner_block_lines(&app, 20));
    // The breakpoint is the banner's own.
    for width in 0..120u16 {
        let badge = !header_text(&app, width).contains('▛');
        assert_eq!(banner_shows_art(&app, width), !badge, "at {width}");
    }
}
