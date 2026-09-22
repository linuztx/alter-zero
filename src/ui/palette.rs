//! The colour **palettes** — one per [`Theme`] — and the ambient *active*
//! one every renderer reads. See `docs/theme.md`.
//!
//! [`super::theme`] names what each colour is *for* (the accent a menu
//! selects with, the tool bullet's success green, the `⎿` gutter's dim, …)
//! as accessor functions; this module holds what each theme *paints* those
//! roles with — the [`Palette`] tables below, one design system per entry
//! — and the one piece of state the whole of `ui` reads them through: the
//! **active theme**, a thread-local the I/O boundary sets at startup and on
//! a `/theme` switch ([`activate_theme`]). The renderers stay pure in the
//! sense that matters — same inputs, same rows — and the palette is an
//! input the boundary injects once rather than a parameter threaded through
//! four hundred call sites; a thread-local rather than a process global so
//! every test thread has its own (the `footer` memo's rule), and so the
//! `/theme` picker can render its preview cells under a theme that is
//! *not* the active one by scoping it ([`with_theme`]).

use std::cell::Cell;

use ratatui::style::Color;

use crate::app::Theme;
use crate::highlight::CodeTheme;

/// One theme's colours, by **role**. Every colour the chrome paints is one
/// of these or derived from one (the accessor functions in [`super::theme`]
/// hold the derivations — a link's blue is also the list marker's and the
/// banner gradient's far end; the running bullet blinks in `dim`), so a
/// new theme is one table of twenty-one values and nothing
/// else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Palette {
    /// The reply text, the composer prompt, a tool's name and arguments and
    /// output, the status verb's shimmer crest — the brightest ink.
    pub text: Color,
    /// A quieter ink: an unselected model id, a settings value, the comet's
    /// mid-tail, the thinking header's shimmer floor.
    pub text_muted: Color,
    /// The dim everything secondary wears: the `⎿` corners and placeholders,
    /// the footer, the hints, the counters, a waiting or resting-running
    /// bullet, the quotes and timestamps.
    pub dim: Color,
    /// The composer box's rules and every framed view's.
    pub border: Color,
    /// The user bubble's ink and ground — the `❯ …` block, muted on purpose:
    /// the user's own words are the one thing on screen they need not
    /// re-read.
    pub user_fg: Color,
    pub user_bg: Color,
    /// The `/resume` picker's selected-row tint.
    pub selection_bg: Color,
    /// The brand accent: what every picker selects with, the system bullet,
    /// inline code, the banner hint, a permission prompt's title, the Ctrl+R
    /// query, the agent session view's composer label chip — and the
    /// gradient's near end.
    pub accent: Color,
    /// Ink over an `accent` fill (the ↓-focused footer chip, the current
    /// ask-question chip, the agent session view's composer label).
    pub on_accent: Color,
    /// The secondary accent: a link's URL, an ordered list's marker, the
    /// context view's user tag — and the gradient's far end.
    pub link: Color,
    /// Success: the finished tool bullet, a diff's `+`, the active model's
    /// ✓, a completed task, a background notice that went well.
    pub success: Color,
    /// Failure: the error bullet, a failed tool, a diff's `-`, the `!` shell
    /// mode's red, an error toast.
    pub error: Color,
    /// Caution: the `retrying n/m` clause, the context view's system tag,
    /// the ask review's unanswered warning.
    pub warning: Color,
    /// The context view's tool tag and the `/resume` toolbar's focus.
    pub purple: Color,
    /// The numbered diff rows' ground — an added row, a removed row, and
    /// the brighter marks under the characters that actually changed
    /// (`docs/inline-diff.md`).
    pub diff_add_bg: Color,
    pub diff_del_bg: Color,
    pub diff_add_mark_bg: Color,
    pub diff_del_mark_bg: Color,
    /// The status verb's resting grey, under the shimmer's sweep to `text`.
    pub shimmer_base: Color,
    /// The bottom of the `pulse` spinner style's breath (its top is the
    /// text colour; the running tool bullet itself blinks in `dim`,
    /// `docs/tool-pulse.md`).
    pub pulse_dim: Color,
    /// The syntax theme the code blocks and file cells are coloured with —
    /// the same family as the chrome, so a reply's code and the frame around
    /// it come from one design system.
    pub code: CodeTheme,
}

