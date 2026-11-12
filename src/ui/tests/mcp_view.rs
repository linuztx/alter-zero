//! The `/mcp` manager's rendering (`docs/tools/mcp.md`).

use super::*;
use crate::app::{McpPage, server_actions};
use crate::mcp::{
    McpAuthState, McpScope, McpServerConfig, McpServerSnapshot, McpServerStatus, McpToolInfo,
};
use crate::ui::mcp_view_lines;
use crate::ui::theme::{
    HOOKS_DETAIL_HINT, MCP_FIELD_COL, MCP_PARAM_BULLET, MCP_PARAM_INDENT, MCP_SESSION_OFF,
    MCP_TOOL_FIELD_GAP, error_color, mcp_description_color, mcp_detail_label_color,
    mcp_detail_state_color, mcp_detail_value_color, model_meta_color, tool_fail_color,
    tool_ok_color,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn snapshot(name: &str, scope: McpScope, status: McpServerStatus) -> McpServerSnapshot {
    // Auth derives the way the manager derives it (`crate::mcp::auth_state`):
    // a row only when the server demands auth or tokens are stored.
    let auth = crate::mcp::auth_state(true, false, false, &status);
    McpServerSnapshot {
        name: name.to_string(),
        scope,
        config_path: match scope {
            McpScope::Project => "~/repo/.mcp.json".to_string(),
            McpScope::User => "~/.alter-zero/mcp.json".to_string(),
        },
        config: McpServerConfig::Http {
            url: format!("https://{name}.test/mcp"),
            headers: Default::default(),
            sse_fallback: false,
            oauth: None,
        },
        status,
        auth,
        identity: Some(crate::mcp::ServerIdentity {
            protocol_version: "2026-07-28".to_string(),
            name: name.to_string(),
            version: "1.0".to_string(),
            capabilities: vec!["tools".to_string()],
            instructions: None,
            tools_list_changed: false,
        }),
        excluded: Vec::new(),
        tools: vec![McpToolInfo {
            name: "ask_question".to_string(),
            description: "Ask any question about a GitHub repository.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "repoName": {"description": "owner/repo"},
                    "question": {"type": "string", "description": "The question."}
                },
                "required": ["repoName", "question"]
            }),
        }],
    }
}

fn mcp_app() -> App {
    let mut app = App::new();
    app.open_mcp_menu(vec![
        snapshot("linear", McpScope::Project, McpServerStatus::NeedsAuth),
        snapshot("deepwiki", McpScope::User, McpServerStatus::Connected),
        snapshot("github", McpScope::User, McpServerStatus::Disabled),
    ]);
    app
}

fn texts(app: &App) -> Vec<String> {
    mcp_view_lines(app, 100).iter().map(plain).collect()
}

#[test]
fn the_list_groups_by_scope_with_status_rows() {
    let app = mcp_app();
    let lines = texts(&app);
    let all = lines.join("\n");
    assert!(all.contains("Manage MCP servers"));
    assert!(all.contains("3 servers"));
    assert!(all.contains("Project MCPs (~/repo/.mcp.json)"));
    assert!(all.contains("User MCPs (~/.alter-zero/mcp.json)"));
    assert!(all.contains("linear · △ needs authentication"));
    assert!(all.contains("deepwiki · ✔ connected · 1 tool"));
    assert!(all.contains("github · ◯ disabled"));
    // The selected row wears the ❯ marker.
    assert!(lines.iter().any(|l| l.contains("❯ linear")));
    assert!(all.contains("↑/↓ to navigate · Enter to confirm · Esc to cancel"));
}

#[test]
fn an_empty_list_names_the_config_files() {
    let mut app = App::new();
    app.open_mcp_menu(vec![]);
    let all = texts(&app).join("\n");
    assert!(all.contains("No MCP servers configured"));
    assert!(all.contains(".mcp.json"));
}

// --- MCP off for the session (the `/settings` **MCP** row, docs/tools/mcp.md) ---

/// `/mcp` opened while the `/settings` **MCP** row is off, over what the
/// manager reports then: an enabled server `◯ off`, a disabled one still
/// disabled.
fn mcp_off_app() -> App {
    let mut app = App::new();
    app.settings_mut().mcp = false;
    app.open_mcp_menu(vec![
        snapshot("deepwiki", McpScope::User, McpServerStatus::Off),
        snapshot("github", McpScope::User, McpServerStatus::Disabled),
    ]);
    app
}

#[test]
fn the_list_says_when_mcp_is_off_for_the_session() {
    // The `/hooks` and `/skills` rule: the rows still browse and toggle, but
    // nothing here connects until the `/settings` row is back on — and the
    // page says where that is, in red, straight under its count.
    let app = mcp_off_app();
    let lines = mcp_view_lines(&app, 100);
    let rows: Vec<String> = lines.iter().map(plain).collect();
    let note = rows
        .iter()
        .position(|row| row.contains(MCP_SESSION_OFF))
        .unwrap_or_else(|| panic!("no off note: {rows:#?}"));
    assert!(rows[note - 1].contains("2 servers"), "{rows:#?}");
    let ink = lines[note]
        .spans
        .iter()
        .find(|span| !span.content.trim().is_empty())
        .and_then(|span| span.style.fg);
    assert_eq!(ink, Some(error_color()), "red, like the hooks note");
    let all = rows.join("\n");
    assert!(all.contains("deepwiki · ◯ off"), "{all}");
    assert!(all.contains("github · ◯ disabled"), "{all}");
}

#[test]
fn the_off_note_wraps_rather_than_clipping_its_advice() {
    // The tail — where MCP is turned back on — is the actionable half.
    let rows = texts_at(&mcp_off_app(), 30);
    assert!(
        squashed(&rows).contains(&MCP_SESSION_OFF.replace(' ', "")),
        "{rows:#?}"
    );
}

#[test]
fn no_off_note_while_mcp_is_on_or_over_an_empty_list() {
    let on = texts(&mcp_app()).join("\n");
    assert!(!on.contains(MCP_SESSION_OFF), "{on}");
    // With nothing declared the row is unavailable and the placeholder
    // answers instead — the `/skills` menu's rule.
    let mut empty = App::new();
    empty.settings_mut().mcp = false;
    empty.open_mcp_menu(vec![]);
    let all = texts(&empty).join("\n");
    assert!(!all.contains(MCP_SESSION_OFF), "{all}");
    assert!(all.contains("No MCP servers configured"), "{all}");
}

