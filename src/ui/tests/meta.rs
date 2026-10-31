//! The dim meta lines' highlights (`docs/meta-highlights.md`): the values a
//! reader scans for in the theme's second ink, the keys a hint names in a
//! softened accent, and the footer's mode and gauge colours.

use super::*;
use crate::app::Theme;
use crate::permission::PermissionMode;
use crate::ui::meta::{MetaSpans, hint_spans};
use crate::ui::palette::{palette_of, with_theme};
use crate::ui::theme::{
    AGENT_BACKGROUNDED, AGENT_MANAGE_HINT, CONTEXT_USAGE_FULL_SHARE, CONTEXT_USAGE_WARN_SHARE,
    EXPAND_HINT, META_TINT_DIM_MIX, STATUS_INTERRUPT_HINT, TOOL_BACKGROUND_HINT, TOOL_BACKGROUNDED,
    TOOL_STOP_WAITING_HINT, footer_color, footer_gauge_color, footer_mode_color,
    menu_selected_color, meta_key_color, meta_value_color, tool_dim_color,
};
use crate::ui::wrap::lerp_color;

fn dim() -> Style {
    Style::new().fg(tool_dim_color())
}

fn text_of(spans: &[Span]) -> String {
    spans.iter().map(|span| span.content.as_ref()).collect()
}

/// `color` softened the feature's one way: mixed toward the dim.
fn softened(color: Color) -> Color {
    lerp_color(color, tool_dim_color(), META_TINT_DIM_MIX)
}

#[test]
fn a_value_is_the_themes_second_ink_in_every_theme() {
    for theme in Theme::ALL {
        with_theme(theme, || {
            assert_eq!(
                meta_value_color(),
                palette_of(theme).text_muted,
                "{theme:?}: a value reads in the theme's quieter ink"
            );
            assert_ne!(
                meta_value_color(),
                tool_dim_color(),
                "{theme:?}: a value stands apart from the dim around it"
            );
        });
    }
}

#[test]
fn a_key_is_the_accent_softened_toward_the_dim_in_every_theme() {
    for theme in Theme::ALL {
        with_theme(theme, || {
            let (accent, dim) = (menu_selected_color(), tool_dim_color());
            let key = meta_key_color();
            assert_ne!(key, dim, "{theme:?}: a key stands apart from the dim");
            match (accent, dim) {
                (Color::Rgb(..), Color::Rgb(..)) => {
                    assert_eq!(key, lerp_color(accent, dim, META_TINT_DIM_MIX), "{theme:?}");
                    assert_ne!(key, accent, "{theme:?}: calmer than the accent itself");
                    // Every channel sits between the two ends: a softened
                    // accent, not a new colour.
                    let ((ka, kb, kc), (aa, ab, ac), (da, db, dc)) =
                        (rgb_of(key), rgb_of(accent), rgb_of(dim));
                    for (k, a, d) in [(ka, aa, da), (kb, ab, db), (kc, ac, dc)] {
                        assert!(
                            (a.min(d)..=a.max(d)).contains(&k),
                            "{theme:?}: channel {k} between {a} and {d}"
                        );
                    }
                }
                // The terminal's own colours have no components to mix: the
                // key keeps the terminal's accent (`docs/theme.md`).
                _ => assert_eq!(key, accent, "{theme:?}"),
            }
        });
    }
}

#[test]
fn the_tint_is_closer_to_the_hue_than_to_the_dim() {
    // Softened, still the hue: past the midpoint the ANSI theme would step
    // to the dim and the tint would vanish.
    const { assert!(META_TINT_DIM_MIX > 0.0 && META_TINT_DIM_MIX < 0.5) };
}

