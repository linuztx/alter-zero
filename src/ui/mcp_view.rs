//! The `/mcp` manager's inline view (`docs/mcp.md`) — the hooks menu's twin:
//! a content-driven framed body ([`mcp_view_lines`] builds it line by line,
//! so the height falls out as `lines.len()` and [`mcp_menu_height`] /
//! [`render_mcp_menu`] can never disagree), the pages walking
//! servers → server detail → tools → tool detail, plus the OAuth page.

use super::menu::centered_window;
use super::model_view::model_rule;
use super::theme::*;
use super::wrap::{cols, ellipsize, wrap_output};
use super::*;

use crate::app::{McpMenu, McpPage, server_actions};
use crate::mcp::{McpServerSnapshot, McpServerStatus, tool_parameters, tool_wire_name};

/// A `MODEL_INDENT`-inset single line in `style`, `…`-cut to the width
/// ([`ellipsize`] — a cut row says so, never a silent clip).
fn mcp_line(text: &str, style: Style, width: u16) -> Line<'static> {
    let room = (width as usize).saturating_sub(cols(MODEL_INDENT)).max(1);
    Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(ellipsize(text, room), style),
    ])
}

fn dim_line(text: &str, width: u16) -> Line<'static> {
    mcp_line(text, Style::new().fg(MODEL_META_COLOR), width)
}

/// The page's headline — cyan, the row that says which of the four pages
/// this is (`docs/mcp.md`).
fn title_line(text: &str, width: u16) -> Line<'static> {
    mcp_line(
        text,
        Style::new()
            .fg(MCP_TITLE_COLOR)
            .add_modifier(Modifier::BOLD),
        width,
    )
}

/// A server name with its first character upper-cased — `deepwiki` →
/// `Deepwiki` — for the detail page's headline alone. A config key is
/// lower-case by convention, which reads as a typo once it opens a sentence;
/// everywhere the name is an *identity* rather than a headline (the list
/// rows, `Tools for …`, the wire name) it stays verbatim.
fn capitalize_first(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// `text` word-wrapped to inset rows in `style`.
fn wrapped(text: &str, style: Style, width: u16) -> Vec<Line<'static>> {
    let room = (width as usize).saturating_sub(cols(MODEL_INDENT)).max(1) as u16;
    text.split('\n')
        .flat_map(|part| wrap_text(part, room))
        .map(|row| Line::from(vec![Span::raw(MODEL_INDENT), Span::styled(row, style)]))
        .collect()
}

/// `text` word-wrapped to inset dim rows.
fn dim_wrapped(text: &str, width: u16) -> Vec<Line<'static>> {
    wrapped(text, Style::new().fg(MODEL_META_COLOR), width)
}

/// `text` word-wrapped to rows carrying a two-space inset past
/// `MODEL_INDENT` — list-item body text (the empty state's config paths).
fn inset_wrapped(text: &str, style: Style, width: u16) -> Vec<Line<'static>> {
    let inset = "  ";
    let room = (width as usize)
        .saturating_sub(cols(MODEL_INDENT) + cols(inset))
        .max(1) as u16;
    wrap_text(text, room)
        .into_iter()
        .map(|row| {
            Line::from(vec![
                Span::raw(MODEL_INDENT),
                Span::raw(inset),
                Span::styled(row, style),
            ])
        })
        .collect()
}

