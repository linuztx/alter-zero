//! The inline `/theme` picker view (`docs/theme.md`).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;
use crate::app::Theme;
use crate::ui::palette::{active_theme, palette_of};
use crate::ui::theme::{HOOKS_MARKER, MODEL_INDENT, THEME_HINT, THEME_SEARCH_ROW};
use crate::ui::theme_view::{theme_menu_rows, theme_view_lines};

/// A terminal wide enough to seat the preview beside the list — codex's
/// layout.
const WIDE: u16 = 100;

/// A terminal too narrow for that: the preview stacks under the list.
const NARROW: u16 = 60;

/// The preview's sample, fragment by fragment: the user bubble, the `Edit`
/// diff cell (head, context row, the removed and added rows) and the reply
/// with its inline code and fenced code line.
const PREVIEW: [&str; 8] = [
    "❯ Rename the greeting in greet.py",
    "● Edit(greet.py)",
    "⎿  Updated greet.py (+1 -1)",
    "def greet(name):",
    "-    print(\"Hello, world\")",
    "+    print(f\"Hello, {name}\")",
    "● Done — greet.py greets by name now:",
    "greet(\"Alter Zero\")  # Hello, Alter Zero",
];

/// An app with session info and the picker open (the ordinary shape).
fn open_app() -> App {
    let mut app = with_session();
    app.open_theme_picker();
    app
}

fn texts(app: &App, width: u16) -> Vec<String> {
    theme_view_lines(app, width).iter().map(plain).collect()
}

/// Whether `text` is the list row for `name`: the two-column inset, then the
/// `❯` marker or its two-space stand-in, then the name. The search line, the
/// preview's bubble and a description mentioning the same word don't count.
fn is_row_for(text: &str, name: &str) -> bool {
    let Some(rest) = text.strip_prefix(MODEL_INDENT) else {
        return false;
    };
    let Some(rest) = rest
        .strip_prefix(HOOKS_MARKER)
        .or_else(|| rest.strip_prefix("  "))
    else {
        return false;
    };
    rest.starts_with(name) && rest[name.len()..].chars().next().is_none_or(|c| c == ' ')
}

/// The page's row for `name`.
fn row_for<'a>(texts: &'a [String], name: &str) -> &'a String {
    texts
        .iter()
        .find(|t| is_row_for(t, name))
        .unwrap_or_else(|| panic!("no row for {name}: {texts:?}"))
}

/// The styled list row for `name`.
fn line_for<'a>(lines: &'a [Line<'static>], name: &str) -> &'a Line<'static> {
    lines
        .iter()
        .find(|l| is_row_for(&plain(l), name))
        .unwrap_or_else(|| panic!("no row for {name}"))
}

/// The preview's `● Edit(greet.py)` header line.
fn edit_header<'a>(lines: &'a [Line<'static>]) -> &'a Line<'static> {
    lines
        .iter()
        .find(|l| plain(l).contains("Edit(greet.py)"))
        .expect("the preview's Edit header")
}

