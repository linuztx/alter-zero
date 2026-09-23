//! The per-theme palettes and the ambient active theme (`docs/theme.md`).

use super::*;
use crate::app::Theme;
use crate::highlight::CodeTheme;
use crate::ui::palette::{activate_theme, active_theme, palette, palette_of, with_theme};
use crate::ui::theme::*;
use crate::ui::wrap::{blend_color, lerp_color};

/// The `(r, g, b)` of an RGB colour.
fn rgb_of(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        other => panic!("expected an RGB colour, got {other:?}"),
    }
}

/// Relative luminance, roughly — enough to say which of two colours is the
/// darker.
fn luma(color: Color) -> u32 {
    let (r, g, b) = rgb_of(color);
    2 * u32::from(r) + 7 * u32::from(g) + u32::from(b)
}

#[test]
fn the_default_theme_is_catppuccin_mocha_on_every_thread() {
    // A fresh thread — every test's — starts on the default, and the
    // accessors read its palette: the sky accent, the green, the mocha code.
    assert_eq!(active_theme(), Theme::Mocha);
    assert_eq!(menu_selected_color(), Color::Rgb(0x89, 0xDC, 0xEB));
    assert_eq!(system_color(), menu_selected_color(), "one accent");
    assert_eq!(tool_ok_color(), Color::Rgb(0xA6, 0xE3, 0xA1));
    assert_eq!(error_color(), Color::Rgb(0xF3, 0x8B, 0xA8));
    assert_eq!(palette().code, CodeTheme::CatppuccinMocha);
}

#[test]
fn with_theme_scopes_the_palette_to_the_closure_and_restores() {
    let inside = with_theme(Theme::Dracula, || {
        assert_eq!(active_theme(), Theme::Dracula);
        menu_selected_color()
    });
    assert_eq!(inside, Color::Rgb(0x8B, 0xE9, 0xFD), "Dracula's cyan");
    assert_eq!(active_theme(), Theme::Mocha, "restored on the way out");
    assert_eq!(menu_selected_color(), Color::Rgb(0x89, 0xDC, 0xEB));
}

#[test]
fn with_theme_restores_even_when_the_closure_panics() {
    let outcome = std::panic::catch_unwind(|| {
        with_theme(Theme::Nord, || panic!("an assertion inside the scope"));
    });
    assert!(outcome.is_err());
    assert_eq!(
        active_theme(),
        Theme::Mocha,
        "a panicking scope must not recolour the tests after it"
    );
}

#[test]
fn activate_theme_switches_the_thread_for_good() {
    activate_theme(Theme::Gruvbox);
    assert_eq!(active_theme(), Theme::Gruvbox);
    assert_eq!(
        menu_selected_color(),
        Color::Rgb(0x8E, 0xC0, 0x7C),
        "the aqua accent"
    );
    assert_eq!(palette().code, CodeTheme::GruvboxDark);
    activate_theme(Theme::default());
    assert_eq!(active_theme(), Theme::Mocha);
}

#[test]
fn every_theme_pairs_its_chrome_with_its_own_code_theme() {
    // The catalog's order and the code themes' order are the same list —
    // the pairing is by position, one design system per entry.
    for (theme, code) in Theme::ALL.iter().zip(CodeTheme::ALL) {
        assert_eq!(palette_of(*theme).code, code, "{theme:?}");
    }
}

#[test]
fn the_semantic_hues_stay_distinct_within_every_theme() {
    for theme in Theme::ALL {
        let p = palette_of(theme);
        let hues = [p.accent, p.success, p.error, p.warning];
        for (i, a) in hues.iter().enumerate() {
            for b in &hues[i + 1..] {
                assert_ne!(a, b, "{theme:?}: two semantic roles share a colour");
            }
        }
        assert_ne!(p.text, p.dim, "{theme:?}: the dim must read as dim");
        assert_ne!(p.accent, p.dim, "{theme:?}: the accent must stand out");
        assert_ne!(
            p.user_fg, p.user_bg,
            "{theme:?}: the bubble's ink must show on its ground"
        );
        assert_ne!(
            p.accent, p.on_accent,
            "{theme:?}: a chip's ink must show on its accent fill"
        );
    }
}