#[test]
fn an_off_server_reads_dim_and_its_page_offers_its_own_switch() {
    let mut app = mcp_off_app();
    let lines = mcp_view_lines(&app, 78);
    let row = lines
        .iter()
        .find(|line| plain(line).contains("deepwiki"))
        .expect("the row");
    let glyph = row
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "◯")
        .expect("the ◯ glyph span");
    assert_eq!(
        glyph.style.fg,
        Some(model_meta_color()),
        "inactive, not a problem"
    );
    app.on_key(key(KeyCode::Enter));
    let all = texts(&app).join("\n");
    assert!(all.contains("◯ off"), "{all}");
    assert!(all.contains("1. Disable"), "{all}");
    assert!(!all.contains("Reconnect"), "{all}");
}

#[test]
fn an_off_servers_page_says_why_it_is_off() {
    // Its one action is its own Disable, so the page itself has to say why
    // the server is not running and where that changes — the list's note,
    // repeated where the user is looking. A server disabled on its own is
    // off for its own reason, and its page does not claim the session's.
    let mut app = mcp_off_app();
    app.on_key(key(KeyCode::Enter)); // deepwiki, `◯ off`
    let lines = mcp_view_lines(&app, 100);
    let note = lines
        .iter()
        .find(|line| plain(line).contains(MCP_SESSION_OFF))
        .unwrap_or_else(|| panic!("no note on the off server's page: {:#?}", texts(&app)));
    let ink = note
        .spans
        .iter()
        .find(|span| !span.content.trim().is_empty())
        .and_then(|span| span.style.fg);
    assert_eq!(ink, Some(error_color()), "red, like the list's note");
    // Wrapped, not clipped: the tail is the actionable half.
    let narrow = texts_at(&app, 30);
    assert!(
        squashed(&narrow).contains(&MCP_SESSION_OFF.replace(' ', "")),
        "{narrow:#?}"
    );
    let mut disabled = mcp_off_app();
    disabled.on_key(key(KeyCode::Down));
    disabled.on_key(key(KeyCode::Enter)); // github, `◯ disabled`
    let all = texts(&disabled).join("\n");
    assert!(all.contains("◯ disabled"), "{all}");
    assert!(!all.contains(MCP_SESSION_OFF), "{all}");
}

#[test]
fn the_server_detail_shows_facts_and_actions() {
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Enter)); // linear detail
    let all = texts(&app).join("\n");
    assert!(all.contains("Linear MCP Server"));
    assert!(all.contains("Status:"));
    assert!(all.contains("△ needs authentication"));
    assert!(all.contains("✘ not authenticated"));
    assert!(all.contains("URL:"));
    assert!(all.contains("https://linear.test/mcp"));
    assert!(all.contains("Config location:"));
    assert!(all.contains("1. Authenticate"));
    assert!(all.contains("2. Disable"));
    assert!(all.contains("❯ 1. Authenticate"), "first action selected");
}

#[test]
fn the_detail_shows_the_negotiated_protocol_and_a_truthful_auth_row() {
    // deepwiki: connected while presenting nothing, so it was never
    // challenged. The row is *shown* and says so — `✘ not authenticated`
    // beside `✔ connected` reported a problem where there is none, and
    // hiding the row entirely just leaves the question unanswered.
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter)); // deepwiki detail
    let all = texts(&app).join("\n");
    assert!(all.contains("Deepwiki MCP Server"));
    assert!(all.contains("Protocol:"), "{all}");
    assert!(all.contains("2026-07-28"));
    assert!(all.contains("Auth:"), "{all}");
    assert!(all.contains("✔ authenticated"), "{all}");
    assert!(!all.contains("not authenticated"), "{all}");
    // An authenticated server still shows its ✔ row.
    let mut authed = snapshot("vercel", McpScope::User, McpServerStatus::Connected);
    authed.auth = Some(McpAuthState::Authenticated);
    let mut app = App::new();
    app.open_mcp_menu(vec![authed]);
    app.on_key(key(KeyCode::Enter));
    let all = texts(&app).join("\n");
    assert!(all.contains("Auth:"));
    assert!(all.contains("✔ authenticated"));
    assert!(all.contains("Protocol:"));
    // A server that never initialized has no revision to report: no row.
    let mut pending = snapshot("slow", McpScope::User, McpServerStatus::Pending);
    pending.identity = None;
    let mut app = App::new();
    app.open_mcp_menu(vec![pending]);
    app.on_key(key(KeyCode::Enter));
    let all = texts(&app).join("\n");
    assert!(!all.contains("Protocol:"));
}

#[test]
fn the_auth_row_distinguishes_a_header_a_dead_grant_and_a_public_server() {
    let row = |auth: Option<McpAuthState>| {
        let mut server = snapshot("s", McpScope::User, McpServerStatus::Connected);
        server.auth = auth;
        let mut app = App::new();
        app.open_mcp_menu(vec![server]);
        app.on_key(key(KeyCode::Enter));
        texts(&app).join("\n")
    };
    // A written-down token is a login the user already has — never "not
    // authenticated", and nothing to re-run or clear.
    let header = row(Some(McpAuthState::Header));
    assert!(
        header.contains("✔ authenticated (config header)"),
        "{header}"
    );
    assert!(!header.contains("Re-authenticate"), "{header}");
    assert!(!header.contains("Clear authentication"), "{header}");
    // A public server reads settled — the same row a stored grant earns,
    // because from the user's side the answer is the same: you are cleared
    // to use it. It stays actionless, though: there is no grant to re-run.
    let public = row(Some(McpAuthState::NotRequired));
    assert!(public.contains("✔ authenticated"), "{public}");
    assert!(!public.contains("Re-authenticate"), "{public}");
    // A grant the server refuses reads expired — and stays removable.
    let expired = row(Some(McpAuthState::Expired));
    assert!(expired.contains("✘ expired"), "{expired}");
    assert!(expired.contains("Clear authentication"), "{expired}");
}

