# `/discord` — the community page

The **`/discord`** command opens a read-only page with the invite to the
Alter Zero Discord server. It is the `/sponsor` page's twin
(`docs/sponsor.md`): the same frame, the same title row, a labelled row
over its value in the rounded box, and the same short-terminal ladder,
built from the parts `donate_view` shares. Where `/donate` and `/sponsor`
ask for support, this page asks the reader in.

## The invite

| label     | note                            | value                           | opens   |
| --------- | ------------------------------- | ------------------------------- | ------- |
| `Discord` | invite to the community server  | `https://discord.gg/EDBSpmq2fV` | the URL |

The invite is the pure `app::DISCORD_INVITE`, a const and never a file, for
`/donate`'s reason: a page that read the invite from disk is a page anyone
with write access to the config home could point at another server, and a
fake Discord invite is a common phishing lure. A test pins the value to the
server's own invite, bare ASCII with no whitespace, on `discord.gg`.

There is one row, so there is no catalog: a list of one would be a type and
a lookup for nothing. A second community link would be a second page's
question (`/community`), not a second row here.

## The page

```
────────────────────────────────────────────────────────────────────────────────
  ♥ Join the Alter Zero Discord

  Ask for help, share what you have built, and hear about new releases first.
  Ideas for new features are welcome too.

  ❯ 1. Discord  invite to the community server
     ╭─────────────────────────────────╮
     │  https://discord.gg/EDBSpmq2fV  │
     ╰─────────────────────────────────╯

  enter/c copy invite link  esc close
────────────────────────────────────────────────────────────────────────────────
```

- **The title** is `/sponsor`'s row with other words around the name: the
  red heart, then `Join the Alter Zero Discord` in the page titles'
  gradient. `title_line` takes the words before and after the app's name,
  so the name still comes from `HEADER_NAME` alone. The heart stays: it is
  the glyph of the three pages about the people behind the project, and it
  is one the app already draws in every theme.
- **The blurb** says what the server is for in two sentences. It wraps,
  never cuts.
- **The row** is `/donate`'s label row (`label_line`): the bold label, a dim
  note that `…`-cuts on a narrow pane, and the `❯` the hidden cursor seats
  on. It is numbered like every row in the family, and `1` copies it.
- **The value** sits in the rounded box (`value_box`), lit in the accent as
  the row the highlight is on, and is an **OSC 8 link** to the invite
  (`docs/links.md`): a click opens it where the terminal follows links,
  and every row of a value wrapped by a narrow pane opens the whole URL.
- **The hint** has no `↑↓ navigate`: there is nothing to move to.

## Keys

Every key is owned while the page is open:

| key             | does                                     |
| --------------- | ---------------------------------------- |
| Enter, `c`, `1` | copy the invite; the page stays open     |
| Esc, Ctrl+C     | close; the composer returns              |
| anything else   | nothing (↑/↓ have no other row to reach) |

A copy goes through `/copy`'s clipboard path (arboard, then the user's
terminal, `docs/copy.md`) and confirms with a toast: `Copied the Discord invite link to
clipboard` (`app::DISCORD_COPIED_NOTICE`). The app opens no browser itself.

It works mid-turn like every composer-replacing page: the streaming strip
keeps its rows above it, and the running cell's Ctrl+B hint is withheld
while the page owns the keys.

## A short terminal

The page is `boxed_list_page` over one entry, so of the four rungs
`/donate` documents two differ: the whole page, then the page without the
title and blurb — the row, its box and the hint. Only a terminal shorter
than that bottom-anchors it (`docs/view-flow.md`). The page is still, so its
flow is signed on its rows.

## Where it lives

| piece | file |
| --- | --- |
| the invite, page state, keys | `src/app/discord.rs` |
| the page's lines | `src/ui/discord_view.rs` (over `donate_view`'s shared parts) |
| the copy and its toast | `src/tui/discord.rs` |
| words | `DISCORD_*` in `src/ui/theme.rs` |
| tests | `src/app/tests/discord.rs`, `src/ui/tests/discord_view.rs`, `scripts/smoke/phases/167-discord.sh` |
