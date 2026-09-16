//! The inline `/login` onboarding — the method root, the subscription list,
//! the sign-in method choice a two-way subscription puts first, the sign-in
//! page, the API-key provider list and its masked key field. See
//! `docs/llm.md`, `docs/copilot.md` and `docs/chatgpt.md`.

use crate::app::ProviderChoice;

use super::model_view::{model_linked_rows, model_placeholder_row, model_rule, model_wrapped_rows};
use super::theme::*;
use super::wrap::{cols, ellipsize, truncate_cols};
use super::*;

/// The `/login` `>` line: the cyan prompt then `text` (the current step's
/// filter, or the masked key). Shared shape with the `/model` search line.
fn login_prompt_line(text: Line<'static>) -> Line<'static> {
    let Line { mut spans, .. } = text;
    let mut out = vec![
        Span::raw(MODEL_INDENT),
        Span::styled(MODEL_PROMPT, Style::new().fg(model_selected_color())),
    ];
    out.append(&mut spans);
    Line::from(out)
}

/// A `/login` page title — `Use a subscription`, `Sign in to GitHub Copilot`,
/// `Enter your Agent Zero API key`. Cyan on every page, so the flow's headings
/// read as one.
fn login_title(text: &str, width: u16) -> Line<'static> {
    model_placeholder_row(text, login_title_color(), width)
}

/// One list row shared by the three `/login` lists:
/// `{marker}{name} · {✔ configured | ◯ unconfigured}`.
///
/// The selected row's marker and name light up cyan (the palette accent), and
/// a row that names something *reachable* — a provider, a subscription —
/// closes with its **status**: `✔ configured` when a key or token already
/// resolves, `◯ unconfigured` when none does. `status` is `None` for the
/// method root's two rows, which are the question rather than an answer and
/// so have nothing to report.
///
/// Only the **`✔` is coloured** — the green the `/model` picker's ✓ wears —
/// because the mark is what the eye hunts for down a column of names. Its
/// word, the `◯`, and the separator ahead of them stay dim: a status is a
/// fact about a row rather than an alert, and colouring the whole tail made a
/// list of facts read as a column of them.
///
/// Spelling the negative out is the point. The row used to carry a green ✓
/// when configured and *nothing at all* when not, so "no key yet" had to be
/// read off the absence of a mark — the one question a sign-in list is opened
/// to answer, answered by a blank.
///
/// The row still says only this much. It used to trail a dim tag as well (the
/// provider's env var, or the subscription's one-line description), which made
/// the three lists read as three shapes and pushed the names apart; the env
/// var is named by the step's own hint and by the save toast, and what a
/// subscription means is the sign-in page's job.
///
/// The description **still steers the type-to-search**
/// ([`KeyOnboarding::subscription_matches`]) — it left the display, not the
/// data — so typing `chatgpt` or `github` finds its row whether or not the
/// words are on screen. Mirrors `model_row`.
///
/// [`KeyOnboarding::subscription_matches`]: crate::app::KeyOnboarding::subscription_matches
fn login_row(name: &str, status: Option<bool>, selected: bool, width: u16) -> Line<'static> {
    let marker = if selected { MODEL_MARKER } else { "  " };
    let tag = status.map(|configured| {
        if configured {
            (
                LOGIN_CONFIGURED_MARK,
                LOGIN_CONFIGURED_LABEL,
                model_active_color(),
            )
        } else {
            (
                LOGIN_UNCONFIGURED_MARK,
                LOGIN_UNCONFIGURED_LABEL,
                model_meta_color(),
            )
        }
    });
    let reserved = cols(marker)
        + tag.map_or(0, |(mark, label, _)| {
            cols(LOGIN_STATUS_SEP) + cols(mark) + cols(label)
        });
    let name_room = (width as usize).saturating_sub(reserved).max(1);
    // `…`-cut like the model id: the status keeps its seat and the cut shows.
    let name = ellipsize(name, name_room);

    let (marker_style, name_style) = if selected {
        (
            Style::new().fg(model_selected_color()),
            Style::new()
                .fg(model_selected_color())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (Style::default(), Style::new().fg(model_id_color()))
    };
    let mut spans = vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(name, name_style),
    ];
    if let Some((mark, label, mark_color)) = tag {
        let dim = Style::new().fg(model_meta_color());
        spans.push(Span::styled(LOGIN_STATUS_SEP, dim));
        spans.push(Span::styled(mark, Style::new().fg(mark_color)));
        spans.push(Span::styled(label, dim));
    }
    Line::from(spans)
}

