//! The `/secrete` page (`docs/secrets.md`): the user's secrets listed by
//! placeholder, and the form that adds or edits one.
//!
//! The `/donate` page's frame — the family's rules and indent, the `❯`
//! marker, the banner-gradient title — built line by line so the height is
//! content-driven (`docs/view-flow.md`). The rule the page exists to keep
//! lives here: **a value is masked in the builder**, never at paint time —
//! the list shows a fixed eight-dot mask whatever the value's length, the
//! form one dot per character typed, capped at the field's width, and no
//! path puts a value into a `Line` (a flowed row is committed to real
//! scrollback as text). [`secrets_build`] returns the form's caret seat with
//! the lines, since three fields make "the row with the `❯`" a question the
//! lines alone cannot answer.

use super::header::gradient_spans;
use super::layout::text_field_width;
use super::model_view::{model_placeholder_row, model_rule, model_wrapped_rows};
use super::theme::*;
use super::wrap::{clamp_spans, cols, ellipsize};
use super::*;

use crate::app::{SecretField, SecretForm, SecretsPage};
use crate::secrets::{SecretMeta, placeholder};
use crate::textarea::TextArea;

/// A built page: its lines, and the form's caret `(column, row)` within
/// them — `None` on the list, whose cursor hides on its `❯`.
pub(super) struct SecretsBuild {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) cursor: Option<(u16, usize)>,
}

/// One block of the page and, when the caret sits in it, its
/// `(column, row within the block)`.
type Block = (Vec<Line<'static>>, Option<(u16, usize)>);

/// A title row: bold, washed in the banner's gradient like the `/donate`
/// page's, `…`-clamped at a width that cannot seat it.
fn title_line(text: &str, width: u16) -> Line<'static> {
    let mut spans = vec![Span::raw(MODEL_INDENT)];
    spans.extend(
        gradient_spans(text, cols(text))
            .into_iter()
            .map(|span| Span::styled(span.content, span.style.add_modifier(Modifier::BOLD))),
    );
    clamp_spans(spans, width as usize)
}

/// One secret's row: the `❯` marker when highlighted, the placeholder padded
/// to `name_col`, the **fixed** mask, and the context in whatever room is
/// left, `…`-cut.
fn list_row(meta: &SecretMeta, selected: bool, name_col: usize, width: u16) -> Line<'static> {
    let accent = Style::new().fg(model_selected_color());
    let dim = Style::new().fg(model_meta_color());
    let (marker, name_style) = if selected {
        (HOOKS_MARKER, accent.add_modifier(Modifier::BOLD))
    } else {
        ("  ", Style::new().fg(model_id_color()))
    };
    let name = ellipsize(&placeholder(&meta.name), name_col);
    let pad = name_col.saturating_sub(cols(&name));
    let used = cols(MODEL_INDENT)
        + cols(marker)
        + name_col
        + 2 * cols(SECRETS_COLUMN_GAP)
        + cols(SECRETS_LIST_MASK);
    let room = (width as usize).saturating_sub(used);
    let context = meta
        .context
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut spans = vec![
        Span::raw(MODEL_INDENT),
        Span::styled(marker, accent),
        Span::styled(name, name_style),
        Span::raw(" ".repeat(pad)),
        Span::raw(SECRETS_COLUMN_GAP),
        Span::styled(SECRETS_LIST_MASK, dim),
    ];
    if !context.is_empty() && room > 0 {
        spans.push(Span::raw(SECRETS_COLUMN_GAP));
        spans.push(Span::styled(ellipsize(&context, room), dim));
    }
    clamp_spans(spans, width as usize)
}

/// The list's blocks: the title and blurb, the rows (or what an empty store
/// says, over its add row), and the hint — or, while a delete waits for its
/// second `d`, the red question in its place.
fn list_blocks(metas: &[SecretMeta], page: &SecretsPage, width: u16) -> Vec<Block> {
    let accent = Style::new().fg(model_selected_color());
    let dim = model_meta_color();
    let mut blocks: Vec<Block> = Vec::new();
    let mut head = vec![title_line(SECRETS_TITLE, width)];
    head.extend(model_wrapped_rows(SECRETS_BLURB, dim, width));
    blocks.push((head, None));
    if metas.is_empty() {
        blocks.push((model_wrapped_rows(SECRETS_EMPTY, dim, width), None));
    }
    let fixed = cols(MODEL_INDENT)
        + cols(HOOKS_MARKER)
        + 2 * cols(SECRETS_COLUMN_GAP)
        + cols(SECRETS_LIST_MASK);
    let widest = metas
        .iter()
        .map(|meta| cols(&placeholder(&meta.name)))
        .max()
        .unwrap_or(0);
    let name_col = widest.min((width as usize).saturating_sub(fixed).max(1));
    let mut rows: Vec<Line<'static>> = metas
        .iter()
        .enumerate()
        .map(|(i, meta)| list_row(meta, i == page.selected, name_col, width))
        .collect();
    let adding = page.selected >= metas.len();
    rows.push(clamp_spans(
        vec![
            Span::raw(MODEL_INDENT),
            Span::styled(if adding { HOOKS_MARKER } else { "  " }, accent),
            Span::styled(
                SECRETS_ADD_ROW,
                if adding {
                    accent.add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(dim)
                },
            ),
        ],
        width as usize,
    ));
    blocks.push((rows, None));
    let hint = match &page.confirm_delete {
        Some(name) => model_wrapped_rows(&secrets_delete_question(name), error_color(), width),
        None if adding => vec![model_placeholder_row(SECRETS_ADD_HINT, dim, width)],
        None => vec![model_placeholder_row(SECRETS_LIST_HINT, dim, width)],
    };
    blocks.push((hint, None));
    blocks
}