#[test]
fn one_dark_is_the_original_look_value_for_value() {
    // The chrome the TUI shipped with — every RGB it used to hard-code —
    // survives as a theme, so whoever liked it keeps it.
    with_theme(Theme::OneDark, || {
        assert_eq!(ai_color(), Color::Rgb(0xFF, 0xFF, 0xFF));
        assert_eq!(model_id_color(), Color::Rgb(0xC8, 0xC8, 0xC8));
        assert_eq!(tool_dim_color(), Color::Rgb(0x8A, 0x8A, 0x8A));
        assert_eq!(border_color(), Color::Rgb(0xAA, 0xAA, 0xAA));
        assert_eq!(user_color(), Color::Rgb(0x6E, 0x6E, 0x6E));
        assert_eq!(user_bg_color(), Color::Rgb(0x2D, 0x2D, 0x2D));
        assert_eq!(resume_selected_bg(), Color::Rgb(0x3A, 0x40, 0x46));
        assert_eq!(menu_selected_color(), Color::Rgb(0x56, 0xB6, 0xC2));
        assert_eq!(inline_code_color(), Color::Rgb(0x56, 0xB6, 0xC2));
        assert_eq!(footer_focus_fg(), Color::Rgb(0x1E, 0x1E, 0x1E));
        assert_eq!(agent_view_label_bg(), Color::Rgb(0x56, 0xB6, 0xC2));
        assert_eq!(agent_view_label_fg(), Color::Rgb(0x1E, 0x1E, 0x1E));
        assert_eq!(link_url_color(), Color::Rgb(0x61, 0xAF, 0xEF));
        assert_eq!(tool_ok_color(), Color::Rgb(0x3F, 0xB9, 0x50));
        assert_eq!(error_color(), Color::Rgb(0xE0, 0x6C, 0x75));
        assert_eq!(status_retry_color(), Color::Rgb(0xE5, 0xC0, 0x7B));
        assert_eq!(context_tool_color(), Color::Rgb(0xC6, 0x78, 0xDD));
        assert_eq!(tool_diff_add_bg(), Color::Rgb(0x21, 0x3A, 0x2B));
        assert_eq!(tool_diff_del_bg(), Color::Rgb(0x4A, 0x22, 0x1D));
        assert_eq!(tool_diff_add_mark_bg(), Color::Rgb(0x2E, 0x6F, 0x3E));
        assert_eq!(tool_diff_del_mark_bg(), Color::Rgb(0x8B, 0x2F, 0x27));
        assert_eq!(shimmer_base(), Color::Rgb(0x88, 0x88, 0x88));
        assert_eq!(tool_pulse_dim(), Color::Rgb(0x4A, 0x4A, 0x4A));
        assert_eq!(tool_pulse_bright(), tool_dim_color());
        assert_eq!(header_gradient_start(), Color::Rgb(0x56, 0xB6, 0xC2));
        assert_eq!(header_gradient_end(), Color::Rgb(0x61, 0xAF, 0xEF));
        assert_eq!(palette().code, CodeTheme::OneDark);
    });
}

#[test]
fn the_ansi_theme_names_no_rgb_colour_anywhere() {
    let p = palette_of(Theme::Ansi);
    let every = [
        p.text,
        p.text_muted,
        p.dim,
        p.border,
        p.user_fg,
        p.user_bg,
        p.selection_bg,
        p.accent,
        p.on_accent,
        p.link,
        p.success,
        p.error,
        p.warning,
        p.purple,
        p.diff_add_bg,
        p.diff_del_bg,
        p.diff_add_mark_bg,
        p.diff_del_mark_bg,
        p.shimmer_base,
        p.pulse_dim,
    ];
    for color in every {
        assert!(
            !matches!(color, Color::Rgb(..)),
            "the terminal theme leaves every colour to the terminal: {color:?}"
        );
    }
    assert_eq!(p.code, CodeTheme::Ansi);
    with_theme(Theme::Ansi, || {
        assert_eq!(menu_selected_color(), Color::Cyan);
        assert_eq!(
            ai_color(),
            Color::Reset,
            "the reply text is the terminal's own"
        );
        // The blends step rather than mix: the gradient's near end, then its far.
        assert_eq!(
            lerp_color(header_gradient_start(), header_gradient_end(), 0.49),
            Color::Cyan
        );
        assert_eq!(
            lerp_color(header_gradient_start(), header_gradient_end(), 0.5),
            Color::Blue
        );
    });
}

#[test]
fn the_light_theme_inverts_the_inks_and_the_tints() {
    let p = palette_of(Theme::Latte);
    assert!(Theme::Latte.is_light());
    assert!(
        luma(p.text) < luma(p.user_bg),
        "dark ink on a pale bubble: {:?} on {:?}",
        p.text,
        p.user_bg
    );
    assert!(
        luma(p.diff_add_bg) > luma(p.text) && luma(p.diff_del_bg) > luma(p.text),
        "pale diff tints under dark text"
    );
    assert!(
        luma(p.pulse_dim) > luma(p.dim),
        "on a light ground the *dim* end of the breath is the paler colour"
    );
    for theme in Theme::ALL
        .iter()
        .filter(|t| !t.is_light() && **t != Theme::Ansi)
    {
        let p = palette_of(*theme);
        assert!(
            luma(p.text) > luma(p.user_bg),
            "{theme:?}: a dark theme's ink is lighter than its bubble"
        );
    }
}

