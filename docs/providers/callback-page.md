# The browser sign-in page

Every browser sign-in comes back through one loopback callback
(`llm::loopback`, whose server `docs/tools/mcp.md` describes): an MCP server's OAuth
flow, ChatGPT's (`docs/providers/chatgpt.md`), the Anthropic Console's
(`docs/providers/claude.md`) and Google Antigravity's (`docs/providers/antigravity.md`). The page
the browser lands on there is the last thing the user sees of a sign-in
before going back to the terminal. It used to be a bare `<h3>` and one
sentence, black on white. It is now a page in the project's own dress,
built by `src/llm/loopback/page.rs`.

```
                          ▙▄▙▄▟▄▟                ← the mascot, glowing
                         ▝▜▄███▄▛▘
                           ▘▘ ▝▝

                  Authorization received         ← serif headline
     You can close this tab and return to Alter Zero.
```

## The look

Browser sign-in pages have settled on one composition: a single pixel icon,
a large serif headline and one muted sentence, centred on a dark ground with
nothing else. The page keeps that composition and dresses it in the
project's identity.

- **The icon is the mascot.** The page reads the default mascot's block art
  (`Mascot::default().art()`, the crested hatchling the startup banner and
  the README's banner draw, `docs/ui/mascot.md`) and draws it as an inline SVG,
  one pixel per quadrant. A quadrant glyph splits a terminal cell into four,
  and a cell is about twice as tall as it is wide, so each pixel is drawn
  10 CSS pixels wide and 20 tall: the creature the terminal shows, not a
  squashed one. Because the page reads the art rather than a copy of it, the
  page and the banner can't drift apart. A run of lit pixels along a row is
  one rectangle and the SVG renders with `crispEdges`, so no seam shows
  between pixels at any zoom.
- **The banner's wash.** The mascot wears the banner's cyan → blue gradient
  (Catppuccin Mocha's `sky` → `blue`, the `accent` → `link` pair the
  default theme washes its banner in) and glows on Mocha's `mantle`, the
  README banner's ground, under a faint vignette.
- **A faint pixel sky.** Forty dim pixel stars stand in for the README
  banner's night. The pattern is fixed (a seeded generator, so every page
  wears the same sky and the test needs no randomness), every fifth star
  twinkles, and none sits inside the clearing the words occupy, where a
  star reads as a speck of dust on the page.
- **Type.** The headline is the system's own serif (New York in Safari,
  Charter on macOS, Bitstream Charter on Linux, Sitka, Cambria or Georgia on
  Windows), the sentence the system's sans, and a command is a monospace
  chip in the accent, the TUI's inline-code dress.
- **Light browsers.** Under `prefers-color-scheme: light` the page takes
  Catppuccin Latte, the same roles from the light flavour, and the stars
  go: dark specks on a light ground read as dirt.

## What the page says at a glance

Each page has a **tone**, which picks the mascot's colours, how it moves and
the tab's icon (`page::Tone`):

| Tone | When | Mascot | Motion |
| --- | --- | --- | --- |
| `success` | the sign-in got its code | sky → blue | hops, squashes as it lands, and four pixel sparkles burst around it at the top of the jump |
| `refused` | the authorization server refused the sign-in (the user declined on its consent screen) | red → peach | shakes its head |
| `notice` | nothing failed and nothing was received: another sign-in's answer, an answer that came too late, a stray request | yellow → peach | tilts its head, puzzled |

On every page the mascot, the headline, the sentence and any detail rise
into place one after another, 80 ms apart. Afterwards the mascot blinks
every five and a half seconds: its eyelids close over its eyes from the top.
The eyes are found, not listed: they are the art's unlit pixels that the
outside can't reach through other unlit pixels (a flood fill from around the
art), so a redrawn mascot keeps blinking. Under
`prefers-reduced-motion: reduce` none of this runs, and every element's
resting state is its finished one, so nothing is left invisible or
mid-motion.

## What each page says

