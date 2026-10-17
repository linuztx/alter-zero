//! Replay a file of content rows through one `Highlighter`, timing each line
//! so the slow one (and its predecessor state) is visible. `\x00GAP` resets.
//!
//! usage: [RETRY_LIMIT=N] cargo run --release --example oneprobe -- <rows.txt> [lang]

use std::io::Read;
use std::time::{Duration, Instant};

use alter_zero::highlight::{CodeTheme, Highlighter};

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: oneprobe <rows.txt> [lang]");
    let lang = std::env::args().nth(2).unwrap_or_else(|| "svelte".into());

    let mut text = String::new();
    std::fs::File::open(&path)
        .expect("open rows")
        .read_to_string(&mut text)
        .unwrap();

    let mut hl = Highlighter::new(Some(&lang), CodeTheme::default());
    let mut worst = Duration::ZERO;
    let mut worst_line = String::new();
    for (i, row) in text.split('\n').enumerate() {
        if row == "\u{0}GAP" {
            hl = Highlighter::new(Some(&lang), CodeTheme::default());
            continue;
        }
        eprintln!(
            "LINE {i} bytes={} :: {}",
            row.len(),
            &row.chars().take(60).collect::<String>()
        );
        let _ = std::io::Write::flush(&mut std::io::stderr());
        let t = Instant::now();
        let _ = hl.line(row);
        let dt = t.elapsed();
        if dt > Duration::from_millis(20) {
            eprintln!("SLOW i={i} {dt:?} :: {row:?}");
        }
        if dt > worst {
            worst = dt;
            worst_line = row.to_string();
        }
    }
    eprintln!("worst = {worst:?} on {worst_line:?}");
}