/// Catppuccin Mocha — the default. The chrome takes the flavour's own
/// roles: `text`/`subtext1` inks, `overlay1`/`overlay2` for the dim and the
/// rules, `surface0`/`surface1` grounds, `sky` for the accent and `blue`
/// for the link (the mascot's gradient), the semantic `green`/`red`/
/// `yellow`/`mauve`; the diff tints are Catppuccin's own DiffAdd/DiffDelete
/// rule — the hue mixed 18% into `base` for a row, 42% for a mark.
const MOCHA: Palette = Palette {
    text: Color::Rgb(0xCD, 0xD6, 0xF4),
    text_muted: Color::Rgb(0xBA, 0xC2, 0xDE),
    dim: Color::Rgb(0x7F, 0x84, 0x9C),
    border: Color::Rgb(0x93, 0x99, 0xB2),
    user_fg: Color::Rgb(0x93, 0x99, 0xB2),
    user_bg: Color::Rgb(0x31, 0x32, 0x44),
    selection_bg: Color::Rgb(0x45, 0x47, 0x5A),
    accent: Color::Rgb(0x89, 0xDC, 0xEB),
    on_accent: Color::Rgb(0x11, 0x11, 0x1B),
    link: Color::Rgb(0x89, 0xB4, 0xFA),
    success: Color::Rgb(0xA6, 0xE3, 0xA1),
    error: Color::Rgb(0xF3, 0x8B, 0xA8),
    warning: Color::Rgb(0xF9, 0xE2, 0xAF),
    purple: Color::Rgb(0xCB, 0xA6, 0xF7),
    diff_add_bg: Color::Rgb(0x36, 0x41, 0x43),
    diff_del_bg: Color::Rgb(0x44, 0x32, 0x44),
    diff_add_mark_bg: Color::Rgb(0x57, 0x71, 0x5E),
    diff_del_mark_bg: Color::Rgb(0x77, 0x4C, 0x61),
    shimmer_base: Color::Rgb(0x7F, 0x84, 0x9C),
    pulse_dim: Color::Rgb(0x58, 0x5B, 0x70),
    code: CodeTheme::CatppuccinMocha,
};

/// Catppuccin Macchiato — the same roles from the medium-dark flavour.
const MACCHIATO: Palette = Palette {
    text: Color::Rgb(0xCA, 0xD3, 0xF5),
    text_muted: Color::Rgb(0xB8, 0xC0, 0xE0),
    dim: Color::Rgb(0x80, 0x87, 0xA2),
    border: Color::Rgb(0x93, 0x9A, 0xB7),
    user_fg: Color::Rgb(0x93, 0x9A, 0xB7),
    user_bg: Color::Rgb(0x36, 0x3A, 0x4F),
    selection_bg: Color::Rgb(0x49, 0x4D, 0x64),
    accent: Color::Rgb(0x91, 0xD7, 0xE3),
    on_accent: Color::Rgb(0x18, 0x19, 0x26),
    link: Color::Rgb(0x8A, 0xAD, 0xF4),
    success: Color::Rgb(0xA6, 0xDA, 0x95),
    error: Color::Rgb(0xED, 0x87, 0x96),
    warning: Color::Rgb(0xEE, 0xD4, 0x9F),
    purple: Color::Rgb(0xC6, 0xA0, 0xF6),
    diff_add_bg: Color::Rgb(0x3B, 0x47, 0x4A),
    diff_del_bg: Color::Rgb(0x48, 0x38, 0x4B),
    diff_add_mark_bg: Color::Rgb(0x5B, 0x72, 0x60),
    diff_del_mark_bg: Color::Rgb(0x78, 0x4F, 0x61),
    shimmer_base: Color::Rgb(0x80, 0x87, 0xA2),
    pulse_dim: Color::Rgb(0x5B, 0x60, 0x78),
    code: CodeTheme::CatppuccinMacchiato,
};

