//! Coding-task request snapshots must share the generated source arguments.
//! Isolated because the resident-memory reading describes the whole process.

use alter_zero::llm::{ChatMessage, ToolCallSpec};

fn rss_bytes() -> Option<usize> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let value = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?;
    let kb: usize = value.split_whitespace().next()?.parse().ok()?;
    Some(kb * 1024)
}

#[test]
fn request_snapshots_share_large_write_arguments() {
    if rss_bytes().is_none() {
        eprintln!("no /proc — skipping the resident-memory probe");
        return;
    }
    // Assemble the source-sized allocation once before the reading; spare
    // buffers here could otherwise subsidize the very copies being measured.
    let mut arguments = String::with_capacity(6 * 1024 * 1024 + 128);
    arguments.push_str(r#"{"path":"tasks.tsx","content":""#);
    while arguments.len() < 6 * 1024 * 1024 {
        arguments.push_str("const task = { title: 'Plan project', done: false };\\n");
    }
    arguments.push_str("\"}");
    let messages = vec![ChatMessage::assistant_tool_calls(
        "",
        vec![ToolCallSpec::function("write_tasks", "write", arguments)],
    )];
    let before = rss_bytes().expect("RSS");
    // The active conversation is copied for the retained wire prefix, each
    // request round, and explicit prompt-caching markers. All remain alive.
    let snapshots: Vec<_> = (0..4).map(|_| messages.clone()).collect();
    let growth = rss_bytes().expect("RSS").saturating_sub(before);
    for snapshot in &snapshots {
        assert_eq!(snapshot, &messages, "the verbatim arguments stay intact");
    }
    eprintln!("four source-sized request snapshots: {growth} bytes resident growth");
    assert!(
        growth < 2 * 1024 * 1024,
        "four snapshots grew RSS by {growth} bytes — generated source is \
         being copied for each request instead of shared"
    );
}