#[test]
fn the_tools_and_tool_detail_pages_render() {
    let mut app = mcp_app();
    // deepwiki (connected) → View tools → tool detail.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter)); // View tools
    let all = texts(&app).join("\n");
    assert!(all.contains("Tools for deepwiki"));
    assert!(all.contains("1 tool"));
    assert!(all.contains("1. ask_question"));
    app.on_key(key(KeyCode::Enter)); // tool detail
    let all = texts(&app).join("\n");
    assert!(all.contains("Tool name:"));
    assert!(all.contains("ask_question"));
    assert!(all.contains("Full name:"));
    assert!(all.contains("mcp__deepwiki__ask_question"));
    assert!(all.contains("Description:"));
    assert!(all.contains("Parameters:"));
    assert!(all.contains("● repoName (required): unknown - owner/repo"));
    assert!(all.contains("● question (required): string - The question."));
    assert!(all.contains("Esc to go back"));
}

/// The `Full name:` row is the name the model actually calls, so a tool
/// whose plain name clashes with a sibling's shows the name it is really
/// offered under — the same one `mcp::wire_names` hands the manager.
#[test]
fn the_full_name_is_the_one_the_model_is_offered() {
    let mut server = snapshot("srv", McpScope::User, McpServerStatus::Connected);
    server.tools = ["files.read", "files_read"]
        .into_iter()
        .map(|name| McpToolInfo {
            name: name.to_string(),
            description: String::new(),
            input_schema: serde_json::json!({"type": "object"}),
        })
        .collect();
    let offered = crate::mcp::wire_names("srv", &["files.read", "files_read"]);
    let mut app = App::new();
    app.open_mcp_menu(vec![server]);
    app.on_key(key(KeyCode::Enter)); // its detail
    app.on_key(key(KeyCode::Enter)); // View tools
    app.on_key(key(KeyCode::Down)); // files_read
    app.on_key(key(KeyCode::Enter));
    let all = texts(&app).join("\n");
    assert!(all.contains(&offered[1].0), "{all}");
    assert!(!all.contains("mcp__srv__files_read\n"), "{all}");
}

/// A tool set aside at connect is named on the server page with the
/// reason, so "why can't the model see `bad`?" has an answer on screen.
#[test]
fn the_server_page_names_the_tools_not_offered() {
    let mut server = snapshot("srv", McpScope::User, McpServerStatus::Connected);
    server.excluded = vec![crate::mcp::ExcludedTool {
        name: "bad".to_string(),
        reason: "its input schema has type \"string\", not \"object\"".to_string(),
    }];
    let mut app = App::new();
    app.open_mcp_menu(vec![server]);
    app.on_key(key(KeyCode::Enter));
    let all = texts(&app).join("\n");
    assert!(all.contains("Not offered to the model:"), "{all}");
    assert!(all.contains("bad — its input schema has type"), "{all}");
}

/// Walk `app` to the `ask_question` detail page (list → deepwiki → tools →
/// the tool), the deepest page and the one the two-tone body lives on.
fn tool_detail_app() -> App {
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Down)); // deepwiki (connected — it has tools)
    app.on_key(key(KeyCode::Enter)); // its detail
    app.on_key(key(KeyCode::Enter)); // View tools
    app.on_key(key(KeyCode::Enter)); // ask_question
    app
}

#[test]
fn every_page_headline_wears_the_title_dress_and_the_server_one_is_capitalised() {
    // The manager is a four-page walk, so the headline is the only row that
    // says where you are — it wears the theme's gradient every page title
    // does (`docs/tools/mcp.md`, `docs/ui/theme.md`).
    let headline = |app: &App| {
        let line = mcp_view_lines(app, 100)
            .into_iter()
            .find(wears_the_title_dress)
            .expect("a headline in the title dress");
        plain(&line).trim().to_string()
    };
    let mut app = mcp_app();
    assert_eq!(headline(&app), "Manage MCP servers");
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter)); // deepwiki detail
    // A config key is lower-case by convention; a headline is a headline.
    assert_eq!(headline(&app), "Deepwiki MCP Server");
    app.on_key(key(KeyCode::Enter)); // View tools
    assert_eq!(headline(&app), "Tools for deepwiki");
    app.on_key(key(KeyCode::Enter)); // the tool detail
    assert_eq!(headline(&app), "ask_question");
    // …over the server it belongs to, dim: the name identifies, it doesn't
    // announce.
    let under = mcp_view_lines(&app, 100)
        .into_iter()
        .find(|line| plain(line).trim() == "deepwiki")
        .expect("the server subtitle");
    assert_eq!(under.spans[1].style.fg, Some(mcp_detail_value_color()));
}

#[test]
fn the_server_detail_lights_its_labels_and_only_the_state_values() {
    // deepwiki: connected, one tool, never challenged for credentials.
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    let lines = mcp_view_lines(&app, 100);
    let find = |text: &str| {
        lines
            .iter()
            .find(|line| plain(line).contains(text))
            .unwrap_or_else(|| panic!("no row containing {text:?}"))
            .clone()
    };
    // The tool count is the `Tools:` row's job — a page that says it twice
    // is a page with a duplicate on it.
    let status = find("Status:");
    assert_eq!(
        plain(&status),
        format!("  {:<MCP_FIELD_COL$}✔ connected", "Status:")
    );
    assert_eq!(status.spans[1].style.fg, Some(mcp_detail_label_color()));
    // The glyph keeps its state colour — it is the one thing on the row that
    // still has to shout when a server is failing — over white words.
    assert_eq!(status.spans[2].content.as_ref(), "✔ ");
    assert_eq!(status.spans[2].style.fg, Some(tool_ok_color()));
    assert_eq!(status.spans[3].content.as_ref(), "connected");
    assert_eq!(status.spans[3].style.fg, Some(mcp_detail_state_color()));
    // A server that needs no login is one you are cleared to use, so it
    // reports the settled row rather than a `◯ not needed` shrug.
    let auth = find("Auth:");
    assert_eq!(
        plain(&auth),
        format!("  {:<MCP_FIELD_COL$}✔ authenticated", "Auth:")
    );
    assert_eq!(auth.spans[1].style.fg, Some(mcp_detail_label_color()));
    assert_eq!(auth.spans[2].style.fg, Some(tool_ok_color()));
    assert_eq!(auth.spans[3].style.fg, Some(mcp_detail_state_color()));
    // The addresses, the revision and the count are what the labels lead to,
    // not what the page is about: bright label, quiet value.
    for row in ["Protocol:", "URL:", "Config location:", "Tools:"] {
        let line = find(row);
        assert_eq!(
            line.spans[1].style.fg,
            Some(mcp_detail_label_color()),
            "{row}"
        );
        assert_eq!(
            line.spans[2].style.fg,
            Some(mcp_detail_value_color()),
            "{row}"
        );
    }
    // What the server can actually do stays lit with the state rows.
    let caps = find("Capabilities:");
    assert_eq!(caps.spans[1].style.fg, Some(mcp_detail_label_color()));
    assert_eq!(caps.spans[2].style.fg, Some(mcp_detail_state_color()));
}

