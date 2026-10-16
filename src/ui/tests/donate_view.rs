//! The read-only `/donate` page (`docs/donate.md`).

use super::*;
use crate::app::DONATION_ADDRESSES;
use crate::ui::donate_view::{donate_menu_rows, donate_view_lines};
use crate::ui::theme::{
    border_color, donate_address_color, donate_caution_color, error_color, model_meta_color,
    model_selected_color,
};

/// An app with session info and the page open (the ordinary shape).
fn open_app() -> App {
    let mut app = with_session();
    app.open_donate_picker();
    app
}

/// The page with the highlight on row `selected`.
fn open_at(selected: usize) -> App {
    let mut app = open_app();
    app.donate_picker.as_mut().expect("open").selected = selected;
    app
}

/// A terminal tall enough for the whole page.
const TALL: u16 = 200;

fn texts(app: &App, width: u16) -> Vec<String> {
    donate_view_lines(app, width, TALL)
        .iter()
        .map(plain)
        .collect()
}

fn is_rule(t: &str) -> bool {
    let t = t.trim();
    !t.is_empty() && t.chars().all(|c| c == '─')
}

/// The index of the row carrying `address` verbatim.
fn address_row(texts: &[String], address: &str) -> usize {
    texts
        .iter()
        .position(|t| t.contains(address))
        .unwrap_or_else(|| panic!("{address} is on the page: {texts:?}"))
}

#[test]
fn closed_builds_nothing() {
    assert!(donate_view_lines(&with_session(), 80, TALL).is_empty());
    assert_eq!(donate_picker_height(&with_session(), 80, 200), None);
}

#[test]
fn the_page_is_framed_with_title_blurb_rows_boxes_caution_and_hint() {
    let texts = texts(&open_app(), 80);
    assert!(is_rule(texts.first().expect("a top rule")), "top rule");
    assert!(is_rule(texts.last().expect("a bottom rule")), "bottom rule");
    for expect in [
        "♥ Support Alter Zero",
        "Free and open source",
        "❯ 1. BTC  Bitcoin",
        "Network: Bitcoin (Native SegWit)",
        "  2. ETH  Ethereum",
        "Networks: Ethereum, Linea, Base, Arbitrum, BNB Chain, OP, Polygon",
        "  3. SOL  Solana",
        "Network: Solana",
        "Send each coin only over a network listed under its address",
        "cannot be recovered",
        "↑↓ navigate  enter/c copy address  esc close",
    ] {
        assert!(
            texts.iter().any(|t| t.contains(expect)),
            "{expect:?} is on the page: {texts:?}"
        );
    }
    for entry in DONATION_ADDRESSES {
        assert!(
            texts.iter().any(|t| t.contains(entry.address)),
            "{} address verbatim: {texts:?}",
            entry.ticker
        );
    }
}

#[test]
fn each_row_names_the_ticker_and_the_coin_and_nothing_after() {
    // The label is `{n}. {TICKER}  {coin}` and stops there: the networks
    // ride the caption under the box, not the row, so the row stays one
    // glance wide however many chains an address answers on. Checked as
    // the whole trimmed row rather than a substring, so a clause appended
    // after the coin fails here.
    let texts = texts(&open_app(), 80);
    for (i, entry) in DONATION_ADDRESSES.iter().enumerate() {
        let marker = if i == 0 { "❯ " } else { "" };
        let expect = format!("{marker}{}. {}  {}", i + 1, entry.ticker, entry.coin);
        assert!(
            texts.iter().any(|t| t.trim() == expect),
            "{}: the row reads {expect:?} and nothing more: {texts:?}",
            entry.ticker
        );
    }
}

#[test]
fn a_networks_caption_sits_under_each_box_naming_where_the_address_is_reachable() {
    // The label row says *what* the address is for; the caption under the
    // box says *where* it may be sent. It is the answer to the caution's
    // question, so it sits with the address rather than in a footnote —
    // and its label agrees with the count: one network is a `Network:`.
    let texts = texts(&open_app(), 100);
    for entry in DONATION_ADDRESSES {
        let row = address_row(&texts, entry.address);
        let label = if entry.networks.len() == 1 {
            "Network: "
        } else {
            "Networks: "
        };
        let expect = format!("{label}{}", entry.networks.join(", "));
        assert_eq!(
            texts[row + 2].trim(),
            expect,
            "{}: the caption sits directly under the box: {texts:?}",
            entry.ticker
        );
    }
}

