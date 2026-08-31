//! The inline `/login` onboarding — the method root, the subscription list and
//! its device-code page, the API-key provider list and its masked key field.
//! See `docs/llm.md` and `docs/copilot.md`.

use super::model_view::{model_placeholder_row, model_rule, model_wrapped_rows};
use super::theme::*;
use super::wrap::{cols, ellipsize, truncate_cols};
use super::*;

/// The `/login` `>` line: the cyan prompt then `text` (the current step's
/// filter, or the masked key). Shared shape with the `/model` search line.
fn login_prompt_line(text: Line<'static>) -> Line<'static> {
    let Line { mut spans, .. } = text;
    let mut out = vec![
        Span::raw(MODEL_INDENT),
        Span::styled(MODEL_PROMPT, Style::new().fg(MODEL_SELECTED_COLOR)),
    ];
    out.append(&mut spans);
    Line::from(out)
}

/// A `/login` page title — `Use a subscription`, `Sign in to GitHub Copilot`,
/// `Enter your Agent Zero API key`. Cyan on every page, so the flow's headings
/// read as one.
fn login_title(text: &str, width: u16) -> Line<'static> {
    model_placeholder_row(text, LOGIN_TITLE_COLOR, width)
}

/// One list row shared by the three `/login` lists: `{marker}{name}{gap}{tag}{✓}`.
/// The selected row's marker and name light up cyan (the palette accent), the
/// trailing `tag` (an env var, or a subscription's description) is dim, and an
/// already-configured row carries a green ✓. Mirrors `model_row`.
fn login_row(name: &str, tag: &str, configured: bool, selected: bool, width: u16) -> Line<'static> {
    let marker = if selected { MODEL_MARKER } else { "  " };
    let check = if configured { MODEL_ACTIVE_MARK } else { "" };
    let reserved = cols(marker) + cols(tag) + cols(check);
    let name_room = (width as usize).saturating_sub(reserved).max(1);
    // `…`-cut like the model id: the tag keeps its seat and the cut shows.
    let name = ellipsize(name, name_room);

    let (marker_style, name_style) = if selected {
        (
            Style::new().fg(MODEL_SELECTED_COLOR),
            Style::new()
                .fg(MODEL_SELECTED_COLOR)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (Style::default(), Style::new().fg(MODEL_ID_COLOR))
    };
    Line::from(vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(name, name_style),
        Span::styled(tag.to_string(), Style::new().fg(MODEL_META_COLOR)),
        Span::styled(check.to_string(), Style::new().fg(MODEL_ACTIVE_COLOR)),
    ])
}

/// The rows of whichever list the current step shows, windowed
/// ([`centered_window`]) to keep the selection **centered** and capped at
/// [`LOGIN_MENU_MAX_ROWS`], or a single placeholder when the filter matches
/// nothing. Its length is what the page height counts, so the reserved and the
/// painted rows agree.
fn login_list_lines(onboarding: &KeyOnboarding, width: u16) -> Vec<Line<'static>> {
    // (name, tag, configured) per row — one shape for all three lists.
    let rows: Vec<(String, String, bool)> = match onboarding.step {
        KeyStep::Method => onboarding
            .method_matches()
            .into_iter()
            .map(|m| (m.label().to_string(), String::new(), false))
            .collect(),
        KeyStep::Subscription => onboarding
            .subscription_matches()
            .into_iter()
            .map(|s| (s.name.clone(), format!("  {}", s.description), s.configured))
            .collect(),
        _ => onboarding
            .matches()
            .into_iter()
            .map(|p| (p.name.clone(), format!(" [{}]", p.env_var), p.configured))
            .collect(),
    };
    if rows.is_empty() {
        let placeholder = match onboarding.step {
            KeyStep::Method => LOGIN_NO_METHOD_MATCH,
            KeyStep::Subscription => LOGIN_NO_SUBSCRIPTION_MATCH,
            _ => LOGIN_NO_MATCH,
        };
        return vec![model_placeholder_row(placeholder, MODEL_META_COLOR, width)];
    }
    let max = LOGIN_MENU_MAX_ROWS as usize;
    let selected = onboarding.selected.min(rows.len() - 1);
    let offset = centered_window(rows.len(), selected, max);
    rows.iter()
        .enumerate()
        .skip(offset)
        .take(max)
        .map(|(i, (name, tag, configured))| login_row(name, tag, *configured, i == selected, width))
        .collect()
}