/// The rows of whichever list the current step shows, windowed
/// ([`centered_window`]) to keep the selection **centered** and capped at
/// [`LOGIN_MENU_MAX_ROWS`], or a single placeholder when the filter matches
/// nothing. Its length is what the page height counts, so the reserved and the
/// painted rows agree.
fn login_list_lines(onboarding: &KeyOnboarding, width: u16) -> Vec<Line<'static>> {
    // (name, status) per row — one shape for all three lists. The method rows
    // report **no** status: they are the question, not an answer.
    let rows: Vec<(String, Option<bool>)> = match onboarding.step {
        KeyStep::Method => onboarding
            .method_matches()
            .into_iter()
            .map(|m| (m.label().to_string(), None))
            .collect(),
        KeyStep::Subscription => onboarding
            .subscription_matches()
            .into_iter()
            .map(|s| (s.name.clone(), Some(s.configured)))
            .collect(),
        _ => onboarding
            .matches()
            .into_iter()
            .map(|p| (p.name.clone(), Some(p.configured)))
            .collect(),
    };
    if rows.is_empty() {
        let placeholder = match onboarding.step {
            KeyStep::Method => LOGIN_NO_METHOD_MATCH,
            KeyStep::Subscription => LOGIN_NO_SUBSCRIPTION_MATCH,
            _ => LOGIN_NO_MATCH,
        };
        return vec![model_placeholder_row(
            placeholder,
            model_meta_color(),
            width,
        )];
    }
    let max = LOGIN_MENU_MAX_ROWS as usize;
    let selected = onboarding.selected.min(rows.len() - 1);
    let offset = centered_window(rows.len(), selected, max);
    rows.iter()
        .enumerate()
        .skip(offset)
        .take(max)
        .map(|(i, (name, status))| login_row(name, *status, i == selected, width))
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
            Style::new().fg(model_meta_color()),
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

/// The host step's title: `Enter your Ollama host` — what it wants, not a
/// key it doesn't (`docs/ollama.md`).
pub(super) fn login_host_prompt(name: &str) -> String {
    format!("Enter your {name} host")
}

/// The `/login` key field. A **secret** is rendered as [`LOGIN_MASK_CHAR`]
/// dots (one per character, truncated to width) over a dim placeholder when
/// empty; a **host** ([`KeyKind::Host`]) is shown as typed — a URL typed blind
/// is a URL typed wrong — over its default, which is what an empty Enter
/// saves.
fn login_key_field(onboarding: &KeyOnboarding, width: u16) -> Line<'static> {
    let room = (width as usize)
        .saturating_sub(cols(MODEL_INDENT) + cols(MODEL_PROMPT))
        .max(1);
    let kind = onboarding.chosen_provider().map(|choice| &choice.key_kind);
    let body = match (kind, onboarding.key_input.is_empty()) {
        (Some(KeyKind::Host { default }), true) => Span::styled(
            ellipsize(default, room),
            Style::new().fg(model_meta_color()),
        ),
        (Some(KeyKind::Host { .. }), false) => Span::styled(
            truncate_cols(&onboarding.key_input, room),
            Style::new().fg(model_id_color()),
        ),
        (_, true) => Span::styled(
            ellipsize(LOGIN_KEY_PLACEHOLDER, room),
            Style::new().fg(model_meta_color()),
        ),
        (_, false) => {
            let dots: String = (0..onboarding.key_input.chars().count())
                .map(|_| LOGIN_MASK_CHAR)
                .collect();
            Span::styled(
                truncate_cols(&dots, room),
                Style::new().fg(model_id_color()),
            )
        }
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
    let border = Style::new().fg(border_color());
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
                    .fg(device_code_color())
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
        // A failure body is the other place a URL turns up here — a provider's
        // own error text, or advice naming a page to visit — so it is linked
        // like the instruction above it rather than left as dead text.
        DeviceStatus::Failed(reason) => model_linked_rows(reason, error_color(), width),
        DeviceStatus::Starting => model_wrapped_rows(
            if browser {
                DEVICE_LINK_STARTING
            } else {
                DEVICE_STARTING
            },
            model_meta_color(),
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
            model_wrapped_rows(&format!("{waiting}{tail}"), model_meta_color(), width)
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
        // The URL wraps rather than being cut — it is the one thing on the
        // page that must arrive exactly — and it is a real **hyperlink**
        // (`docs/links.md`): every wrapped fragment carries the whole target,
        // so clicking the second half opens the same URL as clicking the
        // first. The two pages differ in what leads it and how it is lit. A
        // device page keeps `Visit ` and stays dim, because the code in its
        // box is what the eye should land on. A browser page gives the URL
        // the row **bare and bright**: there is no box to compete with, the
        // link itself is the affordance, and a verb in front of it would only
        // push the target off the start of its own row.
        let mut instruction = model_linked_rows(
            &if browser {
                device.verification_uri.clone()
            } else {
                format!("{DEVICE_VISIT_PREFIX}{}", device.verification_uri)
            },
            if browser {
                device_code_color()
            } else {
                device_uri_color()
            },
            width,
        );
        instruction.push(model_placeholder_row(
            if browser {
                DEVICE_RETURN_LINE
            } else {
                DEVICE_ENTER_LINE
            },
            model_meta_color(),
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
        model_meta_color(),
        width,
    )]);

    login_page(blocks, width)
}

