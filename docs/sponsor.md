# `/sponsor` — the sponsor page

The **`/sponsor`** command opens a read-only page with the two ways to back
the developer or reach them: the GitHub Sponsors page and a contact address.
It is the `/donate` page's twin (`docs/donate.md`) — the same frame, the same
key grammar, the same short-terminal ladder — over a different catalog, for
the reader who would rather sponsor through GitHub than send a coin. The
`/discord` page (`docs/discord.md`) is this page's twin in turn, over the
one invite to the community server.

## The rows

| label             | note                                   | value                                 | opens                      |
| ----------------- | -------------------------------------- | ------------------------------------- | -------------------------- |
| `GitHub Sponsors` | monthly or one-time                    | `https://github.com/sponsors/linuztx` | the URL                    |
| `Email`           | company sponsorships and anything else | `linuztx@gmail.com`                   | `mailto:linuztx@gmail.com` |

The catalog is the pure `app::SPONSOR_LINKS` — one `SponsorLink` per row
(`label`, `note`, `value`, `target`, `noun`). A const, never a file, for
`/donate`'s reason: a page that read a sponsor link from disk is a page
anyone with write access to the config home could point somewhere else. The
catalog tests pin every value to bare ASCII, every target to end in the value
it sits behind, and every `noun` distinct, so the toast always says which
value was copied. `.github/FUNDING.yml` names the same GitHub account.

## The page

```
────────────────────────────────────────────────────────────────────────────────
  ♥ Sponsor Alter Zero

  Alter Zero is built and maintained by one developer. A sponsorship pays for
  the time that goes into it — thank you.

  ❯ 1. GitHub Sponsors  monthly or one-time
     ╭───────────────────────────────────────╮
     │  https://github.com/sponsors/linuztx  │
     ╰───────────────────────────────────────╯

    2. Email  company sponsorships and anything else
     ╭─────────────────────╮
     │  linuztx@gmail.com  │
     ╰─────────────────────╯

  ↑↓ navigate  enter/c copy  esc close
────────────────────────────────────────────────────────────────────────────────
```

- **The title** is `/donate`'s row with its own verb: the red heart, then
  `Sponsor Alter Zero` in the page titles' gradient (`title_line`).
- **The blurb** is two sentences: who is being sponsored and what the money
  buys. It wraps, never cuts.
- **Each row** is `/donate`'s label row (`label_line`): the bold label, then
  a dim note saying what the row is for. The note is the part that gives on
  a narrow pane (`…`-cut).
- **Each value** sits in the rounded box (`value_box`) and is an **OSC 8
  link** (`docs/links.md`): the URL opens itself, the address opens
  `mailto:`. The target is stamped on every row of a value wrapped by a
  narrow pane, so any fragment opens the whole thing, and the value is
  underlined to say it can be clicked. `/donate`'s addresses pass no target
  and stay plain.
- There is **no caution row**. `/donate` has one because a wrong network
  loses the money; nothing on this page can go wrong that way.

## Keys

The `/donate` grammar, every key owned while the page is open:

| key                 | does                                                  |
| ------------------- | ----------------------------------------------------- |
| ↑ / ↓               | move the `❯`, wrapping at the ends                    |
| Home / End          | first / last row                                      |
| Enter, `c`          | copy the highlighted value; the page stays open       |
| `1`, `2`            | jump to that row and copy it                          |
| Esc, Ctrl+C         | close; the composer returns                           |

A copy goes through `/copy`'s clipboard path (arboard, with the OSC 52
fallback) and confirms with a toast: `Copied the sponsor link to clipboard`
or `Copied the email address to clipboard`. The app opens no browser itself;
the link is the terminal's to follow and the copy is for everywhere else.

It works mid-turn like every composer-replacing page: the streaming strip
keeps its rows above it, and the running cell's Ctrl+B hint is withheld
while the page owns the keys.

## A short terminal

`boxed_list_page` is the ladder `/donate` documents, shared by both pages:
the whole page; then the row the highlight is not on folded to its label;
then that without the title and blurb; then the highlighted row alone. The
hint stays on every page, and only a terminal shorter than the last one
bottom-anchors it (`docs/view-flow.md`). The page is still, so its flow is
signed on its rows.

## Where it lives

| piece | file |
| --- | --- |
| catalog, page state, keys | `src/app/sponsor.rs` |
| the page's lines | `src/ui/sponsor_view.rs` (over `donate_view`'s shared parts) |
| the copy and its toast | `src/tui/sponsor.rs` |
| words | `SPONSOR_*` in `src/ui/theme.rs` |
| tests | `src/app/tests/sponsor.rs`, `src/ui/tests/sponsor_view.rs`, `scripts/smoke/phases/156-sponsor.sh` |