#[test]
fn the_networks_caption_aligns_with_its_box_and_stays_dim() {
    // It reads as a caption of the box above it, so it is inset to the
    // box's own left wall and wears the meta ink the coin name does —
    // never the accent, which belongs to the selection alone.
    for selected in 0..DONATION_ADDRESSES.len() {
        let app = open_at(selected);
        let lines = donate_view_lines(&app, 100, TALL);
        let texts: Vec<String> = lines.iter().map(plain).collect();
        for entry in DONATION_ADDRESSES {
            let row = address_row(&texts, entry.address);
            let box_indent = texts[row - 1].len() - texts[row - 1].trim_start().len();
            let caption = &texts[row + 2];
            assert_eq!(
                caption.len() - caption.trim_start().len(),
                box_indent,
                "{}: the caption aligns with the box's wall",
                entry.ticker
            );
            for span in lines[row + 2]
                .spans
                .iter()
                .filter(|s| !s.content.trim().is_empty())
            {
                assert_eq!(
                    span.style.fg,
                    Some(model_meta_color()),
                    "{}: the caption is dim, selection {selected}",
                    entry.ticker
                );
            }
        }
    }
}

#[test]
fn a_narrow_terminal_wraps_the_networks_caption_rather_than_cutting_it() {
    // Seven chain names do not fit a 40-column pane: the caption wraps and
    // every network still reads back, because a name the reader cannot see
    // is a network they cannot know is safe. (The address wraps inside its
    // box at this width too, so the caption is found by its own label.)
    let width = 40u16;
    let texts = texts(&open_app(), width);
    let eth = DONATION_ADDRESSES[1];
    let head = texts
        .iter()
        .position(|t| t.trim().starts_with("Networks:"))
        .unwrap_or_else(|| panic!("the ETH caption is on the page: {texts:?}"));
    let rows: Vec<&str> = texts
        .iter()
        .skip(head)
        .take_while(|t| !t.trim().is_empty())
        .map(|t| t.trim())
        .collect();
    assert_eq!(
        rows.join(" "),
        format!("Networks: {}", eth.networks.join(", ")),
        "the wrapped rows read back as the whole caption: {texts:?}"
    );
    assert!(
        rows.len() > 1,
        "the caption actually wrapped at {width} columns: {texts:?}"
    );
}

#[test]
fn the_title_names_the_app_and_wears_the_banner_gradient() {
    // The one place the app speaks its name here reads APP_NAME, so the
    // page and every other sentence carrying the name can't drift apart.
    let lines = donate_view_lines(&open_app(), 80, TALL);
    let title = lines
        .iter()
        .find(|l| plain(l).contains("Support"))
        .expect("the title row");
    assert!(
        plain(title).contains(crate::APP_NAME),
        "the title names the app: {:?}",
        plain(title)
    );
    // The heart leads, in its own (red) span; the name is washed in the
    // banner's gradient — at least two distinct RGB inks across the title's
    // spans — and bold.
    let heart = title
        .spans
        .iter()
        .find(|s| s.content.contains('♥'))
        .expect("the heart");
    assert_eq!(heart.style.fg, Some(error_color()), "a red heart");
    let inks: std::collections::HashSet<(u8, u8, u8)> = title
        .spans
        .iter()
        .filter(|s| !s.content.contains('♥') && !s.content.trim().is_empty())
        .filter_map(|s| s.style.fg)
        .map(rgb_of)
        .collect();
    assert!(
        inks.len() >= 2,
        "the title is gradient-washed, not one colour: {inks:?}"
    );
    assert!(
        title
            .spans
            .iter()
            .filter(|s| s.content.contains("Support"))
            .all(|s| s.style.add_modifier.contains(Modifier::BOLD)),
        "the title is bold"
    );
}

