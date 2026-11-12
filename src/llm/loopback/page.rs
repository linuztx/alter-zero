//! The page a browser sign-in lands on (`docs/providers/callback-page.md`): every
//! browser sign-in's, since they share one callback (`super`) — an MCP
//! server's, ChatGPT's, the Anthropic Console's and Google Antigravity's.
//!
//! The project's mascot in pixels over a serif headline and one quiet
//! sentence: what happened, and what to do now. The mascot is read off the
//! very block art the terminal banner draws ([`Mascot::art`]), one tall pixel
//! per quadrant, so the page and the banner can't drift apart; its colours
//! and the way it moves say how the sign-in went ([`Tone`]).
//!
//! One self-contained document per response — the stylesheet, the mascot,
//! the sky and the favicon all inline, nothing fetched: the page is served
//! by a terminal app on loopback, so it has to draw offline, and a font or
//! an image from elsewhere would tell a third party about the sign-in. Every
//! word on it that came from outside the code is escaped, and the response
//! forbids whatever the page does not use ([`response`]).

use std::fmt::Write as _;

use crate::app::Mascot;

/// How a page reads at a glance: the mascot's colours and the way it moves
/// (the stylesheet keys both on [`Tone::class`]), and the tab's icon
/// ([`Tone::favicon_wash`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tone {
    /// The sign-in has its answer: the banner's own wash, and a hop.
    Success,
    /// The authorization server said no: red, and a shake of the head.
    Refused,
    /// Nothing failed and nothing was received — a stray request, another
    /// sign-in's answer, one that came too late: amber, and a puzzled tilt.
    Notice,
}

impl Tone {
    /// The class the stylesheet keys this tone's colours and motion on.
    const fn class(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Refused => "refused",
            Self::Notice => "notice",
        }
    }

    /// The favicon's two ends. Fixed where the page's wash follows the
    /// browser's colour scheme: a tab strip may be dark or light whatever
    /// the page is, and Catppuccin Latte's saturated ends read on both.
    const fn favicon_wash(self) -> (&'static str, &'static str) {
        match self {
            Self::Success => ("#04a5e5", "#1e66f5"),
            Self::Refused => ("#d20f39", "#fe640b"),
            Self::Notice => ("#df8e1d", "#fe640b"),
        }
    }
}

/// What one page says. Every string is shown as text, escaped here.
#[derive(Debug, Clone, Copy)]
pub(super) struct Page<'a> {
    pub(super) tone: Tone,
    /// The headline — and the tab's title, ahead of the app's name.
    pub(super) title: &'a str,
    /// The sentence under the headline.
    pub(super) lead: &'a str,
    /// The command that starts a sign-in again (`/mcp`, `/login`), named in
    /// a closing sentence after the lead.
    pub(super) restart: Option<&'a str>,
    /// The authorization server's own words, set apart under the lead.
    pub(super) detail: Option<Detail<'a>>,
}

/// An authorization server's refusal in its own words (RFC 6749
/// §4.1.2.1): the `error` code over its `error_description`.
#[derive(Debug, Clone, Copy)]
pub(super) struct Detail<'a> {
    pub(super) code: &'a str,
    pub(super) text: Option<&'a str>,
}

/// What the page may do, as a `Content-Security-Policy`: draw its own
/// inline styles and `data:` images (the favicon), and nothing else — no
/// script, no fetch, no form, no `<base>`, never inside a frame.
const POLICY: &str = "default-src 'none'; style-src 'unsafe-inline'; img-src data:; \
                      base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// The page as a whole HTTP response — `status` is the status line's tail
/// (`200 OK`). `Referrer-Policy` because the address the browser is on
/// carries the authorization code.
pub(super) fn response(status: &str, page: &Page<'_>) -> String {
    let body = document(page);
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nContent-Security-Policy: {POLICY}\r\n\
         Referrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )
}

/// The page's HTML document.
pub(super) fn document(page: &Page<'_>) -> String {
    let art = Mascot::default().art();
    let app = escape(crate::APP_NAME);
    let title = escape(page.title);
    let mut lead = escape(page.lead);
    if let Some(command) = page.restart {
        let _ = write!(
            lead,
            " To sign in, start again from <code>{}</code> in {app}.",
            escape(command)
        );
    }
    let detail = page.detail.map_or_else(String::new, |detail| {
        format!(
            "\n<div class=\"detail\"><code>{}</code>{}</div>",
            escape(detail.code),
            detail.text.map(escape).unwrap_or_default()
        )
    });
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta name=\"color-scheme\" content=\"dark light\">\n\
         <title>{title} · {app}</title>\n\
         <link rel=\"icon\" href=\"{favicon}\">\n\
         <style>{STYLE}</style>\n\
         </head>\n<body>\n{sky}\n<main class=\"{tone}\">\n\
         <div class=\"icon\">{mascot}</div>\n\
         <h1>{title}</h1>\n\
         <p class=\"lead\">{lead}</p>{detail}\n\
         </main>\n</body>\n</html>\n",
        favicon = favicon(art, page.tone),
        sky = sky(),
        tone = page.tone.class(),
        mascot = mascot_svg(art, page.tone),
    )
}

