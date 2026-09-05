# `/donate` — the crypto donation page

The **`/donate`** command opens a read-only page listing the project's
crypto donation addresses, so a user who wants to support the development
finds the addresses where they already are — in the terminal, one command
away — and copies one without leaving the conversation. It is the eleventh
composer-replacing inline picker, and deliberately the `/hooks` browser's
sibling: **no text entry**, the hardware cursor hidden, every key owned while
open, the composer back on Esc.

## The addresses

| ticker | coin     | network            | address                                      |
| ------ | -------- | ------------------ | -------------------------------------------- |
| `BTC`  | Bitcoin  | the native network | `36ysFtsQDUQtigqGUXoHYr7jYegeCRnqoB`         |
| `ETH`  | Ethereum | the Base network   | `0xF67F3EA18b6156f4ACfEfEf8D96c4F998B354CD6` |

The catalog is the pure `app::DONATION_ADDRESSES` — one `DonationAddress`
per row (`ticker`, `coin`, `network`, `address`, all `&'static str`), in the
order the page lists them. It is a const rather than a file: the addresses
are the project's, not the user's, and a page that read them from disk would
be a page anyone with write access to the config home could redirect. The
catalog tests pin every address to ASCII with no whitespace and every ticker
to a distinct uppercase word, so a typo in a future entry fails the build
rather than the donation.

## The page

```
────────────────────────────────────────────────────────────────
  ♥ Support Alter Zero

  Free and open source, developed in the open. If it earns a place
  in your terminal, a donation keeps the work going — thank you.

  ❯ 1. BTC  Bitcoin · native network
     ╭──────────────────────────────────────╮
     │  36ysFtsQDUQtigqGUXoHYr7jYegeCRnqoB  │
     ╰──────────────────────────────────────╯

    2. ETH  Ethereum · Base network
     ╭──────────────────────────────────────────────╮
     │  0xF67F3EA18b6156f4ACfEfEf8D96c4F998B354CD6  │
     ╰──────────────────────────────────────────────╯

  Send BTC over the Bitcoin network and ETH over Base only — a
  transfer on any other network cannot be recovered.

  ↑↓ navigate  enter/c copy address  esc close
────────────────────────────────────────────────────────────────
```

The frame is the picker family's — the same rules, the same two-column
inset, the same dim hint row — and what sits inside it is built from parts
the chrome already has, so the page reads as *this* app's rather than a
form pasted into it:

- **The title wears the banner's gradient.** `♥ Support Alter Zero` is
  washed left-to-right in the header's own accent → link gradient
  (`ui::header::gradient_spans`, the builder the mascot banner uses), bold,
  behind a heart in the palette's red — the one place that hue means
  affection rather than failure. The name is read from `APP_NAME`, the one
  place the app's name lives, and a test pins it there.
- **The blurb** under it is two dim wrapped rows: what the project is and
  what a donation does. Wrapped, never cut — the page's height is its own
  line count (`docs/view-flow.md`), so a narrow terminal costs a row, not a
  word.
- **Each address is a numbered row over a rounded box.** The row is the
  `/hooks` menu's shape — the `❯` marker on the highlighted one, an absolute
  `{n}.` number, the **ticker** bold, then the coin and its network dim
  (`Bitcoin · native network`) — and the address sits beneath it inside the
  `/login` device page's rounded box, bright and bold like the one-time
  code, because it is the one thing on the page to transcribe. The box is
  sized to the address; on a terminal too narrow to seat it the address
  **wraps inside** the box (`wrap_output`, nothing cut) rather than
  overflowing the frame, and `c` is there for exactly that case.
- **The selection is shown by colour.** The highlighted row's marker,
  number and ticker light up in the accent, and so does its box's border;
  the other rows keep the muted ink and the dim frame border. It is the
  palette's rule — the whole selected row lights up, no second caret — and
  it is what makes the box under the `❯` read as *the* address rather than
  one of two.
- **The caution is amber.** `Send BTC over the Bitcoin network and ETH over
  Base only …` wears the palette's warning colour, the ask review's
  unanswered-question hue, on a page where everything else is dim or
  bright: a wrong-network transfer is the one irreversible mistake the page
  can lead to, so it is the one line the eye is pulled to before the
  address is used.
- **One blank row between blocks, never two.** The page is built as blocks
  joined by exactly one gap (the `/login` page rule), and a test walks every
  width asserting no two blank rows ever stack.