/// Catppuccin Frappé — the same roles from the lightest dark flavour.
const FRAPPE: Palette = Palette {
    text: Color::Rgb(0xC6, 0xD0, 0xF5),
    text_muted: Color::Rgb(0xB5, 0xBF, 0xE2),
    dim: Color::Rgb(0x83, 0x8B, 0xA7),
    border: Color::Rgb(0x94, 0x9C, 0xBB),
    user_fg: Color::Rgb(0x94, 0x9C, 0xBB),
    user_bg: Color::Rgb(0x41, 0x45, 0x59),
    selection_bg: Color::Rgb(0x51, 0x57, 0x6D),
    accent: Color::Rgb(0x99, 0xD1, 0xDB),
    on_accent: Color::Rgb(0x23, 0x26, 0x34),
    link: Color::Rgb(0x8C, 0xAA, 0xEE),
    success: Color::Rgb(0xA6, 0xD1, 0x89),
    error: Color::Rgb(0xE7, 0x82, 0x84),
    warning: Color::Rgb(0xE5, 0xC8, 0x90),
    purple: Color::Rgb(0xCA, 0x9E, 0xE6),
    diff_add_bg: Color::Rgb(0x45, 0x50, 0x52),
    diff_del_bg: Color::Rgb(0x51, 0x42, 0x51),
    diff_add_mark_bg: Color::Rgb(0x62, 0x76, 0x62),
    diff_del_mark_bg: Color::Rgb(0x7D, 0x55, 0x60),
    shimmer_base: Color::Rgb(0x83, 0x8B, 0xA7),
    pulse_dim: Color::Rgb(0x62, 0x68, 0x80),
    code: CodeTheme::CatppuccinFrappe,
};

/// Catppuccin Latte — the light flavour, for a light terminal: the inks are
/// dark (`text` is `#4C4F69`), the grounds and tints pale, and the "bright"
/// end of every blend is the *darker* colour, since on a light ground that
/// is the one that stands out. The accent chip's ink is `base`.
const LATTE: Palette = Palette {
    text: Color::Rgb(0x4C, 0x4F, 0x69),
    text_muted: Color::Rgb(0x5C, 0x5F, 0x77),
    dim: Color::Rgb(0x8C, 0x8F, 0xA1),
    border: Color::Rgb(0x7C, 0x7F, 0x93),
    user_fg: Color::Rgb(0x6C, 0x6F, 0x85),
    user_bg: Color::Rgb(0xCC, 0xD0, 0xDA),
    selection_bg: Color::Rgb(0xBC, 0xC0, 0xCC),
    accent: Color::Rgb(0x04, 0xA5, 0xE5),
    on_accent: Color::Rgb(0xEF, 0xF1, 0xF5),
    link: Color::Rgb(0x1E, 0x66, 0xF5),
    success: Color::Rgb(0x40, 0xA0, 0x2B),
    error: Color::Rgb(0xD2, 0x0F, 0x39),
    warning: Color::Rgb(0xDF, 0x8E, 0x1D),
    purple: Color::Rgb(0x88, 0x39, 0xEF),
    diff_add_bg: Color::Rgb(0xD0, 0xE2, 0xD1),
    diff_del_bg: Color::Rgb(0xEA, 0xC8, 0xD3),
    diff_add_mark_bg: Color::Rgb(0xA6, 0xCF, 0xA0),
    diff_del_mark_bg: Color::Rgb(0xE3, 0x92, 0xA6),
    shimmer_base: Color::Rgb(0x8C, 0x8F, 0xA1),
    pulse_dim: Color::Rgb(0xAC, 0xB0, 0xBE),
    code: CodeTheme::CatppuccinLatte,
};

