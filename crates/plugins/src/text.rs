//! Text from a catalog, a manifest or `git` made safe to print
//! (`docs/plugins.md` *Nothing runs unseen*): a marketplace is somebody
//! else's, and a description carrying an escape sequence would reach the
//! terminal through the CLI and the `/plugins` page alike — OSC 52 writes
//! the clipboard, OSC 8 dresses a link as something else, a CSI moves the
//! cursor over what was printed. So every display string is cleaned where
//! it is decoded, and no consumer has to remember to.

use std::borrow::Cow;

/// Is `c` one this client never prints from somebody else's text: a C0
/// control, DEL, a C1 control, or a Unicode bidi embedding, override or
/// isolate (which reorder what follows them on screen).
fn is_unprintable(c: char) -> bool {
    c.is_ascii_control()
        || ('\u{80}'..='\u{9f}').contains(&c)
        || ('\u{202a}'..='\u{202e}').contains(&c)
        || ('\u{2066}'..='\u{2069}').contains(&c)
}

/// `text` with every control character gone: a line break or a tab becomes
/// a space (a description wrapped in its JSON reads as one line), anything
/// else unprintable is dropped. Borrowed when there was nothing to clean.
#[must_use]
pub fn clean_display(text: &str) -> Cow<'_, str> {
    if !text.chars().any(is_unprintable) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.chars()
            .filter_map(|c| match c {
                '\n' | '\r' | '\t' => Some(' '),
                c if is_unprintable(c) => None,
                c => Some(c),
            })
            .collect(),
    )
}

/// [`clean_display`] for a description, which keeps its line breaks: the
/// CLI indents every line of one and the page wraps it, so a paragraph
/// break is layout, not an escape. A `\r` is dropped (it would return the
/// cursor over what the line printed), a tab is a space.
#[must_use]
pub fn clean_paragraphs(text: &str) -> Cow<'_, str> {
    if !text.chars().any(|c| c != '\n' && is_unprintable(c)) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.chars()
            .filter_map(|c| match c {
                '\n' => Some('\n'),
                '\t' => Some(' '),
                c if is_unprintable(c) => None,
                c => Some(c),
            })
            .collect(),
    )
}

/// [`clean_paragraphs`] over an optional field; a field left empty is
/// dropped.
#[must_use]
pub fn cleaned_paragraphs(text: Option<String>) -> Option<String> {
    text.map(|text| match clean_paragraphs(&text) {
        Cow::Borrowed(_) => text,
        Cow::Owned(clean) => clean,
    })
    .filter(|t| !t.trim().is_empty())
}

/// [`clean_display`] over an owned string, in place of it.
#[must_use]
pub fn cleaned(text: String) -> String {
    match clean_display(&text) {
        Cow::Borrowed(_) => text,
        Cow::Owned(clean) => clean,
    }
}

/// [`cleaned`] over an optional field; a field left empty is dropped.
#[must_use]
pub fn cleaned_opt(text: Option<String>) -> Option<String> {
    text.map(cleaned).filter(|t| !t.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_sequences_and_controls_are_dropped_and_breaks_become_spaces() {
        assert_eq!(
            clean_display("a\u{1b}]52;c;AAAA\u{7}b"),
            "a]52;c;AAAAb",
            "an OSC 52 loses its ESC and BEL"
        );
        assert_eq!(clean_display("x\u{1b}[2Jy\u{7f}"), "x[2Jy");
        assert_eq!(clean_display("one\ntwo\tthree\r"), "one two three ");
        assert_eq!(clean_display("c1\u{9b}31m\u{85}"), "c131m");
        assert_eq!(
            clean_display("\u{202e}evil\u{202c} \u{2066}x\u{2069}"),
            "evil x"
        );
        assert!(matches!(
            clean_display("plain — ünïcode ✓"),
            Cow::Borrowed(_)
        ));
        assert_eq!(cleaned("ok".into()), "ok");
        assert_eq!(cleaned_opt(Some("\u{7}".into())), None);
        assert_eq!(cleaned_opt(Some("a\u{7}".into())).as_deref(), Some("a"));
        assert_eq!(
            clean_paragraphs("One.\r\n\nTwo\t\u{1b}[1m."),
            "One.\n\nTwo [1m."
        );
        assert!(matches!(clean_paragraphs("a\nb"), Cow::Borrowed(_)));
    }
}