/// `{n} server(s)` / `{n} tool(s)`.
fn count_noun(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{n} {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// A status's glyph colour — the reference's success/warning/error/inactive.
fn status_color(status: &McpServerStatus) -> Color {
    match status {
        McpServerStatus::Connected => TOOL_OK_COLOR,
        // The ask review page's amber — the needs-attention colour, which an
        // untrusted project server also is (`/trust` is the way in).
        McpServerStatus::NeedsAuth | McpServerStatus::Untrusted => ASK_WARNING_COLOR,
        McpServerStatus::Failed(_) => TOOL_FAIL_COLOR,
        McpServerStatus::Pending | McpServerStatus::Disabled => MODEL_META_COLOR,
    }
}

/// One server row's spans: `{name} · {glyph} {status}` — the name lit when
/// selected, the status two-tone (glyph in its state's colour, text dim).
fn server_row(server: &McpServerSnapshot, selected: bool, width: u16) -> Line<'static> {
    let marker = if selected {
        Span::styled(
            HOOKS_MARKER.to_string(),
            Style::new().fg(MODEL_SELECTED_COLOR),
        )
    } else {
        Span::raw(" ".repeat(cols(HOOKS_MARKER)))
    };
    let name_style = if selected {
        Style::new()
            .fg(MODEL_SELECTED_COLOR)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(MODEL_ID_COLOR)
    };
    let room = (width as usize)
        .saturating_sub(cols(MODEL_INDENT) + cols(HOOKS_MARKER))
        .max(1);
    let name = ellipsize(&server.name, room);
    let used = cols(&name);
    let mut spans = vec![
        Span::raw(MODEL_INDENT),
        marker,
        Span::styled(name, name_style),
    ];
    // Three spans, not two: the separator is **chrome**, so it stays dim
    // at every state and only the glyph carries the status colour. Riding
    // it along with the glyph painted a connected row's first `·` green
    // while the `·` before its tool count stayed dim. When the row is too
    // narrow for the whole status the words `…`-cut but the **glyph
    // survives** — dropping the suffix whole made a Failed server's red
    // state vanish at narrow widths.
    let glyph = server.status.glyph();
    let words = server.status_line()[glyph.len()..].to_string();
    let avail = room.saturating_sub(used);
    if avail >= cols(MCP_ROW_SEPARATOR) + cols(glyph) {
        spans.push(Span::styled(
            MCP_ROW_SEPARATOR.to_string(),
            Style::new().fg(MODEL_META_COLOR),
        ));
        spans.push(Span::styled(
            glyph.to_string(),
            Style::new().fg(status_color(&server.status)),
        ));
        let words_room = avail - cols(MCP_ROW_SEPARATOR) - cols(glyph);
        spans.push(Span::styled(
            ellipsize(&words, words_room),
            Style::new().fg(MODEL_META_COLOR),
        ));
    }
    Line::from(spans)
}

/// The list page: the title + count over the scope-grouped rows (dim
/// headings naming each scope's config file), windowed around the selection
/// with `↑ N more above` / `↓ N more below` markers.
fn list_lines(menu: &McpMenu, width: u16) -> Vec<Line<'static>> {
    let mut lines = vec![
        model_rule(width),
        Line::default(),
        title_line(MCP_TITLE, width),
        dim_line(&count_noun(menu.servers.len(), "server"), width),
        Line::default(),
    ];
    if menu.servers.is_empty() {
        lines.push(dim_line(MCP_NONE_FOUND, width));
        // Wrapped: the paths are the instruction, and the clip used to eat
        // exactly the actionable tail (`~/.alter-zero/mcp.json`).
        lines.extend(inset_wrapped(
            "{project}/.mcp.json · ~/.alter-zero/mcp.json",
            Style::new().fg(AI_COLOR),
            width,
        ));
    } else {
        // The display rows: a heading per scope run + one row per server,
        // windowed by SERVER index (headings ride their first server).
        let selected = menu.selected.min(menu.servers.len() - 1);
        let window = HOOKS_MENU_MAX_ROWS.max(1);
        let offset = centered_window(menu.servers.len(), selected, window);
        let end = (offset + window).min(menu.servers.len());
        if offset > 0 {
            lines.push(dim_line(
                &format!("{HOOKS_UP_MARKER}{} more above", offset),
                width,
            ));
        }
        let mut last_heading: Option<String> = None;
        for (i, server) in menu.servers.iter().enumerate().take(end).skip(offset) {
            let heading = format!("{} ({})", server.scope.heading(), server.config_path);
            if last_heading.as_deref() != Some(&heading) {
                if last_heading.is_some() {
                    lines.push(Line::default());
                }
                lines.push(mcp_line(
                    &format!("  {heading}"),
                    Style::new().fg(MODEL_META_COLOR),
                    width,
                ));
                last_heading = Some(heading);
            }
            lines.push(server_row(server, i == selected, width));
        }
        if end < menu.servers.len() {
            lines.push(dim_line(
                &format!("{HOOKS_DOWN_MARKER}{} more below", menu.servers.len() - end),
                width,
            ));
        }
    }
    lines.extend([
        Line::default(),
        dim_line(MCP_LIST_HINT, width),
        Line::default(),
        model_rule(width),
    ]);
    lines
}

