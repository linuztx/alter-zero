//! Inline-span link marking (`docs/links.md`): bare URLs and markdown links
//! carry their full target through the wrap, so a hard-broken URL's every
//! fragment still opens whole.

use super::*;
use crate::links::{TargetDisplay, style_link, with_target_display};
use crate::markdown::parse_inline;
use crate::ui::inline::{hides_target, inline_spans, wrap_inline};
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
        crate::ui::inline::linkify_code_segments(segments.clone()),
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
    let actual = crate::ui::inline::linkify_code_segments(segments.clone());
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

// --- Labelled links: the label alone, its target behind it (docs/links.md) ---

/// `source`'s segments as a terminal known to open hyperlinks is shown them.
fn label_only(source: &str, base: Style) -> Vec<(String, Style)> {
    with_target_display(TargetDisplay::Hidden, || {
        inline_spans(&parse_inline(source), base)
    })
}

/// The visible text of `segments`.
fn text_of(segments: &[(String, Style)]) -> String {
    segments.iter().map(|(text, _)| text.as_str()).collect()
}

/// Whether `style` is the link dress: the link colour, underlined.
fn wears_link_dress(style: &Style) -> bool {
    style.fg == Some(link_url_color()) && style.add_modifier.contains(Modifier::UNDERLINED)
}