#[test]
fn each_address_sits_in_a_rounded_box() {
    let texts = texts(&open_app(), 80);
    for entry in DONATION_ADDRESSES {
        let i = address_row(&texts, entry.address);
        let top = texts[i - 1].trim();
        let mid = texts[i].trim();
        let bottom = texts[i + 1].trim();
        assert!(
            top.starts_with('╭') && top.ends_with('╮'),
            "{}: a rounded top over the address: {top:?}",
            entry.ticker
        );
        assert!(
            mid.starts_with('│') && mid.ends_with('│'),
            "{}: the address row is walled: {mid:?}",
            entry.ticker
        );
        assert!(
            bottom.starts_with('╰') && bottom.ends_with('╯'),
            "{}: a rounded bottom under the address: {bottom:?}",
            entry.ticker
        );
        assert_eq!(
            crate::ui::wrap::cols(top),
            crate::ui::wrap::cols(mid),
            "{}: the box is a rectangle",
            entry.ticker
        );
        assert_eq!(crate::ui::wrap::cols(top), crate::ui::wrap::cols(bottom));
        // The row above the box is the address's own numbered label.
        assert!(
            texts[i - 2].contains(entry.ticker),
            "{}: the label sits over the box: {:?}",
            entry.ticker,
            texts[i - 2]
        );
    }
}

#[test]
fn the_selection_lights_its_row_and_box() {
    // The palette's rule: the whole selected row lights up in the accent
    // and so does the frame of the box under it, while the other addresses
    // keep the muted label and the dim border. Read straight off the
    // spans, for every highlight.
    for selected in 0..DONATION_ADDRESSES.len() {
        let app = open_at(selected);
        let lines = donate_view_lines(&app, 80, TALL);
        let texts: Vec<String> = lines.iter().map(plain).collect();
        for (i, entry) in DONATION_ADDRESSES.iter().enumerate() {
            let row = address_row(&texts, entry.address);
            let label = &lines[row - 2];
            let top = &lines[row - 1];
            let has_marker = plain(label).contains('❯');
            assert_eq!(
                has_marker,
                i == selected,
                "{}: the ❯ marks the selection",
                entry.ticker
            );
            let expected = if i == selected {
                model_selected_color()
            } else {
                border_color()
            };
            let border = top
                .spans
                .iter()
                .find(|s| s.content.contains('╭'))
                .expect("the box top");
            assert_eq!(
                border.style.fg,
                Some(expected),
                "{}: box border colour with row {selected} selected",
                entry.ticker
            );
            let ticker = label
                .spans
                .iter()
                .find(|s| s.content.as_ref() == entry.ticker)
                .expect("the ticker span");
            assert!(
                ticker.style.add_modifier.contains(Modifier::BOLD),
                "{}: the ticker is bold",
                entry.ticker
            );
            if i == selected {
                assert_eq!(ticker.style.fg, Some(model_selected_color()));
            } else {
                assert_ne!(ticker.style.fg, Some(model_selected_color()));
            }
        }
    }
}

#[test]
fn the_address_is_bright_and_bold_and_the_caution_is_amber() {
    let lines = donate_view_lines(&open_app(), 80, TALL);
    for entry in DONATION_ADDRESSES {
        let span = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.as_ref() == entry.address)
            .expect("the address is one span");
        assert_eq!(span.style.fg, Some(donate_address_color()));
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
    }
    let caution = lines
        .iter()
        .find(|l| plain(l).contains("cannot be recovered"))
        .expect("the caution row");
    let text = caution
        .spans
        .iter()
        .find(|s| s.content.contains("recovered"))
        .expect("the caution text span");
    assert_eq!(text.style.fg, Some(donate_caution_color()));
}

#[test]
fn a_narrow_terminal_wraps_the_address_inside_its_box() {
    // Nothing is ever cut: on a terminal too narrow to seat the whole
    // address, it wraps across the box's rows and reads back whole.
    let width = 30u16;
    let lines = donate_view_lines(&open_app(), width, TALL);
    for line in &lines {
        assert!(
            crate::ui::wrap::cols(&plain(line)) <= width as usize,
            "{:?} overflows {width}",
            plain(line)
        );
    }
    let texts: Vec<String> = lines.iter().map(plain).collect();
    // Each box's walled rows (between its ╭ and ╰), their inner text joined.
    let boxes: Vec<String> = texts
        .iter()
        .enumerate()
        .filter(|(_, t)| t.trim().starts_with('╭'))
        .map(|(i, _)| {
            texts
                .iter()
                .skip(i + 1)
                .take_while(|t| t.trim().starts_with('│'))
                .map(|t| t.trim().trim_matches('│').trim().to_string())
                .collect()
        })
        .collect();
    assert_eq!(boxes.len(), DONATION_ADDRESSES.len(), "one box per address");
    for entry in DONATION_ADDRESSES {
        assert!(
            boxes.iter().any(|b| b == entry.address),
            "{}: the wrapped rows read back as the whole address: {boxes:?}",
            entry.ticker
        );
    }
    let walled = texts.iter().filter(|t| t.trim().starts_with('│')).count();
    assert!(
        walled > DONATION_ADDRESSES.len(),
        "the addresses wrapped onto extra rows: {texts:?}"
    );
}