/// A form row: the `❯` marker and the label in the accent on the focused
/// field, dim on the others — or, for a continuation row (`label` `None`),
/// blanks of the same width — then `content`, from [`SECRETS_FIELD_COL`].
fn field_line(
    label: Option<&str>,
    focused: bool,
    content: Vec<Span<'static>>,
    width: u16,
) -> Line<'static> {
    let accent = Style::new().fg(model_selected_color());
    let mut spans = vec![Span::raw(MODEL_INDENT)];
    match label {
        Some(label) => {
            let (marker, style) = if focused {
                (HOOKS_MARKER, accent.add_modifier(Modifier::BOLD))
            } else {
                ("  ", Style::new().fg(model_meta_color()))
            };
            spans.push(Span::styled(marker, accent));
            spans.push(Span::styled(
                format!("{label:<width$}", width = SECRETS_LABEL_WIDTH),
                style,
            ));
        }
        None => spans.push(Span::raw(" ".repeat(2 + SECRETS_LABEL_WIDTH))),
    }
    spans.extend(content);
    clamp_spans(spans, width as usize)
}

/// A text field's rows — its text wrapped to the field (one column kept for
/// the caret, `text_field_width`), or the dim `empty` hint — and the caret's
/// `(column, row)` within them.
fn text_rows(
    area: &TextArea,
    empty: &str,
    style: Style,
    room: usize,
) -> (Vec<Vec<Span<'static>>>, (usize, usize)) {
    if area.is_empty() {
        let hint = Span::styled(ellipsize(empty, room), Style::new().fg(model_meta_color()));
        return (vec![vec![hint]], (0, 0));
    }
    let wrap = text_field_width(u16::try_from(room).unwrap_or(u16::MAX));
    let (row, col) = area.cursor_row_col(wrap);
    let rows = area
        .display_rows(wrap)
        .into_iter()
        .map(|text| vec![Span::styled(text, style)])
        .collect();
    (rows, (col, row))
}

