//! Inline-span link marking (`docs/links.md`): bare URLs and markdown links
//! carry their full target through the wrap, so a hard-broken URL's every
//! fragment still opens whole.

use super::*;
use crate::links::style_link;
use crate::markdown::parse_inline;
use crate::ui::inline::{inline_spans, wrap_inline};
use crate::ui::theme::link_url_color;

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
    assert_eq!(url_style.fg, Some(link_url_color()));
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
fn code_spans_link_urls_without_changing_their_text_or_style() {
    let base = Style::new().add_modifier(Modifier::BOLD);
    let code =
        "curl  \"https://one.example.test/?token=0123456789abcdef\" https://two.example.test/path";
    let segs = inline_spans(&parse_inline(&format!("`{code}`")), base);
    assert_eq!(
        linked_segments(&segs),
        vec![
            (
                "https://one.example.test/?token=0123456789abcdef".into(),
                "https://one.example.test/?token=0123456789abcdef".into(),
            ),
            (
                "https://two.example.test/path".into(),
                "https://two.example.test/path".into(),
            ),
        ],
        "code URLs need explicit targets too: {segs:?}"
    );
    let joined: String = segs.iter().map(|(text, _)| text.as_str()).collect();
    assert_eq!(joined, code, "link marking must preserve code verbatim");
    for (text, mut style) in segs {
        style.underline_color = None; // Strip only the invisible carrier.
        assert_eq!(
            style,
            base.fg(crate::ui::theme::inline_code_color()),
            "code keeps its dress, including around a URL: {text:?}"
        );
    }
}

#[test]
fn an_inline_code_url_preserves_its_literal_trailing_punctuation() {
    for suffix in ["!", "?", ".", ",", ";", ":", "'", ")", "]", "}"] {
        let url = format!("https://example.test/?token=abc{suffix}");
        let segs = inline_spans(&parse_inline(&format!("`{url}`")), Style::default());
        assert_eq!(linked_segments(&segs), vec![(url.clone(), url)]);
    }
    let prose = inline_spans(
        &parse_inline("see https://example.test/?token=abc!"),
        Style::default(),
    );
    assert_eq!(
        linked_segments(&prose),
        vec![(
            "https://example.test/?token=abc".into(),
            "https://example.test/?token=abc".into()
        )],
        "prose punctuation trimming remains unchanged"
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

#[test]
fn a_markdown_targets_decoration_parens_stay_prose() {
    // The reported bug (`docs/links.md` *The decoration parens*): the ` (`
    // and `)` framing the shown target wore the link's own colour and
    // underline, so the eye read the brackets as part of the link — while a
    // click, which only the *marked* run carries, opened the URL without
    // them. They are decoration the renderer adds, not link text: prose
    // dress, no carrier. A bare URL in prose parens — `(https://x.com/y)` —
    // already read that way, so this is also what makes the two agree.
    let base = Style::new().fg(crate::ui::theme::ai_color());
    let segs = inline_spans(&parse_inline("see [docs](https://x.com/d) ok"), base);
    let joined: String = segs.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(joined, "see docs (https://x.com/d) ok", "text unchanged");
    for (text, style) in &segs {
        if text.contains(['(', ')']) {
            assert_eq!(*style, base, "the parens keep the prose dress: {text:?}");
            assert_eq!(style_link(style), None, "and carry no target: {text:?}");
        }
    }
    // The target between them still wears the link dress and the carrier.
    let (_, url_style) = segs
        .iter()
        .find(|(t, _)| t == "https://x.com/d")
        .expect("the shown URL segment");
    assert_eq!(url_style.fg, Some(link_url_color()));
    assert!(url_style.add_modifier.contains(Modifier::UNDERLINED));
    assert_eq!(style_link(url_style).as_deref(), Some("https://x.com/d"));
    // And the wrap can't smuggle the dress back in at any width: a row's
    // spans are re-coalesced per style, so a paren that kept the link
    // colour would come back as its own underlined span.
    for width in [5, 12, 80] {
        for span in wrap_inline(&segs, width).iter().flatten() {
            if span.content.contains(['(', ')']) {
                assert_eq!(span.style, base, "wrapped at {width}: {span:?}");
                assert_eq!(style_link(&span.style), None, "wrapped at {width}");
            }
        }
    }
}

#[test]
fn literal_url_marking_handles_every_style_boundary_and_multiple_urls() {
    let first = Style::new().add_modifier(Modifier::BOLD);
    let second = Style::new().add_modifier(Modifier::ITALIC);
    let text =
        "λ ['https://one.example.test/a_(b)!','https://two.example.test/日本?q=one&two=2?'] end";
    let urls = [
        "https://one.example.test/a_(b)!",
        "https://two.example.test/日本?q=one&two=2?",
    ];
    // Include splits at both ends, at the scheme/body boundary, and within
    // each target. Empty highlighter spans must not create phantom links.
    for split in (0..=text.len()).filter(|&i| text.is_char_boundary(i)) {
        let segments = vec![
            (text[..split].to_string(), first),
            (String::new(), second),
            (text[split..].to_string(), second),
        ];
        let actual = crate::ui::inline::linkify_code_segments(segments);
        assert_eq!(
            actual.iter().map(|(s, _)| s.as_str()).collect::<String>(),
            text,
            "literal bytes survive at {split}"
        );
        let marked = linked_segments(&actual);
        assert!(
            marked
                .iter()
                .all(|(s, u)| !s.is_empty() && urls.contains(&u.as_str()))
        );
        for url in urls {
            let linked: String = marked
                .iter()
                .filter(|(_, target)| target == url)
                .map(|(s, _)| s.as_str())
                .collect();
            assert_eq!(linked, url, "full target survives at {split}");
        }
    }
}

#[test]
fn literal_url_marking_leaves_non_url_segments_unchanged() {
    let segments = vec![
        ("let x = ".into(), Style::new().add_modifier(Modifier::BOLD)),
        ("42;  // plain code".into(), Style::default()),
        (String::new(), Style::default()),
    ];
    assert_eq!(
        crate::ui::inline::linkify_segments(segments.clone()),
        segments
    );
}

#[test]
fn literal_url_marking_crosses_syntax_styles_and_preserves_unicode_and_spacing() {
    let first = Style::new().add_modifier(Modifier::BOLD);
    let second = Style::new().add_modifier(Modifier::ITALIC);
    let segments = vec![
        ("λ  \"htt".to_string(), first),
        ("ps://example.test/日本?q=one".to_string(), second),
        ("&two=2\"  end".to_string(), first),
    ];
    let actual = crate::ui::inline::linkify_segments(segments.clone());
    assert_eq!(
        linked_segments(&actual)
            .iter()
            .map(|(s, _)| s.as_str())
            .collect::<String>(),
        "https://example.test/日本?q=one&two=2"
    );
    for (_, target) in linked_segments(&actual) {
        assert_eq!(target, "https://example.test/日本?q=one&two=2");
    }
    let characters = |segments: Vec<(String, Style)>| {
        segments
            .into_iter()
            .flat_map(|(text, mut style)| {
                style.underline_color = None;
                text.chars().map(|c| (c, style)).collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(characters(actual), characters(segments));
}