/// The sign-in method choice ([`KeyStep::SigninMethod`], `docs/chatgpt.md`):
/// a cyan `Select {subscription} login method:` title over the ways in the
/// chosen subscription offers — `Browser login (default)` / `Device code
/// login (headless)` — in the root's own row dress (no status: the rows are
/// the answers, not things that can be configured), over the root's hint.
///
/// **No `❯` filter and no counter.** Two rows are a question, not a list to
/// search, and the page exists so the row's Enter can ask it rather than
/// silently opening one flow and hiding the other. Built as blocks like the
/// sign-in page, so the shape is rule, gap, title, gap, rows, gap, hint, gap,
/// rule — the shape the user asked for, and the one `smoke.sh` Phase 119
/// reads back.
fn signin_method_page_lines(onboarding: &KeyOnboarding, width: u16) -> Vec<Line<'static>> {
    let name = onboarding
        .chosen_subscription()
        .map_or("", |s| s.name.as_str());
    // A subscription's name is the file's to choose; `login_title` `…`-cuts
    // one that would spill past the frame and take the rule with it.
    let title =
        format!("{LOGIN_SIGNIN_METHOD_TITLE_PREFIX}{name}{LOGIN_SIGNIN_METHOD_TITLE_SUFFIX}");
    let rows: Vec<Line<'static>> = onboarding
        .signin_method_labels()
        .iter()
        .enumerate()
        .map(|(i, label)| login_row(label, None, i == onboarding.selected, width))
        .collect();
    login_page(
        vec![
            vec![login_title(&title, width)],
            rows,
            vec![model_placeholder_row(
                LOGIN_SIGNIN_METHOD_HINT,
                model_meta_color(),
                width,
            )],
        ],
        width,
    )
}

/// A framed `/login` page built from content **blocks**: top rule, gap, each
/// non-empty block separated by exactly one blank row, gap, bottom rule.
///
/// An empty block contributes nothing at all, which is what lets a page grow
/// and shrink without leaving a band of blanks behind — the sign-in page
/// before its code has arrived, the key page of a provider the file describes
/// in no words.
fn login_page(blocks: Vec<Vec<Line<'static>>>, width: u16) -> Vec<Line<'static>> {
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

/// What the key step says about the provider it is asking for: the one-line
/// description from `providers.toml` over the page its keys are created on,
/// both dim, the URL a real hyperlink like every other URL this flow shows
/// (`docs/links.md`).
///
/// **Wrapped, never cut.** A description that stops mid-word explains nothing
/// and a clipped link opens nothing; the page's height is its own line count
/// (`docs/view-flow.md`), so a continuation row costs only itself.
///
/// Empty when the file names neither, so a provider with nothing to add gets
/// exactly the page it had before.
fn provider_about_lines(choice: &ProviderChoice, width: u16) -> Vec<Line<'static>> {
    let mut text = choice.description.clone();
    if !choice.key_url.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        // A provider that takes no key has none to create: what its link is
        // for is the server the host field is asking about (`docs/ollama.md`).
        text.push_str(if choice.key_kind.is_host() {
            LOGIN_HOST_URL_PREFIX
        } else {
            LOGIN_KEY_URL_PREFIX
        });
        text.push_str(&choice.key_url);
    }
    if text.is_empty() {
        return Vec::new();
    }
    model_linked_rows(&text, model_meta_color(), width)
}

