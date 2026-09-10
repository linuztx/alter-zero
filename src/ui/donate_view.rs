//! The read-only `/donate` page. See `docs/donate.md`.
//!
//! The `/hooks` menu's frame — the family's rules and two-column inset, the
//! `❯` marker on the highlighted row, the dim hint — over the const
//! donation-address catalog. [`donate_view_lines`] builds the whole framed
//! body line by line, so the height is content-driven and falls out as
//! `lines.len()`: the blurb, each address's networks caption and the
//! caution all wrap to the width, and on a terminal too narrow to seat an
//! address whole it wraps **inside** its box rather than overflowing the
//! frame. What sits in the frame is built from
//! parts the chrome already has — the banner's gradient on the title
//! ([`gradient_spans`]), the `/login` device page's rounded box around each
//! address — so the page reads as this app's rather than a form pasted into
//! it.

use super::header::gradient_spans;
use super::model_view::{model_placeholder_row, model_rule, model_wrapped_rows};
use super::theme::*;
use super::wrap::{clamp_spans, cols, ellipsize, wrap_output};
use super::*;

use crate::app::{DONATION_ADDRESSES, DonationAddress};

/// The title row: `♥ Support Alter Zero` — the heart in its own red, the
/// verb and the name bold and washed left-to-right in the banner's accent →
/// link gradient (the mascot's own wash, `docs/header.md`). Clamped with a
/// trailing `…` at a width that can't seat it, like every banner row.
fn title_line(width: u16) -> Line<'static> {
    let title = format!("{DONATE_TITLE_PREFIX}{HEADER_NAME}");
    let mut spans = vec![
        Span::raw(MODEL_INDENT),
        Span::styled(DONATE_HEART, Style::new().fg(donate_heart_color())),
    ];
    spans.extend(
        gradient_spans(&title, cols(&title))
            .into_iter()
            .map(|span| Span::styled(span.content, span.style.add_modifier(Modifier::BOLD))),
    );
    clamp_spans(spans, width as usize)
}

/// One address's label row: `{marker}{n}. {TICKER}  {coin}` — the `❯`
/// marker, number and ticker in the accent on the highlighted row (the
/// palette's whole-row rule), the muted ink and a blank marker on the
/// others; the ticker bold either way, the coin name dim and `…`-cut to the
/// room left (it is the one part of the row that can give). Nothing follows
/// the coin — the row says what the address is *for*, and where it may be
/// sent is the caption under its box ([`network_rows`]), which keeps this
/// row one glance wide however many chains an address answers on.
fn label_line(index: usize, entry: &DonationAddress, selected: bool, width: u16) -> Line<'static> {
    let accent = Style::new().fg(model_selected_color());
    let dim = Style::new().fg(model_meta_color());
    let (marker, marker_style, number_style, ticker_style) = if selected {
        (
            HOOKS_MARKER,
            accent,
            accent,
            accent.add_modifier(Modifier::BOLD),
        )
    } else {
        (
            "  ",
            Style::default(),
            dim,
            Style::new()
                .fg(model_id_color())
                .add_modifier(Modifier::BOLD),
        )
    };
    let number = format!("{}. ", index + 1);
    let used = cols(MODEL_INDENT)
        + cols(marker)
        + cols(&number)
        + cols(entry.ticker)
        + cols(DONATE_LABEL_GAP);
    let room = (width as usize).saturating_sub(used);
    clamp_spans(
        vec![
            Span::raw(MODEL_INDENT),
            Span::styled(marker.to_string(), marker_style),
            Span::styled(number, number_style),
            Span::styled(entry.ticker.to_string(), ticker_style),
            Span::raw(DONATE_LABEL_GAP),
            Span::styled(ellipsize(entry.coin, room), dim),
        ],
        width as usize,
    )
}

/// The address inside its rounded box, indented past the label like the
/// device page's code box. The box is sized to the address; on a width too
/// narrow to seat it the address **wraps** across the box's rows
/// (`wrap_output` hard-breaks the one long word on grapheme boundaries —
/// nothing is ever cut) and the walls follow. The border lights in the
/// accent under the highlighted row and stays the dim frame colour under the
/// others; the address itself is bright and bold on every row.
fn address_box(entry: &DonationAddress, selected: bool, width: u16) -> Vec<Line<'static>> {
    let indent = format!("{MODEL_INDENT}{DEVICE_BOX_INDENT}");
    let pad = cols(DEVICE_BOX_PAD);
    // Walls + padding on both sides + at least one content column.
    let min_box = 2 + 2 * pad + 1;
    let avail = (width as usize).saturating_sub(cols(&indent));
    let natural = cols(entry.address) + 2 * pad + 2;
    let box_width = natural.min(avail).max(min_box);
    let inner = box_width - 2 - 2 * pad;
    let border = Style::new().fg(if selected {
        model_selected_color()
    } else {
        border_color()
    });
    let address_style = Style::new()
        .fg(donate_address_color())
        .add_modifier(Modifier::BOLD);
    let bar = DEVICE_BOX_HORIZONTAL.repeat(box_width - 2);
    let mut lines = vec![clamp_spans(
        vec![
            Span::raw(indent.clone()),
            Span::styled(
                format!("{DEVICE_BOX_TOP_LEFT}{bar}{DEVICE_BOX_TOP_RIGHT}"),
                border,
            ),
        ],
        width as usize,
    )];
    for row in wrap_output(entry.address, inner as u16) {
        let fill = inner.saturating_sub(cols(&row));
        lines.push(clamp_spans(
            vec![
                Span::raw(indent.clone()),
                Span::styled(DEVICE_BOX_VERTICAL, border),
                Span::raw(DEVICE_BOX_PAD),
                Span::styled(row, address_style),
                Span::raw(" ".repeat(fill)),
                Span::raw(DEVICE_BOX_PAD),
                Span::styled(DEVICE_BOX_VERTICAL, border),
            ],
            width as usize,
        ));
    }
    lines.push(clamp_spans(
        vec![
            Span::raw(indent),
            Span::styled(
                format!("{DEVICE_BOX_BOTTOM_LEFT}{bar}{DEVICE_BOX_BOTTOM_RIGHT}"),
                border,
            ),
        ],
        width as usize,
    ));
    lines
}