#[test]
fn the_page_never_exceeds_the_width() {
    for width in [20u16, 24, 40, 60, 80, 120] {
        for selected in 0..DONATION_ADDRESSES.len() {
            for line in donate_view_lines(&open_at(selected), width, TALL) {
                assert!(
                    crate::ui::wrap::cols(&plain(&line)) <= width as usize,
                    "width {width} selection {selected}: {:?} overflows",
                    plain(&line)
                );
            }
        }
    }
}

#[test]
fn no_page_ever_stacks_two_blank_rows() {
    for width in [30u16, 80] {
        for selected in 0..DONATION_ADDRESSES.len() {
            let texts: Vec<String> = texts(&open_at(selected), width)
                .iter()
                .map(|t| t.trim_end().to_string())
                .collect();
            for pair in texts.windows(2) {
                assert!(
                    !(pair[0].is_empty() && pair[1].is_empty()),
                    "width {width} selection {selected} stacked two blank rows: {texts:?}"
                );
            }
            assert_eq!(texts[1], "", "one gap under the top rule: {texts:?}");
            assert_eq!(
                texts[texts.len() - 2],
                "",
                "one gap over the bottom rule: {texts:?}"
            );
        }
    }
}

#[test]
fn height_is_the_line_count_clamped_to_the_terminal() {
    let app = open_app();
    let lines = donate_view_lines(&app, 80, TALL).len() as u16;
    assert_eq!(donate_menu_rows(&app, 80, TALL), lines);
    assert_eq!(
        donate_picker_height(&app, 80, 200),
        Some(lines),
        "an idle app has no strip above the page"
    );
    assert_eq!(
        donate_picker_height(&app, 80, 10),
        Some(10),
        "clamped to a short terminal"
    );
}

#[test]
fn the_page_flows_like_its_family() {
    // A short terminal bottom-anchors the page and flows the skipped top
    // into scrollback (docs/view-flow.md) — the page must be flow-eligible
    // like /hooks, or its title would silently clip.
    // Even the smallest page is taller than a 10-row terminal.
    let app = open_app();
    let page = donate_view_lines(&app, 80, 10).len();
    let flow = crate::ui::view_flow(&app, 80, 10, 1000).expect("flows on a 10-row terminal");
    assert_eq!(flow.lines.len(), page - 10, "the skipped top flows");
}

#[test]
fn render_paints_the_lines_and_the_cursor_hides_seated_on_the_marker() {
    let app = open_app();
    let height = donate_picker_height(&app, 80, 200).unwrap();
    let mut buf = buffer(80, height);
    render_donate_picker(buf.area, &mut buf, &app);
    assert!(row(&buf, 0, 80).starts_with('─'));
    let painted: Vec<String> = (0..height).map(|y| row(&buf, y, 80)).collect();
    for entry in DONATION_ADDRESSES {
        assert!(
            painted.iter().any(|r| r.contains(entry.address)),
            "{} painted: {painted:?}",
            entry.ticker
        );
    }
    // No text entry anywhere on the page — the /hooks rule: no hardware
    // cursor at all, while the *seat* still tracks the highlighted ❯ row.
    assert!(
        !cursor_visible(&app),
        "a read-only page has nothing for a cursor to point at"
    );
    let area = Rect::new(0, 0, 80, height);
    let marker_row = (0..height)
        .find(|&y| row(&buf, y, 80).trim_start().starts_with('❯'))
        .expect("the selected row wears the marker");
    assert_eq!(
        cursor_position(area, &app),
        (2, marker_row),
        "the seat is the highlighted row's marker"
    );
    // …and it follows the highlight.
    let app = open_at(1);
    let mut buf = buffer(80, height);
    render_donate_picker(buf.area, &mut buf, &app);
    let marker_row_2 = (0..height)
        .find(|&y| row(&buf, y, 80).trim_start().starts_with('❯'))
        .expect("the marker moved");
    assert!(marker_row_2 > marker_row);
    assert_eq!(cursor_position(area, &app), (2, marker_row_2));
}