#[test]
fn a_failing_server_keeps_its_red_glyph_beside_the_white_words() {
    // The two-tone must not launder a failure into a calm white row.
    let mut server = snapshot(
        "broken",
        McpScope::User,
        McpServerStatus::Failed("no".into()),
    );
    server.auth = Some(McpAuthState::Expired);
    let mut app = App::new();
    app.open_mcp_menu(vec![server]);
    app.on_key(key(KeyCode::Enter));
    let lines = mcp_view_lines(&app, 100);
    let find = |text: &str| {
        lines
            .iter()
            .find(|line| plain(line).contains(text))
            .unwrap_or_else(|| panic!("no row containing {text:?}"))
            .clone()
    };
    let status = find("Status:");
    assert_eq!(status.spans[2].style.fg, Some(tool_fail_color()));
    assert_eq!(status.spans[3].style.fg, Some(mcp_detail_state_color()));
    let auth = find("Auth:");
    assert_eq!(
        plain(&auth),
        format!("  {:<MCP_FIELD_COL$}✘ expired", "Auth:")
    );
    assert_eq!(auth.spans[2].style.fg, Some(tool_fail_color()));
}

#[test]
fn the_tool_detail_reads_label_bright_and_value_dim() {
    let app = tool_detail_app();
    let lines = mcp_view_lines(&app, 100);
    let find = |text: &str| {
        lines
            .iter()
            .find(|line| plain(line).contains(text))
            .unwrap_or_else(|| panic!("no row containing {text:?}"))
            .clone()
    };
    // One space after the label, not the server page's 18-column pad — the
    // two labels are the same width, so they line up on their own.
    let name = find("Tool name:");
    assert_eq!(
        plain(&name),
        format!("  Tool name:{MCP_TOOL_FIELD_GAP}ask_question")
    );
    assert_eq!(name.spans[1].style.fg, Some(mcp_detail_label_color()));
    assert_eq!(name.spans[2].style.fg, Some(mcp_detail_value_color()));
    let full = find("Full name:");
    assert_eq!(
        plain(&full),
        format!("  Full name:{MCP_TOOL_FIELD_GAP}mcp__deepwiki__ask_question")
    );
    assert_eq!(full.spans[1].style.fg, Some(mcp_detail_label_color()));
    assert_eq!(full.spans[2].style.fg, Some(mcp_detail_value_color()));
    // The description's label is bright like every other; the prose under it
    // is **half white** — a third tone, so it can't be mistaken for either
    // the labels organising the page or the schema boilerplate below it.
    assert_eq!(
        find("Description:").spans[1].style.fg,
        Some(mcp_detail_label_color())
    );
    assert_eq!(
        find("Ask any question").spans[1].style.fg,
        Some(mcp_description_color())
    );
    assert_ne!(mcp_description_color(), mcp_detail_label_color());
    assert_ne!(mcp_description_color(), mcp_detail_value_color());
    assert_eq!(
        find("Parameters:").spans[1].style.fg,
        Some(mcp_detail_label_color())
    );
}

#[test]
fn a_parameter_lights_its_name_and_dims_what_it_introduces() {
    // Narrow enough that the first parameter wraps: the name leads its row
    // bright, the schema prose after it dims, and the continuation row —
    // carrying no name — is dim throughout.
    let app = tool_detail_app();
    let lines = mcp_view_lines(&app, 40);
    let at = lines
        .iter()
        .position(|line| plain(line).contains("● repoName"))
        .expect("the repoName row");
    let head = &lines[at];
    assert_eq!(head.spans[1].content.as_ref(), MCP_PARAM_BULLET);
    assert_eq!(head.spans[1].style.fg, Some(mcp_detail_label_color()));
    assert_eq!(head.spans[2].content.as_ref(), "repoName");
    assert_eq!(head.spans[2].style.fg, Some(mcp_detail_label_color()));
    assert!(plain(head).contains("(required): unknown"), "{head:?}");
    assert_eq!(head.spans[3].style.fg, Some(mcp_detail_value_color()));
    let tail = &lines[at + 1];
    assert_eq!(plain(tail).trim(), "owner/repo");
    assert_eq!(tail.spans[1].content.as_ref(), MCP_PARAM_INDENT);
    for span in tail.spans.iter().skip(1) {
        assert_eq!(span.style.fg, Some(mcp_detail_value_color()), "{span:?}");
    }
}

#[test]
fn the_menu_hides_the_cursor_seated_on_the_selection_and_auth_brings_it_back() {
    let app = mcp_app();
    // The permission prompt's rule (docs/tools/mcp.md): a menu shows no hardware
    // cursor — a kitty cursor animation blinks at whatever seat it picks —
    // while the seat itself tracks the highlighted `❯` row.
    assert!(!crate::ui::cursor_visible(&app));
    let height = crate::ui::mcp_menu_height(&app, 100, 60).unwrap();
    let mut buf = buffer(100, height);
    render_mcp_menu(buf.area, &mut buf, &app);
    let marker_row = (0..height)
        .find(|&y| row(&buf, y, 100).trim_start().starts_with('❯'))
        .expect("the selected server wears the marker");
    assert_eq!(crate::ui::cursor_position(buf.area, &app), (2, marker_row));

    // The Auth page's `URL >` field is typed into, so its caret comes back
    // (the permission prompt's amend-field exception).
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Enter)); // linear
    app.on_key(key(KeyCode::Enter)); // Authenticate
    assert_eq!(app.mcp_menu.as_ref().unwrap().page, McpPage::Auth);
    assert!(crate::ui::cursor_visible(&app));
}