/// One `{label:<18}{value}` field row of the server detail page — the label
/// always bright, the value styled by what it *is*
/// ([`MCP_DETAIL_VALUE_COLOR`] for an address or a count,
/// [`MCP_DETAIL_STATE_COLOR`] for a value that is itself the answer). The
/// value stays one row (`…`-cut when it can't fit) — bounded values only;
/// the fields a user opens the page to *read whole* go through
/// [`field_lines`].
fn field_line(label: &str, value: &str, value_style: Style, width: u16) -> Line<'static> {
    let room = (width as usize)
        .saturating_sub(cols(MODEL_INDENT) + MCP_FIELD_COL)
        .max(1);
    Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(
            format!("{label:<MCP_FIELD_COL$}"),
            Style::new().fg(MCP_DETAIL_LABEL_COLOR),
        ),
        Span::styled(ellipsize(value, room), value_style),
    ])
}

/// [`field_line`]'s multi-row sibling: the value **wrapped** with a hanging
/// indent at the field column, for the values the user opens the page to
/// read whole — the server's URL or launch command and its config path
/// routinely outrun a narrow terminal, and the tail is the distinguishing
/// part.
fn field_lines(label: &str, value: &str, value_style: Style, width: u16) -> Vec<Line<'static>> {
    let room = (width as usize)
        .saturating_sub(cols(MODEL_INDENT) + MCP_FIELD_COL)
        .max(1) as u16;
    let mut rows = wrap_text(value, room).into_iter();
    let first = rows.next().unwrap_or_default();
    let mut lines = vec![Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(
            format!("{label:<MCP_FIELD_COL$}"),
            Style::new().fg(MCP_DETAIL_LABEL_COLOR),
        ),
        Span::styled(first, value_style),
    ])];
    lines.extend(rows.map(|row| {
        Line::from(vec![
            Span::raw(MODEL_INDENT),
            Span::raw(" ".repeat(MCP_FIELD_COL)),
            Span::styled(row, value_style),
        ])
    }));
    lines
}

/// A `{label:<18}{glyph} {words}` state row — `Status:` and `Auth:`, the two
/// rows that answer "is this working?". The glyph keeps its state's colour
/// (it is the one thing on the row that still has to shout when a server is
/// failing) over white words, so the row reads settled without laundering a
/// failure into a calm white line.
fn state_field_line(
    label: &str,
    glyph: &str,
    glyph_color: Color,
    words: &str,
    width: u16,
) -> Line<'static> {
    let room = (width as usize)
        .saturating_sub(cols(MODEL_INDENT) + MCP_FIELD_COL + cols(glyph) + 1)
        .max(1);
    Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(
            format!("{label:<MCP_FIELD_COL$}"),
            Style::new().fg(MCP_DETAIL_LABEL_COLOR),
        ),
        Span::styled(format!("{glyph} "), Style::new().fg(glyph_color)),
        Span::styled(
            ellipsize(words, room),
            Style::new().fg(MCP_DETAIL_STATE_COLOR),
        ),
    ])
}

/// The `{label} {value}` rows of the **tool** detail page — the compact
/// sibling of [`field_lines`]: one space instead of the server page's
/// [`MCP_FIELD_COL`] pad (its two labels are the same width, so they line up
/// on their own), and the two-tone reversed — the label bright, the value it
/// introduces dim. The value **wraps** under the label's column: the
/// `{server}__{tool}` wire name is what the user copies for allowlisting and
/// debugging, so a clipped one is unusable.
fn tool_field_lines(label: &str, value: &str, width: u16) -> Vec<Line<'static>> {
    let lead = cols(label) + cols(MCP_TOOL_FIELD_GAP);
    let room = (width as usize)
        .saturating_sub(cols(MODEL_INDENT) + lead)
        .max(1) as u16;
    let mut rows = wrap_text(value, room).into_iter();
    let first = rows.next().unwrap_or_default();
    let mut lines = vec![Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(
            format!("{label}{MCP_TOOL_FIELD_GAP}"),
            Style::new().fg(MCP_DETAIL_LABEL_COLOR),
        ),
        Span::styled(first, Style::new().fg(MCP_DETAIL_VALUE_COLOR)),
    ])];
    lines.extend(rows.map(|row| {
        Line::from(vec![
            Span::raw(MODEL_INDENT),
            Span::raw(" ".repeat(lead)),
            Span::styled(row, Style::new().fg(MCP_DETAIL_VALUE_COLOR)),
        ])
    }));
    lines
}

