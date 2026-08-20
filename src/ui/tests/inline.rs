//! Inline-span link marking (`docs/links.md`): bare URLs and markdown links
//! carry their full target through the wrap, so a hard-broken URL's every
//! fragment still opens whole.

use super::*;
use crate::links::style_link;
use crate::markdown::parse_inline;
use crate::ui::inline::{inline_spans, wrap_inline};
use crate::ui::theme::LINK_URL_COLOR;

/// The `(text, target)` pairs of the segments that carry a link.
fn linked_segments(segments: &[(String, Style)]) -> Vec<(String, String)> {
    segments
        .iter()
        .filter_map(|(t, s)| style_link(s).map(|url| (t.clone(), url.to_string())))
        .collect()
}

#[test]
fn a_bare_url_is_marked_with_its_full_target() {
    let segs = inline_spans(
        &parse_inline("the credit lives at https://github.com/linuztx. That is"),
        Style::default(),
    );
    assert_eq!(
        linked_segments(&segs),
        vec![(
            "https://github.com/linuztx".to_string(),
            "https://github.com/linuztx".to_string()
        )],
        "exactly the URL text carries the link: {segs:?}"
    );
    // The URL takes the link dress (a URL is a URL); the prose around it is
    // untouched, and the visible text is byte-identical to before.
    let url_style = segs
        .iter()
        .find(|(t, _)| t == "https://github.com/linuztx")
        .map(|(_, s)| *s)
        .expect("the URL segment");
    assert_eq!(url_style.fg, Some(LINK_URL_COLOR));
    assert!(url_style.add_modifier.contains(Modifier::UNDERLINED));
    let joined: String = segs.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(
        joined,
        "the credit lives at https://github.com/linuztx. That is"
    );
}

#[test]
fn a_markdown_link_marks_its_text_and_shown_url() {
    let segs = inline_spans(
        &parse_inline("see [docs](https://x.com/d) ok"),
        Style::default(),
    );
    let linked = linked_segments(&segs);
    assert!(
        linked.contains(&("docs".to_string(), "https://x.com/d".to_string())),
        "the link text is clickable: {linked:?}"
    );
    assert!(
        linked.contains(&("https://x.com/d".to_string(), "https://x.com/d".to_string())),
        "the shown URL is clickable: {linked:?}"
    );
    // The visible rendering is unchanged: text, then ` (url)`.
    let joined: String = segs.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(joined, "see docs (https://x.com/d) ok");
    // The decoration parens stay prose — only the URL inside them links.
    for (text, style) in &segs {
        if text == " (" || text == ")" {
            assert_eq!(style_link(style), None, "parens are not a link: {text:?}");
        }
    }
}

#[test]
fn nested_emphasis_inside_a_link_text_keeps_the_target() {
    let segs = inline_spans(
        &parse_inline("[**bold** docs](https://x.com/n)"),
        Style::default(),
    );
    let bold = segs
        .iter()
        .find(|(t, _)| t == "bold")
        .expect("the bold text segment");
    assert!(bold.1.add_modifier.contains(Modifier::BOLD), "style kept");
    assert_eq!(
        style_link(&bold.1).as_deref(),
        Some("https://x.com/n"),
        "the nested text still carries the target"
    );
}

#[test]
fn code_spans_stay_unlinked() {
    let segs = inline_spans(
        &parse_inline("run `https://x.com/verbatim` now"),
        Style::default(),
    );
    assert_eq!(
        linked_segments(&segs),
        Vec::<(String, String)>::new(),
        "a code span is verbatim by intent: {segs:?}"
    );
}

#[test]
fn a_bare_url_inside_emphasis_is_still_marked() {
    let segs = inline_spans(
        &parse_inline("**see https://x.com/e now**"),
        Style::default(),
    );
    let linked = linked_segments(&segs);
    assert_eq!(
        linked,
        vec![("https://x.com/e".to_string(), "https://x.com/e".to_string())],
        "autolinking recurses through emphasis: {segs:?}"
    );
}

#[test]
fn wrap_keeps_the_target_on_every_fragment_of_a_hard_broken_url() {
    // The reported bug: a URL wider than the row hard-breaks, and the
    // terminal's own per-row detection then opened only the first fragment.
    // Every wrapped fragment must carry the FULL target.
    let url = "https://github.com/linuztx";
    let segs = inline_spans(
        &parse_inline("created by linuztx at https://github.com/linuztx today"),
        Style::default(),
    );
    let rows = wrap_inline(&segs, 12); // far narrower than the URL
    let fragments: Vec<(String, String)> = rows
        .iter()
        .flat_map(|row| row.iter())
        .filter_map(|span| {
            style_link(&span.style).map(|u| (span.content.to_string(), u.to_string()))
        })
        .collect();
    assert!(
        fragments.len() >= 2,
        "the URL hard-breaks across rows at width 12: {rows:?}"
    );
    for (_, target) in &fragments {
        assert_eq!(target, url, "every fragment opens the whole URL");
    }
    let rejoined: String = fragments.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(rejoined, url, "the fragments are exactly the URL's text");
}