/// `text` safe as HTML text and inside a quoted attribute.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// the stylesheet

/// The page's whole stylesheet. Colours are Catppuccin's own — Mocha, the
/// default theme's flavour, and Latte for a light browser, the flavours
/// `ui::palette`'s `MOCHA` and `LATTE` tables are drawn from; a page in a
/// browser can't follow `/theme`. Each tone sets the mascot's wash and glow (`--wash-a`,
/// `--wash-b`, `--glow`) and its one motion: a hop on a success, a shake on
/// a refusal, a tilt on a notice. Everything moves once on arrival, then
/// only the blink and the twinkling stars — and nothing at all for a
/// reader who asked for reduced motion, every element resting in its
/// finished state.
const STYLE: &str = r#"
:root {
  color-scheme: dark light;
  --bg: #181825; --bg-lift: #1e1e2e; --text: #cdd6f4; --muted: #a6adc8;
  --surface: #1e1e2e; --border: #313244; --star: #cdd6f4;
  --sky: #89dceb; --blue: #89b4fa; --red: #f38ba8; --peach: #fab387; --yellow: #f9e2af;
  --sky-glow: rgb(137 220 235 / .34); --red-glow: rgb(243 139 168 / .3);
  --yellow-glow: rgb(249 226 175 / .24); --code-bg: rgb(137 220 235 / .09);
}
@media (prefers-color-scheme: light) {
  :root {
    --bg: #eff1f5; --bg-lift: #f6f7fa; --text: #4c4f69; --muted: #5c5f77;
    --surface: #e6e9ef; --border: #ccd0da;
    --sky: #04a5e5; --blue: #1e66f5; --red: #d20f39; --peach: #fe640b; --yellow: #df8e1d;
    --sky-glow: rgb(4 165 229 / .2); --red-glow: rgb(210 15 57 / .16);
    --yellow-glow: rgb(223 142 29 / .18); --code-bg: rgb(4 165 229 / .1);
  }
  .sky { display: none; }
}
* { box-sizing: border-box; margin: 0; }
html { height: 100%; background: var(--bg); }
body {
  min-height: 100%; display: grid; place-items: center; padding: 48px 24px;
  color: var(--text);
  background: radial-gradient(ellipse 60% 50% at 50% 42%, var(--bg-lift), transparent 70%);
  font: 18px/1.6 system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
  -webkit-font-smoothing: antialiased; text-rendering: optimizeLegibility;
}
.sky {
  position: fixed; inset: 0; z-index: -1; width: 100%; height: 100%;
  fill: var(--star); pointer-events: none;
}
.sky .tw { animation: twinkle 5s ease-in-out infinite; }
.sky .tw:nth-child(even) { animation-duration: 8s; animation-delay: -3s; }
main {
  --wash-a: var(--sky); --wash-b: var(--blue); --glow: var(--sky-glow);
  display: flex; flex-direction: column; align-items: center;
  width: 100%; max-width: 34rem; text-align: center;
}
.refused { --wash-a: var(--red); --wash-b: var(--peach); --glow: var(--red-glow); }
.notice { --wash-a: var(--yellow); --wash-b: var(--peach); --glow: var(--yellow-glow); }
.icon { margin-bottom: 44px; }
.mascot {
  display: block; max-width: 40vw; height: auto; overflow: visible;
  transform-origin: 50% 100%; filter: drop-shadow(0 0 22px var(--glow));
}
.mascot stop { stop-color: var(--wash-a); }
.mascot stop + stop { stop-color: var(--wash-b); }
.lid {
  transform-box: fill-box; transform-origin: 50% 0; transform: scaleY(0);
  animation: blink 5.5s 2.4s infinite;
}
.spark {
  transform-box: fill-box; transform-origin: center; opacity: 0; fill: var(--wash-a);
  animation: sparkle .9s cubic-bezier(.2, .8, .3, 1) both;
}
.spark:nth-child(1) { animation-delay: .78s; }
.spark:nth-child(2) { animation-delay: .86s; }
.spark:nth-child(3) { animation-delay: .95s; }
.spark:nth-child(4) { animation-delay: 1.04s; }
.success .mascot { animation: hop .8s .55s cubic-bezier(.3, .7, .4, 1) both; }
.refused .mascot { animation: shake .6s .5s ease-in-out both; }
.notice .mascot { animation: tilt 1.6s .5s cubic-bezier(.4, 0, .2, 1) both; }
h1 {
  margin-bottom: 16px; letter-spacing: -.015em; text-wrap: balance;
  font: 400 clamp(2rem, 1rem + 4vw, 3rem)/1.15 ui-serif, Charter, "Bitstream Charter",
    "Iowan Old Style", "Sitka Heading", Cambria, Georgia, serif;
}
.lead { color: var(--muted); text-wrap: pretty; }
code {
  font-family: ui-monospace, "SF Mono", SFMono-Regular, "JetBrains Mono", "Cascadia Code",
    Menlo, Consolas, "DejaVu Sans Mono", monospace;
}
.lead code {
  padding: .2em .45em; border-radius: 6px; background: var(--code-bg);
  color: var(--sky); font-size: .85em; white-space: nowrap;
}
.detail {
  width: 100%; margin-top: 28px; padding: 14px 18px; border: 1px solid var(--border);
  border-radius: 12px; background: var(--surface); font-size: 15px; line-height: 1.5;
  text-align: left; overflow-wrap: anywhere;
}
.detail code { display: block; margin-bottom: 2px; color: var(--red); font-size: 14px; }
.icon, h1, .lead, .detail { animation: rise .7s cubic-bezier(.2, .8, .2, 1) both; }
h1 { animation-delay: .08s; }
.lead { animation-delay: .16s; }
.detail { animation-delay: .24s; }
@keyframes rise { from { opacity: 0; transform: translateY(10px); } }
@keyframes hop {
  35% { transform: translateY(-14px); }
  62% { transform: scale(1.06, .92); }
  80% { transform: scale(.98, 1.02); }
}
@keyframes shake {
  20% { transform: translateX(-7px); }
  40% { transform: translateX(6px); }
  60% { transform: translateX(-4px); }
  80% { transform: translateX(2px); }
}
@keyframes tilt { 20%, 65% { transform: rotate(-8deg); } 82% { transform: rotate(2deg); } }
@keyframes blink { 0%, 6%, 100% { transform: scaleY(0); } 3% { transform: scaleY(1); } }
@keyframes sparkle {
  0% { opacity: 0; transform: scale(0) rotate(-45deg); }
  35% { opacity: 1; transform: scale(1.1); }
  100% { opacity: 0; transform: scale(.3) rotate(45deg); }
}
@keyframes twinkle { 50% { opacity: .04; } }
@media (prefers-reduced-motion: reduce) {
  *, ::before, ::after { animation: none !important; }
}
"#;