#[test]
fn the_auth_page_shows_the_url_and_paste_field() {
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Enter)); // linear
    app.on_key(key(KeyCode::Enter)); // Authenticate
    assert_eq!(app.mcp_menu.as_ref().unwrap().page, McpPage::Auth);
    let all = texts(&app).join("\n");
    assert!(all.contains("Authenticating with linear…"));
    assert!(all.contains("A browser window will open"));
    // No URL yet: the waiting note stands in.
    assert!(all.contains("Preparing the authorization request…"));
    app.set_mcp_auth_url("linear", "https://as.test/authorize?client_id=c1");
    let all = texts(&app).join("\n");
    assert!(all.contains("copy this URL manually (c to copy)"));
    assert!(all.contains("https://as.test/authorize?client_id=c1"));
    assert!(all.contains("URL > "));
    assert!(all.contains("Press Esc to go back"));
    // Typed text lands on the field row.
    app.on_key(key(KeyCode::Char('h')));
    app.on_key(key(KeyCode::Char('i')));
    let all = texts(&app).join("\n");
    assert!(all.contains("URL > hi"));
}

#[test]
fn every_page_hugs_its_frame() {
    // The page's headline straight under the opening rule, its hint — or the
    // auth page's closing note — straight over the closing one, at every
    // stop of the walk.
    let mut empty = App::new();
    empty.open_mcp_menu(vec![]);
    let mut server = mcp_app();
    server.on_key(key(KeyCode::Enter)); // linear's detail
    let mut failed = App::new();
    failed.open_mcp_menu(vec![snapshot(
        "broken",
        McpScope::User,
        McpServerStatus::Failed("connection refused".to_string()),
    )]);
    failed.on_key(key(KeyCode::Enter));
    let mut tools = mcp_app();
    tools.on_key(key(KeyCode::Down));
    tools.on_key(key(KeyCode::Enter));
    tools.on_key(key(KeyCode::Enter)); // View tools
    let auth = |url: bool, submitted: bool| {
        let mut app = mcp_app();
        app.on_key(key(KeyCode::Enter)); // linear
        app.on_key(key(KeyCode::Enter)); // Authenticate
        if url {
            app.set_mcp_auth_url("linear", "https://as.test/authorize?client_id=c1");
        }
        app.mcp_menu.as_mut().unwrap().auth.submitted = submitted;
        app
    };
    let mut off_page = mcp_off_app();
    off_page.on_key(key(KeyCode::Enter));
    for (what, app) in [
        ("the server list", mcp_app()),
        ("the server list with MCP off", mcp_off_app()),
        ("an off server's page", off_page),
        ("an empty list", empty),
        ("a server's detail", server),
        ("a failed server's detail", failed),
        ("a server's tools", tools),
        ("a tool's detail", tool_detail_app()),
        ("the auth page waiting for its URL", auth(false, false)),
        ("the auth page with its URL", auth(true, false)),
        ("the auth page checking a pasted URL", auth(true, true)),
    ] {
        for width in [40u16, 100] {
            assert_frame_hugs_content(
                &format!("{what} at width {width}"),
                &mcp_view_lines(&app, width),
            );
        }
    }
}

#[test]
fn the_auth_caret_sits_on_the_url_field_at_any_width() {
    // The `URL >` field is the one typed row on the page, and its caret is
    // found in the page the paint builds rather than counted up from the
    // bottom: the notes under it wrap on a narrow terminal, and a counted
    // seat landed on the blank row under the field there.
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Enter)); // linear
    app.on_key(key(KeyCode::Enter)); // Authenticate
    app.set_mcp_auth_url("linear", "https://as.test/authorize?client_id=c1");
    app.on_key(key(KeyCode::Char('h')));
    app.on_key(key(KeyCode::Char('i')));
    for submitted in [false, true] {
        app.mcp_menu.as_mut().unwrap().auth.submitted = submitted;
        for width in [40u16, 100] {
            let lines = mcp_view_lines(&app, width);
            let field = lines
                .iter()
                .position(|line| plain(line).contains("URL > hi"))
                .expect("the URL field");
            let area = Rect::new(0, 0, width, lines.len() as u16);
            // indent(2) + `URL > `(6) + "hi"(2) = 10.
            assert_eq!(
                crate::ui::cursor_position(area, &app),
                (10, field as u16),
                "the caret ends the typed text (submitted {submitted}, width {width})"
            );
        }
    }
}

#[test]
fn height_agrees_with_the_built_lines_and_none_when_closed() {
    let mut app = mcp_app();
    let height = crate::ui::mcp_menu_height(&app, 100, 60).expect("open menu has a height");
    assert!(height > 0);
    app.close_mcp_menu();
    assert!(crate::ui::mcp_menu_height(&app, 100, 60).is_none());
    assert!(mcp_view_lines(&app, 100).is_empty());
}

#[test]
fn a_long_list_windows_with_overflow_markers() {
    let mut app = App::new();
    let servers: Vec<_> = (0..9)
        .map(|i| {
            snapshot(
                &format!("srv{i}"),
                McpScope::User,
                McpServerStatus::Connected,
            )
        })
        .collect();
    app.open_mcp_menu(servers);
    // Jump to the end so the window slides.
    app.on_key(key(KeyCode::End));
    let all = texts(&app).join("\n");
    assert!(all.contains("more above"), "{all}");
    assert!(all.contains("srv8"));
    // The action list still derives (sanity: server_actions is exercised).
    assert_eq!(
        server_actions(&app.mcp_menu.as_ref().unwrap().servers[0]).len(),
        3
    );
}