/// The numbered action rows (`❯ 1. Authenticate`).
fn action_rows(labels: &[&'static str], selected: usize, width: u16) -> Vec<Line<'static>> {
    labels
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let marker = if i == selected {
                Span::styled(
                    HOOKS_MARKER.to_string(),
                    Style::new().fg(MODEL_SELECTED_COLOR),
                )
            } else {
                Span::raw(" ".repeat(cols(HOOKS_MARKER)))
            };
            let style = if i == selected {
                Style::new().fg(MODEL_SELECTED_COLOR)
            } else {
                Style::new().fg(MODEL_ID_COLOR)
            };
            let room = (width as usize)
                .saturating_sub(cols(MODEL_INDENT) + cols(HOOKS_MARKER) + 3)
                .max(1);
            Line::from(vec![
                Span::raw(MODEL_INDENT),
                marker,
                Span::styled(format!("{}. ", i + 1), style),
                Span::styled(ellipsize(label, room), style),
            ])
        })
        .collect()
}

/// The server detail page: the fact rows the state affords, then the actions.
fn server_lines(menu: &McpMenu, server: &McpServerSnapshot, width: u16) -> Vec<Line<'static>> {
    // An address, a revision, a count: what the label leads to, not what the
    // page is about.
    let value = Style::new().fg(MCP_DETAIL_VALUE_COLOR);
    // A value that *is* the answer — what this server can do.
    let state = Style::new().fg(MCP_DETAIL_STATE_COLOR);
    let mut lines = vec![
        model_rule(width),
        Line::default(),
        title_line(
            &format!("{} MCP Server", capitalize_first(&server.name)),
            width,
        ),
        Line::default(),
    ];
    // Status: the glyph in its state colour over white words — and **no tool
    // count**, which is the `Tools:` row's own job three lines down.
    lines.push(state_field_line(
        "Status:",
        server.status.glyph(),
        status_color(&server.status),
        server.status.label(),
        width,
    ));
    if let Some(auth) = server.auth {
        // Red only for a state the user should act on; everything else is a
        // settled green ✔ (`docs/mcp.md`).
        let glyph_color = if auth.is_problem() {
            TOOL_FAIL_COLOR
        } else {
            TOOL_OK_COLOR
        };
        lines.push(state_field_line(
            "Auth:",
            auth.glyph(),
            glyph_color,
            auth.label(),
            width,
        ));
    }
    // The protocol revision the handshake settled on — a fact only a server
    // that actually initialized can report.
    if let Some(version) = server
        .identity
        .as_ref()
        .map(|identity| identity.protocol_version.trim())
        .filter(|v| !v.is_empty())
    {
        lines.push(field_line("Protocol:", version, value, width));
    }
    let target_label = if server.config.is_remote() {
        "URL:"
    } else {
        "Command:"
    };
    // The two values the user opens the page to read whole — the `/trust`
    // review's verbatim rule: they wrap under their labels, never clip.
    lines.extend(field_lines(
        target_label,
        &server.config.target(),
        value,
        width,
    ));
    lines.extend(field_lines(
        "Config location:",
        &server.config_path,
        value,
        width,
    ));
    if let Some(identity) = &server.identity {
        if !identity.capabilities.is_empty() {
            lines.push(field_line(
                "Capabilities:",
                &identity.capabilities.join(", "),
                state,
                width,
            ));
        }
        if !server.tools.is_empty() {
            lines.push(field_line(
                "Tools:",
                &count_noun(server.tools.len(), "tool"),
                value,
                width,
            ));
        }
    }
    if let McpServerStatus::Failed(reason) = &server.status {
        lines.push(Line::default());
        lines.extend(dim_wrapped(&format!("Error: {reason}"), width));
    }
    lines.push(Line::default());
    let labels: Vec<&'static str> = server_actions(server)
        .into_iter()
        .map(crate::app::McpServerAction::label)
        .collect();
    lines.extend(action_rows(&labels, menu.selected, width));
    lines.extend([
        Line::default(),
        dim_line(MCP_SERVER_HINT, width),
        Line::default(),
        model_rule(width),
    ]);
    lines
}