/// One Dark — the TUI's **original** chrome, value for value: the white
/// reply text, the `#8A8A8A` dim, the `#56B6C2` cyan accent and `#61AFEF`
/// link blue the banner gradient ran between, GitHub's `#3FB950` success
/// green, One Dark's red/amber/purple, the codex diff tints, and codex's
/// `#888888` shimmer grey. Only the code changes: Atom's One Dark instead of
/// the Catppuccin Mocha the blocks used to wear under this chrome.
const ONE_DARK: Palette = Palette {
    text: Color::Rgb(0xFF, 0xFF, 0xFF),
    text_muted: Color::Rgb(0xC8, 0xC8, 0xC8),
    dim: Color::Rgb(0x8A, 0x8A, 0x8A),
    border: Color::Rgb(0xAA, 0xAA, 0xAA),
    user_fg: Color::Rgb(0x6E, 0x6E, 0x6E),
    user_bg: Color::Rgb(0x2D, 0x2D, 0x2D),
    selection_bg: Color::Rgb(0x3A, 0x40, 0x46),
    accent: Color::Rgb(0x56, 0xB6, 0xC2),
    on_accent: Color::Rgb(0x1E, 0x1E, 0x1E),
    link: Color::Rgb(0x61, 0xAF, 0xEF),
    success: Color::Rgb(0x3F, 0xB9, 0x50),
    error: Color::Rgb(0xE0, 0x6C, 0x75),
    warning: Color::Rgb(0xE5, 0xC0, 0x7B),
    purple: Color::Rgb(0xC6, 0x78, 0xDD),
    diff_add_bg: Color::Rgb(0x21, 0x3A, 0x2B),
    diff_del_bg: Color::Rgb(0x4A, 0x22, 0x1D),
    diff_add_mark_bg: Color::Rgb(0x2E, 0x6F, 0x3E),
    diff_del_mark_bg: Color::Rgb(0x8B, 0x2F, 0x27),
    shimmer_base: Color::Rgb(0x88, 0x88, 0x88),
    pulse_dim: Color::Rgb(0x4A, 0x4A, 0x4A),
    code: CodeTheme::OneDark,
};

/// Dracula — its cyan accent over a purple link (the gradient), the comment
/// blue-grey as the dim, the current-line ground for the bubble.
const DRACULA: Palette = Palette {
    text: Color::Rgb(0xF8, 0xF8, 0xF2),
    text_muted: Color::Rgb(0xCB, 0xD0, 0xDB),
    dim: Color::Rgb(0x62, 0x72, 0xA4),
    border: Color::Rgb(0x7C, 0x83, 0xA9),
    user_fg: Color::Rgb(0xAD, 0xB5, 0xCB),
    user_bg: Color::Rgb(0x44, 0x47, 0x5A),
    selection_bg: Color::Rgb(0x56, 0x5A, 0x72),
    accent: Color::Rgb(0x8B, 0xE9, 0xFD),
    on_accent: Color::Rgb(0x28, 0x2A, 0x36),
    link: Color::Rgb(0xBD, 0x93, 0xF9),
    success: Color::Rgb(0x50, 0xFA, 0x7B),
    error: Color::Rgb(0xFF, 0x55, 0x55),
    warning: Color::Rgb(0xFF, 0xB8, 0x6C),
    purple: Color::Rgb(0xFF, 0x79, 0xC6),
    diff_add_bg: Color::Rgb(0x2F, 0x4F, 0x42),
    diff_del_bg: Color::Rgb(0x4F, 0x32, 0x3C),
    diff_add_mark_bg: Color::Rgb(0x39, 0x81, 0x53),
    diff_del_mark_bg: Color::Rgb(0x82, 0x3C, 0x43),
    shimmer_base: Color::Rgb(0x62, 0x72, 0xA4),
    pulse_dim: Color::Rgb(0x44, 0x47, 0x5A),
    code: CodeTheme::Dracula,
};