// --- a short terminal keeps the highlighted address on screen ---

/// The page on a terminal `height` rows tall, as trimmed plain rows.
fn page_at(app: &App, width: u16, height: u16) -> Vec<String> {
    donate_view_lines(app, width, height)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

/// How many address boxes the page draws.
fn boxes(texts: &[String]) -> usize {
    texts.iter().filter(|t| t.contains('╭')).count()
}

fn has_title(texts: &[String]) -> bool {
    texts.iter().any(|t| t.contains("Support"))
}

/// The row of `entry`'s label (`1. BTC  Bitcoin`).
fn label_row(texts: &[String], index: usize) -> usize {
    let entry = &DONATION_ADDRESSES[index];
    let label = format!("{}. {}  {}", index + 1, entry.ticker, entry.coin);
    texts
        .iter()
        .position(|t| t.contains(&label))
        .unwrap_or_else(|| panic!("{label} is listed: {texts:#?}"))
}

/// The height one row short of `texts`.
fn short_of(texts: &[String]) -> u16 {
    u16::try_from(texts.len()).expect("rows") - 1
}

#[test]
fn a_short_terminal_collapses_the_addresses_the_highlight_is_not_on() {
    // The whole page boxes every address and is about thirty rows; anchored
    // on a shorter terminal it kept the last addresses and the caution, and
    // the highlighted one — the one Enter copies — flowed off the top. The
    // boxes the highlight is not on yield first: those addresses keep only
    // their label rows, and the highlighted one keeps its box and its
    // networks, while the header stays as long as it fits.
    let app = open_app();
    let whole = page_at(&app, 80, TALL);
    assert_eq!(boxes(&whole), 3, "every address boxed: {whole:#?}");
    let texts = page_at(&app, 80, short_of(&whole));
    assert!(texts.len() < whole.len(), "{texts:#?}");
    assert!(
        has_title(&texts),
        "the header stays while it fits: {texts:#?}"
    );
    assert_eq!(boxes(&texts), 1, "one box: {texts:#?}");
    assert!(
        texts
            .iter()
            .any(|t| t.contains(DONATION_ADDRESSES[0].address)),
        "{texts:#?}"
    );
    for (i, entry) in DONATION_ADDRESSES.iter().enumerate().skip(1) {
        label_row(&texts, i);
        assert!(
            !texts.iter().any(|t| t.contains(entry.address)),
            "{texts:#?}"
        );
    }
    assert!(
        texts.iter().any(|t| t.contains("Network: Bitcoin")),
        "{texts:#?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("cannot be recovered")),
        "{texts:#?}"
    );
    assert!(texts.iter().any(|t| t.contains("esc close")), "{texts:#?}");
}

#[test]
fn the_highlighted_address_opens_under_its_own_label() {
    // An accordion: the list keeps its order and the box opens in place.
    let app = open_at(1);
    let whole = page_at(&app, 80, TALL);
    let texts = page_at(&app, 80, short_of(&whole));
    let (btc, eth, sol) = (
        label_row(&texts, 0),
        label_row(&texts, 1),
        label_row(&texts, 2),
    );
    let top = texts.iter().position(|t| t.contains('╭')).expect("the box");
    assert!(btc < eth && eth < top && top < sol, "{texts:#?}");
    assert_eq!(
        top,
        eth + 1,
        "the box sits right under its label: {texts:#?}"
    );
    assert!(texts[eth].trim_start().starts_with('❯'), "{texts:#?}");
    assert!(
        texts
            .iter()
            .any(|t| t.contains(DONATION_ADDRESSES[1].address)),
        "{texts:#?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("Networks: Ethereum")),
        "{texts:#?}"
    );
}

#[test]
fn a_shorter_terminal_drops_the_header_too() {
    let app = open_app();
    let collapsed = page_at(&app, 80, short_of(&page_at(&app, 80, TALL)));
    let texts = page_at(&app, 80, short_of(&collapsed));
    assert!(texts.len() < collapsed.len(), "{texts:#?}");
    assert!(!has_title(&texts), "no header: {texts:#?}");
    assert_eq!(boxes(&texts), 1, "{texts:#?}");
    for i in 0..DONATION_ADDRESSES.len() {
        label_row(&texts, i);
    }
    assert!(
        texts.iter().any(|t| t.contains("cannot be recovered")),
        "{texts:#?}"
    );
    assert!(texts.iter().any(|t| t.contains("esc close")), "{texts:#?}");
    assert!(
        is_rule(&texts[0]) && is_rule(texts.last().expect("rows")),
        "{texts:#?}"
    );
}

#[test]
fn every_short_donate_page_fits_and_never_overflows_or_stacks_blanks() {
    for width in [40u16, 60, 80] {
        for selected in 0..DONATION_ADDRESSES.len() {
            let app = open_at(selected);
            let smallest = page_at(&app, width, 1).len();
            for height in [12u16, 16, 20, 24, 28] {
                let texts = page_at(&app, width, height);
                let context = format!("width {width} height {height} address {selected}");
                if usize::from(height) >= smallest {
                    assert!(texts.len() <= usize::from(height), "{context}: {texts:#?}");
                }
                for text in &texts {
                    assert!(
                        crate::ui::wrap::cols(text) <= usize::from(width),
                        "{context}: {text:?} overflows"
                    );
                }
                for pair in texts.windows(2) {
                    assert!(
                        !(pair[0].is_empty() && pair[1].is_empty()),
                        "{context} stacked two blank rows: {texts:#?}"
                    );
                }
            }
        }
    }
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

#[test]
fn shorter_than_the_accordion_only_the_highlighted_address_stays() {
    // Three label rows over a box and its caption make even the accordion
    // about sixteen rows. Shorter than that, the list windows to the one
    // address the highlight is on, between the ↑/↓ overflow slots that name
    // the others, and the caution and the hint stay.
    let app = open_at(1);
    let levels = levels(&app, 80);
    assert_eq!(levels.len(), 4, "{levels:#?}");
    let texts = &levels[3];
    assert!(!has_title(texts), "{texts:#?}");
    assert_eq!(boxes(texts), 1, "{texts:#?}");
    let eth = label_row(texts, 1);
    assert!(texts[eth].trim_start().starts_with('❯'), "{texts:#?}");
    assert!(texts[eth + 1].contains('╭'), "{texts:#?}");
    assert!(!texts.iter().any(|t| t.contains("1. BTC")), "{texts:#?}");
    assert!(!texts.iter().any(|t| t.contains("3. SOL")), "{texts:#?}");
    assert!(texts.iter().any(|t| t == "  ↑ 1 more above"), "{texts:#?}");
    assert!(texts.iter().any(|t| t == "  ↓ 1 more below"), "{texts:#?}");
    assert!(
        texts.iter().any(|t| t.contains("Networks: Ethereum")),
        "{texts:#?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("cannot be recovered")),
        "{texts:#?}"
    );
    assert!(texts.iter().any(|t| t.contains("esc close")), "{texts:#?}");
    assert!(
        is_rule(&texts[0]) && is_rule(texts.last().expect("rows")),
        "{texts:#?}"
    );
}

#[test]
fn on_twelve_rows_the_highlighted_label_stays_on_screen() {
    // The smoke suite's short pane: whatever the anchor skips, the row
    // saying which coin Enter copies is painted.
    for (selected, entry) in DONATION_ADDRESSES.iter().enumerate() {
        let app = open_at(selected);
        let height = donate_picker_height(&app, 90, 12).expect("open");
        let area = Rect::new(0, 0, 90, height);
        let mut buf = buffer(90, height);
        render_donate_picker(area, &mut buf, &app);
        let label = format!("{}. {}", selected + 1, entry.ticker);
        let marker_row = (0..height)
            .find(|&y| row(&buf, y, 90).trim_start().starts_with('❯'))
            .unwrap_or_else(|| panic!("address {selected}: the label is painted"));
        assert!(row(&buf, marker_row, 90).contains(&label), "{selected}");
        assert_eq!(cursor_position(area, &app), (2, marker_row), "{selected}");
    }
}