/// The form's blocks: the title (`New secret`, or `Edit <secrete:NAME>`)
/// and blurb; the three fields — the name previewing its placeholder, the
/// value **masked**, the context wrapped — with the caret in the focused
/// one; the refusal in red when there is one; and the hint.
fn form_blocks(form: &SecretForm, width: u16) -> Vec<Block> {
    let dim = model_meta_color();
    let ink = Style::new().fg(model_id_color());
    let room = (width as usize)
        .saturating_sub(usize::from(SECRETS_FIELD_COL))
        .max(1);
    let title = match &form.original {
        Some(name) => format!("{SECRETS_EDIT_TITLE}{}", placeholder(name)),
        None => SECRETS_NEW_TITLE.to_string(),
    };
    let mut head = vec![title_line(&title, width)];
    head.extend(model_wrapped_rows(SECRETS_FORM_BLURB, dim, width));

    let mut fields: Vec<Line<'static>> = Vec::new();
    let mut caret = (0_usize, 0_usize);
    let mut push_rows = |fields: &mut Vec<Line<'static>>,
                         label: &str,
                         field: SecretField,
                         rows: Vec<Vec<Span<'static>>>,
                         seat: (usize, usize)| {
        let focused = form.focus == field;
        if focused {
            caret = (seat.0, fields.len() + seat.1);
        }
        for (i, content) in rows.into_iter().enumerate() {
            let label = (i == 0).then_some(label);
            fields.push(field_line(label, focused, content, width));
        }
    };

    let (rows, seat) = text_rows(
        &form.name,
        SECRETS_NAME_EMPTY,
        ink.add_modifier(Modifier::BOLD),
        room,
    );
    push_rows(
        &mut fields,
        SECRETS_NAME_LABEL,
        SecretField::Name,
        rows,
        seat,
    );
    if !form.name.is_empty() {
        let preview = vec![
            Span::styled(SECRETS_USE_AS, Style::new().fg(dim)),
            Span::styled(
                placeholder(form.name.text()),
                Style::new().fg(model_selected_color()),
            ),
        ];
        fields.push(field_line(None, false, preview, width));
    }

    // The value: never its text — one dot per character typed, capped at
    // the field (one column kept for the caret), so a keystroke shows
    // without the value being readable.
    let typed = form.value.char_count();
    let (rows, seat) = if typed == 0 {
        let hint = if form.original.is_some() {
            SECRETS_VALUE_KEEP
        } else {
            SECRETS_VALUE_EMPTY
        };
        (
            vec![vec![Span::styled(
                ellipsize(hint, room),
                Style::new().fg(dim),
            )]],
            (0, 0),
        )
    } else {
        let shown = typed.min(room.saturating_sub(1).max(1));
        let mask: String = std::iter::repeat_n(LOGIN_MASK_CHAR, shown).collect();
        (vec![vec![Span::styled(mask, ink)]], (shown, 0))
    };
    push_rows(
        &mut fields,
        SECRETS_VALUE_LABEL,
        SecretField::Value,
        rows,
        seat,
    );

    let (rows, seat) = text_rows(&form.context, SECRETS_CONTEXT_EMPTY, ink, room);
    push_rows(
        &mut fields,
        SECRETS_CONTEXT_LABEL,
        SecretField::Context,
        rows,
        seat,
    );

    let col = SECRETS_FIELD_COL.saturating_add(u16::try_from(caret.0).unwrap_or(u16::MAX));
    let mut blocks: Vec<Block> = vec![(head, None), (fields, Some((col, caret.1)))];
    if let Some(error) = &form.error {
        blocks.push((
            model_wrapped_rows(
                &format!("{SECRETS_ERROR_MARK}{error}"),
                error_color(),
                width,
            ),
            None,
        ));
    }
    let hint = if form.focus == SecretField::Context {
        SECRETS_FORM_HINT_SAVE
    } else {
        SECRETS_FORM_HINT_NEXT
    };
    blocks.push((vec![model_placeholder_row(hint, dim, width)], None));
    blocks
}

/// The whole framed page and the form's caret: a top rule, the blocks
/// joined by exactly one blank row (the `/login` page rule, so no two blank
/// rows ever stack), a bottom rule. Empty when the page is closed.
pub(super) fn secrets_build(app: &App, width: u16) -> SecretsBuild {
    let Some(page) = app.secrets_page.as_ref() else {
        return SecretsBuild {
            lines: Vec::new(),
            cursor: None,
        };
    };
    let blocks = match &page.form {
        Some(form) => form_blocks(form, width),
        None => list_blocks(app.secret_metas(), page, width),
    };
    let mut lines = vec![model_rule(width), Line::default()];
    let mut cursor = None;
    for (i, (block, seat)) in blocks.into_iter().filter(|b| !b.0.is_empty()).enumerate() {
        if i > 0 {
            lines.push(Line::default());
        }
        if let Some((col, row)) = seat {
            cursor = Some((col, lines.len() + row));
        }
        lines.extend(block);
    }
    lines.push(Line::default());
    lines.push(model_rule(width));
    SecretsBuild { lines, cursor }
}

/// The page as lines (`secrets_build`'s) — what [`render_secrets_picker`]
/// paints (bottom-anchored), what `secrets_menu_rows` counts and what flows
/// (`docs/view-flow.md`), so the reserved height and the painted rows can
/// never disagree. Empty when the page is closed.
#[must_use]
pub fn secrets_view_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    secrets_build(app, width).lines
}

/// The rows the page's own frame occupies — the built page's line count.
pub(super) fn secrets_menu_rows(app: &App, width: u16) -> u16 {
    u16::try_from(secrets_view_lines(app, width).len()).unwrap_or(u16::MAX)
}

/// The inline live-region height when the `/secrete` page is open, or `None`
/// when it isn't. Like every sibling it **replaces** the composer — and only
/// the composer: a running turn's strip keeps its rows above it. Clamped to
/// the terminal height.
#[must_use]
pub fn secrets_picker_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.secrets_page.as_ref()?;
    Some(super::layout::view_height(
        app,
        width,
        secrets_menu_rows(app, width),
        term_height,
    ))
}

/// Render the **inline** `/secrete` page into the live region, in place of
/// the composer — bottom-anchored, so a squeezed area keeps the fields, the
/// hint and the closing rule on screen while the skipped top flows into
/// scrollback (`docs/view-flow.md`). Pure — `render_live` paints this.
pub fn render_secrets_picker(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(area, buf, secrets_view_lines(app, area.width));
}