#[test]
fn the_footer_mode_is_coloured_by_how_much_runs_unasked() {
    for theme in Theme::ALL {
        with_theme(theme, || {
            let palette = palette_of(theme);
            assert_eq!(
                footer_mode_color(PermissionMode::Manual),
                footer_color(),
                "{theme:?}: everything asks — nothing to flag"
            );
            assert_eq!(
                footer_mode_color(PermissionMode::Edit),
                softened(palette.purple),
                "{theme:?}: edits run unasked"
            );
            assert_eq!(
                footer_mode_color(PermissionMode::Auto),
                meta_key_color(),
                "{theme:?}: the default, the softened accent"
            );
            assert_eq!(
                footer_mode_color(PermissionMode::Master),
                softened(palette.error),
                "{theme:?}: everything runs unasked"
            );
            let colours = [
                PermissionMode::Manual,
                PermissionMode::Edit,
                PermissionMode::Auto,
                PermissionMode::Master,
            ]
            .map(footer_mode_color);
            for (i, a) in colours.iter().enumerate() {
                for b in &colours[i + 1..] {
                    assert_ne!(a, b, "{theme:?}: every mode reads apart");
                }
            }
        });
    }
}

#[test]
fn the_footer_gauge_turns_as_the_window_fills() {
    for theme in Theme::ALL {
        with_theme(theme, || {
            let palette = palette_of(theme);
            assert_eq!(footer_gauge_color(0.0), meta_value_color(), "{theme:?}");
            assert_eq!(footer_gauge_color(0.5), meta_value_color(), "{theme:?}");
            assert_eq!(
                footer_gauge_color(CONTEXT_USAGE_WARN_SHARE - 0.001),
                meta_value_color(),
                "{theme:?}: room to spare is a plain value"
            );
            assert_eq!(
                footer_gauge_color(CONTEXT_USAGE_WARN_SHARE),
                softened(palette.warning),
                "{theme:?}: filling — the caution amber"
            );
            assert_eq!(
                footer_gauge_color(CONTEXT_USAGE_FULL_SHARE),
                softened(palette.error),
                "{theme:?}: at the auto-compact threshold — the error red"
            );
            assert_eq!(
                footer_gauge_color(1.4),
                softened(palette.error),
                "{theme:?}: past the window is still red"
            );
        });
    }
}

#[test]
fn a_hint_lifts_its_key_and_keeps_its_words_dim() {
    let spans = hint_spans(EXPAND_HINT, dim());
    assert_eq!(text_of(&spans), EXPAND_HINT, "the text is unchanged");
    assert_eq!(pieces_in(&spans, meta_key_color()), ["ctrl+o"]);
    assert_eq!(
        text_of(&spans).replace("ctrl+o", ""),
        pieces_in(&spans, tool_dim_color()).concat(),
        "every other piece stays dim"
    );
}

#[test]
fn a_hint_with_two_clauses_lifts_both_keys() {
    let spans = hint_spans(AGENT_MANAGE_HINT, dim());
    assert_eq!(text_of(&spans), AGENT_MANAGE_HINT);
    assert_eq!(pieces_in(&spans, meta_key_color()), ["↓", "ctrl+o"]);
}

#[test]
fn the_words_before_a_hints_parenthesis_are_never_a_key() {
    // `Running in the background (↓ to manage)`: only the parenthesised
    // clause is a hint.
    let spans = hint_spans(TOOL_BACKGROUNDED, dim());
    assert_eq!(text_of(&spans), TOOL_BACKGROUNDED);
    assert_eq!(pieces_in(&spans, meta_key_color()), ["↓"]);
}

#[test]
fn a_hint_without_parentheses_is_one_clause() {
    let spans = hint_spans(STATUS_INTERRUPT_HINT, dim());
    assert_eq!(text_of(&spans), STATUS_INTERRUPT_HINT);
    assert_eq!(pieces_in(&spans, meta_key_color()), ["esc"]);
}