#[test]
fn a_tool_page_taller_than_its_area_is_bottom_anchored() {
    // The framed body paints its LAST rows when the area is shorter than the
    // page, so the interactive tail — the `Esc to go back` hint and the bottom
    // rule — is always on screen; the skipped top flows into scrollback
    // instead (docs/ui/view-flow.md). The old top-aligned paint clipped exactly
    // the rows that say the page has ended.
    let mut app = mcp_app();
    {
        let menu = app.mcp_menu.as_mut().unwrap();
        menu.servers[0].tools[0].description = "word ".repeat(400);
        menu.page = McpPage::Tool { tool: 0 };
    }
    let (width, height) = (60u16, 12u16);
    let lines = mcp_view_lines(&app, width);
    assert!(
        lines.len() > usize::from(height),
        "the page overflows the area: {} rows",
        lines.len()
    );
    let area = Rect::new(0, 0, width, height);
    let mut buf = Buffer::empty(area);
    render_mcp_menu(area, &mut buf, &app);
    // Painted rows == the page's tail, row for row.
    let expected: Vec<String> = lines[lines.len() - usize::from(height)..]
        .iter()
        .map(plain)
        .collect();
    for (y, want) in expected.iter().enumerate() {
        let got = row(&buf, y as u16, width);
        assert_eq!(got.trim_end(), want.trim_end(), "row {y} is the tail's");
    }
    let all: Vec<String> = (0..height).map(|y| row(&buf, y, width)).collect();
    assert!(
        all.iter().any(|r| r.contains(HOOKS_DETAIL_HINT)),
        "the Esc hint stays on screen: {all:?}"
    );
    assert!(
        all.last().unwrap().contains("──"),
        "the bottom rule stays on screen: {:?}",
        all.last()
    );
}

// --- the row separators stay dim at every status (docs/tools/mcp.md) ---

#[test]
fn a_row_paints_only_its_glyph_in_the_status_colour() {
    // ` · ` is chrome, not status: a connected row used to paint its first
    // separator green (it rode the glyph's span) while the second stayed dim,
    // so one row carried two colours of the same mark. Only the glyph is
    // coloured now; both separators read dim at every status.
    let app = mcp_app();
    let lines = mcp_view_lines(&app, 78);
    let connected = lines
        .iter()
        .find(|l| plain(l).contains("deepwiki"))
        .expect("the connected row");
    for span in &connected.spans {
        if span.content.contains('·') {
            assert_eq!(
                span.style.fg,
                Some(model_meta_color()),
                "a separator span is dim: {:?}",
                span.content
            );
        }
    }
    let glyph = connected
        .spans
        .iter()
        .find(|s| s.content.contains('✔'))
        .expect("the ✔ glyph span");
    assert_eq!(
        glyph.style.fg,
        Some(tool_ok_color()),
        "the glyph keeps the connected green"
    );
    assert_eq!(
        glyph.content.as_ref(),
        "✔",
        "the glyph span carries the glyph alone"
    );
}

/// Rejoin rows squashing all whitespace, for needles that wrap mid-token.
fn squashed(rows: &[String]) -> String {
    rows.join("").replace(' ', "")
}

/// The built body as plain rows at `width` (the shared `texts` is 100-wide).
fn texts_at(app: &App, width: u16) -> Vec<String> {
    mcp_view_lines(app, width)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

#[test]
fn the_server_detail_wraps_the_target_and_config_paths() {
    // A remote URL / stdio command and the config path are exactly what the
    // user opens the page to read — they wrap whole under their labels.
    let mut app = App::new();
    app.open_mcp_menu(vec![snapshot(
        "a-very-long-subdomain-for-the-docs-server",
        McpScope::User,
        McpServerStatus::Connected,
    )]);
    app.on_key(key(KeyCode::Enter)); // the server's detail page
    let rows = texts_at(&app, 40);
    for row in &rows {
        assert!(
            crate::ui::wrap::cols(row) <= 40,
            "no row leaks past the width: {row:?}"
        );
    }
    let all = squashed(&rows);
    assert!(
        all.contains("test/mcp"),
        "the URL's tail survives: {rows:#?}"
    );
    assert!(
        all.contains("mcp.json"),
        "the config path's filename survives: {rows:#?}"
    );
}

#[test]
fn a_narrow_server_row_keeps_its_status_glyph() {
    // The `· {glyph} {status}` suffix used to be dropped whole when it did
    // not fit — a Failed server's red state vanished at narrow widths. The
    // glyph (the state) now survives with the words `…`-cut.
    let mut app = App::new();
    app.open_mcp_menu(vec![snapshot(
        "a-server-with-a-fairly-long-name",
        McpScope::User,
        McpServerStatus::Failed("boom".into()),
    )]);
    let rows = texts_at(&app, 44);
    let glyph = crate::mcp::McpServerStatus::Failed(String::new()).glyph();
    let row = rows
        .iter()
        .find(|r| r.contains("a-server"))
        .expect("the server row");
    assert!(
        row.contains(glyph),
        "the failure glyph survives the squeeze: {row:?}"
    );
    assert!(
        crate::ui::wrap::cols(row) <= 44,
        "and the row still fits: {row:?}"
    );
}

#[test]
fn the_empty_list_paths_wrap_instead_of_clipping() {
    let mut app = App::new();
    app.open_mcp_menu(vec![]);
    let rows = texts_at(&app, 30);
    let all = squashed(&rows);
    assert!(
        all.contains("mcp.json"),
        "the add-one-at paths keep their filenames: {rows:#?}"
    );
}

/// A rejected paste's reason sits under the field; the page still hugs its
/// rules.
#[test]
fn a_rejected_paste_shows_its_reason_under_the_field() {
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Enter)); // linear
    app.on_key(key(KeyCode::Enter)); // Authenticate
    app.reject_mcp_auth_paste("linear", "the response belongs to another sign-in attempt");
    for width in [40u16, 100] {
        let lines = mcp_view_lines(&app, width);
        let text: Vec<String> = lines.iter().map(plain).collect();
        let field = text
            .iter()
            .position(|line| line.contains("URL >"))
            .expect("the URL field");
        let below = text[field + 1..]
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            below.contains("the response belongs to another sign-in attempt"),
            "{text:?}"
        );
        assert_frame_hugs_content(&format!("a rejected paste at width {width}"), &lines);
    }
}