/// The row of `lines` carrying the `❯` prompt — a list step's filter, or the
/// key step's field — and `None` on a page that has neither (the sign-in
/// page, which is a wait rather than a field).
///
/// **Found, not counted.** The key field used to sit on a constant row, which
/// stopped being true the moment the provider's description block moved in
/// above it: that block is as tall as the terminal is narrow. Reading the row
/// back out of the very page the paint builds is `menu_marker_seat`'s rule,
/// and it cannot drift from what is on screen.
pub(super) fn login_prompt_row(lines: &[Line<'static>]) -> Option<u16> {
    lines
        .iter()
        .position(|line| line.spans.iter().any(|s| s.content == MODEL_PROMPT))
        .and_then(|row| u16::try_from(row).ok())
}

/// A `/login` list page: top rule, gap, the `❯` filter, gap, the windowed
/// list, a `(n/total)` counter, gap, the step's dim hint rows, gap, bottom
/// rule.
///
/// **No title.** The two lists below the root used to repeat the method row
/// that opened them (`Use a subscription` / `Use an API key`) as a cyan
/// heading, which said nothing the rows and the hint under them don't — and
/// cost every row two lines of a region that is already sharing the terminal
/// with a running turn. All three lists are one shape now, and the root never
/// had one to begin with: there its two rows *are* the question.
fn list_page_lines(onboarding: &KeyOnboarding, width: u16) -> Vec<Line<'static>> {
    let mut lines = vec![model_rule(width), Line::default()];
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
            model_meta_color(),
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
    lines.push(model_placeholder_row(hint, model_meta_color(), width));
    lines.push(Line::default());
    lines.push(model_rule(width));
    lines
}

/// The whole framed page as lines, per step — the three lists share
/// [`list_page_lines`], the sign-in method choice is
/// [`signin_method_page_lines`], the device page is [`device_page_lines`], and the key
/// step is a cyan `Enter your {provider} API key` title over what the provider
/// file says the provider *is* ([`provider_about_lines`] — omitted when it
/// says nothing), the masked `❯` field, and a dim `Enter to save · Esc to go
/// back` hint. What [`render_key_onboarding`] paints (bottom-anchored) and
/// `layout::key_onboarding_rows` counts, so the reserved height and the
/// painted rows can never disagree (`docs/view-flow.md`).
pub(super) fn key_onboarding_lines(onboarding: &KeyOnboarding, width: u16) -> Vec<Line<'static>> {
    match onboarding.step {
        KeyStep::Method | KeyStep::Subscription | KeyStep::Provider => {
            list_page_lines(onboarding, width)
        }
        KeyStep::SigninMethod => signin_method_page_lines(onboarding, width),
        KeyStep::Device => match &onboarding.device {
            Some(device) => device_page_lines(device, width),
            // Unreachable in practice (the step and the page open together),
            // but a torn-down page must never paint a frameless void.
            None => vec![model_rule(width), Line::default(), model_rule(width)],
        },
        KeyStep::Key => {
            let chosen = onboarding.chosen_provider();
            let name = chosen.map_or("the provider", |c| c.name.as_str());
            // A host field asks for where the server is and defaults on an
            // empty Enter; a secret field asks for the key and waits for one.
            let host = chosen.is_some_and(|c| c.key_kind.is_host());
            let title = if host {
                login_host_prompt(name)
            } else {
                login_key_prompt(name)
            };
            let hint = if host {
                LOGIN_HOST_HINT
            } else {
                LOGIN_KEY_HINT
            };
            login_page(
                vec![
                    vec![login_title(&title, width)],
                    chosen.map_or_else(Vec::new, |c| provider_about_lines(c, width)),
                    vec![login_key_field(onboarding, width)],
                    vec![model_placeholder_row(hint, model_meta_color(), width)],
                ],
                width,
            )
        }
    }
}

/// Render the **inline** `/login` onboarding flow into the live region, in
/// place of the composer. Six steps sharing the `/model` picker's framed look
/// — bottom-anchored like every framed view (`docs/view-flow.md`). Pure —
/// `render_live` paints this. See `docs/llm.md` and `docs/copilot.md`.
pub fn render_key_onboarding(area: Rect, buf: &mut Buffer, onboarding: &KeyOnboarding) {
    super::view_flow::render_framed_tail(area, buf, key_onboarding_lines(onboarding, area.width));
}