/// The text of each linked span on `row`, in order.
fn linked_runs(row: &[Span<'static>]) -> Vec<String> {
    row.iter()
        .filter(|span| style_link(&span.style).is_some())
        .map(|span| span.content.to_string())
        .collect()
}

#[test]
fn a_web_link_is_shown_as_its_label_with_the_target_behind_it() {
    // The report: `Source: FIDE (https://www.fide.com/geneva-to-host-…/)`
    // spent a row and a half on an address nobody reads. Where the terminal
    // opens hyperlinks the label is the link — dressed as one, the whole
    // address behind it — and the address is off the screen.
    let base = Style::new().fg(crate::ui::theme::ai_color());
    let url = "https://www.fide.com/geneva-to-host-fide-world-championship-match-2026/";
    let segs = label_only(&format!("Source: [FIDE]({url}) today"), base);
    assert_eq!(text_of(&segs), "Source: FIDE today", "{segs:?}");
    assert_eq!(
        linked_segments(&segs),
        vec![("FIDE".to_string(), url.to_string())],
        "the label carries the whole target, and nothing else does"
    );
    for (text, style) in &segs {
        if text == "FIDE" {
            assert!(wears_link_dress(style), "the label reads as a link");
        } else {
            assert_eq!(*style, base, "the prose around it is untouched: {text:?}");
        }
    }
}

#[test]
fn a_hidden_targets_label_keeps_its_emphasis_under_the_link_dress() {
    let url = "https://x.com/n";
    let segs = label_only(
        &format!("[**bold** *slanted* `code` plain]({url})"),
        Style::default(),
    );
    assert_eq!(text_of(&segs), "bold slanted code plain");
    for (text, style) in &segs {
        assert!(wears_link_dress(style), "every run is the link: {text:?}");
        assert_eq!(style_link(style).as_deref(), Some(url), "{text:?}");
    }
    let style_of = |needle: &str| {
        segs.iter()
            .find(|(text, _)| text == needle)
            .map(|(_, style)| *style)
            .unwrap_or_else(|| panic!("no {needle:?} run in {segs:?}"))
    };
    assert!(style_of("bold").add_modifier.contains(Modifier::BOLD));
    assert!(style_of("slanted").add_modifier.contains(Modifier::ITALIC));
    // A code span in a label gives up its own colour: one link, one colour.
    assert_eq!(style_of("code").fg, Some(link_url_color()));
}

#[test]
fn a_label_of_several_words_is_one_unbroken_link_on_its_row() {
    // The wrap rebuilds a row from its words and puts single spaces back
    // between them. A space between two words of one label is the link too
    // — underlined with them, opening what they open — or the label reads
    // as three separate links with holes in the line under it.
    let url = "https://example.com/docs";
    let segs = label_only(
        &format!("please read [the official documentation]({url}) first"),
        Style::default(),
    );
    let rows = wrap_inline(&segs, 80);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let runs: Vec<&Span> = rows[0]
        .iter()
        .filter(|span| style_link(&span.style).is_some())
        .collect();
    assert_eq!(runs.len(), 1, "one run, its spaces included: {:?}", rows[0]);
    assert_eq!(runs[0].content, "the official documentation");
    assert!(wears_link_dress(&runs[0].style));
    assert_eq!(style_link(&runs[0].style).as_deref(), Some(url));
    let text: String = rows[0].iter().map(|span| span.content.as_ref()).collect();
    assert_eq!(text, "please read the official documentation first");
    // Broken between its words, each row holds one run of the label and no
    // run opens or closes on a space.
    for width in [14, 20, 30] {
        let mut label = Vec::new();
        for row in wrap_inline(&segs, width) {
            let runs = linked_runs(&row);
            assert!(runs.len() <= 1, "at {width}: {row:?}");
            for run in runs {
                assert_eq!(run.trim(), run, "no dangling space at {width}");
                label.push(run);
            }
        }
        assert_eq!(label.join(" "), "the official documentation", "at {width}");
    }
}

#[test]
fn a_space_at_the_edge_of_a_label_is_not_part_of_the_link() {
    // Only a space *between* two words of the label joins them: one the
    // author left inside the brackets, at either end, is the gap to the
    // prose beside it and stays prose.
    for source in [
        "see [ docs ](https://x.com/d) ok",
        "see [docs ](https://x.com/d) ok",
        "see [ docs](https://x.com/d) ok",
        "see[ docs](https://x.com/d) ok",
    ] {
        let rows = wrap_inline(&label_only(source, Style::default()), 80);
        assert_eq!(linked_runs(&rows[0]), ["docs"], "{source:?}: {rows:?}");
    }
}

#[test]
fn two_links_to_one_target_stay_two_links() {
    let segs = label_only(
        "[one](https://x.com/s) [two](https://x.com/s)",
        Style::default(),
    );
    let rows = wrap_inline(&segs, 80);
    assert_eq!(
        linked_runs(&rows[0]),
        ["one", "two"],
        "the space between them belongs to neither: {rows:?}"
    );
}

#[test]
fn a_shown_targets_label_is_one_hyperlink_across_its_words() {
    // The other presentation gains the same thing: the label's words used
    // to be a hyperlink each, the spaces between them opening nothing.
    let base = Style::new().fg(crate::ui::theme::ai_color());
    let url = "https://example.com/docs";
    let segs = inline_spans(
        &parse_inline(&format!("read [the official documentation]({url}) first")),
        base,
    );
    let rows = wrap_inline(&segs, 120);
    assert_eq!(
        linked_runs(&rows[0]),
        ["the official documentation", url],
        "{rows:?}"
    );
    let text: String = rows[0].iter().map(|span| span.content.as_ref()).collect();
    assert_eq!(
        text,
        format!("read the official documentation ({url}) first")
    );
}

#[test]
fn a_hidden_targets_every_wrapped_fragment_opens_the_whole_address() {
    let url = "https://example.com/a/long/path/that/would/never/fit?on=one&row=at&all=true";
    let segs = label_only(
        &format!("the [supercalifragilistic expialidocious write-up]({url}) is out"),
        Style::default(),
    );
    for width in [6, 11, 24] {
        let rows = wrap_inline(&segs, width);
        let text: String = rows
            .iter()
            .flatten()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(
            !text.contains("https://"),
            "no address at {width}: {text:?}"
        );
        let mut label = String::new();
        for span in rows.iter().flatten() {
            if let Some(target) = style_link(&span.style) {
                assert_eq!(target.as_ref(), url, "at {width}: {span:?}");
                label.push_str(&span.content.replace(' ', ""));
            }
        }
        assert_eq!(
            label, "supercalifragilisticexpialidociouswrite-up",
            "at {width}"
        );
    }
}

#[test]
fn a_target_that_is_not_a_web_address_stays_on_screen() {
    // Only an `http(s)://` address is hidden. A path, an anchor or another
    // scheme is the information itself — `the notes (docs/links.md)` — and
    // nothing here can promise a click on it opens anything.
    for target in [
        "AGENTS.md",
        "docs/links.md#testing",
        "/abs/path.rs",
        "#anchor",
        "mailto:me@example.com",
        "file:///etc/hosts",
        "ftp://files.example/a.zip",
        "https://",
    ] {
        let source = format!("see [the notes]({target}) ok");
        let shown = inline_spans(&parse_inline(&source), Style::default());
        assert_eq!(
            label_only(&source, Style::default()),
            shown,
            "{target}: the two presentations are one"
        );
        assert_eq!(text_of(&shown), format!("see the notes ({target}) ok"));
    }
}

#[test]
fn a_link_with_nothing_to_click_is_its_target() {
    // A hidden target needs a label to stand behind, and a shown one needs
    // a label to follow: with none — an empty label, one of spaces or
    // zero-width characters, an image with no alt text — the target itself
    // is the link, on every terminal, rather than a parenthesis around the
    // only thing on screen.
    for source in [
        "see [](https://x.com/e) ok",
        "see [ ](https://x.com/e) ok",
        "see [\u{200d}](https://x.com/e) ok",
        "see [\u{1}](https://x.com/e) ok",
        "see [![](https://img.example/b.png)](https://x.com/e) ok",
    ] {
        let shown = inline_spans(&parse_inline(source), Style::default());
        assert_eq!(label_only(source, Style::default()), shown, "{source:?}");
        assert_eq!(text_of(&shown), "see https://x.com/e ok", "{source:?}");
        assert_eq!(
            linked_segments(&shown),
            vec![("https://x.com/e".to_string(), "https://x.com/e".to_string())],
            "{source:?}"
        );
        let (_, style) = shown
            .iter()
            .find(|(text, _)| text == "https://x.com/e")
            .expect("the target segment");
        assert!(wears_link_dress(style), "{source:?}");
    }
    // Whatever the target is: a path stands in for a missing label too.
    let path = inline_spans(&parse_inline("[](docs/links.md)"), Style::default());
    assert_eq!(text_of(&path), "docs/links.md");
    assert_eq!(
        linked_segments(&path),
        vec![("docs/links.md".to_string(), "docs/links.md".to_string())]
    );
}

#[test]
fn a_label_that_shows_another_address_keeps_the_real_one_on_screen() {
    // A hidden target is taken on trust, and the label is all there is to
    // trust: one that spells a different address than the click opens is
    // the one case where the reader must be shown both.
    let target = "https://evil.example/login";
    for label in [
        "https://bank.example",
        "sign in at https://bank.example today",
        "`https://bank.example`",
        "<https://bank.example>",
        "**https://bank.example/account**",
    ] {
        let source = format!("[{label}]({target})");
        let shown = inline_spans(&parse_inline(&source), Style::default());
        let hidden = label_only(&source, Style::default());
        assert_eq!(hidden, shown, "{source:?}");
        assert!(
            text_of(&hidden).ends_with(&format!(" ({target})")),
            "{source:?}: {hidden:?}"
        );
    }
}

#[test]
fn a_label_that_is_its_own_target_is_shown_once() {
    for (source, label, target) in [
        (
            "[https://x.com/d](https://x.com/d)",
            "https://x.com/d",
            "https://x.com/d",
        ),
        // A closing slash is the same page.
        (
            "[https://x.com](https://x.com/)",
            "https://x.com",
            "https://x.com/",
        ),
        (
            "[https://x.com/](https://x.com)",
            "https://x.com/",
            "https://x.com",
        ),
        (
            "[see https://x.com/d now](https://x.com/d)",
            "see https://x.com/d now",
            "https://x.com/d",
        ),
    ] {
        let segs = label_only(source, Style::default());
        assert_eq!(text_of(&segs), label, "{source:?}");
        for (text, style) in &segs {
            assert!(wears_link_dress(style), "{source:?}: {text:?}");
            assert_eq!(style_link(style).as_deref(), Some(target), "{source:?}");
        }
    }
}

#[test]
fn a_target_that_could_not_be_marked_is_never_hidden() {
    // The interner holds eight million targets and marks nothing past that
    // (`links::carrier`). A label with its address hidden and no hyperlink
    // behind it would be a URL nobody can reach, so an unmarked link keeps
    // the address on screen.
    let label = [("docs".to_string(), Style::default())];
    let url = "https://x.com/unmarked";
    let carrier = crate::links::carrier(url);
    assert!(carrier.is_some(), "the interner has room in a test");
    with_target_display(TargetDisplay::Hidden, || {
        assert!(hides_target(&label, url, carrier));
        assert!(!hides_target(&label, url, None));
    });
    // And the other presentation hides nothing, marked or not.
    assert!(!hides_target(&label, url, carrier));
}

#[test]
fn a_label_that_is_its_own_target_is_written_once_on_every_terminal() {
    // `[https://x.com/d](https://x.com/d)` is how a model often writes a
    // link, and the other presentation printed it twice — the address, then
    // the same address in parentheses. A web address that is its own label
    // is written once wherever it is shown.
    for (source, text, target) in [
        (
            "[https://x.com/d](https://x.com/d)",
            "https://x.com/d",
            "https://x.com/d",
        ),
        (
            "[https://x.com](https://x.com/)",
            "https://x.com",
            "https://x.com/",
        ),
        (
            "[<https://x.com/d>](https://x.com/d)",
            "https://x.com/d",
            "https://x.com/d",
        ),
    ] {
        let segs = inline_spans(&parse_inline(source), Style::default());
        assert_eq!(text_of(&segs), text, "{source:?}");
        assert_eq!(
            linked_segments(&segs),
            vec![(text.to_string(), target.to_string())],
            "{source:?}"
        );
        assert!(wears_link_dress(&segs[0].1), "{source:?}");
        assert_eq!(label_only(source, Style::default()), segs, "{source:?}");
    }
    // A label that only *holds* its target is still a label, and a target
    // that is no web address is still spelled out after its name.
    for (source, text) in [
        (
            "[see https://x.com/d now](https://x.com/d)",
            "see https://x.com/d now (https://x.com/d)",
        ),
        ("[AGENTS.md](AGENTS.md)", "AGENTS.md (AGENTS.md)"),
    ] {
        let segs = inline_spans(&parse_inline(source), Style::default());
        assert_eq!(text_of(&segs), text, "{source:?}");
    }
}

#[test]
fn a_target_too_long_for_a_hyperlink_stays_on_screen() {
    // Terminals cap what a hyperlink may carry — VTE and iTerm2 drop one
    // whose address is past 2083 bytes — so a longer address has no
    // hyperlink to hide behind. The cap is on the bytes written, and a
    // non-ASCII byte is written as three.
    let of_len = |len: usize| format!("https://x.com/{}", "a".repeat(len - "https://x.com/".len()));
    let fits = of_len(crate::links::HYPERLINK_URI_MAX_BYTES);
    let over = of_len(crate::links::HYPERLINK_URI_MAX_BYTES + 1);
    let hidden = label_only(&format!("the [signed link]({fits}) here"), Style::default());
    assert_eq!(text_of(&hidden), "the signed link here");
    for url in [
        over,
        // 700 two-byte characters are 4200 bytes on the wire.
        format!("https://x.com/{}", "é".repeat(700)),
    ] {
        let source = format!("the [signed link]({url}) here");
        let shown = inline_spans(&parse_inline(&source), Style::default());
        assert_eq!(label_only(&source, Style::default()), shown);
        assert_eq!(text_of(&shown), format!("the signed link ({url}) here"));
    }
}

#[test]
fn an_address_spelled_across_a_labels_runs_still_keeps_the_real_one_on_screen() {
    // What the reader sees is the label's text as one string, whatever runs
    // it was written in. A label could spell `https://bank.example` with the
    // scheme split by emphasis or a code span, or carry it as an image's alt
    // text: no single run holds an address, so none was marked — and the
    // real target went behind a label naming somewhere else. The same for a
    // character that takes no column: a zero-width space inside `https://`
    // is invisible on screen, so it cannot hide the address from the check.
    let target = "https://evil.example/login";
    for label in [
        "ht**tps://bank.example**",
        "ht`tps://bank.example`",
        "https://*bank.example*/login",
        "https&#58;//bank.example",
        "https\\://bank.example",
        "![https://bank.example](https://img.example/b.png)",
        "h\u{200b}ttps://bank.example",
        "https:/\u{200d}/bank.example",
        // A control character is never painted, so it hides nothing either.
        "ht\u{1}tps://bank.example",
        "https:\u{7f}//bank.example",
        "https://bank\u{85}.example",
    ] {
        let source = format!("[{label}]({target})");
        let shown = inline_spans(&parse_inline(&source), Style::default());
        let hidden = label_only(&source, Style::default());
        assert_eq!(hidden, shown, "{source:?}");
        assert!(
            text_of(&hidden).ends_with(&format!(" ({target})")),
            "{source:?}: {hidden:?}"
        );
    }
    // Split the same way, a label that spells its own target still stands
    // alone — and reads as the link whichever way it was written.
    for display in [TargetDisplay::Hidden, TargetDisplay::Shown] {
        let segs = with_target_display(display, || {
            inline_spans(
                &parse_inline("[ht**tps://x.com/d**](https://x.com/d)"),
                Style::default(),
            )
        });
        assert_eq!(text_of(&segs), "https://x.com/d", "{display:?}");
        for (text, style) in &segs {
            assert!(wears_link_dress(style), "{display:?}: {text:?}");
            assert_eq!(style_link(style).as_deref(), Some("https://x.com/d"));
        }
    }
}