#[test]
fn text_that_names_no_key_comes_back_whole_in_the_base() {
    for text in [
        "Running…",
        "(no output)",
        "",
        "plain words to nowhere",
        "Done (3 tool uses)",
    ] {
        let spans = hint_spans(text, dim());
        assert_eq!(text_of(&spans), text, "{text:?} unchanged");
        assert!(
            pieces_in(&spans, meta_key_color()).is_empty(),
            "{text:?} names no key"
        );
        assert!(
            spans.iter().all(|span| span.style == dim()),
            "{text:?} stays in the base"
        );
    }
}

#[test]
fn every_inline_hint_names_its_keys() {
    // The hints the inline rows print, each pinned to the grammar the
    // highlight reads: a new one parses, or this fails.
    for (hint, keys) in [
        (EXPAND_HINT, &["ctrl+o"][..]),
        (STATUS_INTERRUPT_HINT, &["esc"]),
        (TOOL_BACKGROUND_HINT, &["ctrl+b"]),
        (TOOL_STOP_WAITING_HINT, &["ctrl+b"]),
        (TOOL_BACKGROUNDED, &["↓"]),
        (AGENT_MANAGE_HINT, &["↓", "ctrl+o"]),
        (AGENT_BACKGROUNDED, &["↓", "ctrl+o"]),
    ] {
        let spans = hint_spans(hint, dim());
        assert_eq!(text_of(&spans), hint);
        assert_eq!(pieces_in(&spans, meta_key_color()), keys, "{hint:?}");
    }
}

#[test]
fn a_hint_keeps_the_bases_modifiers_on_its_key() {
    let base = dim().add_modifier(Modifier::ITALIC);
    let spans = hint_spans(EXPAND_HINT, base);
    let key = spans
        .iter()
        .find(|span| span.content == "ctrl+o")
        .expect("the key");
    assert!(key.style.add_modifier.contains(Modifier::ITALIC));
}

#[test]
fn the_builder_lifts_values_and_keeps_its_words_in_the_base() {
    let mut meta = MetaSpans::new(dim());
    meta.text("Worked for ")
        .value("45s")
        .text(" · ")
        .value("12.3k")
        .text(" tokens");
    let spans = meta.into_spans();
    assert_eq!(text_of(&spans), "Worked for 45s · 12.3k tokens");
    assert_eq!(pieces_in(&spans, meta_value_color()), ["45s", "12.3k"]);
    assert_eq!(
        pieces_in(&spans, tool_dim_color()),
        ["Worked for ", " · ", " tokens"]
    );
}

#[test]
fn the_builder_reads_a_hint_by_its_grammar() {
    let mut meta = MetaSpans::new(dim());
    meta.text("Thought for ").value("12s").hint(EXPAND_HINT);
    let spans = meta.into_spans();
    assert_eq!(text_of(&spans), format!("Thought for 12s{EXPAND_HINT}"));
    assert_eq!(pieces_in(&spans, meta_value_color()), ["12s"]);
    assert_eq!(pieces_in(&spans, meta_key_color()), ["ctrl+o"]);
}

#[test]
fn a_value_keeps_the_bases_modifiers() {
    let mut meta = MetaSpans::new(dim().add_modifier(Modifier::BOLD));
    meta.value("3");
    let spans = meta.into_spans();
    assert_eq!(spans[0].style.fg, Some(meta_value_color()));
    assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn an_empty_piece_adds_no_span() {
    let mut meta = MetaSpans::new(dim());
    meta.text("").value("").hint("").text("x");
    let spans = meta.into_spans();
    assert_eq!(spans.len(), 1, "{spans:?}");
}

#[test]
fn the_builder_hands_its_pieces_to_a_wrapper() {
    // `summary_lines` wraps the pieces with `wrap_inline`, which takes
    // (text, style) segments.
    let mut meta = MetaSpans::new(dim());
    meta.text("Done for ").value("20s");
    assert_eq!(
        meta.into_segments(),
        vec![
            ("Done for ".to_string(), dim()),
            ("20s".to_string(), dim().fg(meta_value_color())),
        ]
    );
}
