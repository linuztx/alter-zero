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

A markdown link's target rides the same escape — and where the terminal is
known to open it, the target no longer needs to be on screen at all.

## Labelled links

A reply's `[label](url)` has two presentations. Which one a session uses is
decided once, at startup, by the terminal it finds itself in.

**The label alone**, where the terminal is known to open hyperlinks:

```
  Source: FIDE
```

`FIDE` wears the link dress — the link colour, underlined — and carries the
whole address as the hyperlink a click opens. The address is not on screen.

**`label (url)`**, everywhere else:

```
  Source: FIDE
  (https://www.fide.com/geneva-to-host-fide-world-championship-match-2026/)
```

The label in its own dress, then the address in the link dress between
parentheses; both carry the target. This was the only presentation, and it
is the report this section answers: a sourced reply spent a row and a half
per source on an address nobody reads, and a table with a link in a cell
was sized to the URL.

### Why it is the terminal's call

An address hidden where nothing opens it is simply gone — not on screen, not
clickable, not recoverable. So the target is hidden only where something is
*known* to open it, and every case that is not a known yes is a no.
`links::target_display_from_env` decides, pure over the environment:

1. With `ALTER_ZERO_HYPERLINKS` off no hyperlink is written at all, so the
   address is shown — whatever the override below says.
2. `ALTER_ZERO_LINK_TARGETS` decides when it says `hide` or `show`.
3. Under a multiplexer (`TMUX`, `STY`, `ZELLIJ`, or a `tmux`/`screen`
   `TERM`) the address is shown, as on a `dumb` or `linux` console. tmux
   forwards a hyperlink only to a terminal its own `terminal-features` list
   says takes one, screen never does, and the variables naming the terminal
   the session was *started* from say nothing of the one attached now.
4. The innermost thing drawing is the one that counts, and whatever is
   started inside a terminal inherits that terminal's variables. So an
   **editor's own terminal** — Neovim's and Vim's `:terminal` (`NVIM`,
   `VIM_TERMINAL`), an Emacs terminal buffer (`INSIDE_EMACS`), a JetBrains
   IDE's (`TERMINAL_EMULATOR`) — is unknown whatever surrounds it, and a
   terminal that names itself in `TERM_PROGRAM` is judged by that name
   alone: an unknown name is unknown however much of kitty is still in the
   environment.
5. Otherwise the variables and `TERM` values the known terminals set for
   their own children decide; with none of them, the address is shown.

| Hidden for | Named by |
| --- | --- |
| kitty | `TERM` holding `kitty`, `KITTY_WINDOW_ID`, `TERM_PROGRAM=kitty` |
| Ghostty | `TERM` holding `ghostty`, `GHOSTTY_RESOURCES_DIR`, `TERM_PROGRAM=ghostty` |
| WezTerm | `TERM=wezterm`, `WEZTERM_VERSION`, `TERM_PROGRAM=WezTerm` |
| iTerm2 | `TERM_PROGRAM=iTerm.app`, `ITERM_SESSION_ID` |
| VS Code's terminal | `TERM_PROGRAM=vscode` |
| Alacritty | `TERM=alacritty`, `ALACRITTY_WINDOW_ID`, `ALACRITTY_SOCKET` |
| foot | `TERM=foot`, `foot-extra` |
| GNOME Terminal and the VTE family | `GNOME_TERMINAL_SCREEN`, `VTE_VERSION` ≥ 5000 (0.50, the first that read the escape) |
| Windows Terminal | `WT_SESSION` |
| a herdr pane | `TERM_PROGRAM=herdr` (`docs/herdr.md` *Links in a pane*) |

Left off on purpose: Apple's Terminal, which does not read the escape;
Konsole, which reads it only once its profile allows it (*Allow escape
sequences for links*); and any name not listed. A wrong yes hides a URL; a
wrong no only keeps it on screen. For a
terminal the list gets wrong either way — a tmux configured to forward
hyperlinks, a terminal with hyperlinks switched off in its settings —
`ALTER_ZERO_LINK_TARGETS=hide` or `=show` is the user's word, and it is
final.