| The request | Status | Headline | Sentence |
| --- | --- | --- | --- |
| this sign-in's code | 200 | the flow's own (`Pages::received`): `Authorization received`, `Signed in to ChatGPT` | You can close this tab and return to Alter Zero. |
| this sign-in's refusal | 200 | Sign-in refused | The authorization server turned the sign-in down. Return to Alter Zero to try again. |
| not this sign-in's answer | 400 | Not this sign-in's response | the reason, as a sentence, then: To sign in, start again from `/mcp` in Alter Zero. |
| an answer after the sign-in stopped waiting | 410 | This sign-in has ended | It stopped waiting before this answer arrived. To sign in, start again from `/mcp` in Alter Zero. |
| the callback path with neither a code nor an error | 400 | Not a sign-in response | It carries neither an authorization code nor an error. |
| any other path (a hand-typed address) | 404 | Nothing here | This address only receives sign-in responses. |

The command is the flow's own (`Pages::restart`): `/mcp` for an MCP server,
`/login` for the providers. The app's name is `crate::APP_NAME`, the one
constant every string that names the app reads.

A refusal's page sets the authorization server's own words apart in a detail
box, its `error` code in red monospace over its `error_description`. That is
why `loopback::Rejected::Refused` carries a `Refusal` with the two kept apart
rather than a finished sentence: the page shows the parts, and the flow and
the terminal still read the sentence they always did
(`the authorization server refused the sign-in: access_denied (…)`,
`Refusal`'s `Display`).

## Self-contained, and locked down

- **Nothing is fetched.** No web font, no image, no script, no stylesheet
  link: the page is served by a terminal app on loopback, so it has to draw
  offline, and a font from a CDN would tell a third party about the sign-in.
  The stylesheet, the mascot, the sky and the favicon are all inline. The
  favicon is the mascot again, as an SVG `data:` URL in the tone's colours,
  taken from Latte because its saturated ends read on a dark tab strip and
  a light one alike. Inlining it also means the browser never asks the
  callback for `/favicon.ico`.
- **Everything from outside is escaped.** An authorization server's error
  and description and a redirect's issuer are HTML-escaped wherever the page
  shows them, the tab's title included.
- **The response allows only what the page uses.** Its
  `Content-Security-Policy` is `default-src 'none'` plus the page's inline
  style and `data:` images: no script at all, no form, no `<base>`, never
  inside a frame. `Referrer-Policy: no-referrer` because the address the
  browser is on carries the authorization code, and
  `X-Content-Type-Options: nosniff` beside the `Cache-Control: no-store` the
  page always had.

## Why the colours are fixed

The terminal's colours follow `/theme` (`docs/ui/theme.md`); the page's do not.
A browser can't read the terminal's palette, the `ansi` theme has no RGB
values to give it, and the page is the project's face rather than a view of
the session. Its colours are Catppuccin's own, Mocha (the default theme's
flavour) and Latte for a light browser, the flavours `ui/palette.rs`'s
`MOCHA` and `LATTE` tables are drawn from. They live in the page's
stylesheet as CSS rather than as `Color` values, so the rule against colour
literals outside `palette.rs` does not reach them. For the same reason the page draws the default mascot
rather than the directory's `/mascot` choice, which the sign-in workers
never see: it shows the project's mark.

## Tests

`llm::loopback::page::tests` pins the page: every glyph any mascot is drawn
with decodes, the default mascot decodes to the banner's creature with its
eyes where the art has holes, the SVG keeps the terminal's 1:2 pixels inside
its own box, only a success sparkles, every word from outside is escaped,
nothing on the page is fetched, the favicon wears the tone, the response
carries its policy headers and an exact `Content-Length`, the sky is fixed
and keeps the words clear, and the motion gives way to reduced motion.
`llm::loopback::tests` holds each verdict and each stray request to its
page, and the real-socket tests check what a browser actually receives.
`scripts/smoke.sh 148` signs in to Google Antigravity against a stub and
reads the callback page the real binary serves.
