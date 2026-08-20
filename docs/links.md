# Clickable links — OSC 8 hyperlinks for wrapped URLs

## The bug

A URL wider than the room left on its row hard-breaks across display rows —
`wrap_inline`'s grapheme hard-break, the correct *visual* behaviour for a word
no row can hold (a narrow terminal, a table cell's column, a deep list
indent):

```
  the credit lives at https://github.com/linuz
  tx. That is the loop …
```

The terminal's own URL detection is what made that text clickable, and it
works on the **visible row text**: it sees `https://github.com/linuz` on one
row and an unrelated `tx.` on the next, so clicking opens the truncated URL.
No wrap policy can fix this — a URL wider than the content width *must* split
visually — so the fix has to divorce the click target from the visual wrap.

## The fix: explicit OSC 8 hyperlinks

[OSC 8](https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda)
(`ESC ] 8 ; params ; URI ST`) attaches a full target URI to whatever cells
are printed while it is open, independent of where the text sits or wraps.
Every rendered fragment of a detected URL is bracketed by the sequence, so
clicking **any** fragment opens the **whole** URL. Terminals that support it
(kitty, iTerm2, WezTerm, Ghostty, foot, VTE, Windows Terminal, tmux ≥ 3.4)
also hover-underline the fragments as one link via the `id=` parameter;
terminals that don't simply consume the sequence (ECMA-48 OSC), so the screen
is byte-for-byte what it was — the wrap, the styling, and the old
auto-detection behaviour are all untouched there.

Markdown links get the same treatment for free: `[text](url)` renders
`text (url)` exactly as before, but both the text and the shown URL now carry
the real target.

## Where the pieces live

The full URL is only known **before** wrapping splits it, and the escape can
only be written **at** the terminal — so the design threads a link identity
from the renderer to the paint boundary through the one per-cell channel that
survives the trip.

1. **`src/links.rs` — the pure core** (the `markdown` module's sibling):
   - `find_urls` detects bare `http://`/`https://` URLs in a text node: the
     scheme must open at a non-alphanumeric boundary, the body takes
     everything printable that isn't whitespace/`<`/`>`/`"`/`` ` ``/`|`, and
     the tail is trimmed of closing punctuation (`.,;:!?'"`) and of
     *unbalanced* closers (`)`/`]`/`}` — `…/Foo_(bar)` keeps its `)`, a URL
     inside `(see …)` gives it back). Control characters (an `ESC` smuggled
     into model output) terminate the URL — they can never enter a target.
   - A process-global, append-only **URL interner** (the `highlight`
     grammar-singleton precedent: a render-layer cache, not app state) maps
     each URL to a stable 24-bit id and back.
   - The **carrier**: `linked(style, url)` stamps the id into
     `Style::underline_color` as `Color::Rgb(id>>16, id>>8, id)`. Nothing
     else in the crate sets an underline colour, id `0` (plain black) is
     reserved as "not a link", and every other colour kind decodes to `None`
     — so the channel is unambiguous. The carrier survives `Span` →
     `Cell::set_style`, `Buffer::diff`, the transcript cache, and a reflow's
     re-render (the interner hands the same URL the same id), and the paint
     boundary strips it before the terminal ever sees it, so no stray
     `SGR 58` underline colour can leak.
   - The **framing**: `osc8_open(id, url)` / `OSC8_CLOSE` build the escape,
     percent-encoding every byte a terminal could mis-parse (controls, space,
     DEL, non-ASCII — the kitty spec's rule) as escape-injection defence in
     depth.
2. **`src/ui/inline.rs` — the marking.** `inline_spans` is the one funnel all
   assistant prose runs through (plain lines, list items, blockquotes, *and*
   table cells), so marking there covers every surface that can wrap a URL:
   - `Inline::Text` autolinks: the URL slice takes the markdown-URL dress
     (`LINK_URL_COLOR` + underline — a URL is a URL) plus the carrier.
   - `Inline::Link { text, url }` marks the text spans (their visible style
     untouched) and the `url` inside the ` (url)` suffix with the target.
   - `Inline::Code` stays link-free (verbatim by intent), as do fenced code
     blocks, headings, and the non-markdown roles (user/system/shell text)
     and tool output — the terminal's own detection still covers their
     unwrapped URLs exactly as before.
3. **`src/term.rs` — the emission.** All four cell-writing paths — scrollback
   commits + reflow (`draw_lines`), the live-region blit, the live-region
   diff, and the alt-screen overlay (`draw_overlay`) — already funnel into
   `Backend::draw`; they now go through one `draw_cells` choke point that
   groups consecutive cells by carrier id, passes unmarked runs through
   untouched (zero copies), and brackets each marked run with
   `osc8_open` … cells-with-the-carrier-stripped … `OSC8_CLOSE`. Covering
   *all* paths is what keeps the link alive everywhere a cell can be painted
   (a committed message, a resize rebuild, the streaming strip, the Ctrl+O
   transcript) and what keeps the carrier from ever reaching the terminal as
   a real underline colour. Hyperlinks are per printed cell in every
   terminal, so a diff repainting half a URL re-links exactly those cells and
   the rest keep theirs.

`ALTER_ZERO_HYPERLINKS` gates it (default **on**; a falsy value — `0`,
`false`, `no` — turns emission off for a terminal that misbehaves, the
`ALTER_ZERO_DISABLE_KEYBOARD_ENHANCEMENT` pattern). The gate is read once at
`InlineViewport::init`; disabled still strips the carrier — the id must never
paint as a colour — it just writes no OSC.

## Invariants held

- **Zero visual change.** The wrap, the row text, and the styling are
  byte-identical to before on every terminal; the only new bytes are OSC
  sequences terminals render as nothing. (The one deliberate dress change:
  bare URLs take the link blue + underline that ` (url)` targets always had.)
- **Prefix-stability untouched** (invariant 2): marking happens inside a
  line's own render — `find_urls` never looks across lines — so a completed
  line's rows still never change. The strip's preview of a *partial* URL
  links the partial (it interns a few short-lived ids as it grows — bounded,
  bytes-cheap); the committed row links the whole thing.
- **The three `mod.rs` facades are untouched**: `links` is a new top-level
  pure module (like `markdown`), reachable by `ui` and `term` without
  widening the `ui` surface `tests/api_surface.rs` locks.

## Testing

The pure core (detection, interner, carrier round-trip, framing/encoding,
run grouping, the env predicate) is unit-tested in `links.rs`; the marking
and the *wrapped-fragments-still-carry-the-whole-URL* regression are
unit-tested in `ui` (an `assistant_lines` render at a width that hard-breaks
the URL). `term.rs` is the I/O boundary, so `smoke.sh` **Phase 87** drives
the real binary in a pane narrow enough to split the dummy reply's
`https://github.com/linuztx`, captures the raw byte stream with
`tmux pipe-pane`, and asserts the full URL rides an OSC 8 open while the
visible pane shows the split text — and that `ALTER_ZERO_HYPERLINKS=0` emits
none.

## Limitations

- URLs inside `` `code` `` spans and fenced code blocks are not linked
  (verbatim by intent); short ones still auto-detect in the terminal.
- Scheme-less `www.…` text is not linked (OSC 8 needs a real URI; fabricating
  a scheme guesses).
- Tool/shell output is not linked yet — its wrap (`wrap_output`) breaks at
  word boundaries, so a URL there only splits when wider than the terminal;
  the same carrier would extend there if it earns its keep.