/// The tools page: `Tools for {server}` over the numbered tool names.
fn tools_lines(menu: &McpMenu, server: &McpServerSnapshot, width: u16) -> Vec<Line<'static>> {
    let mut lines = vec![
        model_rule(width),
        Line::default(),
        title_line(&format!("Tools for {}", server.name), width),
        dim_line(&count_noun(server.tools.len(), "tool"), width),
        Line::default(),
    ];
    let selected = menu.selected.min(server.tools.len().saturating_sub(1));
    let window = HOOKS_MENU_MAX_ROWS.max(1);
    let offset = centered_window(server.tools.len(), selected, window);
    let end = (offset + window).min(server.tools.len());
    if offset > 0 {
        lines.push(dim_line(
            &format!("{HOOKS_UP_MARKER}{offset} more above"),
            width,
        ));
    }
    for (i, tool) in server.tools.iter().enumerate().take(end).skip(offset) {
        let marker = if i == selected {
            Span::styled(
                HOOKS_MARKER.to_string(),
                Style::new().fg(MODEL_SELECTED_COLOR),
            )
        } else {
            Span::raw(" ".repeat(cols(HOOKS_MARKER)))
        };
        let style = if i == selected {
            Style::new().fg(MODEL_SELECTED_COLOR)
        } else {
            Style::new().fg(MODEL_ID_COLOR)
        };
        let room = (width as usize)
            .saturating_sub(cols(MODEL_INDENT) + cols(HOOKS_MARKER) + 4)
            .max(1);
        // `…`-cut: MCP tool names are long and prefix-heavy (search_x,
        // search_y) — silently clipped, two rows read as the same tool.
        lines.push(Line::from(vec![
            Span::raw(MODEL_INDENT),
            marker,
            Span::styled(format!("{}. ", i + 1), style),
            Span::styled(ellipsize(&tool.name, room), style),
        ]));
    }
    if end < server.tools.len() {
        lines.push(dim_line(
            &format!("{HOOKS_DOWN_MARKER}{} more below", server.tools.len() - end),
            width,
        ));
    }
    lines.extend([
        Line::default(),
        dim_line(MCP_SERVER_HINT, width),
        Line::default(),
        model_rule(width),
    ]);
    lines
}

/// The tool detail page: names, description, and the parameter listing.
fn tool_lines_page(
    server: &McpServerSnapshot,
    tool_index: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let Some(tool) = server.tools.get(tool_index) else {
        return vec![model_rule(width), Line::default(), model_rule(width)];
    };
    let label = Style::new().fg(MCP_DETAIL_LABEL_COLOR);
    let dim = Style::new().fg(MCP_DETAIL_VALUE_COLOR);
    let mut lines = vec![
        model_rule(width),
        Line::default(),
        title_line(&tool.name, width),
        dim_line(&server.name, width),
        Line::default(),
    ];
    lines.extend(tool_field_lines("Tool name:", &tool.name, width));
    lines.extend(tool_field_lines(
        "Full name:",
        &tool_wire_name(&server.name, &tool.name),
        width,
    ));
    if !tool.description.trim().is_empty() {
        lines.push(Line::default());
        lines.push(mcp_line("Description:", label, width));
        // The one paragraph on the page written *for* a reader, so it reads
        // as brightly as the label announcing it.
        lines.extend(wrapped(
            tool.description.trim(),
            Style::new().fg(MCP_DESCRIPTION_COLOR),
            width,
        ));
    }
    let parameters = tool_parameters(&tool.input_schema);
    if !parameters.is_empty() {
        lines.push(Line::default());
        lines.push(mcp_line("Parameters:", label, width));
        let room = (width as usize)
            .saturating_sub(cols(MODEL_INDENT) + cols(MCP_PARAM_BULLET))
            .max(1) as u16;
        for parameter in parameters {
            let requirement = if parameter.required {
                " (required)"
            } else {
                ""
            };
            let text = if parameter.description.is_empty() {
                format!("{}{requirement}: {}", parameter.name, parameter.kind)
            } else {
                format!(
                    "{}{requirement}: {} - {}",
                    parameter.name, parameter.kind, parameter.description
                )
            };
            for (i, row) in wrap_output(&text, room).into_iter().enumerate() {
                let first = i == 0;
                let lead = if first {
                    MCP_PARAM_BULLET
                } else {
                    MCP_PARAM_INDENT
                };
                let mut spans = vec![
                    Span::raw(MODEL_INDENT),
                    Span::styled(lead, if first { label } else { dim }),
                ];
                // The name leads its row bright — it is what the reader is
                // scanning this column for; the schema's boilerplate around
                // it (the requirement, the type, the server's prose) and
                // every continuation row stay dim.
                match row.strip_prefix(parameter.name.as_str()).filter(|_| first) {
                    Some(rest) => {
                        spans.push(Span::styled(parameter.name.clone(), label));
                        spans.push(Span::styled(rest.to_string(), dim));
                    }
                    None => spans.push(Span::styled(row, dim)),
                }
                lines.push(Line::from(spans));
            }
        }
    }
    lines.extend([
        Line::default(),
        dim_line(HOOKS_DETAIL_HINT, width),
        Line::default(),
        model_rule(width),
    ]);
    lines
}