/// The networks caption under an address's box: `Networks: Ethereum, Linea,
/// …` — the answer to the question the caution asks. It sits with the
/// address rather than on the label row above the box for two reasons: the
/// label stays scannable (`2. ETH  Ethereum` is one glance), and a caption
/// *under* the box reads as a note about the thing above it, which is what
/// it is. Indented to the box's own left wall for that reason, and wrapped
/// there — seven chain names do not fit a narrow pane, and a network the
/// reader cannot see is a network they cannot know is safe.
///
/// Dim on every row, selected or not, like the coin name it continues: the
/// accent belongs to the selection alone, and lighting the caption too
/// would leave the box's border competing with it for the eye.
///
/// An entry with no networks would render a dangling `Network:` here; it
/// cannot happen, and the guard is the catalog test rather than a branch —
/// `DONATION_ADDRESSES` is the one caller and every entry in it is pinned
/// non-empty, so an empty one fails the build instead of the page.
fn network_rows(entry: &DonationAddress, width: u16) -> Vec<Line<'static>> {
    let indent = format!("{MODEL_INDENT}{DEVICE_BOX_INDENT}");
    let label = if entry.networks.len() == 1 {
        DONATE_NETWORK_LABEL
    } else {
        DONATE_NETWORKS_LABEL
    };
    let text = format!("{label}{}", entry.networks.join(DONATE_NETWORK_SEPARATOR));
    let room = (width as usize).saturating_sub(cols(&indent)).max(1) as u16;
    super::wrap::wrap_text(&text, room)
        .into_iter()
        .map(|row| {
            clamp_spans(
                vec![
                    Span::raw(indent.clone()),
                    Span::styled(row, Style::new().fg(model_meta_color())),
                ],
                width as usize,
            )
        })
        .collect()
}

/// The whole framed page as lines: a top rule, the gradient title, the dim
/// blurb, one labelled box per address, the amber caution, the key hint,
/// and a bottom rule — built as **blocks** joined by exactly one blank row
/// (the `/login` page rule), so no two blank rows ever stack. What
/// [`render_donate_picker`] paints (bottom-anchored) and `donate_menu_rows`
/// counts, so the reserved height and the painted rows can never disagree
/// (`docs/view-flow.md`). Empty when the page is closed.
#[must_use]
pub fn donate_view_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let Some(picker) = app.donate_picker.as_ref() else {
        return Vec::new();
    };
    let selected = picker.selected;
    let mut blocks: Vec<Vec<Line<'static>>> = vec![
        vec![title_line(width)],
        model_wrapped_rows(DONATE_BLURB, model_meta_color(), width),
    ];
    for (i, entry) in DONATION_ADDRESSES.iter().enumerate() {
        let mut block = vec![label_line(i, entry, i == selected, width)];
        block.extend(address_box(entry, i == selected, width));
        block.extend(network_rows(entry, width));
        blocks.push(block);
    }
    blocks.push(model_wrapped_rows(
        DONATE_CAUTION,
        donate_caution_color(),
        width,
    ));
    blocks.push(vec![model_placeholder_row(
        DONATE_HINT,
        model_meta_color(),
        width,
    )]);

    let mut lines = vec![model_rule(width), Line::default()];
    for (i, block) in blocks.into_iter().filter(|b| !b.is_empty()).enumerate() {
        if i > 0 {
            lines.push(Line::default());
        }
        lines.extend(block);
    }
    lines.push(Line::default());
    lines.push(model_rule(width));
    lines
}

/// The rows the page's own frame occupies — the built page's line count
/// ([`donate_view_lines`]). What [`donate_picker_height`] reserves under the
/// strip, and what [`render_live`] hands [`render_donate_picker`].
pub(super) fn donate_menu_rows(app: &App, width: u16) -> u16 {
    u16::try_from(donate_view_lines(app, width).len()).unwrap_or(u16::MAX)
}

/// The inline live-region height when the `/donate` page is open, or `None`
/// when it isn't (the caller then falls back to [`live_height`]). Like every
/// sibling picker it **replaces** the composer — and only the composer: the
/// streaming strip keeps its rows above it, so opening `/donate` mid-turn
/// never hides the running turn. Clamped to the terminal height.
#[must_use]
pub fn donate_picker_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.donate_picker.as_ref()?;
    Some(super::layout::view_height(
        app,
        width,
        donate_menu_rows(app, width),
        term_height,
    ))
}

/// Render the **inline** `/donate` page into the live region, in place of
/// the composer: the gradient title, the blurb, each address's labelled
/// rounded box, the caution, the hint, between the family's rules —
/// bottom-anchored, so a squeezed area keeps the boxes, the hint and the
/// closing rule on screen while the skipped top flows into scrollback
/// (`docs/view-flow.md`). Pure — `render_live` paints this. See
/// `docs/donate.md`.
pub fn render_donate_picker(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(area, buf, donate_view_lines(app, area.width));
}