The page is **still** — nothing on it ticks — so its scrollback flow is
signed on its rows like the `/mascot` picker's, and on a short terminal it
bottom-anchors and flows its top into scrollback like every framed view
(`docs/view-flow.md`): the hint and the closing rule stay on screen, the
title scrolls up where the terminal's own scrolling reads it.

## Keys

The page owns **every** key while open (routed at the top of `App::on_key`,
before the composer's global Ctrl+C/Ctrl+O), the `/hooks` grammar:

| key                | does                                                          |
| ------------------ | ------------------------------------------------------------- |
| ↑ / ↓              | move the highlight, **wrapping** at the ends (`wrap_step`)    |
| Home / End         | jump to the first / last address                              |
| `1`–`9`            | jump to that address **and copy it** (the ask modal's rule)   |
| Enter / `c`        | copy the highlighted address to the clipboard                 |
| Esc / Ctrl+C       | close — the composer returns                                  |

Anything else is swallowed: a printable key never reaches the composer draft
underneath, and neither does a paste (nothing anyone pastes is a donation
address — the `/settings` swallow rule). Copying **keeps the page open**: the
confirming toast lands above the frame with the address still in view, and a
user who wants both addresses copies the second without reopening. The page
works **mid-turn** like every picker — it only replaces the composer, and the
streaming strip keeps its rows above it — and, like every picker, it blanks
the running cell's `(ctrl+b to run in background)` hint while open
(`App::command_elapsed`), since it would swallow the Ctrl+B the hint
advertises.

No text is entered anywhere on the page, so the hardware cursor is
**hidden** (`ui::cursor_visible` — the permission prompt's rule: a kitty
cursor trail would streak across the boxes on every ↑/↓) while its *seat*
still tracks the highlighted `❯` row (`menu_marker_seat`, the `/hooks` seat),
so the cursor's return when the page closes starts somewhere sensible.

## Copying an address

Enter, `c`, or a digit returns `Action::CopyDonationAddress(address)` — the
pure core decides *what* is copied, the loop does the I/O
(`tui::donate::Session::copy_donation_address`): the `/copy` clipboard path
(`clipboard::copy_to_clipboard` — arboard, with the OSC 52 fallback for a
headless, SSH or tmux session; the native lease held for the app's lifetime,
`docs/copy.md`), then a transient toast — `Copied the BTC address to
clipboard`, worded for what was actually copied, the `/login` device page's
`Copied the code to clipboard` sibling — or a red `Copy failed: {reason}`.
Never a scrollback bullet (`docs/toast.md`).

## API

- `app::DonationAddress` / `app::DONATION_ADDRESSES` — the catalog.
- `app::DonatePicker` — the open page's state (the highlighted row);
  `App::open_donate_picker`, `close_donate_picker`, `highlighted_donation`,
  `on_key_donate_picker`.
- `Action::OpenDonatePicker` / `CloseDonatePicker` /
  `CopyDonationAddress(DonationAddress)`.
- `ui::donate_view` — `donate_view_lines` (the page builder; its length is
  the reserved height), `donate_picker_height`, `render_donate_picker`.
- `tui::donate::Session::copy_donation_address` — the clipboard write and
  the toast.

## Tests

- `app/tests/donate.rs` — the catalog (two addresses, BTC first, the exact
  strings, ASCII and whitespace-free, distinct uppercase tickers), the
  `/donate` command opening the page (idle and mid-turn) and abandoning the
  bands that share the composer, and the whole key grammar (wrapping ↑/↓,
  Home/End, Enter/`c`/digit copy, Esc/Ctrl+C close, owns-every-key, the
  Ctrl+B hint clock suppressed).
- `ui/tests/donate_view.rs` — the framed page (rules, the gradient title
  naming `APP_NAME`, the blurb, both rows and boxes, the caution, the
  hint), the selection lighting its row and box, the address wrapping
  inside its box at a narrow width, width safety, the height contract,
  flow eligibility, the hidden cursor seated on the `❯`, and the
  no-stacked-blanks rule.
- `scripts/smoke.sh` Phase 112 — the page end to end in a real terminal:
  open from the palette, both addresses in their boxes, ↓ moves the `❯`,
  `c` copies the highlighted address (the toast, and the OSC 52 fallback
  landing in tmux's paste buffer verbatim), Esc brings the composer back.