/// How many rows the current list step offers — the `(n/total)` counter's total.
fn login_list_len(onboarding: &KeyOnboarding) -> usize {
    match onboarding.step {
        KeyStep::Method => onboarding.method_matches().len(),
        KeyStep::Subscription => onboarding.subscription_matches().len(),
        _ => onboarding.matches().len(),
    }
}

/// The `(selected+1/total)` counter under a `/login` list, or a blank line when
/// nothing is selectable.
fn login_counter_line(onboarding: &KeyOnboarding) -> Line<'static> {
    let len = login_list_len(onboarding);
    if len == 0 {
        return Line::default();
    }
    let selected = onboarding.selected.min(len - 1);
    Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(
            format!("({}/{})", selected + 1, len),
            Style::new().fg(MODEL_META_COLOR),
        ),
    ])
}

/// The key-step prompt naming the provider — `Enter your {name} API key`, but
/// avoiding a doubled "API" when the name already ends in it (so "Agent Zero
/// API" reads `Enter your Agent Zero API key`, not `… API API key`).
pub(super) fn login_key_prompt(name: &str) -> String {
    if name.trim_end().to_ascii_lowercase().ends_with("api") {
        format!("Enter your {name} key")
    } else {
        format!("Enter your {name} API key")
    }
}

/// The `/login` masked key field: the entered key rendered as [`LOGIN_MASK_CHAR`]
/// dots (one per character, truncated to width), or a dim placeholder when empty.
fn login_key_field(onboarding: &KeyOnboarding, width: u16) -> Line<'static> {
    let room = (width as usize)
        .saturating_sub(cols(MODEL_INDENT) + cols(MODEL_PROMPT))
        .max(1);
    let body = if onboarding.key_input.is_empty() {
        Span::styled(
            ellipsize(LOGIN_KEY_PLACEHOLDER, room),
            Style::new().fg(MODEL_META_COLOR),
        )
    } else {
        let dots: String = (0..onboarding.key_input.chars().count())
            .map(|_| LOGIN_MASK_CHAR)
            .collect();
        Span::styled(truncate_cols(&dots, room), Style::new().fg(MODEL_ID_COLOR))
    };
    login_prompt_line(Line::from(vec![body]))
}

/// `mm:ss` — how the device page counts a code's remaining life down. Minutes
/// are **not** clamped to two digits: a fifteen-minute code reads `14:11`, and
/// a hypothetical longer one must not silently wrap.
pub(super) fn countdown(remaining: std::time::Duration) -> String {
    let secs = remaining.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// The one-time code inside its rounded box, three rows indented past the
/// page's own inset. The box is sized to the code, so a provider that issues a
/// longer one still gets a snug frame.
fn device_code_box(code: &str) -> Vec<Line<'static>> {
    let indent = format!("{MODEL_INDENT}{DEVICE_BOX_INDENT}");
    let inner = cols(DEVICE_BOX_PAD) * 2 + cols(code);
    let bar = DEVICE_BOX_HORIZONTAL.repeat(inner);
    let border = Style::new().fg(BORDER_COLOR);
    vec![
        Line::from(vec![
            Span::raw(indent.clone()),
            Span::styled(
                format!("{DEVICE_BOX_TOP_LEFT}{bar}{DEVICE_BOX_TOP_RIGHT}"),
                border,
            ),
        ]),
        Line::from(vec![
            Span::raw(indent.clone()),
            Span::styled(DEVICE_BOX_VERTICAL, border),
            Span::raw(DEVICE_BOX_PAD),
            Span::styled(
                code.to_string(),
                Style::new()
                    .fg(DEVICE_CODE_COLOR)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(DEVICE_BOX_PAD),
            Span::styled(DEVICE_BOX_VERTICAL, border),
        ]),
        Line::from(vec![
            Span::raw(indent),
            Span::styled(
                format!("{DEVICE_BOX_BOTTOM_LEFT}{bar}{DEVICE_BOX_BOTTOM_RIGHT}"),
                border,
            ),
        ]),
    ]
}

/// The device page's status row: the wait (with the code's countdown when the
/// boundary has fed one) or, on a failure, the reason in red — the page stays
/// up either way, so the reason is readable until Esc takes it down.
fn device_status_lines(device: &DeviceLogin, width: u16) -> Vec<Line<'static>> {
    let browser = device.kind == SigninKind::BrowserLink;
    match &device.status {
        DeviceStatus::Failed(reason) => model_wrapped_rows(reason, ERROR_COLOR, width),
        DeviceStatus::Starting => model_wrapped_rows(
            if browser {
                DEVICE_LINK_STARTING
            } else {
                DEVICE_STARTING
            },
            MODEL_META_COLOR,
            width,
        ),
        DeviceStatus::Waiting => {
            let expired = if browser {
                DEVICE_LINK_EXPIRED
            } else {
                DEVICE_EXPIRED
            };
            let tail = match device.remaining {
                // A code whose clock ran out says so rather than reading
                // `expires in 0:00` forever — the poll reports the expiry too,
                // but the countdown reaches zero first.
                Some(left) if left.is_zero() => expired.to_string(),
                Some(left) => format!("{DEVICE_EXPIRES_PREFIX}{}", countdown(left)),
                None => String::new(),
            };
            let waiting = if browser {
                DEVICE_LINK_WAITING
            } else {
                DEVICE_WAITING
            };
            model_wrapped_rows(&format!("{waiting}{tail}"), MODEL_META_COLOR, width)
        }
    }
}

