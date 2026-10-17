//! Hang probe: replay every file-cell in a rollout through the real
//! `highlight::Highlighter`, line by line (exactly as `file_cell_lines` does).
//! Prints each (event, line, lang, bytes) to stderr *before* highlighting it,
//! so an external `timeout` turns the last printed line into the culprit.
//!
//! usage: cargo run --release --example hangprobe -- <rollout.jsonl>

use std::io::Write;

use alter_zero::highlight::{CodeTheme, Highlighter};

/// Replica of `ui::file_cell::parse_file_row`'s content extraction.
fn row_text(line: &str, signed: bool) -> Option<String> {
    let trimmed = line.trim_start_matches(' ');
    if trimmed.starts_with('…') || trimmed == "⋮" {
        return None;
    }
    let indent = line.len() - trimmed.len();
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let rest = &line[indent + digits..];
    if rest.is_empty() {
        return Some(String::new());
    }
    let rest = rest.strip_prefix(' ')?;
    if signed {
        let sign = rest.chars().next().unwrap_or(' ');
        if !matches!(sign, '+' | '-' | ' ') {
            return None;
        }
        Some(rest.get(1..).unwrap_or("").to_string())
    } else {
        Some(rest.to_string())
    }
}

fn lang_of(args: &str) -> Option<String> {
    let name = args.rsplit(['/', '\\']).next().unwrap_or(args);
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty() && !ext.contains(' ')).then(|| ext.to_string())
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: hangprobe <rollout.jsonl>");
    let file = std::fs::File::open(&path).expect("open rollout");
    let reader = std::io::BufReader::new(file);

    let mut ev_idx = 0usize;
    let mut checked = 0usize;
    for line in std::io::BufRead::lines(reader) {
        let line = line.expect("read line");
        let v: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("tool") {
            continue;
        }
        ev_idx += 1;
        let payload = &v["payload"];
        let name = payload.get("name").and_then(|n| n.as_str()).unwrap_or("");
        if !matches!(
            name,
            "Read" | "Write" | "Edit" | "Update" | "Create" | "New"
        ) {
            continue;
        }
        let args = payload.get("args").and_then(|a| a.as_str()).unwrap_or("");
        let output = payload.get("output").and_then(|o| o.as_str()).unwrap_or("");
        let lang = lang_of(args);

        // `signed` follows the head line (Updated -> diff, Wrote/Created -> plain),
        // exactly as `parse_file_cell` decides it.
        let mut all = output.split('\n');
        let head = all.next().unwrap_or("");
        let signed = if head.starts_with("Updated ") {
            true
        } else if head.starts_with("Wrote ") || head.starts_with("Created ") {
            false
        } else {
            continue;
        };

        let mut hl = Highlighter::new(lang.as_deref(), CodeTheme::default());
        for (ln_idx, raw) in all.enumerate() {
            // A `⋮` hunk gap re-synchronizes the lexer: the state resets too.
            if raw.trim_start_matches(' ').trim_end() == "⋮" {
                hl = Highlighter::new(lang.as_deref(), CodeTheme::default());
                continue;
            }
            let Some(text) = row_text(raw, signed) else {
                continue;
            };
            checked += 1;
            // Progress marker, flushed, so the last one before a hang names it.
            let preview: String = text.chars().take(120).collect();
            eprintln!(
                "TRY ev={ev_idx} name={name} lang={lang:?} ln={ln_idx} bytes={} :: {preview:?}",
                text.len(),
            );
            let _ = std::io::stderr().flush();
            let _ = hl.line(&text);
        }
    }
    let _ = writeln!(
        std::io::stderr(),
        "no hang found; checked {checked} content lines across {ev_idx} file tools"
    );
}
