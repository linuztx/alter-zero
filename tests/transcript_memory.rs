//! The warmed Ctrl+O transcript must retain compact rows even after many
//! code-generation turns. Run alone: the resident-set reading is process-wide.

use alter_zero::app::App;
use alter_zero::llm::tools::{DIFF_MAX_LINES, write_report};
use alter_zero::ui::TranscriptCache;

fn rss_bytes() -> Option<usize> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let field = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?;
    Some(field.split_whitespace().next()?.parse::<usize>().ok()? * 1024)
}

fn source(lines: usize) -> String {
    (0..lines)
        .map(|i| {
            format!("    return {{ id: {i}, title: 'Task {i}', done: false, priority: 2 }};\n")
        })
        .collect()
}

fn record_write(app: &mut App, file: usize, content: &str) {
    let path = format!("tasks-{file}.ts");
    app.start_tool("Write", &path, None);
    app.end_tool(&write_report(&path, content), true);
}

#[test]
fn warmed_codegen_transcript_retains_compact_rows_across_turns() {
    let Some(_) = rss_bytes() else {
        eprintln!("no /proc — skipping the resident-memory probe");
        return;
    };
    let mut app = App::new();
    let mut cache = TranscriptCache::new();
    // Load the grammar/theme before measuring transcript growth.
    record_write(&mut app, 0, &source(10));
    cache.warm(&app, 120);

    let content = source(800);
    let before = rss_bytes().expect("rss");
    for file in 1..=12 {
        record_write(&mut app, file, &content);
        cache.warm(&app, 120);
    }
    let after = rss_bytes().expect("rss");
    let growth = after.saturating_sub(before);
    // Includes the app's numbered outputs, compact spans/text, and allocator
    // slack from rendering one item. A full Line/Span/String per syntax token
    // exceeded this by several times; no output may be discarded to fit.
    let budget = 7 * 12 * content.len();
    eprintln!("transcript: {growth} resident bytes; budget {budget}");
    assert!(
        growth < budget,
        "warming 12 code-generation results grew RSS by {growth} bytes \
         (budget {budget}) — expanded transcript spans are retained"
    );
    // Write reports already cap their numbered body at DIFF_MAX_LINES. Every
    // retained report row must survive compaction, including its cap note.
    assert!(cache.line_count(&app, 120) > 12 * DIFF_MAX_LINES);
    let lines = cache.lines(&app, 120);
    let text: String = lines.iter().map(ToString::to_string).collect();
    for file in 1..=12 {
        assert!(text.contains(&format!("Write(tasks-{file}.ts)")));
    }
    assert_eq!(text.matches("… 400 more lines").count(), 12);
}