/// The whole sign-in page as lines: title, the two-row instruction naming the
/// URL, the code box (a device flow only), the status row, and the copy/cancel
/// hint. No browser is launched either way — the URL is text the user opens
/// themselves, which is also what makes the flow work over SSH once the port
/// is forwarded.
fn device_page_lines(device: &DeviceLogin, width: u16) -> Vec<Line<'static>> {
    // Built as **blocks** joined by exactly one blank row, with an empty block
    // contributing nothing at all. Two of the four only exist once GitHub has
    // answered — before that there is no URL to name and no code to box — and
    // reserving their rows anyway left a band of blanks under the title while
    // the page did the one thing it says it is doing.
    let mut blocks: Vec<Vec<Line<'static>>> = vec![vec![login_title(
        &format!("{DEVICE_TITLE_PREFIX}{}", device.provider_name),
        width,
    )]];
    let browser = device.kind == SigninKind::BrowserLink;
    if !device.verification_uri.is_empty() {
        // The URL is its own wrapped row so a narrow terminal never cuts it —
        // it is the one thing on the page that must be typed exactly. Dim,
        // like the sentence under it: the code in its box is what the eye
        // should land on, and a lit URL competed with it. A browser sign-in
        // has no box, so there the URL *is* what the eye should land on and
        // the row below says what happens next rather than pointing at one.
        let prefix = if browser {
            DEVICE_OPEN_PREFIX
        } else {
            DEVICE_VISIT_PREFIX
        };
        let mut instruction = model_wrapped_rows(
            &format!("{prefix}{}", device.verification_uri),
            if browser {
                DEVICE_CODE_COLOR
            } else {
                DEVICE_URI_COLOR
            },
            width,
        );
        instruction.push(model_placeholder_row(
            if browser {
                DEVICE_RETURN_LINE
            } else {
                DEVICE_ENTER_LINE
            },
            MODEL_META_COLOR,
            width,
        ));
        blocks.push(instruction);
    }
    if let Some(code) = device.code() {
        blocks.push(device_code_box(code));
    }
    blocks.push(device_status_lines(device, width));
    blocks.push(vec![model_placeholder_row(
        if browser {
            DEVICE_LINK_HINT
        } else {
            DEVICE_HINT
        },
        MODEL_META_COLOR,
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

/// A `/login` list page: top rule, gap, an optional cyan title, the `❯` filter,
/// gap, the windowed list, a `(n/total)` counter, gap, the step's dim hint
/// rows, gap, bottom rule. The method step is the root and carries no title —
/// the two rows *are* the question.
fn list_page_lines(onboarding: &KeyOnboarding, width: u16) -> Vec<Line<'static>> {
    let mut lines = vec![model_rule(width), Line::default()];
    let title = match onboarding.step {
        KeyStep::Method => None,
        KeyStep::Subscription => Some(LOGIN_METHOD_SUBSCRIPTION),
        _ => Some(LOGIN_METHOD_API_KEY),
    };
    if let Some(title) = title {
        lines.push(login_title(title, width));
        lines.push(Line::default());
    }
    lines.push(login_prompt_line(Line::from(onboarding.query.clone())));
    lines.push(Line::default());
    lines.extend(login_list_lines(onboarding, width));
    // The counter takes a row only when something is selectable; with nothing
    // matched it collapsed to a blank line stacked on the gap below it (the
    // `/model` picker's placeholder rule). The method step never shows one —
    // it is a fixed two-row question, and "(1/2)" under it says nothing the
    // rows don't.
    if onboarding.step != KeyStep::Method && login_list_len(onboarding) > 0 {
        lines.push(login_counter_line(onboarding));
    }
    lines.push(Line::default());
    // Wrapped, not clipped: the `.env` path — where the secret is stored — is
    // the tail, so it was the first thing a narrow terminal lost. It sits below
    // the cursor's search row, so nothing moves above.
    if onboarding.step == KeyStep::Provider {
        lines.extend(model_wrapped_rows(
            &format!("{LOGIN_PROVIDER_HINT_PREFIX}{}", onboarding.env_path),
            MODEL_META_COLOR,
            width,
        ));
    }
    let hint = match onboarding.step {
        KeyStep::Method => LOGIN_METHOD_HINT,
        KeyStep::Subscription => LOGIN_SUBSCRIPTION_HINT,
        _ => LOGIN_PROVIDER_HINT,
    };
    // Placed, not wrapped: `wrap_text` is the *message* wrapper and collapses
    // runs of whitespace, which is exactly the double space separating one
    // `{key} {thing}` pair from the next. A hint is short and fixed, so an
    // `…`-cut at a narrow width costs nothing the path row above it doesn't
    // already say.
    lines.push(model_placeholder_row(hint, MODEL_META_COLOR, width));
    lines.push(Line::default());
    lines.push(model_rule(width));
    lines
}

/// The whole framed page as lines, per step — the three lists share
/// [`list_page_lines`], the device page is [`device_page_lines`], and the key
/// step is a fixed height: top rule, gap, a cyan `Enter your {provider} API
/// key` title, gap, the masked `❯` field, gap, a dim `Enter to save · Esc to
/// go back` hint, gap, bottom rule. What [`render_key_onboarding`] paints
/// (bottom-anchored) and `layout::key_onboarding_rows` counts, so the reserved
/// height and the painted rows can never disagree (`docs/view-flow.md`).
pub(super) fn key_onboarding_lines(onboarding: &KeyOnboarding, width: u16) -> Vec<Line<'static>> {
    match onboarding.step {
        KeyStep::Method | KeyStep::Subscription | KeyStep::Provider => {
            list_page_lines(onboarding, width)
        }
        KeyStep::Device => match &onboarding.device {
            Some(device) => device_page_lines(device, width),
            // Unreachable in practice (the step and the page open together),
            // but a torn-down page must never paint a frameless void.
            None => vec![model_rule(width), Line::default(), model_rule(width)],
        },
        KeyStep::Key => {
            let name = onboarding
                .chosen_provider()
                .map_or("the provider", |c| c.name.as_str());
            vec![
                model_rule(width),
                Line::default(),
                login_title(&login_key_prompt(name), width),
                Line::default(),
                login_key_field(onboarding, width),
                Line::default(),
                model_placeholder_row(LOGIN_KEY_HINT, MODEL_META_COLOR, width),
                Line::default(),
                model_rule(width),
            ]
        }
    }
}

/// Render the **inline** `/login` onboarding flow into the live region, in
/// place of the composer. Five steps sharing the `/model` picker's framed look
/// — bottom-anchored like every framed view (`docs/view-flow.md`). Pure —
/// `render_live` paints this. See `docs/llm.md` and `docs/copilot.md`.
pub fn render_key_onboarding(area: Rect, buf: &mut Buffer, onboarding: &KeyOnboarding) {
    super::view_flow::render_framed_tail(area, buf, key_onboarding_lines(onboarding, area.width));
}