The choice is a per-thread cell the boundary sets before the first row is
rendered (`links::set_target_display`, the active theme's arrangement), and
its default is **shown**: a caller that never learned the terminal opens
hyperlinks — a unit test, a headless run — never hides the only usable URL.
It does not change mid-session, so no row cache keys on it.

### What is never hidden

Even where the terminal opens hyperlinks, `ui::inline`'s `hides_target`
keeps the address on screen — the `label (url)` shape, exactly — when hiding
it would cost the reader something:

- **The target is not a web address.** Only a whole `http://`/`https://`
  URL is hidden. `[the notes](docs/links.md)`, an `#anchor`, `mailto:`,
  `file://` and every other scheme stay `the notes (docs/links.md)`: there
  the target is the information, it rides the escape as authored (only the
  file tool header resolves a path to an absolute `file://` URI, since only
  it is handed the session's cwd), and nothing here can promise a click on
  it opens anything.
- **The target is too long for a hyperlink to carry.** VTE and iTerm2 drop
  an OSC 8 whose URI is past 2083 bytes and print its text unlinked
  (`links::HYPERLINK_URI_MAX_BYTES`, counted as written — a non-ASCII byte
  is three). Such a target is still marked, for a terminal with no cap, but
  it must not be the only place the address is.
- **The label shows another address.** A hidden target is taken on trust,
  and the label is all there is to trust. A label reading
  `https://bank.example` over a link to `https://evil.example/login` keeps
  `https://bank.example (https://evil.example/login)` on screen. A label
  that *is* its target, give or take a closing `/`, shows once; letter case
  counts, since `https://bit.ly/AbCd` and `https://bit.ly/aBcD` are two
  pages. The address is read two ways (`shows_another_address`), because
  either alone can be written around:
  - off the label's text **as the reader sees it** — its runs joined into
    one string, every character that never reaches the screen left out
    (`is_seen`: one that takes no column, and a control character, which a
    terminal acts on or drops and draws neither). A label can spell an
    address with the scheme split by emphasis or a code span
    (`` ht`tps://bank.example` ``, which the link dress then paints as one
    unbroken address), in an image's alt text, or around a zero-width space
    or a control character: no single run holds an address then, and the
    reader sees one all the same. An address needs the `:` of its scheme,
    so a label without one — nearly every label — is told apart before any
    text is put together;
  - off the **carriers** its runs already wear: an autolinked address
    (`<me@bank.example>`) is no `http(s)://` text to find, but it was marked
    with its own target as the label rendered.
- **The link could not be marked.** The interner holds eight million
  targets; past that a label has no hyperlink behind it.

### The same on every terminal

Two links read alike in both presentations, because the target written
after the label would add nothing:

- **A link with no label to see** — `[](url)`, a label of spaces or of
  characters that never reach the screen (zero-width ones, control
  characters), an image with no alt text — is its target, in the link
  dress. It used to print ` (url)`: a parenthesis around the only thing on
  screen.
- **A label that is its own web address** — `[https://x.com](https://x.com/)`,
  give or take the closing `/`, which is how a model often writes a link —
  is written once. It used to print the address and then the same address
  in parentheses. A label that only *holds* its target
  (`see https://x.com now`) is still a label, and a target that is no web
  address keeps its `name (target)` shape whatever the name is (*The
  decoration parens*).

### The label's dress

Every run of a hidden target's label takes the link colour and the
underline **over** whatever emphasis it has — bold stays bold, italic
italic — and a code span in a label gives up its own colour: one link, one
colour.

A label of several words is **one** link. `wrap_inline` rebuilds a row from
its words and puts single spaces back between them; those spaces used to be
plain, which for an underlined label means holes in the line under it and a
hyperlink per word, the spaces between opening nothing. `tokenize_words` now
records, per word, the style of the space in front of it (`Word::joiner`):
plain, except where the whitespace it replaces was written inside a link
whose words are on both sides of it. A space the author left at the *edge* of
a label (`[ docs ](…)`) is the gap to the prose beside it and stays prose,
and the space between two separate links to one target belongs to neither.
The other presentation gains the same thing — its label paints as one
hyperlink rather than one per word.

### The address in text

Hiding the address is a property of a screen that can open it. What leaves
the screen as text keeps it: `/export` renders with the address shown
whatever the terminal (`ui::export_text` — text has nothing behind it, and a
style is exactly what an export drops), `/copy` copies the reply's markdown
source, and the Ctrl+D context view and the rollout hold the source too.

### The decoration parens

In the `label (url)` presentation the ` (` and `)` around the shown target
are **decoration this renderer adds** — they are in neither the source line
nor the URL — so they wear the surrounding prose's dress and carry no
target. They used to take the link's own blue + underline, which read as one
continuous link out to the closing paren while a click, which only the
*marked* run carries, opened the URL without them: the reported bug. Giving
them back to the prose is also what makes the shapes agree — a bare URL that
prose happens to wrap in parens (`(see https://x.com/y)`) always read that
way, since `find_urls` hands the `)` back under its unbalanced-closer rule,
and a `● Read({path})` header's parens have always stayed plain.

So `` [`AGENTS.md`](AGENTS.md) `` shows `AGENTS.md (AGENTS.md)` on every
terminal — its target is no web address: the label in the inline-code
colour, the shown target in link blue + underline, the parens (and any
quotes around the whole thing) in the reply's own prose colour, and exactly
those two names carrying the verbatim `AGENTS.md` target.

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
   - `is_whole_url` answers the other question: is a text, whole, *one*
     URL — the scheme at its first byte and nothing in it a URL cannot hold
     (the same body rule: no whitespace, control or `<>"`` ` ``|`)? Such a
     text is linked **verbatim**, since the tail trimming above is prose's
     rule — a sentence owns its closing `.`, a parenthetical its `)` — and
     a text that *is* the URL has no prose to give anything back to. Only
     `http://`/`https://` qualify, so no other scheme becomes a target this
     way.
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
   - A `Carrier` is one interned target ready to mark any number of
     styles: `carrier(url)` looks the URL up once and `Carrier::stamp` copies
     the id — what a link whose label is several styled runs uses, where a
     `linked` per run hashed the whole URL again for each.
   - The **framing**: `osc8_open(id, url)` / `OSC8_CLOSE` build the escape,
     percent-encoding every byte a terminal could mis-parse (controls, space,
     DEL, non-ASCII — the kitty spec's rule) as escape-injection defence in
     depth.
   - `TargetDisplay` (`Shown` / `Hidden`) and its per-thread cell
     (`target_display`, `set_target_display`, `with_target_display`), with
     `target_display_from_env`, the pure decision *Labelled links* lists.
2. **`src/ui/inline.rs` — the marking.** `inline_spans` is the one funnel all
   assistant prose runs through (plain lines, list items, blockquotes, *and*
   table cells), so marking there covers every surface that can wrap a URL:
   - `Inline::Text` autolinks: the URL slice takes the markdown-URL dress
     (`link_url_color()` + underline — a URL is a URL) plus the carrier.
   - `Inline::Link { text, url }` is `link_spans`: the label alone in the
     link dress where `hides_target` says so, else the label (its visible
     style untouched) and the `url` inside the ` (url)` suffix — every run
     marked with the target either way, from one `Carrier`.
   - `tokenize_words` gives each word the style of the space in front of it
     (`Word::joiner`), so `wrap_inline` keeps a label of several words one
     underlined, clickable run (*The label's dress*).
   - `Inline::Code` also marks bare URLs, but preserves its literal text and
     code colour — no blue/underline override. Previously, putting a long
     token URL inside backticks silently bypassed OSC 8: only the first
     wrapped row auto-detected, and clicking lost the rest of the token.
   - `linkify_code_segments` attaches targets to already-styled,
     **unwrapped** code without changing its bytes or visible styling. It
     scans the whole source line, then intersects URL byte ranges with its
     style spans, so syntax highlighting that splits a scheme, path, or query
     still gives every piece the same full target. Code detection
     (`links::find_code_urls`) respects an opening quote, so `').json()` and
     adjacent quoted strings are never swallowed into the target. A quoted
     URL or one starting a code literal keeps trailing punctuation exactly
     (`?token=abc!` includes `!`); the decision depends only on text before
     the URL, so later streamed code cannot change an already-committed
     target. Escapes and interpolations in source strings are not evaluated.
     `AssistantRenderer` marks code before `code_content_rows`; code spacing
     and syntax colours stay intact. Separate source lines are never joined
     into a guessed URL.
   - A **heading's** text is inline markdown like prose (`docs/markdown.md`
     *Headings*), so a bare URL in it takes the prose dress and carrier
     through `inline_spans`, under the heading's modifiers; adjacent
     equal-style heading spans are merged so combining marks stay attached to
     their preceding glyph. An **autolink** (`<https://…>`) is
     `Inline::Url`: its bare URL — the angle brackets dropped — in the link
     dress, carrying its target (`mailto:` for an address).
   - Non-markdown roles (user/system/shell text) and tool output still rely
     on the terminal's detection of unwrapped URLs.
3. **`src/ui/model_view.rs` — the picker family's marking.** Assistant prose
   is not the only place a URL is shown: the `/login` sign-in pages print one
   the user is meant to *act* on. `model_linked_rows` is
   `model_wrapped_rows`'s linking sibling — it marks the URLs in the
   **unwrapped** text and wraps through `inline::wrap_inline`, so the carrier
   survives the hard break exactly as it does in prose. It differs from the
   prose funnel in one way on purpose: it keeps the **caller's colour** and
   adds only the underline, because these pages light their URL deliberately
   (the device page dims it so the code in its box stays what the eye lands
   on — `docs/copilot.md`) and the chat link dress would overrule that. The
   browser sign-in page then needs no verb in front of its URL at all
   (`docs/chatgpt.md`): the link is the affordance.

   The `/mcp` manager's sign-in walk (`docs/mcp.md`) shows URLs too, and used
   to print them as plain rows — so its **Authenticate** page's authorize
   URL, hundreds of characters wide and always hard-broken, opened only its
   first row's fragment when clicked and nothing at all from the rows under
   it, while the same click on `/login`'s page worked: the reported bug. It
   now goes through the family's builders, three of them:
   - `model_url_rows` for a text that **is** one URL
     (`links::is_whole_url`): `model_linked_rows`' dress, but the whole
     text is the target, verbatim (`whole_url_run`). The authorize URL is
     the one thing the page shows, the URL `c` copies and the browser was
     handed, so the click must open exactly that — even where it ends on a
     `.` or an unbalanced `)` that prose detection would give back to a
     sentence that isn't there. It keeps the accent the page always lit it
     in, underlined.
   - `whole_url_run` on the **server page's `URL:` row**, which wraps under
     its label (`field_rows` lays the wrapped spans out, so the row reads
     exactly as it did); a stdio server's `Command:` row is no address and
     stays plain, as does any value that is not one whole URL.
   - `model_linked_rows` for the two **reasons** the walk shows — a paste
     the sign-in turned away (an authorization server's `error_uri`) and a
     server that failed to connect (a console to enable an API in): a URL
     inside the prose is linked, the reason's own colour kept, the `/login`
     failures' rule.

   The tool pages' server-written text (a description, a parameter's) is
   not linked, the *Limitations* rule for tool output.
4. **`src/term.rs` — the emission.** All four cell-writing paths — scrollback
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
5. **`src/ui/tool.rs` — the file tool header.** A `● Read/Write/Edit({path})`
   header's path is a link to the **file**: `tool_header_lines` stamps the
   path's spans with `links::file_url` — the file's *absolute* `file://` URI
   (a relative argument resolved against the session's cwd through
   `app::PathDisplay::file_url`, `.`/`..` collapsed, every byte outside `/`
   and `A-Za-z0-9-._~` percent-encoded, `Path.as_uri()`'s rule, so a space
   or a `#` in a name can never read as a delimiter) — whatever short form
   the row paints (`cat_poem.txt`, `~/hello.py`, `/tmp/x.py`; `docs/tools.md`
   *Path display*). The target is stamped **before** the header wraps, so a
   long path hard-broken across continuation rows, or cut to the collapsed
   header's column budget, opens the whole file from any fragment. **Only the
   path**: the `(`/`)` framing it, the bullet and the name stay plain, and the
   `⎿ Wrote 21 lines to cat_poem.txt` / `⎿ Updated cat_poem.txt (+4 -4)` /
   `⎿ Read image (PNG, …)` corner rows beneath are never linked — the header
   says *which* file, and is the one row a click should open. The visible
   text and style are byte-identical (the link wears the args' own bold
   white; a supporting terminal hover-underlines it), a `Bash`/`Agent`/MCP
   header carries nothing, and a path the policy cannot place — a relative
   argument with no cwd, the verbatim policy's one gap — paints as before,
   unlinked. Every surface sharing the header builder links alike: the
   inline cell, the live strip's running cell (a `Write` waiting on the
   permission gate), the Ctrl+O transcript and a subagent session view.

`ALTER_ZERO_HYPERLINKS` gates it (default **on**; a falsy value — `0`,
`false`, `no` — turns emission off for a terminal that misbehaves, the
`ALTER_ZERO_DISABLE_KEYBOARD_ENHANCEMENT` pattern). The gate is read once at
`InlineViewport::init`; disabled still strips the carrier — the id must never
paint as a colour — it just writes no OSC.

## Invariants held

- **The escape is invisible; the dress is scoped.** An OSC sequence is
  rendered as nothing, so marking a run moves no character: a bare URL, a
  code URL and a `label (url)` link wrap exactly as they did before they
  were clickable. The dress follows the click: bare URLs took the link blue
  + underline a ` (url)` target always had, a markdown target's decoration
  parens gave that dress back, and a label standing in for its target wears
  it whole, spaces included — so the run the eye reads as the link and the
  run a click opens are the same run everywhere.
- **A hidden address is one something opens.** The label-only presentation
  changes the row text, and it is used only where the terminal is known to
  open the hyperlink, the target is a web address a hyperlink can carry, and
  the label names no other address (*Labelled links*). Everything else — an
  unknown terminal, a multiplexer, `ALTER_ZERO_HYPERLINKS=0`, an export —
  keeps the address on screen.
- **Prefix-stability untouched** (invariant 2): marking happens inside a
  line's own render — `find_urls` never looks across lines — so a completed
  line's rows still never change. The strip's preview of a *partial* URL
  links the partial (it interns a few short-lived ids as it grows — bounded,
  bytes-cheap); the committed row links the whole thing. Within the
  **trailing, still-growing** line, detection is retroactive — `ht` is plain
  prose until enough of `http://…` arrives to restyle the whole word, and a
  URL whose body reaches the line's end keeps growing (trimmed tail
  punctuation re-joins: `http://e.` → `http://e.com`) — so
  `links::has_forming_url` reports both shapes and `StreamRender::commit`
  withholds the line while one holds (the `markdown::has_open_inline`
  pattern; a stress-fuzz find — a plain-styled `h` row committed at a narrow
  width used to repaint blue+underlined). A URL already sealed by a stopper,
  and a mid-word fragment the scheme boundary rule would never link, stay
  settled and commit progressively. Whether a link's target is hidden is
  decided by the **finished** link — its target and its label — and an
  unfinished one (`[label](https://…` with no `)` yet) is already withheld
  whole by `markdown::has_open_inline`, so no committed row is restyled when
  the `)` lands.
- **The three `mod.rs` facades are untouched**: `links` is a new top-level
  pure module (like `markdown`), reachable by `ui` and `term` without
  widening the `ui` surface `tests/api_surface.rs` locks.

## Testing

The pure core (detection, interner, carrier round-trip, framing/encoding,
run grouping, the env predicate) is unit-tested in `links.rs`; the marking
and the *wrapped-fragments-still-carry-the-whole-URL* regression are
unit-tested in `ui`. A synthetic token URL is rendered at several narrow
widths in plain prose, inline code, fenced/highlighted and indented code,
headings, lists, quotes, and tables. Tests also split URLs across every
possible syntax-style boundary, preserve Unicode and spacing, and compare
every streamed prefix including carriers against batch rendering so no
partial target commits. Quote-delimited code URLs, punctuation-ending
literals, carrier-only diff repaints, and combining marks in painted
headings have explicit regressions. The decoration parens have both halves
too — the segments' own dress in `ui/tests/inline.rs`, and the whole
`` "[`AGENTS.md`](AGENTS.md)" `` render through `message_lines` in
`ui/tests/assistant.rs`, each over several widths, since the wrap
re-coalesces a row's spans by style and a paren that kept the link dress
would come back as its own underlined span.

`term.rs` is the I/O boundary, so `smoke.sh` **Phase 87** drives
the real binary in a pane narrow enough to split the dummy reply's
`https://github.com/linuztx`, captures the raw byte stream with
`tmux pipe-pane`, and asserts the full URL rides an OSC 8 open while the
visible pane shows the split text — and that `ALTER_ZERO_HYPERLINKS=0` emits
none. The file tool header has the same two halves: `links::file_url`'s
resolution and encoding are unit-tested in `links.rs`, the header's stamp —
the path linked, the parens and corner rows not, every wrapped or cut
fragment carrying the whole file, `Bash`/`Agent` headers plain, the live
cell and the transcript alike — in `ui/tests/tool.rs`, and **Phase 113**
reads the `file://` open for the dummy's `● Read(about.py)` off the raw
stream while the pane shows the unchanged header text.

The `/mcp` sign-in walk has both halves as well. `ui/tests/mcp_view.rs`
renders the Authenticate page at widths that break the authorize URL and
requires every fragment to carry the whole of it, underlined in the page's
accent, as the page's one link; requires the target to equal what `c`
copies for URLs ending on `.`, `!` and an unbalanced `)`; and links the
reasons' URLs and the server page's `URL:` row (byte-identical rows) while
a `Command:` row stays plain — `is_whole_url`'s two sides are unit-tested
in `links.rs`. **Phase 149** runs the real flow against a stub server that
demands OAuth (a 401, RFC 9728 and 8414 metadata, dynamic registration) in
a 60-column pane, and reads off the raw stream an OSC 8 open carrying the
whole authorize URL for every row the page paints it on — the URL the page
spells, and the one a stand-in `xdg-open` was handed — with no open
anywhere carrying a fragment of it.

Labelled links have the same halves. `links.rs` holds the decision: every
terminal on the list by each variable that names it, the ones kept off it,
a multiplexer over any of them, an editor's own terminal and `TERM_PROGRAM`
each outranking what an outer terminal left behind, the override both ways,
the hyperlink gate outranking the override, and the scoped cell restoring
what it found.
`ui/tests/inline.rs` holds the render: the reported `Source: [FIDE](…)` as
`Source: FIDE` with the whole address behind exactly that word; emphasis
kept under the dress and a code span's colour given up; a label of several
words as one run on its row, at widths that break it between words and
inside one; edge spaces and two links to one target left alone; each case
that keeps the address — a target that is no web address or is past the
length a hyperlink carries, a label showing another address (whole, split
across emphasis or a code span, in an image's alt text, around a zero-width
or control character), an unmarked link — as *equal to* the other
presentation, segment for segment; and the
two links that read alike in both, a label-less one as its target and a
self-labelled one written once.
`ui/tests/assistant.rs` runs the label through a paragraph, a list item, a
quote, a heading and a table cell, and requires a table to narrow to its
labels while staying a grid; `ui/tests/stream_stress.rs` drives the
streaming differential — committed rows a stable prefix of the batch
render, scrollback plus strip the reply so far, carriers compared — over
link-heavy documents and a sweep of the hostile soup under this
presentation; `ui/tests/export_view.rs` requires the export to spell the
address out while the screen shows the label. **Phase 158** plays the
dummy's markdown tour three times in a tmux pane — which the app's own rule
counts as a terminal that keeps the address: with
`ALTER_ZERO_LINK_TARGETS=hide` the pane reads `a link and a bare URL` with
the address nowhere on it, the label underlined, and the raw stream opening
a hyperlink to the whole address around exactly that word; with nothing set
the address is spelled out; and with `hide` beside
`ALTER_ZERO_HYPERLINKS=0` it is spelled out again and no escape is written.

## Cost

The label-only presentation renders less — no ` (url)` segments, fewer
wrapped rows, fewer cells inside a hyperlink — and the work that made it
possible was paid for elsewhere on the same path:

- a link's target is interned **once** (`links::carrier`) and stamped onto
  each run of its label, where every run used to hash the URL again;
- the bare-URL scan (`next_scheme`), which runs over every prose and code
  line the renderer sees, goes straight to the next `h` instead of holding
  every byte up against both schemes — 3.6× faster, which takes about 4%
  off a prose render with no link in it;
- `tokenize_words`, where every wrapped row of every reply, table cell and
  tool header starts, copies each run of a segment as one slice rather than
  pushing it a character at a time, and follows a link's spaces only on a
  line that has a link in it.

`cargo run --release --example link_probe` measures it: a 99 KB reply of
300 paragraphs (two labelled links and a bare URL each) and a 300-row table
of links, rendered whole and streamed in 20-character chunks, beside the
same reply with its links taken out. At 80 columns, best of 15, on a
four-core Linux machine, against the build before this change:

| | Before | `label (url)` | Label only |
| --- | --- | --- | --- |
| Whole render | 7.15 ms | 6.24 ms | 4.59 ms |
| Streamed, total | 1855 ms | 1776 ms | 1490 ms |
| Rows | 2403 | 2403 | 1503 |
| Cells inside a hyperlink | 78,840 | 80,640 | 27,270 |
| Hyperlink runs painted | 4200 | 2400 | 1200 |
| Escape bytes for those runs | 348 KB | 187 KB | 86 KB |
| The same reply without links, whole | 3.08 ms | 2.88 ms | |
| …streamed | 348 ms | 347 ms | |

Nothing got slower, a reply with no link in it included. The `label (url)`
presentation paints fewer runs because a label of several words is one run
now instead of one per word.

## Limitations

- The list of terminals that open hyperlinks is a list: a terminal not on
  it keeps the address on screen until `ALTER_ZERO_LINK_TARGETS=hide` says
  otherwise, and one on it with hyperlinks switched off in its own settings
  hides an address nothing opens until `=show` does. Nothing can ask a
  terminal whether it opens them, and nothing here can see a transport that
  drops the escape on its way there (mosh): `=show` is the answer to that
  too.
- A label is compared with its target as text. A look-alike domain (a
  letter borrowed from another alphabet) or a label naming a site without
  a scheme (`bank.example` over a link elsewhere) is a label like any
  other: what a click opens is what the terminal shows on hover.
- A hidden address is read by hovering or clicking — a mouse selection of
  the row copies the label, as it does in a browser. `/copy` and `/export`
  carry the address.
- OSC 8 support must be enabled in the terminal and any multiplexer between
  it and the app. If the sequences are disabled or discarded, the terminal's
  ordinary per-row auto-detection cannot recover a wrapped URL. The default
  app gate is on; check that `ALTER_ZERO_HYPERLINKS` is not falsy.
- A real newline inside a source URL is not a visual wrap: separate source
  lines stay separate rather than guessing whether to concatenate them.
- Scheme-less `www.…` text is not linked (OSC 8 needs a real URI; fabricating
  a scheme guesses).
- Tool/shell output is not linked yet — its wrap (`wrap_output`) breaks at
  word boundaries, so a URL there only splits when wider than the terminal;
  the same carrier would extend there if it earns its keep. The file tool
  **header** is the one tool surface that links (to the file, not a URL in
  the text); the `⎿` corner rows under it are deliberately plain.
- The `file://` target names the file as the session sees it: a relative
  argument resolves against the process cwd lexically, symlinks are never
  consulted, and a session `/resume`d from another directory links where the
  header now reads (the same rule the shown path follows). What the click
  *does* with a `file://` URI is the terminal's choice — kitty, Ghostty,
  WezTerm and iTerm2 hand it to the system opener, VS Code's terminal opens
  it in the editor.