/// The colour of the `Edit` header's bullet — the span just ahead of the bold
/// tool name, wherever the header sits on its row.
fn edit_bullet(lines: &[Line<'static>]) -> Option<Color> {
    let line = edit_header(lines);
    let name = line
        .spans
        .iter()
        .position(|s| s.content == "Edit")
        .expect("the tool name span");
    line.style.patch(line.spans[name - 1].style).fg
}

/// The ground under the span carrying `text` — the line's own style patched
/// with the span's, which is what the paint shows.
fn ground_under(line: &Line<'static>, text: &str) -> Option<Color> {
    let span = line
        .spans
        .iter()
        .find(|s| s.content.contains(text))
        .unwrap_or_else(|| panic!("no span carries {text:?}: {line:?}"));
    line.style.patch(span.style).bg
}

/// The display column `needle` starts at in `text`.
fn column_of(text: &str, needle: &str) -> Option<usize> {
    text.find(needle)
        .map(|at| crate::ui::wrap::cols(&text[..at]))
}

/// Whether the page at this width seats the preview beside the list: the
/// bubble shares its row with a list row.
fn preview_beside(app: &App, width: u16) -> bool {
    texts(app, width).iter().any(|t| {
        t.contains("Rename the greeting") && Theme::ALL.iter().any(|th| is_row_for(t, th.name()))
    })
}

#[test]
fn closed_builds_nothing() {
    assert!(theme_view_lines(&with_session(), 80).is_empty());
    assert_eq!(theme_picker_height(&with_session(), 80, 200), None);
}

#[test]
fn the_page_is_framed_with_search_rows_counter_preview_and_hint() {
    for width in [NARROW, WIDE] {
        let texts = texts(&open_app(), width);
        let is_rule = |t: &String| !t.trim().is_empty() && t.trim().chars().all(|c| c == '─');
        assert!(is_rule(texts.first().expect("a top rule")), "top rule");
        assert!(is_rule(texts.last().expect("a bottom rule")), "bottom rule");
        assert_eq!(
            texts[usize::from(THEME_SEARCH_ROW)].trim_end(),
            "  ❯",
            "the ❯ search line on its row at {width}: {texts:?}"
        );
        assert!(
            row_for(&texts, "mocha").starts_with("  ❯ mocha"),
            "the open seats the chevron on the default at {width}: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("(1/11)")),
            "the counter over the eleven-theme catalog at {width}: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains(Theme::Mocha.description())),
            "the highlighted theme's description at {width}: {texts:?}"
        );
        // One dim row, `…`-cut on a terminal too narrow for all of it (the
        // family's hint rule) — whole from 68 columns.
        assert!(
            texts.iter().any(|t| t
                .trim_start()
                .starts_with("Type to search · ↑/↓ to navigate")),
            "the key hint at {width}: {texts:?}"
        );
    }
    assert!(
        texts(&open_app(), 68)
            .iter()
            .any(|t| t.trim() == THEME_HINT),
        "the whole hint at 68 columns"
    );
}

#[test]
fn the_hint_names_the_arrow_keys() {
    assert_eq!(
        THEME_HINT,
        "Type to search · ↑/↓ to navigate · Enter to choose · Esc to cancel"
    );
}

#[test]
fn the_rows_wear_the_mcp_lists_chevron() {
    // `  ❯ mocha` on the highlight, `    macchiato` everywhere else — the
    // `/mcp` lists' rows, the two-column inset and the marker's own width.
    for width in [NARROW, WIDE] {
        let lines = theme_view_lines(&open_app(), width);
        let texts: Vec<String> = lines.iter().map(plain).collect();
        assert!(
            row_for(&texts, "mocha").starts_with("  ❯ mocha "),
            "the highlight wears the chevron at {width}: {texts:?}"
        );
        assert!(
            row_for(&texts, "macchiato").starts_with("    macchiato "),
            "an unselected row keeps the marker's width at {width}: {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains('→')),
            "no arrow marker is left at {width}: {texts:?}"
        );
        let accent = palette_of(Theme::Mocha).accent;
        let selected = line_for(&lines, "mocha");
        let chevron = selected
            .spans
            .iter()
            .find(|s| s.content == HOOKS_MARKER)
            .expect("the chevron span");
        assert_eq!(chevron.style.fg, Some(accent), "the chevron is the accent");
        let name = selected
            .spans
            .iter()
            .find(|s| s.content == "mocha")
            .expect("the selected name");
        assert_eq!(name.style.fg, Some(accent), "so is the selected name");
        assert!(name.style.add_modifier.contains(Modifier::BOLD), "bold");
        let other = line_for(&lines, "macchiato")
            .spans
            .iter()
            .find(|s| s.content == "macchiato")
            .expect("an unselected name");
        assert_eq!(
            other.style.fg,
            Some(palette_of(Theme::Mocha).text_muted),
            "an unselected name is the muted ink"
        );
    }
}

#[test]
fn the_window_says_how_many_themes_it_hides() {
    // Ten rows over eleven themes: one is always out of view, and a dim
    // `↓ 1 more below` / `↑ 1 more above` row says which way — the `/mcp`
    // lists' overflow rows — right above the counter.
    let mut app = open_app();
    for width in [NARROW, WIDE] {
        app.theme_picker.as_mut().expect("open").selected = 0;
        let lines = theme_view_lines(&app, width);
        let shown: Vec<String> = lines.iter().map(plain).collect();
        let below = shown
            .iter()
            .position(|t| t.trim_end() == "  ↓ 1 more below")
            .unwrap_or_else(|| panic!("the hidden ansi is counted below at {width}: {shown:?}"));
        assert!(
            !shown.iter().any(|t| t.contains("more above")),
            "nothing hides above the first row at {width}: {shown:?}"
        );
        assert!(
            is_row_for(&shown[below - 1], "monokai") && shown[below + 1].starts_with("  (1/11)"),
            "under the last visible row, over the counter at {width}: {shown:?}"
        );
        assert_eq!(
            lines[below].spans[1].style.fg,
            Some(palette_of(Theme::Mocha).dim),
            "the overflow row is dim"
        );
        app.theme_picker.as_mut().expect("open").selected = Theme::ALL.len() - 1;
        let texts = texts(&app, width);
        let above = texts
            .iter()
            .position(|t| t.starts_with("  ↑ 1 more above"))
            .unwrap_or_else(|| panic!("the hidden mocha is counted above at {width}: {texts:?}"));
        assert!(
            is_row_for(&texts[above + 1], "macchiato"),
            "over the first visible row at {width}: {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains("more below")),
            "nothing hides below the last row at {width}: {texts:?}"
        );
    }
    // A search that fits the window hides nothing, so it says nothing.
    let picker = app.theme_picker.as_mut().expect("open");
    picker.query = "ca".to_string();
    picker.selected = 0;
    assert!(
        !texts(&app, WIDE).iter().any(|t| t.contains(" more ")),
        "no overflow row over four catppuccin flavours"
    );
}

#[test]
fn a_wide_terminal_seats_the_preview_beside_the_list() {
    let app = open_app();
    let texts = texts(&app, WIDE);
    // Every fragment is whole, and every one shares its row with the list
    // column (a list row, an overflow row or the counter) — beside it, not
    // under it.
    let first = texts
        .iter()
        .position(|t| t.trim_start().starts_with('❯') && is_row_for(t, "mocha"))
        .expect("the first list row");
    let counter = texts
        .iter()
        .position(|t| t.contains("(1/11)"))
        .expect("the counter");
    for fragment in PREVIEW {
        let at = texts
            .iter()
            .position(|t| t.contains(fragment))
            .unwrap_or_else(|| panic!("missing {fragment:?}: {texts:?}"));
        assert!(
            (first..=counter).contains(&at),
            "{fragment:?} sits beside the list column: {texts:?}"
        );
    }
    // The preview's cells all open in one column, right of the list's widest
    // row: the bubble's `❯`, the two `●` bullets.
    let col = |fragment: &str| {
        let line = texts
            .iter()
            .find(|t| t.contains(fragment))
            .expect("present");
        column_of(line, fragment).expect("found")
    };
    let preview_col = col("❯ Rename the greeting");
    assert_eq!(col("● Edit(greet.py)"), preview_col, "{texts:?}");
    assert_eq!(col("● Done — greet.py"), preview_col, "{texts:?}");
    // …clear of the list column, a gap between them on every row (the list
    // column's glyphs are all one column wide, so a char is a column).
    for t in &texts[first..=counter] {
        let list_part: String = t.chars().take(preview_col).collect();
        assert!(
            crate::ui::wrap::cols(list_part.trim_end()) + 2 <= preview_col,
            "a list row runs into the preview column: {t:?}"
        );
    }
}

#[test]
fn a_narrow_terminal_stacks_the_preview_under_the_list() {
    let texts = texts(&open_app(), NARROW);
    let counter = texts
        .iter()
        .position(|t| t.contains("(1/11)"))
        .expect("the counter");
    for fragment in PREVIEW {
        let at = texts
            .iter()
            .position(|t| t.contains(fragment))
            .unwrap_or_else(|| panic!("missing {fragment:?}: {texts:?}"));
        assert!(at > counter, "{fragment:?} sits under the list: {texts:?}");
    }
    assert!(
        texts
            .iter()
            .filter(|t| t.contains("Rename the greeting"))
            .all(|t| t.starts_with("  ❯ Rename")),
        "the stacked preview is inset two columns: {texts:?}"
    );
    assert!(
        !Theme::ALL.iter().any(|th| {
            texts.iter().any(|t| {
                is_row_for(t, th.name())
                    && (t.contains("Rename") || t.contains("Edit(") || t.contains('⎿'))
            })
        }),
        "no list row carries a preview fragment: {texts:?}"
    );
}

#[test]
fn an_80_column_terminal_gets_the_side_by_side_layout() {
    // The narrowest width that seats the preview beside the list leaves it
    // room for its widest row (nothing in it wraps), every wider terminal
    // keeps the layout, and the standard 80 columns are past it.
    let app = open_app();
    let threshold = (24..=200)
        .find(|&w| preview_beside(&app, w))
        .expect("some width seats the preview beside the list");
    assert!(
        threshold <= 80,
        "an 80-column terminal is side by side (threshold {threshold})"
    );
    assert!(
        (threshold..=200).all(|w| preview_beside(&app, w)),
        "every wider terminal keeps it"
    );
    let texts = texts(&app, threshold);
    for fragment in PREVIEW {
        assert!(
            texts.iter().any(|t| t.contains(fragment)),
            "{fragment:?} is whole at the threshold ({threshold}): {texts:?}"
        );
    }
}

#[test]
fn side_by_side_the_page_fits_a_standard_terminal() {
    // The point of the layout: list and preview side by side are one block
    // the height of the list, so the whole picker fits an 80×24 terminal
    // instead of flowing half its rows into scrollback.
    let rows = theme_view_lines(&open_app(), 80).len();
    assert!(rows <= 24, "the page is {rows} rows");
    assert!(
        rows < theme_view_lines(&open_app(), NARROW).len(),
        "shorter than the stacked page"
    );
}

#[test]
fn the_preview_column_holds_still_while_browsing() {
    // The list column is sized off the whole catalog, so neither the
    // highlight, the ✓ scrolling out of view, nor a search narrowing the
    // rows moves the preview sideways.
    let mut app = open_app();
    let mut columns = Vec::new();
    for query in ["", "ca", "o"] {
        let len = {
            let picker = app.theme_picker.as_mut().expect("open");
            picker.query = query.to_string();
            picker.selected = 0;
            app.theme_rows().len()
        };
        for i in 0..len {
            app.theme_picker.as_mut().expect("open").selected = i;
            let texts = texts(&app, WIDE);
            let bubble = texts
                .iter()
                .find(|t| t.contains("❯ Rename the greeting"))
                .expect("the bubble");
            columns.push(column_of(bubble, "❯ Rename the greeting").expect("found"));
        }
    }
    assert!(
        columns.windows(2).all(|pair| pair[0] == pair[1]),
        "the preview column moved: {columns:?}"
    );
}

#[test]
fn the_rows_wear_their_own_swatches() {
    // Five swatches per row, each in that theme's own accent, link,
    // success, warning and error — read off its palette, never the active
    // theme's, so Dracula's row is Dracula-coloured while Mocha is active.
    for width in [NARROW, WIDE] {
        let lines = theme_view_lines(&open_app(), width);
        for theme in Theme::ALL.iter().take(8) {
            let p = palette_of(*theme);
            let swatches: Vec<Color> = line_for(&lines, theme.name())
                .spans
                .iter()
                .filter(|s| s.content == "●")
                .filter_map(|s| s.style.fg)
                .collect();
            assert_eq!(
                swatches,
                [p.accent, p.link, p.success, p.warning, p.error],
                "{theme:?}'s swatches at {width}"
            );
        }
    }
    assert_eq!(active_theme(), Theme::Mocha, "the swatches leak nothing");
}

#[test]
fn the_preview_is_the_sample_conversations_real_cells() {
    for width in [NARROW, WIDE] {
        let texts = texts(&open_app(), width);
        for fragment in PREVIEW {
            assert!(
                texts.iter().any(|t| t.contains(fragment)),
                "missing {fragment:?} at {width}: {texts:?}"
            );
        }
    }
}

#[test]
fn the_preview_wears_the_highlighted_theme_and_the_frame_the_active_one() {
    for width in [NARROW, WIDE] {
        let mut app = open_app();
        let mocha = theme_view_lines(&app, width);
        assert_eq!(
            edit_bullet(&mocha),
            Some(palette_of(Theme::Mocha).success),
            "the Edit bullet is Mocha's green while Mocha is highlighted"
        );
        // dracula is the sixth row.
        app.theme_picker.as_mut().expect("open").selected = 5;
        let dracula = theme_view_lines(&app, width);
        assert_eq!(
            edit_bullet(&dracula),
            Some(palette_of(Theme::Dracula).success),
            "…and Dracula's green once Dracula is highlighted"
        );
        // The code line is syntax-coloured by the highlighted theme's own
        // code theme, so the two previews disagree on the `def` keyword.
        let keyword = |lines: &[Line<'static>]| {
            let line = lines
                .iter()
                .find(|l| plain(l).contains("def greet(name):"))
                .expect("the context row");
            let span = line
                .spans
                .iter()
                .find(|s| s.content.contains("def"))
                .expect("the keyword segment");
            line.style.patch(span.style).fg
        };
        assert!(keyword(&mocha).is_some());
        assert_ne!(keyword(&mocha), keyword(&dracula), "two code themes");
        // The frame around the preview stays in the ACTIVE theme: the `❯`
        // search prompt and the highlighted row's chevron are Mocha's accent
        // on both pages, and nothing leaked.
        let prompt =
            |lines: &[Line<'static>]| lines[usize::from(THEME_SEARCH_ROW)].spans[1].style.fg;
        let mocha_accent = Some(palette_of(Theme::Mocha).accent);
        assert_eq!(prompt(&mocha), mocha_accent);
        assert_eq!(prompt(&dracula), mocha_accent);
        let chevron = line_for(&dracula, "dracula")
            .spans
            .iter()
            .find(|s| s.content == HOOKS_MARKER)
            .expect("the chevron")
            .style
            .fg;
        assert_eq!(chevron, mocha_accent, "the list wears the active accent");
        assert_eq!(active_theme(), Theme::Mocha);
        assert!(
            texts(&app, width)
                .iter()
                .any(|t| t.contains(Theme::Dracula.description())),
            "…with Dracula's description"
        );
    }
}

#[test]
fn the_diff_rows_and_the_bubble_wear_the_highlighted_themes_grounds() {
    for width in [NARROW, WIDE] {
        let mut app = open_app();
        app.theme_picker.as_mut().expect("open").selected = 3; // latte
        let lines = theme_view_lines(&app, width);
        let p = palette_of(Theme::Latte);
        let removed = lines
            .iter()
            .find(|l| plain(l).contains("-    print(\"Hello, world\")"))
            .expect("the removed row");
        assert!(
            removed.spans.iter().any(
                |s| s.style.bg == Some(p.diff_del_bg) || s.style.bg == Some(p.diff_del_mark_bg)
            ),
            "the removed row is tinted in Latte's red at {width}: {removed:?}"
        );
        let added = lines
            .iter()
            .find(|l| plain(l).contains("+    print(f\"Hello, {name}\")"))
            .expect("the added row");
        assert!(
            added.spans.iter().any(
                |s| s.style.bg == Some(p.diff_add_bg) || s.style.bg == Some(p.diff_add_mark_bg)
            ),
            "the added row is tinted in Latte's green at {width}: {added:?}"
        );
        let bubble = lines
            .iter()
            .find(|l| plain(l).contains("Rename the greeting"))
            .expect("the user bubble");
        assert_eq!(
            ground_under(bubble, "Rename the greeting"),
            Some(p.user_bg),
            "the bubble is Latte's pale ground at {width}"
        );
    }
}

#[test]
fn beside_the_list_the_bubble_is_grounded_to_the_edge_and_no_further_left() {
    // Painted: the bubble's ground runs from the preview column to the
    // right edge, the conversation's full-width bubble at the preview's
    // width — and never under the list row it shares the line with.
    let app = open_app();
    let page = theme_menu_rows(&app, WIDE);
    let area = Rect::new(0, 0, WIDE, page);
    let mut buf = Buffer::empty(area);
    render_theme_picker(area, &mut buf, &app);
    let y = (0..page)
        .find(|&y| row(&buf, y, WIDE).contains("Rename the greeting"))
        .expect("the bubble row");
    let text = row(&buf, y, WIDE);
    let start = u16::try_from(
        text[..text.find("❯ Rename").expect("the bubble")]
            .chars()
            .count(),
    )
    .expect("fits");
    let ground = palette_of(Theme::Mocha).user_bg;
    assert!(start > 0, "the bubble sits right of the list: {text:?}");
    for x in start..WIDE {
        assert_eq!(buf[(x, y)].bg, ground, "column {x} is the bubble's ground");
    }
    for x in 0..start {
        assert_ne!(
            buf[(x, y)].bg,
            ground,
            "column {x} is the list's, not the bubble's"
        );
    }
}

#[test]
fn the_page_height_is_stable_across_selections() {
    // The sample is the same shape for every theme and the window always
    // hides one row, so moving the selection must not move the frame (the
    // /settings always-emit rule) — even onto the ANSI theme, whose cells
    // carry no RGB.
    for width in [NARROW, 80, WIDE] {
        let mut app = open_app();
        let h0 = theme_view_lines(&app, width).len();
        for i in 1..Theme::ALL.len() {
            app.theme_picker.as_mut().expect("open").selected = i;
            assert_eq!(
                theme_view_lines(&app, width).len(),
                h0,
                "selection {i} changed the page height at {width}"
            );
        }
    }
}

#[test]
fn the_active_theme_wears_the_check_and_the_open_seats_on_it() {
    for width in [NARROW, WIDE] {
        let mut app = with_session();
        app.set_theme(Theme::Nord);
        app.open_theme_picker();
        let texts = texts(&app, width);
        assert!(
            row_for(&texts, "nord").contains('✓'),
            "nord carries the ✓ at {width}: {texts:?}"
        );
        assert_eq!(
            texts.iter().filter(|t| t.contains('✓')).count(),
            1,
            "only the active row is marked at {width}: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("(7/11)")),
            "the highlight seats on the active theme at {width}: {texts:?}"
        );
        assert!(
            row_for(&texts, "nord").starts_with("  ❯ nord"),
            "…and wears the chevron at {width}: {texts:?}"
        );
    }
}

#[test]
fn an_unmatched_search_collapses_to_placeholder_and_hint() {
    for width in [NARROW, WIDE] {
        let mut app = open_app();
        app.theme_picker.as_mut().expect("open").query = "zzz".to_string();
        let texts: Vec<String> = texts(&app, width)
            .iter()
            .map(|t| t.trim_end().to_string())
            .collect();
        let is_rule = |t: &str| !t.is_empty() && t.chars().all(|c| c == '─');
        assert_eq!(texts.len(), 9, "the collapsed page is 9 rows: {texts:?}");
        assert!(is_rule(&texts[0]), "{texts:?}");
        assert_eq!(texts[1], "", "{texts:?}");
        assert!(texts[2].contains('❯'), "the search line: {texts:?}");
        assert_eq!(texts[3], "", "{texts:?}");
        assert!(
            texts[4].contains("No matching themes"),
            "the placeholder: {texts:?}"
        );
        assert_eq!(texts[5], "", "one gap under the placeholder: {texts:?}");
        assert!(texts[6].contains("Type to search"), "the hint: {texts:?}");
        assert_eq!(texts[7], "", "{texts:?}");
        assert!(is_rule(&texts[8]), "{texts:?}");
        assert!(
            !texts
                .iter()
                .any(|t| t.contains("/11)") || t.contains(" more ")),
            "no counter and no overflow row over an empty list: {texts:?}"
        );
    }
}

#[test]
fn no_page_ever_stacks_two_blank_rows() {
    let mut app = open_app();
    for width in [NARROW, 80, WIDE] {
        for query in ["", "ca", "drac", "zzz"] {
            let picker = app.theme_picker.as_mut().expect("open");
            picker.query = query.to_string();
            picker.selected = 0;
            let texts: Vec<String> = texts(&app, width)
                .iter()
                .map(|t| t.trim_end().to_string())
                .collect();
            for pair in texts.windows(2) {
                assert!(
                    !(pair[0].is_empty() && pair[1].is_empty()),
                    "query {query:?} at {width} stacked two blank rows: {texts:?}"
                );
            }
        }
    }
}

#[test]
fn the_page_never_exceeds_the_width() {
    let mut app = open_app();
    let widths = [
        24u16, 40, 60, 66, 67, 68, 69, 70, 71, 72, 73, 74, 80, 100, 120,
    ];
    for width in widths {
        for i in 0..Theme::ALL.len() {
            app.theme_picker.as_mut().expect("open").selected = i;
            for line in theme_view_lines(&app, width) {
                assert!(
                    crate::ui::wrap::cols(&plain(&line)) <= width as usize,
                    "width {width} selection {i}: {:?} overflows",
                    plain(&line)
                );
            }
        }
    }
}

#[test]
fn height_is_the_line_count_clamped_to_the_terminal() {
    for width in [NARROW, WIDE] {
        let app = open_app();
        let lines = theme_view_lines(&app, width).len() as u16;
        assert_eq!(theme_menu_rows(&app, width), lines);
        assert_eq!(
            theme_picker_height(&app, width, 200),
            Some(lines),
            "an idle app has no strip above the picker"
        );
        assert_eq!(
            theme_picker_height(&app, width, 10),
            Some(10),
            "clamped to a short terminal"
        );
    }
}

#[test]
fn the_cursor_sits_at_the_end_of_the_search_query() {
    let mut app = open_app();
    for c in "nor".chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    for width in [NARROW, WIDE] {
        let height = theme_picker_height(&app, width, 200).expect("open");
        let area = Rect::new(0, 3, width, height);
        let (x, y) = cursor_position(area, &app);
        assert_eq!(y, 3 + THEME_SEARCH_ROW, "on the search row at {width}");
        // indent (2) + `❯ ` (2) + the 3-character query.
        assert_eq!(x, 2 + 2 + 3, "after the query at {width}");
    }
}

#[test]
fn the_picker_flows_like_its_family_and_a_keystroke_resigns_the_flow() {
    let mut app = open_app();
    let page = theme_view_lines(&app, 80).len();
    let flow = crate::ui::view_flow(&app, 80, 10, 1000).expect("flows on a 10-row terminal");
    assert_eq!(flow.lines.len(), page - 10, "the skipped top flows");
    // A still page signs its rows: moving the highlight re-flows it.
    let before = crate::ui::view_flow_signature(&app, 80, 10, 1000).expect("overflows");
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let moved = crate::ui::view_flow_signature(&app, 80, 10, 1000).expect("overflows");
    assert_ne!(before, moved, "moving the highlight re-signs the flow");
}

#[test]
fn the_picker_renders_into_a_buffer_bottom_anchored() {
    for width in [NARROW, WIDE] {
        let app = open_app();
        let area = Rect::new(0, 0, width, 12);
        let mut buf = Buffer::empty(area);
        render_theme_picker(area, &mut buf, &app);
        let rows: Vec<String> = (0..12).map(|y| row(&buf, y, width)).collect();
        assert!(
            rows[11].trim().chars().all(|c| c == '─') && !rows[11].trim().is_empty(),
            "the closing rule sits on the last row at {width}: {rows:?}"
        );
        assert!(
            rows.iter().any(|r| r.contains("Type to search")),
            "the hint stays on screen at {width}: {rows:?}"
        );
    }
}