/// Nord — the frost cyan (`nord8`) accent over the frost blue (`nord9`)
/// link, the snow-storm inks, the polar-night grounds and aurora hues.
const NORD: Palette = Palette {
    text: Color::Rgb(0xEC, 0xEF, 0xF4),
    text_muted: Color::Rgb(0xD8, 0xDE, 0xE9),
    dim: Color::Rgb(0x7B, 0x88, 0xA1),
    border: Color::Rgb(0x61, 0x6E, 0x88),
    user_fg: Color::Rgb(0x92, 0x9A, 0xAA),
    user_bg: Color::Rgb(0x3B, 0x42, 0x52),
    selection_bg: Color::Rgb(0x4C, 0x56, 0x6A),
    accent: Color::Rgb(0x88, 0xC0, 0xD0),
    on_accent: Color::Rgb(0x2E, 0x34, 0x40),
    link: Color::Rgb(0x81, 0xA1, 0xC1),
    success: Color::Rgb(0xA3, 0xBE, 0x8C),
    error: Color::Rgb(0xBF, 0x61, 0x6A),
    warning: Color::Rgb(0xEB, 0xCB, 0x8B),
    purple: Color::Rgb(0xB4, 0x8E, 0xAD),
    diff_add_bg: Color::Rgb(0x43, 0x4D, 0x4E),
    diff_del_bg: Color::Rgb(0x48, 0x3C, 0x48),
    diff_add_mark_bg: Color::Rgb(0x5F, 0x6E, 0x60),
    diff_del_mark_bg: Color::Rgb(0x6B, 0x47, 0x52),
    shimmer_base: Color::Rgb(0x7B, 0x88, 0xA1),
    pulse_dim: Color::Rgb(0x4C, 0x56, 0x6A),
    code: CodeTheme::Nord,
};

/// Gruvbox Dark (medium) — the aqua accent over the blue link, the warm
/// `fg`/`fg2` inks, `gray` for the dim, `bg1`/`bg2` grounds.
const GRUVBOX: Palette = Palette {
    text: Color::Rgb(0xEB, 0xDB, 0xB2),
    text_muted: Color::Rgb(0xD5, 0xC4, 0xA1),
    dim: Color::Rgb(0x92, 0x83, 0x74),
    border: Color::Rgb(0xA8, 0x99, 0x84),
    user_fg: Color::Rgb(0xA8, 0x99, 0x84),
    user_bg: Color::Rgb(0x3C, 0x38, 0x36),
    selection_bg: Color::Rgb(0x50, 0x49, 0x45),
    accent: Color::Rgb(0x8E, 0xC0, 0x7C),
    on_accent: Color::Rgb(0x28, 0x28, 0x28),
    link: Color::Rgb(0x83, 0xA5, 0x98),
    success: Color::Rgb(0xB8, 0xBB, 0x26),
    error: Color::Rgb(0xFB, 0x49, 0x34),
    warning: Color::Rgb(0xFA, 0xBD, 0x2F),
    purple: Color::Rgb(0xD3, 0x86, 0x9B),
    diff_add_bg: Color::Rgb(0x42, 0x42, 0x28),
    diff_del_bg: Color::Rgb(0x4E, 0x2E, 0x2A),
    diff_add_mark_bg: Color::Rgb(0x64, 0x66, 0x27),
    diff_del_mark_bg: Color::Rgb(0x81, 0x36, 0x2D),
    shimmer_base: Color::Rgb(0x92, 0x83, 0x74),
    pulse_dim: Color::Rgb(0x66, 0x5C, 0x54),
    code: CodeTheme::GruvboxDark,
};