/// The OAuth page: the flow's narration, the authorize URL (`c` to copy),
/// and the `URL >` paste-the-redirect fallback.
fn auth_lines(menu: &McpMenu, server: &McpServerSnapshot, width: u16) -> Vec<Line<'static>> {
    let mut lines = vec![
        model_rule(width),
        Line::default(),
        title_line(&format!("Authenticating with {}…", server.name), width),
        Line::default(),
    ];
    // Wrapped like its sibling notes — the three narrate one flow.
    lines.extend(dim_wrapped(MCP_AUTH_BROWSER_NOTE, width));
    lines.push(Line::default());
    match &menu.auth.url {
        Some(url) => {
            lines.extend(dim_wrapped(MCP_AUTH_COPY_NOTE, width));
            let room = (width as usize).saturating_sub(cols(MODEL_INDENT)).max(1) as u16;
            for row in wrap_output(url, room) {
                lines.push(mcp_line(&row, Style::new().fg(MODEL_SELECTED_COLOR), width));
            }
        }
        None => lines.push(dim_line(MCP_AUTH_WAITING, width)),
    }
    lines.push(Line::default());
    lines.extend(dim_wrapped(MCP_AUTH_PASTE_NOTE, width));
    // The `URL > {input}` field — the caret parks at its end
    // (`cursor_position`'s branch), so it reads as the one typable line.
    let field_room = (width as usize)
        .saturating_sub(cols(MODEL_INDENT) + cols(MCP_AUTH_PROMPT))
        .max(1);
    let shown: String = if cols(&menu.auth.input) > field_room {
        // Keep the tail — the interesting end of a long pasted URL.
        let mut tail = menu.auth.input.clone();
        while cols(&tail) > field_room {
            tail.remove(0);
        }
        tail
    } else {
        menu.auth.input.clone()
    };
    lines.push(Line::from(vec![
        Span::raw(MODEL_INDENT),
        Span::styled(MCP_AUTH_PROMPT, Style::new().fg(MODEL_META_COLOR)),
        Span::styled(shown, Style::new().fg(AI_COLOR)),
    ]));
    if menu.auth.submitted {
        lines.push(dim_line(MCP_AUTH_SUBMITTED, width));
    }
    lines.push(Line::default());
    // Wrapped: "Press Esc to go back" is the note's actionable tail.
    lines.extend(dim_wrapped(MCP_AUTH_RETURN_NOTE, width));
    lines.push(Line::default());
    lines.push(model_rule(width));
    lines
}

/// The whole framed body for the menu's current page — empty when the menu
/// is closed. What [`render_mcp_menu`] paints and [`mcp_menu_height`] counts.
#[must_use]
pub fn mcp_view_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let Some(menu) = &app.mcp_menu else {
        return Vec::new();
    };
    match &menu.page {
        McpPage::List => list_lines(menu, width),
        page => {
            let Some(server) = menu.current_server() else {
                return list_lines(menu, width);
            };
            match page {
                McpPage::Server => server_lines(menu, server, width),
                McpPage::Tools => tools_lines(menu, server, width),
                McpPage::Tool { tool } => tool_lines_page(server, *tool, width),
                McpPage::Auth => auth_lines(menu, server, width),
                McpPage::List => unreachable!("matched above"),
            }
        }
    }
}

/// The inline live-region height when the `/mcp` manager is open, or `None`
/// when it isn't — the hooks menu's rule: the streaming strip keeps its rows
/// above the menu (`layout::view_height`), the body is the built
/// line count, clamped to the terminal.
#[must_use]
pub fn mcp_menu_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.mcp_menu.as_ref()?;
    let body = u16::try_from(mcp_view_lines(app, width).len()).unwrap_or(u16::MAX);
    Some(super::layout::view_height(app, width, body, term_height))
}

/// Render the inline `/mcp` manager into the live region, in place of the
/// composer — bottom-anchored, so a page taller than the area keeps its tail
/// (the hint, the bottom rule) on screen while the skipped top flows into
/// scrollback (`docs/view-flow.md`). Pure — `render_live` paints this. See
/// `docs/mcp.md`.
pub fn render_mcp_menu(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(area, buf, mcp_view_lines(app, area.width));
}