// --- Clickable URLs on the sign-in walk (docs/ui/links.md, docs/tools/mcp.md) ---

/// A realistic authorize URL: far wider than any terminal row, as a real one
/// always is (the PKCE challenge, the state, the percent-encoded redirect and
/// resource), so the page has to hard-break it across rows.
const AUTH_URL: &str = "https://auth.linear.test/oauth/authorize?response_type=code\
    &client_id=dyn-4f2c9a7e&redirect_uri=http%3A%2F%2F127.0.0.1%3A49152%2Fcallback\
    &code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM&code_challenge_method=S256\
    &state=6a1f0c2e9b&resource=https%3A%2F%2Flinear.test%2Fmcp";

/// The `linear` server's auth page, showing `url` as its authorize URL.
fn auth_page(url: &str) -> App {
    let mut app = mcp_app();
    app.on_key(key(KeyCode::Enter)); // linear
    app.on_key(key(KeyCode::Enter)); // Authenticate
    assert_eq!(app.mcp_menu.as_ref().unwrap().page, McpPage::Auth);
    app.set_mcp_auth_url("linear", url);
    app
}

/// Every distinct link target the spans of `lines` carry, in page order.
fn link_targets(lines: &[Line<'static>]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for span in lines.iter().flat_map(|l| l.spans.iter()) {
        if let Some(url) = crate::links::style_link(&span.style)
            && !out.iter().any(|seen| seen.as_str() == &*url)
        {
            out.push(url.to_string());
        }
    }
    out
}