// ---------------------------------------------------------------------------
// the mascot

/// CSS pixels across one quadrant column. A quadrant glyph splits a terminal
/// cell into four, and a cell is about twice as tall as it is wide, so a
/// quadrant is drawn [`PIXEL_HEIGHT`] tall: the creature the terminal shows,
/// not a squashed one.
const PIXEL: i32 = 10;

/// CSS pixels down one quadrant row.
const PIXEL_HEIGHT: i32 = 2 * PIXEL;

/// The quadrants a block glyph lights, as bits — upper left `8`, upper
/// right `4`, lower left `2`, lower right `1` — or `None` for a glyph that
/// is not one of the sixteen quadrant patterns (a space is the empty one).
fn quadrants(glyph: char) -> Option<u8> {
    Some(match glyph {
        ' ' => 0b0000,
        '▗' => 0b0001,
        '▖' => 0b0010,
        '▄' => 0b0011,
        '▝' => 0b0100,
        '▐' => 0b0101,
        '▞' => 0b0110,
        '▟' => 0b0111,
        '▘' => 0b1000,
        '▚' => 0b1001,
        '▌' => 0b1010,
        '▙' => 0b1011,
        '▀' => 0b1100,
        '▜' => 0b1101,
        '▛' => 0b1110,
        '█' => 0b1111,
        _ => return None,
    })
}

/// Block art as pixels: two columns and two rows of quadrants per glyph.
struct Grid {
    width: usize,
    height: usize,
    lit: Vec<bool>,
}