#[test]
fn the_gradient_and_the_blends_follow_the_palette() {
    // The banner gradient runs accent → link in every theme (the mascot,
    // the sparkle, the blocks and the two braille tracks all walk it), and
    // the shimmer crests at the text colour.
    for theme in Theme::ALL {
        with_theme(theme, || {
            assert_eq!(header_gradient_start(), menu_selected_color(), "{theme:?}");
            assert_eq!(header_gradient_end(), link_url_color(), "{theme:?}");
            assert_eq!(shimmer_highlight(), ai_color(), "{theme:?}");
            assert_eq!(tool_pulse_bright(), tool_dim_color(), "{theme:?}");
        });
    }
    // RGB ends mix; the midpoint of black and white is mid-grey.
    assert_eq!(
        lerp_color(Color::Rgb(0, 0, 0), Color::Rgb(0xFF, 0xFF, 0xFF), 0.5),
        Color::Rgb(0x80, 0x80, 0x80)
    );
    assert_eq!(
        blend_color(Color::Rgb(0xFF, 0xFF, 0xFF), Color::Rgb(0, 0, 0), 1.0),
        Color::Rgb(0xFF, 0xFF, 0xFF),
        "alpha 1 is pure fg"
    );
    assert_eq!(
        blend_color(Color::Rgb(0xFF, 0xFF, 0xFF), Color::Rgb(0, 0, 0), 0.0),
        Color::Rgb(0, 0, 0),
        "alpha 0 is pure bg"
    );
    // A named end cannot mix: the blend steps at the midpoint.
    assert_eq!(
        blend_color(Color::White, Color::DarkGray, 0.6),
        Color::White
    );
    assert_eq!(
        blend_color(Color::White, Color::DarkGray, 0.4),
        Color::DarkGray
    );
    assert_eq!(
        lerp_color(Color::Cyan, Color::Rgb(1, 2, 3), 0.99),
        Color::Rgb(1, 2, 3)
    );
}

#[test]
fn an_error_toast_is_its_themes_red_softened_toward_the_dim() {
    // A toast is quiet chrome — its info line wears the dim — so its failure
    // line is the theme's red pulled toward that dim: still a red at a
    // glance, never as loud as the error bullet (docs/toast.md).
    let spread = |color: Color| {
        let (r, g, b) = rgb_of(color);
        r.max(g).max(b) - r.min(g).min(b)
    };
    let dist2 = |a: Color, b: Color| {
        let ((r0, g0, b0), (r1, g1, b1)) = (rgb_of(a), rgb_of(b));
        [(r0, r1), (g0, g1), (b0, b1)]
            .into_iter()
            .map(|(x, y)| u32::from(x.abs_diff(y)).pow(2))
            .sum::<u32>()
    };
    for theme in Theme::ALL.into_iter().filter(|t| *t != Theme::Ansi) {
        with_theme(theme, || {
            let soft = toast_error_color();
            let (r, g, b) = rgb_of(soft);
            assert!(r > g && r > b, "{theme:?}: still a red: {soft:?}");
            assert!(
                spread(soft) < spread(error_color()),
                "{theme:?}: quieter than the error red: {soft:?} vs {:?}",
                error_color()
            );
            // A red channel that still leads is not enough: over a bluish dim
            // a mostly-grey mix keeps it. The colour must sit nearer the red.
            assert!(
                dist2(soft, error_color()) < dist2(soft, toast_color()),
                "{theme:?}: nearer the red than the dim: {soft:?}"
            );
        });
    }
    // The default theme, value for value.
    assert_eq!(toast_error_color(), Color::Rgb(0xCA, 0x89, 0xA4));
    // A named colour has nothing to mix: the terminal theme keeps the
    // terminal's own red.
    with_theme(Theme::Ansi, || assert_eq!(toast_error_color(), Color::Red));
}

#[test]
fn a_rendered_cell_wears_the_active_theme() {
    // The point of the ambient palette: the same builder paints the same
    // cell in whichever theme is active, so a purge rebuild after a switch
    // recolours everything with no renderer knowing a switch happened.
    let bullet_under = |theme: Theme| {
        with_theme(theme, || {
            message_lines(Role::System, "notice", 40)[0].spans[0]
                .style
                .fg
        })
    };
    assert_eq!(
        bullet_under(Theme::Mocha),
        Some(Color::Rgb(0x89, 0xDC, 0xEB))
    );
    assert_eq!(
        bullet_under(Theme::Dracula),
        Some(Color::Rgb(0x8B, 0xE9, 0xFD))
    );
    assert_eq!(bullet_under(Theme::Ansi), Some(Color::Cyan));
    let bubble_under =
        |theme: Theme| with_theme(theme, || message_lines(Role::User, "hi", 40)[0].style.bg);
    assert_eq!(
        bubble_under(Theme::Latte),
        Some(Color::Rgb(0xCC, 0xD0, 0xDA))
    );
    assert_eq!(
        bubble_under(Theme::Nord),
        Some(Color::Rgb(0x3B, 0x42, 0x52))
    );
}