/// Solarized Dark — `base1`/`base0` inks, `base01`/`base00` for the dim and
/// the rules, `base02` grounds, the cyan accent over the blue link. Its
/// `base03` is so dark a blue that the diff tints mix a quarter of the hue
/// in (half for a mark) where the others mix 18% (42%).
const SOLARIZED: Palette = Palette {
    text: Color::Rgb(0x93, 0xA1, 0xA1),
    text_muted: Color::Rgb(0x83, 0x94, 0x96),
    dim: Color::Rgb(0x58, 0x6E, 0x75),
    border: Color::Rgb(0x65, 0x7B, 0x83),
    user_fg: Color::Rgb(0x65, 0x7B, 0x83),
    user_bg: Color::Rgb(0x07, 0x36, 0x42),
    selection_bg: Color::Rgb(0x1F, 0x47, 0x51),
    accent: Color::Rgb(0x2A, 0xA1, 0x98),
    on_accent: Color::Rgb(0x00, 0x2B, 0x36),
    link: Color::Rgb(0x26, 0x8B, 0xD2),
    success: Color::Rgb(0x85, 0x99, 0x00),
    error: Color::Rgb(0xDC, 0x32, 0x2F),
    warning: Color::Rgb(0xB5, 0x89, 0x00),
    purple: Color::Rgb(0x6C, 0x71, 0xC4),
    diff_add_bg: Color::Rgb(0x21, 0x46, 0x28),
    diff_del_bg: Color::Rgb(0x37, 0x2D, 0x34),
    diff_add_mark_bg: Color::Rgb(0x42, 0x62, 0x1B),
    diff_del_mark_bg: Color::Rgb(0x6E, 0x2E, 0x32),
    shimmer_base: Color::Rgb(0x58, 0x6E, 0x75),
    pulse_dim: Color::Rgb(0x3F, 0x5A, 0x60),
    code: CodeTheme::SolarizedDark,
};

/// Monokai — the cyan accent over the purple link, the comment brown-grey
/// as the dim, the line-highlight ground for the bubble; with only six hues
/// the "purple" role takes the yellow, the orange being the warning.
const MONOKAI: Palette = Palette {
    text: Color::Rgb(0xF8, 0xF8, 0xF2),
    text_muted: Color::Rgb(0xCF, 0xCF, 0xC2),
    dim: Color::Rgb(0x75, 0x71, 0x5E),
    border: Color::Rgb(0x90, 0x90, 0x8A),
    user_fg: Color::Rgb(0xB6, 0xB4, 0xA8),
    user_bg: Color::Rgb(0x3E, 0x3D, 0x32),
    selection_bg: Color::Rgb(0x49, 0x48, 0x3E),
    accent: Color::Rgb(0x66, 0xD9, 0xEF),
    on_accent: Color::Rgb(0x27, 0x28, 0x22),
    link: Color::Rgb(0xAE, 0x81, 0xFF),
    success: Color::Rgb(0xA6, 0xE2, 0x2E),
    error: Color::Rgb(0xF9, 0x26, 0x72),
    warning: Color::Rgb(0xFD, 0x97, 0x1F),
    purple: Color::Rgb(0xE6, 0xDB, 0x74),
    diff_add_bg: Color::Rgb(0x3E, 0x49, 0x24),
    diff_del_bg: Color::Rgb(0x4D, 0x28, 0x30),
    diff_add_mark_bg: Color::Rgb(0x5C, 0x76, 0x27),
    diff_del_mark_bg: Color::Rgb(0x7F, 0x27, 0x44),
    shimmer_base: Color::Rgb(0x75, 0x71, 0x5E),
    pulse_dim: Color::Rgb(0x49, 0x48, 0x3E),
    code: CodeTheme::Monokai,
};