impl Grid {
    /// The pixels `art` lights, a row of glyphs per string. A glyph that
    /// is no quadrant pattern lights nothing (the catalog has none —
    /// `every_mascot_glyph_is_a_quadrant_pattern`).
    fn of(art: &[&str]) -> Self {
        let width = 2 * art.iter().map(|row| row.chars().count()).max().unwrap_or(0);
        let height = 2 * art.len();
        let mut lit = vec![false; width * height];
        for (row, glyphs) in art.iter().enumerate() {
            for (column, glyph) in glyphs.chars().enumerate() {
                let bits = quadrants(glyph).unwrap_or(0);
                for (bit, dx, dy) in [
                    (0b1000, 0, 0),
                    (0b0100, 1, 0),
                    (0b0010, 0, 1),
                    (0b0001, 1, 1),
                ] {
                    if bits & bit != 0 {
                        lit[(2 * row + dy) * width + 2 * column + dx] = true;
                    }
                }
            }
        }
        Self { width, height, lit }
    }

    /// Whether the pixel at column `x`, row `y` is lit — `false` off the art.
    fn lit(&self, x: usize, y: usize) -> bool {
        x < self.width && y < self.height && self.lit[y * self.width + x]
    }

    /// The lit pixels, row by row.
    fn pixels(&self) -> Vec<(usize, usize)> {
        (0..self.height)
            .flat_map(|y| (0..self.width).map(move |x| (x, y)))
            .filter(|&(x, y)| self.lit(x, y))
            .collect()
    }

    /// The art's eyes, row by row: the unlit pixels the outside can't reach
    /// through other unlit pixels — a flood from a one-pixel margin around
    /// the art — so a notch open to the outside is no eye and a sealed hole
    /// is one, whatever its shape.
    fn eyes(&self) -> Vec<(usize, usize)> {
        // The flood runs on margin coordinates: the art's (x, y) is (x + 1, y + 1).
        let (width, height) = (self.width + 2, self.height + 2);
        let lit = |x: usize, y: usize| x > 0 && y > 0 && self.lit(x - 1, y - 1);
        let mut outside = vec![false; width * height];
        let mut flood = vec![(0, 0)];
        while let Some((x, y)) = flood.pop() {
            if outside[y * width + x] || lit(x, y) {
                continue;
            }
            outside[y * width + x] = true;
            if x > 0 {
                flood.push((x - 1, y));
            }
            if x + 1 < width {
                flood.push((x + 1, y));
            }
            if y > 0 {
                flood.push((x, y - 1));
            }
            if y + 1 < height {
                flood.push((x, y + 1));
            }
        }
        (0..self.height)
            .flat_map(|y| (0..self.width).map(move |x| (x, y)))
            .filter(|&(x, y)| !self.lit(x, y) && !outside[(y + 1) * width + x + 1])
            .collect()
    }

    /// The box the lit pixels fill, in CSS pixels: `(x, y, width, height)`
    /// — `None` for art that lights nothing.
    fn bounds(&self) -> Option<(i32, i32, i32, i32)> {
        let pixels = self.pixels();
        let left = pixels.iter().map(|&(x, _)| x).min()?;
        let right = pixels.iter().map(|&(x, _)| x).max()? + 1;
        let top = pixels.iter().map(|&(_, y)| y).min()?;
        let bottom = pixels.iter().map(|&(_, y)| y).max()? + 1;
        Some((
            css(left) * PIXEL,
            css(top) * PIXEL_HEIGHT,
            css(right - left) * PIXEL,
            css(bottom - top) * PIXEL_HEIGHT,
        ))
    }
}

