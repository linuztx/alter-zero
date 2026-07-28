//! The inline `/login` API-key onboarding. See `docs/llm.md`.

use super::model_view::{model_placeholder_row, model_rule};
use super::theme::*;
use super::wrap::{cols, truncate_cols};
use super::*;

/// The `/login` `>` line: the cyan prompt then `text` (the provider filter, or
/// the masked key). Shared shape with the `/model` search line.
fn login_prompt_line(text: Line<'static>) -> Line<'static> {
    let Line { mut spans, .. } = text;
    let mut out = vec![
        Span::raw(MODEL_INDENT),
        Span::styled(MODEL_PROMPT, Style::new().fg(MODEL_SELECTED_COLOR)),
    ];
    out.append(&mut spans);
    Line::from(out)
}

/// One provider row in the `/login` list: `{marker}{name} [{env_var}]{✓}` — the
/// selected row lights up cyan (the palette accent), the `[env_var]` tag is dim,
/// and an already-configured provider carries a green ✓. Mirrors [`model_row`].
fn login_provider_row(choice: &ProviderChoice, selected: bool, width: u16) -> Line<'static> {
    let marker = if selected { MODEL_MARKER } else { "  " };
    let tag = format!(" [{}]", choice.env_var);
    let check = if choice.configured {
        MODEL_ACTIVE_MARK
    } else {
        ""
    };
    let reserved = cols(marker) + cols(&tag) + cols(check);
    let name_room = (width as usize).saturating_sub(reserved).max(1);
    let name = truncate_cols(&choice.name, name_room);

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
        Span::styled(tag, Style::new().fg(MODEL_META_COLOR)),
        Span::styled(check.to_string(), Style::new().fg(MODEL_ACTIVE_COLOR)),
    ])
}

/// The `/login` provider list: a single `No matching providers` placeholder when
/// the filter matches nothing, else the rows windowed ([`centered_window`]) to
/// keep the selection **centered** and capped at [`LOGIN_MENU_MAX_ROWS`]. Its
/// length equals [`login_provider_list_rows`] so the reserved height and painted
/// rows agree.
fn login_provider_list_lines(onboarding: &KeyOnboarding, width: u16) -> Vec<Line<'static>> {
    let matches = onboarding.matches();
    if matches.is_empty() {
        return vec![model_placeholder_row(
            LOGIN_NO_MATCH,
            MODEL_META_COLOR,
            width,
        )];
    }
    let max = LOGIN_MENU_MAX_ROWS as usize;
    let selected = onboarding.selected.min(matches.len() - 1);
    let offset = centered_window(matches.len(), selected, max);
    matches
        .iter()
        .enumerate()
        .skip(offset)
        .take(max)
        .map(|(i, c)| login_provider_row(c, i == selected, width))
        .collect()
}

/// The `(selected+1/total)` counter under the `/login` provider list, or a blank
/// line when nothing is selectable.
fn login_counter_line(onboarding: &KeyOnboarding) -> Line<'static> {
    let matches = onboarding.matches();
    if matches.is_empty() {
        return Line::default();
    }
    let selected = onboarding.selected.min(matches.len() - 1);
    Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(
            format!("({}/{})", selected + 1, matches.len()),
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
            truncate_cols(LOGIN_KEY_PLACEHOLDER, room),
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

/// Render the **inline** `/login` API-key onboarding flow into the live region,
/// in place of the composer. Two steps sharing the `/model` picker's framed
/// look: the provider list ([`KeyStep::Provider`]) and the masked key field
/// ([`KeyStep::Key`]). Pure — `render_live` paints this. See `docs/llm.md`.
pub fn render_key_onboarding(area: Rect, buf: &mut Buffer, onboarding: &KeyOnboarding) {
    match onboarding.step {
        KeyStep::Provider => render_login_provider_step(area, buf, onboarding),
        KeyStep::Key => render_login_key_step(area, buf, onboarding),
    }
}

/// The provider-selection step (headerless, like `/model`): top rule, gap, `❯`
/// filter, gap, the provider list, a `(n/total)` counter, gap, a dim
/// `Keys are saved to {.env path}` hint, gap, bottom rule.
fn render_login_provider_step(area: Rect, buf: &mut Buffer, onboarding: &KeyOnboarding) {
    let [
        top_rule,
        _gap1,
        search,
        _gap2,
        list,
        counter,
        _gap3,
        hint,
        _gap4,
        bottom_rule,
    ] = Layout::vertical([
        Constraint::Length(1), // top rule
        Constraint::Length(1), // gap
        Constraint::Length(1), // search
        Constraint::Length(1), // gap
        Constraint::Min(0),    // provider list
        Constraint::Length(1), // counter
        Constraint::Length(1), // gap
        Constraint::Length(1), // hint
        Constraint::Length(1), // gap
        Constraint::Length(1), // bottom rule
    ])
    .areas(area);

    Paragraph::new(model_rule(area.width)).render(top_rule, buf);
    Paragraph::new(login_prompt_line(Line::from(onboarding.query.clone()))).render(search, buf);
    Paragraph::new(login_provider_list_lines(onboarding, area.width)).render(list, buf);
    Paragraph::new(login_counter_line(onboarding)).render(counter, buf);
    Paragraph::new(model_placeholder_row(
        &format!("{LOGIN_PROVIDER_HINT_PREFIX}{}", onboarding.env_path),
        MODEL_META_COLOR,
        area.width,
    ))
    .render(hint, buf);
    Paragraph::new(model_rule(area.width)).render(bottom_rule, buf);
}

/// The key-entry step: top rule, gap, a periwinkle `Enter your {provider} API
/// key` prompt, gap, the masked `❯` field, gap, a dim
/// `Enter to save · Esc to go back` hint, gap, bottom rule.
fn render_login_key_step(area: Rect, buf: &mut Buffer, onboarding: &KeyOnboarding) {
    let [
        top_rule,
        _gap1,
        prompt,
        _gap2,
        field,
        _gap3,
        hint,
        _gap4,
        bottom_rule,
    ] = Layout::vertical([
        Constraint::Length(1), // top rule
        Constraint::Length(1), // gap
        Constraint::Length(1), // prompt
        Constraint::Length(1), // gap
        Constraint::Length(1), // masked field
        Constraint::Length(1), // gap
        Constraint::Length(1), // hint
        Constraint::Length(1), // gap
        Constraint::Length(1), // bottom rule
    ])
    .areas(area);

    let name = onboarding
        .chosen_provider()
        .map_or("the provider", |c| c.name.as_str());

    Paragraph::new(model_rule(area.width)).render(top_rule, buf);
    Paragraph::new(model_placeholder_row(
        &login_key_prompt(name),
        LOGIN_KEY_PROMPT_COLOR,
        area.width,
    ))
    .render(prompt, buf);
    Paragraph::new(login_key_field(onboarding, area.width)).render(field, buf);
    Paragraph::new(model_placeholder_row(
        LOGIN_KEY_HINT,
        MODEL_META_COLOR,
        area.width,
    ))
    .render(hint, buf);
    Paragraph::new(model_rule(area.width)).render(bottom_rule, buf);
}