/// The terminal's own sixteen colours — no RGB anywhere, so the TUI wears
/// whatever palette the terminal is configured with, Claude Code's
/// "ANSI colors only" mode. What the 16-colour palette cannot express goes
/// without: the diff rows keep the terminal ground (only the marks tint,
/// both on the bright-black so a row's sign says which), the gradient and
/// the shimmer step between their two ends instead of blending
/// ([`super::wrap::lerp_color`]), and the `pulse` spinner holds still,
/// since its breath's two ends are the same bright-black — while the
/// running bullet blinks exactly as it does everywhere, a blink needing no
/// second shade. The reply text is the terminal's default foreground, which
/// is the point.
const ANSI: Palette = Palette {
    text: Color::Reset,
    text_muted: Color::Gray,
    dim: Color::DarkGray,
    border: Color::Gray,
    user_fg: Color::Gray,
    user_bg: Color::DarkGray,
    selection_bg: Color::DarkGray,
    accent: Color::Cyan,
    on_accent: Color::Black,
    link: Color::Blue,
    success: Color::Green,
    error: Color::Red,
    warning: Color::Yellow,
    purple: Color::Magenta,
    diff_add_bg: Color::Reset,
    diff_del_bg: Color::Reset,
    diff_add_mark_bg: Color::DarkGray,
    diff_del_mark_bg: Color::DarkGray,
    shimmer_base: Color::DarkGray,
    pulse_dim: Color::DarkGray,
    code: CodeTheme::Ansi,
};

/// The palette `theme` paints with.
#[must_use]
pub(super) const fn palette_of(theme: Theme) -> &'static Palette {
    match theme {
        Theme::Mocha => &MOCHA,
        Theme::Macchiato => &MACCHIATO,
        Theme::Frappe => &FRAPPE,
        Theme::Latte => &LATTE,
        Theme::OneDark => &ONE_DARK,
        Theme::Dracula => &DRACULA,
        Theme::Nord => &NORD,
        Theme::Gruvbox => &GRUVBOX,
        Theme::Solarized => &SOLARIZED,
        Theme::Monokai => &MONOKAI,
        Theme::Ansi => &ANSI,
    }
}

thread_local! {
    /// The theme every renderer on this thread paints with. The event loop
    /// draws on one thread, so one cell is the whole of the app's state;
    /// each test thread gets its own, seeded at the default, so a test that
    /// renders under another theme (through [`with_theme`]) disturbs no
    /// other.
    static ACTIVE: Cell<Theme> = const { Cell::new(Theme::Mocha) };
}

/// The theme the renderers are currently painting with.
#[must_use]
pub fn active_theme() -> Theme {
    ACTIVE.with(Cell::get)
}

/// Make `theme` the one every renderer on this thread paints with, from now
/// on — the boundary's call: at bootstrap, before the banner is built, and
/// on a `/theme` switch, before the purge rebuild that repaints the
/// conversation in it (`docs/theme.md`). The pure [`crate::app::App::theme`]
/// records the same choice; the two are kept equal there.
pub fn activate_theme(theme: Theme) {
    ACTIVE.with(|active| active.set(theme));
}

/// Run `f` with `theme` active, then restore whatever was active before —
/// how the `/theme` picker renders its preview cells in the highlighted
/// theme while the rest of the screen keeps the session's (`docs/theme.md`),
/// and how a test renders under a theme without leaking it. Restores on a
/// panic too, so an assertion failing inside the closure cannot recolour the
/// tests that follow it on the same thread.
pub fn with_theme<T>(theme: Theme, f: impl FnOnce() -> T) -> T {
    struct Restore(Theme);
    impl Drop for Restore {
        fn drop(&mut self) {
            activate_theme(self.0);
        }
    }
    let _restore = Restore(active_theme());
    activate_theme(theme);
    f()
}

/// The active theme's palette — what every colour accessor in
/// [`super::theme`] reads.
#[must_use]
pub(super) fn palette() -> &'static Palette {
    palette_of(active_theme())
}