/// The rows `url` is painted on — from the row that opens it to the last row
/// whose text still continues it — and what those rows spell together.
fn url_rows<'a>(lines: &'a [Line<'static>], url: &str) -> (&'a [Line<'static>], String) {
    let start = lines
        .iter()
        .position(|l| {
            let text = plain(l);
            let text = text.trim();
            !text.is_empty() && url.starts_with(text)
        })
        .expect("a row opening the URL");
    let mut spelled = String::new();
    let mut end = start;
    while end < lines.len() {
        let text = plain(&lines[end]).trim().to_string();
        if text.is_empty() || !url[spelled.len()..].starts_with(&text) {
            break;
        }
        spelled.push_str(&text);
        end += 1;
    }
    (&lines[start..end], spelled)
}

/// The spans of `rows` that paint something (the inset is not part of a link).
fn inked<'a>(rows: &'a [Line<'static>]) -> impl Iterator<Item = &'a Span<'static>> {
    rows.iter()
        .flat_map(|l| l.spans.iter())
        .filter(|s| !s.content.trim().is_empty())
}

#[test]
fn the_auth_pages_url_opens_the_whole_target_from_every_fragment() {
    // The reported bug: `/login`'s sign-in URL opens when clicked, `/mcp`'s
    // did not. An authorize URL is wider than any row, so it hard-breaks,
    // and a terminal's own URL detection only ever reads row text: the first
    // row opened a truncated URL and the rows under it were no links at all.
    // Every fragment must carry the WHOLE target — the carrier `term` turns
    // into one OSC 8 hyperlink (docs/ui/links.md).
    let app = auth_page(AUTH_URL);
    for width in [40u16, 80, 120] {
        let lines = mcp_view_lines(&app, width);
        let (rows, spelled) = url_rows(&lines, AUTH_URL);
        assert_eq!(
            spelled, AUTH_URL,
            "the rows still spell the whole URL (width {width})"
        );
        assert!(
            rows.len() >= 2,
            "the URL must wrap for this test to mean anything (width {width})"
        );
        for span in inked(rows) {
            assert_eq!(
                crate::links::style_link(&span.style).as_deref(),
                Some(AUTH_URL),
                "every fragment opens the whole URL, not its own row text: {:?} (width {width})",
                span.content
            );
        }
    }
}

#[test]
fn the_auth_url_keeps_its_accent_gains_an_underline_and_is_the_pages_one_link() {
    // The `/login` family's dress for a URL the user acts on: the page's own
    // colour kept — the accent this page has always lit its URL in — and an
    // underline that says "clickable". Nothing else on the page links: not
    // the notes around it, not the paste field.
    use crate::ui::theme::model_selected_color;
    let app = auth_page(AUTH_URL);
    let lines = mcp_view_lines(&app, 80);
    assert_eq!(link_targets(&lines), vec![AUTH_URL.to_string()]);
    let (rows, _) = url_rows(&lines, AUTH_URL);
    for span in inked(rows) {
        assert_eq!(
            span.style.fg,
            Some(model_selected_color()),
            "{:?}",
            span.content
        );
        assert!(
            span.style.add_modifier.contains(Modifier::UNDERLINED),
            "{:?}",
            span.content
        );
    }
}

#[test]
fn the_auth_link_opens_exactly_what_c_copies_whatever_it_ends_on() {
    // The page shows one thing, the URL: the target `c` copies and the
    // browser was handed. Prose detection gives a trailing `.`, `!` or
    // unbalanced `)` back to the sentence around a URL — there is no
    // sentence here, so the link is the text verbatim, and a click can never
    // open a different URL than the copy.
    use crate::app::{Action, McpOp};
    for url in [
        "https://as.test/authorize?client_id=c1&resource=https%3A%2F%2Fmcp.test.",
        "https://as.test/authorize?client_id=c1&state=s!",
        "https://as.test/o(auth)/authorize?client_id=c1)",
    ] {
        let mut app = auth_page(url);
        let lines = mcp_view_lines(&app, 100);
        assert_eq!(link_targets(&lines), vec![url.to_string()], "{url}");
        let Action::McpOp(McpOp::CopyAuthUrl { url: copied }) = app.on_key(key(KeyCode::Char('c')))
        else {
            panic!("c copies the authorize URL");
        };
        assert_eq!(copied, url, "the link and the copy are one URL");
    }
}

#[test]
fn a_rejected_pastes_reason_links_a_url_it_names() {
    // A reason can name a page to visit (an authorization server's
    // `error_uri`), and the reason is what the user acts on next — the
    // `/login` failures' rule: the URL inside the prose linked, the error's
    // red kept, the prose around it plain.
    use crate::ui::theme::error_color;
    let help = "https://as.test/errors/access_denied?topic=consent";
    let mut app = auth_page(AUTH_URL);
    app.reject_mcp_auth_paste(
        "linear",
        &format!("the authorization server refused it (see {help})"),
    );
    for width in [40u16, 100] {
        let lines = mcp_view_lines(&app, width);
        assert_eq!(
            link_targets(&lines),
            vec![AUTH_URL.to_string(), help.to_string()],
            "the reason's URL is a link, its `)` given back (width {width})"
        );
        let reason: Vec<&Span<'static>> = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.style.fg == Some(error_color()) && !s.content.trim().is_empty())
            .collect();
        assert!(!reason.is_empty(), "the reason is painted in red");
        for span in &reason {
            let target = crate::links::style_link(&span.style);
            if help.contains(span.content.trim()) && span.content.trim().len() > 1 {
                assert_eq!(target.as_deref(), Some(help), "{:?}", span.content);
                assert!(span.style.add_modifier.contains(Modifier::UNDERLINED));
            } else {
                assert!(target.is_none(), "prose is no link: {:?}", span.content);
            }
        }
    }
}

#[test]
fn a_failed_servers_reason_links_the_url_it_names() {
    // Where a server that could not connect says what to do — and a page
    // to do it on is a link, the `/login` failures' rule again. The prose
    // stays plain, the reason's dim colour is kept, and the sentence's own
    // comma is given back.
    let console = "https://console.linear.test/settings/api?enable=mcp";
    let mut app = App::new();
    app.open_mcp_menu(vec![snapshot(
        "linear",
        McpScope::User,
        McpServerStatus::Failed(format!(
            "HTTP 403: enable the API at {console}, then reconnect"
        )),
    )]);
    app.on_key(key(KeyCode::Enter)); // the server's detail
    for width in [40u16, 100] {
        let lines = mcp_view_lines(&app, width);
        assert!(
            link_targets(&lines).contains(&console.to_string()),
            "the reason's URL is a link (width {width}): {:?}",
            link_targets(&lines)
        );
        let error_at = lines
            .iter()
            .position(|l| plain(l).trim_start().starts_with("Error:"))
            .expect("the Error: row");
        for span in inked(&lines[error_at..]) {
            if span.content.contains("Error:") || span.content.contains("reconnect") {
                assert!(
                    crate::links::style_link(&span.style).is_none(),
                    "prose is no link: {:?}",
                    span.content
                );
            }
            if crate::links::style_link(&span.style).as_deref() == Some(console) {
                assert_eq!(span.style.fg, Some(model_meta_color()), "still dim");
                assert!(span.style.add_modifier.contains(Modifier::UNDERLINED));
            }
        }
    }
}

#[test]
fn the_remote_url_row_opens_the_whole_address_from_every_row_and_reads_as_before() {
    // The server's address wraps under its label — it is one of the two
    // values the page is opened to read whole — and a wrapped URL is exactly
    // where a terminal's own detection opens a truncated target. Linked
    // verbatim, its rows read exactly as they did.
    let url = "https://a-very-long-subdomain-for-the-docs-server.test/mcp";
    let mut app = App::new();
    app.open_mcp_menu(vec![snapshot(
        "a-very-long-subdomain-for-the-docs-server",
        McpScope::User,
        McpServerStatus::Connected,
    )]);
    app.on_key(key(KeyCode::Enter)); // the server's detail
    let width = 40u16;
    let lines = mcp_view_lines(&app, width);
    let start = lines
        .iter()
        .position(|l| plain(l).trim_start().starts_with("URL:"))
        .expect("the URL: row");
    let indent = crate::ui::theme::MODEL_INDENT;
    let room = usize::from(width) - crate::ui::wrap::cols(indent) - MCP_FIELD_COL;
    let expected = crate::ui::wrap::wrap_text(url, room as u16);
    assert!(expected.len() >= 2, "the address must wrap: {expected:?}");
    for (i, text) in expected.iter().enumerate() {
        let line = &lines[start + i];
        let lead = if i == 0 {
            format!("{indent}{:<MCP_FIELD_COL$}", "URL:")
        } else {
            " ".repeat(crate::ui::wrap::cols(indent) + MCP_FIELD_COL)
        };
        assert_eq!(
            plain(line),
            format!("{lead}{text}"),
            "row {i} reads as before"
        );
        let value: Vec<&Span<'static>> = line
            .spans
            .iter()
            .filter(|s| !s.content.trim().is_empty() && s.content.trim() != "URL:")
            .collect();
        assert!(!value.is_empty(), "row {i} paints part of the address");
        for span in value {
            assert_eq!(
                crate::links::style_link(&span.style).as_deref(),
                Some(url),
                "row {i}'s fragment opens the whole address: {:?}",
                span.content
            );
            assert_eq!(span.style.fg, Some(mcp_detail_value_color()), "still quiet");
            assert!(span.style.add_modifier.contains(Modifier::UNDERLINED));
        }
    }
    let label = lines[start]
        .spans
        .iter()
        .find(|s| s.content.contains("URL:"))
        .expect("the label");
    assert!(
        crate::links::style_link(&label.style).is_none(),
        "the label is no link"
    );
}

#[test]
fn a_stdio_servers_command_row_is_no_link() {
    // A launch command is not an address — the page links only what opens.
    let mut server = snapshot("local", McpScope::User, McpServerStatus::Connected);
    server.config = McpServerConfig::Stdio {
        command: "npx".to_string(),
        args: vec!["-y".to_string(), "some-mcp-server".to_string()],
        env: Default::default(),
    };
    let mut app = App::new();
    app.open_mcp_menu(vec![server]);
    app.on_key(key(KeyCode::Enter)); // the server's detail
    let lines = mcp_view_lines(&app, 100);
    let command = lines
        .iter()
        .map(plain)
        .find(|row| row.contains("Command:"))
        .expect("the Command: row");
    assert!(command.contains("npx -y some-mcp-server"), "{command:?}");
    assert!(
        link_targets(&lines).is_empty(),
        "{:?}",
        link_targets(&lines)
    );
}