/// A pixel coordinate as an SVG one — the art is a handful of glyphs wide,
/// so it always fits.
fn css(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// SVG path data for `pixels` (row by row), each run of neighbours along a
/// row one rectangle, so no seam can show between them.
fn path(pixels: &[(usize, usize)]) -> String {
    let mut data = String::new();
    let mut at = 0;
    while let Some(&(x, y)) = pixels.get(at) {
        let mut run = 1;
        while pixels.get(at + run) == Some(&(x + run, y)) {
            run += 1;
        }
        let (left, top, width) = (css(x) * PIXEL, css(y) * PIXEL_HEIGHT, css(run) * PIXEL);
        let _ = write!(data, "M{left} {top}h{width}v{PIXEL_HEIGHT}h-{width}z");
        at += run;
    }
    data
}

/// A sparkle: a plus of five `cell`-sized squares, its box's corner at
/// (`x`, `y`).
fn plus(x: i32, y: i32, cell: i32) -> String {
    let c = cell;
    format!(
        "M{} {y}h{c}v{c}h{c}v{c}h-{c}v{c}h-{c}v-{c}h-{c}v-{c}h{c}z",
        x + c
    )
}

/// The mascot as an inline SVG: washed across its width in the tone's
/// colours (the stylesheet's `--wash-a` → `--wash-b`), eyelids over its
/// eyes for the blink, and on a success the four sparkles its hop throws,
/// two by its crest and two by its feet. Decorative — the headline says
/// what it shows — so screen readers skip it.
fn mascot_svg(art: &[&str], tone: Tone) -> String {
    let grid = Grid::of(art);
    let Some((x, y, width, height)) = grid.bounds() else {
        return String::new();
    };
    let mut svg = format!(
        "<svg class=\"mascot\" viewBox=\"{x} {y} {width} {height}\" width=\"{width}\" \
         height=\"{height}\" shape-rendering=\"crispEdges\" aria-hidden=\"true\">\
         <defs><linearGradient id=\"wash\" gradientUnits=\"userSpaceOnUse\" x1=\"{x}\" \
         x2=\"{}\"><stop offset=\"0\"/><stop offset=\"1\"/></linearGradient></defs>\
         <path fill=\"url(#wash)\" d=\"{}\"/>",
        x + width,
        path(&grid.pixels())
    );
    let eyes = grid.eyes();
    if !eyes.is_empty() {
        let _ = write!(
            svg,
            "<path class=\"lid\" fill=\"url(#wash)\" d=\"{}\"/>",
            path(&eyes)
        );
    }
    if tone == Tone::Success {
        let (right, bottom) = (x + width, y + height);
        svg.push_str("<g>");
        for (sx, sy, cell) in [
            (x - 40, y - 8, 9),
            (right + 14, y + 4, 7),
            (x - 25, bottom - 17, 6),
            (right + 6, bottom - 28, 8),
        ] {
            let _ = write!(svg, "<path class=\"spark\" d=\"{}\"/>", plus(sx, sy, cell));
        }
        svg.push_str("</g>");
    }
    svg.push_str("</svg>");
    svg
}

/// The tab's icon: the mascot alone, centred in a square box and washed in
/// the tone's fixed colours, as a `data:` URL safe inside a double-quoted
/// attribute.
fn favicon(art: &[&str], tone: Tone) -> String {
    let grid = Grid::of(art);
    let Some((x, y, width, height)) = grid.bounds() else {
        return String::new();
    };
    let side = width.max(height);
    let (from, to) = tone.favicon_wash();
    let svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' viewBox='{} {} {side} {side}' \
         shape-rendering='crispEdges'><linearGradient id='w' gradientUnits='userSpaceOnUse' \
         x1='{x}' x2='{}'><stop stop-color='{from}'/><stop offset='1' stop-color='{to}'/>\
         </linearGradient><path fill='url(#w)' d='{}'/></svg>",
        x - (side - width) / 2,
        y - (side - height) / 2,
        x + width,
        path(&grid.pixels())
    );
    let mut url = String::from("data:image/svg+xml,");
    for byte in svg.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~!$'()*+,;=:@/".contains(&byte) {
            url.push(char::from(byte));
        } else {
            let _ = write!(url, "%{byte:02X}");
        }
    }
    url
}

// ---------------------------------------------------------------------------
// the sky

/// How many stars the sky holds.
const STARS: usize = 40;

/// The sky's own box, scaled to cover the window (`slice`), its middle the
/// window's.
const SKY_WIDTH: u32 = 1600;
const SKY_HEIGHT: u32 = 1000;

/// One star: where, how big, how bright (an opacity in hundredths), and
/// whether it twinkles.
struct Star {
    x: u32,
    y: u32,
    size: u32,
    alpha: u32,
    twinkles: bool,
}

/// The stars, the same on every page: a fixed seed through a small linear
/// congruential generator, every fifth star twinkling, none in the
/// clearing.
fn stars() -> Vec<Star> {
    let mut seed: u32 = 0x2545_F491;
    let mut next = |n: u32| {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) % n
    };
    let mut stars = Vec::with_capacity(STARS);
    while stars.len() < STARS {
        let (x, y) = (next(SKY_WIDTH), next(SKY_HEIGHT));
        let size = if next(4) == 0 { 3 } else { 2 };
        let alpha = 12 + next(34);
        if !in_clearing(x, y) {
            let twinkles = stars.len() % 5 == 0;
            stars.push(Star {
                x,
                y,
                size,
                alpha,
                twinkles,
            });
        }
    }
    stars
}

/// Whether (`x`, `y`) is in the clearing the words sit in — an ellipse
/// about the sky's middle, raised a little to where the page's content
/// sits — where a star would read as a speck of dust on the page.
fn in_clearing(x: u32, y: u32) -> bool {
    let (dx, dy) = (i64::from(x) - 800, i64::from(y) - 480);
    let (rx, ry) = (520, 300);
    dx * dx * ry * ry + dy * dy * rx * rx < rx * rx * ry * ry
}

/// The sky as an inline SVG behind the page.
fn sky() -> String {
    let mut svg = format!(
        "<svg class=\"sky\" viewBox=\"0 0 {SKY_WIDTH} {SKY_HEIGHT}\" \
         preserveAspectRatio=\"xMidYMid slice\" aria-hidden=\"true\">"
    );
    for star in stars() {
        let class = if star.twinkles { " class=\"tw\"" } else { "" };
        let _ = write!(
            svg,
            "<rect{class} x=\"{}\" y=\"{}\" width=\"{size}\" height=\"{size}\" opacity=\".{:02}\"/>",
            star.x,
            star.y,
            star.alpha,
            size = star.size
        );
    }
    svg.push_str("</svg>");
    svg
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(tone: Tone) -> Page<'static> {
        Page {
            tone,
            title: "Authorization received",
            lead: "You can close this tab and return to Alter Zero.",
            restart: None,
            detail: None,
        }
    }

    const TONES: [Tone; 3] = [Tone::Success, Tone::Refused, Tone::Notice];

    /// `html` with every quoted `data:` URL's contents taken out.
    fn without_data_urls(html: &str) -> String {
        let mut page = String::new();
        let mut rest = html;
        while let Some(at) = rest.find("\"data:") {
            page.push_str(&rest[..=at]);
            rest = &rest[at + 1..];
            rest = &rest[rest.find('"').unwrap_or(rest.len())..];
        }
        page.push_str(rest);
        page
    }

    /// The value of the response head's `name` header.
    fn header<'a>(head: &'a str, name: &str) -> &'a str {
        head.lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(": "))
            .unwrap_or_else(|| panic!("no {name} header in {head}"))
    }

    /// Every glyph any mascot is drawn with decodes to its quadrants: a
    /// mascot drawn with a glyph the page can't read would come out with
    /// pixels missing.
    #[test]
    fn every_mascot_glyph_is_a_quadrant_pattern() {
        for mascot in Mascot::ALL {
            for row in mascot.art() {
                for glyph in row.chars() {
                    assert!(
                        quadrants(glyph).is_some(),
                        "{} is drawn with {glyph:?}",
                        mascot.name()
                    );
                }
            }
        }
    }

    /// The page's mascot is the creature the terminal banner draws — four
    /// crest spikes, a head with two eyes, a body, four legs — read off the
    /// same art, one pixel per quadrant.
    #[test]
    fn the_mascot_decodes_to_the_banner_creature() {
        let grid = Grid::of(Mascot::Crest.art());
        let rows: Vec<String> = (0..grid.height)
            .map(|y| {
                (0..grid.width)
                    .map(|x| if grid.lit(x, y) { '█' } else { '·' })
                    .collect()
            })
            .collect();
        assert_eq!(
            rows,
            [
                "··█···█····█···█··",
                "··██████████████··",
                "·███··██████··███·",
                "···████████████···",
                "····█·█····█·█····",
                "··················",
            ]
        );
    }

    /// The eyes are the holes the outside can't reach — what the eyelids
    /// close over when the mascot blinks. A notch open to the outside is no
    /// eye; a hole sealed on every side is one, however wide.
    #[test]
    fn the_eyes_are_the_holes_the_outside_cannot_reach() {
        assert_eq!(
            Grid::of(Mascot::Crest.art()).eyes(),
            [(4, 2), (5, 2), (12, 2), (13, 2)]
        );
        assert_eq!(Grid::of(&["█ █", "███"]).eyes(), []);
        assert_eq!(
            Grid::of(&["▛▀▜", "▙▄▟"]).eyes(),
            [
                (1, 1),
                (2, 1),
                (3, 1),
                (4, 1),
                (1, 2),
                (2, 2),
                (3, 2),
                (4, 2)
            ]
        );
    }

    /// The mascot's box is trimmed to its lit pixels and drawn ten CSS
    /// pixels to a quadrant column, twice that to a row — the terminal
    /// cell's own 1:2 — with its eyelids over the eyes.
    #[test]
    fn the_mascot_is_drawn_in_tall_pixels_inside_its_own_box() {
        let svg = mascot_svg(Mascot::Crest.art(), Tone::Notice);
        assert!(svg.starts_with("<svg class=\"mascot\""), "{svg}");
        assert!(svg.contains(r#"viewBox="10 0 160 100""#), "{svg}");
        assert!(svg.contains(r#"width="160" height="100""#), "{svg}");
        // The first crest spike: one quadrant at column 2 of the top row.
        assert!(svg.contains("M20 0h10v20h-10z"), "{svg}");
        // The eyelids: the eye holes, two quadrants each.
        assert!(
            svg.contains(
                r#"<path class="lid" fill="url(#wash)" d="M40 40h20v20h-20zM120 40h20v20h-20z"/>"#
            ),
            "{svg}"
        );
        assert!(svg.contains(r#"aria-hidden="true""#), "{svg}");
    }

    /// Only a sign-in that got its answer celebrates.
    #[test]
    fn only_a_success_sparkles() {
        let sparks = |tone| {
            mascot_svg(Mascot::Crest.art(), tone)
                .matches("class=\"spark\"")
                .count()
        };
        assert_eq!(sparks(Tone::Success), 4);
        assert_eq!(sparks(Tone::Refused), 0);
        assert_eq!(sparks(Tone::Notice), 0);
    }

    #[test]
    fn a_page_says_what_happened_in_its_own_words() {
        let html = document(&Page {
            title: "Signed in to ChatGPT",
            ..page(Tone::Success)
        });
        assert!(html.starts_with("<!doctype html>"), "{html}");
        assert!(
            html.contains("<title>Signed in to ChatGPT · Alter Zero</title>"),
            "{html}"
        );
        assert!(html.contains("<h1>Signed in to ChatGPT</h1>"), "{html}");
        assert!(html.contains(r#"<main class="success">"#), "{html}");
        assert!(
            html.contains(
                r#"<p class="lead">You can close this tab and return to Alter Zero.</p>"#
            ),
            "{html}"
        );
        assert!(html.contains("<svg class=\"mascot\""), "{html}");
        assert!(html.contains("<svg class=\"sky\""), "{html}");
        for (tone, class) in [(Tone::Refused, "refused"), (Tone::Notice, "notice")] {
            let html = document(&page(tone));
            assert!(
                html.contains(&format!(r#"<main class="{class}">"#)),
                "{html}"
            );
        }
    }

    /// Where a sign-in starts again is named after the lead, as the command
    /// the user types.
    #[test]
    fn a_restart_command_closes_the_lead() {
        let html = document(&Page {
            lead: "The response belongs to another sign-in attempt.",
            restart: Some("/mcp"),
            ..page(Tone::Notice)
        });
        assert!(
            html.contains(
                "The response belongs to another sign-in attempt. To sign in, start again \
                 from <code>/mcp</code> in Alter Zero.</p>"
            ),
            "{html}"
        );
    }

    /// The authorization server's own words sit apart from the page's, its
    /// error code over its description — or the code alone.
    #[test]
    fn the_servers_words_sit_in_a_detail_box() {
        let with = |text| {
            document(&Page {
                detail: Some(Detail {
                    code: "access_denied",
                    text,
                }),
                ..page(Tone::Refused)
            })
        };
        assert!(
            with(Some("The user declined")).contains(
                r#"<div class="detail"><code>access_denied</code>The user declined</div>"#
            )
        );
        assert!(with(None).contains(r#"<div class="detail"><code>access_denied</code></div>"#));
        assert!(!document(&page(Tone::Refused)).contains("class=\"detail\""));
    }

    /// Every word on the page that came from outside the code is escaped:
    /// an authorization server's reason, a redirect's issuer.
    #[test]
    fn everything_the_page_shows_is_escaped() {
        let hostile = "<script>alert(1)</script> & \"quoted\" 'single'";
        let html = document(&Page {
            tone: Tone::Refused,
            title: hostile,
            lead: hostile,
            restart: Some(hostile),
            detail: Some(Detail {
                code: hostile,
                text: Some(hostile),
            }),
        });
        assert!(!html.contains("<script>"), "{html}");
        // The tab's title, the headline, the lead, the restart command, and
        // the detail's code and text.
        assert_eq!(
            html.matches(
                "&lt;script&gt;alert(1)&lt;/script&gt; &amp; &quot;quoted&quot; &#39;single&#39;"
            )
            .count(),
            6
        );
    }

    /// A page served by a terminal app on loopback draws offline and tells
    /// no third party about the sign-in: nothing on it is fetched.
    #[test]
    fn the_page_fetches_nothing() {
        for tone in TONES {
            let html = document(&page(tone));
            assert!(!html.contains("src="), "{html}");
            assert!(!html.contains("@import"), "{html}");
            assert!(!html.contains("<script"), "{html}");
            for (at, _) in html.match_indices("href=\"") {
                assert!(html[at + 6..].starts_with("data:"), "{}", &html[at..]);
            }
            // A data URL is the resource itself — what is inside one (the
            // favicon's reference to its own gradient) fetches nothing.
            let page = without_data_urls(&html);
            for (at, _) in page.match_indices("url(") {
                assert!(page[at + 4..].starts_with('#'), "{}", &page[at..]);
            }
        }
    }

    /// The tab's icon is the mascot in the page's colours, as a data URL
    /// that is safe inside a double-quoted attribute.
    #[test]
    fn the_favicon_is_the_mascot_in_the_pages_tone() {
        let art = Mascot::Crest.art();
        assert!(favicon(art, Tone::Success).starts_with("data:image/svg+xml,"));
        assert!(favicon(art, Tone::Success).contains("%2304a5e5"));
        assert!(favicon(art, Tone::Refused).contains("%23d20f39"));
        assert!(favicon(art, Tone::Notice).contains("%23df8e1d"));
        for tone in TONES {
            let icon = favicon(art, tone);
            assert!(!icon.contains(['"', '<', '>', '&', '#', ' ']), "{icon}");
            // The page's own path, centred in a square box.
            assert!(icon.contains("M20%200h10v20h-10z"), "{icon}");
            assert!(icon.contains("viewBox='10%20-30%20160%20160'"), "{icon}");
        }
        let html = document(&page(Tone::Refused));
        assert!(
            html.contains(&format!(
                r#"<link rel="icon" href="{}">"#,
                favicon(art, Tone::Refused)
            )),
            "{html}"
        );
    }

    /// The response says what the page may do — draw its own inline
    /// styles and data images, nothing else, never inside a frame — and
    /// keeps the redirect's address, which carries the authorization code,
    /// out of any request the page could make.
    #[test]
    fn the_response_locks_the_page_down() {
        let reply = response("200 OK", &page(Tone::Success));
        let (head, body) = reply.split_once("\r\n\r\n").expect("a head");
        assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "{head}");
        assert_eq!(header(head, "Content-Type"), "text/html; charset=utf-8");
        assert_eq!(header(head, "Cache-Control"), "no-store");
        let policy = header(head, "Content-Security-Policy");
        assert!(policy.starts_with("default-src 'none';"), "{policy}");
        assert!(policy.contains("style-src 'unsafe-inline'"), "{policy}");
        assert!(policy.contains("img-src data:"), "{policy}");
        assert!(policy.contains("frame-ancestors 'none'"), "{policy}");
        assert!(!policy.contains("script-src"), "{policy}");
        assert_eq!(header(head, "Referrer-Policy"), "no-referrer");
        assert_eq!(header(head, "X-Content-Type-Options"), "nosniff");
        assert_eq!(header(head, "Connection"), "close");
        // Content-Length counts bytes, the page's non-ASCII included.
        assert_eq!(
            header(head, "Content-Length").parse::<usize>().unwrap(),
            body.len()
        );
        assert_eq!(body, document(&page(Tone::Success)));
    }

    /// The sky is the same on every page and keeps its stars out of the
    /// clearing the words sit in, where one would read as a speck of dust.
    #[test]
    fn the_sky_is_fixed_and_keeps_the_words_clear() {
        let sky = stars();
        assert_eq!(sky.len(), STARS);
        assert!(
            sky.iter().all(|star| !in_clearing(star.x, star.y)),
            "a star in the clearing"
        );
        let again: Vec<_> = stars().iter().map(|s| (s.x, s.y)).collect();
        assert_eq!(sky.iter().map(|s| (s.x, s.y)).collect::<Vec<_>>(), again);
        assert!(in_clearing(800, 480));
        assert!(!in_clearing(0, 0));
    }

    /// Every motion on the page stops for a reader who asked for less.
    #[test]
    fn motion_gives_way_to_reduced_motion() {
        assert!(STYLE.contains("@media (prefers-reduced-motion: reduce)"));
        assert!(STYLE.contains("animation: none !important"));
    }
}
