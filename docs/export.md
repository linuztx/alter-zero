# `/export` — the conversation as plain text

Date: 2026-09-22

## Goal

An **`/export`** slash command that hands the user the **whole
conversation as plain text** — every message and every tool call's full
output, as the Ctrl+O transcript shows it — either **copied to the
clipboard** or **saved to a file** in the working directory, named
`conversation-YYYY-MM-DD-HHMMSS.txt`. It is `/copy`'s sibling (`/copy` is
the last reply; `/export` is everything) and, as a page, the `/donate`
page's: a read-only composer-replacing picker with **no text entry**, two
rows that are the two answers, the hardware cursor hidden, every key owned
while open, the composer back on Esc — and back on the pick too, since the
pick is the whole point of the page.

## The page

```
────────────────────────────────────────────────────────────────────────

  Export conversation

  The whole transcript as plain text — every message and every tool
  call's full output, as Ctrl+O shows it.

  ❯ 1. Copy to clipboard
    2. Save to file

  Copies the transcript to the system clipboard.

  ↑↓ navigate  enter select  esc close

────────────────────────────────────────────────────────────────────────
```

The frame is the picker family's — the same rules, the same two-column
inset, the same dim hint row — and what sits inside it follows the two
siblings it borrows from:

- **The title** is bold in the `/login` pages' title colour: a heading
  over a question, not a banner.
- **The blurb** under it says what the export *is* — the one thing a
  user hovering over two rows wants to know before picking either. Two dim
  wrapped rows, never cut: the page's height is its own line count
  (`docs/view-flow.md`), so a narrow terminal costs a row, not a word.
- **The rows are the two answers**, the `/donate` page's row shape: the
  `❯` marker on the highlighted one, an absolute `{n}.` number, the label —
  and nothing after it. What each row *does* is not a clause on the row
  (the rows stay one glance wide) but the **description under the list**,
  the `/settings` menu's shape: the highlighted row's alone. The clipboard
  row's says where the text goes; the file row's names the file's shape and
  the directory it lands in — `Writes conversation-YYYY-MM-DD-HHMMSS.txt
  into ~/alter-zero.` — the session's cwd as the footer shows it, so the
  user knows where to look before the file exists (`the working directory`
  until the boundary has injected one; the pure core never reads the
  environment).
- **The selection is shown by colour**: the highlighted row's marker,
  number and label light up in the accent (the label bold), the other row
  keeps the muted ink. The palette's rule — the whole selected row lights
  up, no second caret.
- **One blank row between blocks, never two.** The page is built as blocks
  joined by exactly one gap (the `/login` page rule), and a test walks every
  width asserting no two blank rows ever stack.

The page is **still** — nothing on it ticks — so its scrollback flow is
signed on its rows like the `/donate` page's, and on a short terminal it
bottom-anchors and flows its top into scrollback like every framed view
(`docs/view-flow.md`).

## Keys

The page owns **every** key while open (routed at the top of `App::on_key`,
before the composer's global Ctrl+C/Ctrl+O), the `/donate` grammar:

| key          | does                                                       |
| ------------ | ---------------------------------------------------------- |
| ↑ / ↓        | move the highlight, **wrapping** at the ends (`wrap_step`) |
| Home / End   | jump to the first / last row                               |
| `1` / `2`    | jump to that row **and pick it** (the ask modal's rule)    |
| Enter        | pick the highlighted row                                   |
| Esc / Ctrl+C | close — the composer returns                               |

A digit past the rows names nothing and is ignored; anything else is
swallowed — a printable key never reaches the composer draft underneath,
and neither does a paste (the `/settings` swallow rule). **A pick closes
the page.** This is the one place the page parts from `/donate`, where
copying keeps the page open because a second address is one key away: here
there is nothing else to take from the same page, and a page left open over
a `Saved the conversation to …` toast would read as a page still waiting
for an answer. The page works **mid-turn** like every picker — it only
replaces the composer, the streaming strip keeps its rows above it, and the
export then carries the live tail exactly as Ctrl+O shows it — and, like
every picker, it blanks the running cell's `(ctrl+b to run in background)`
hint while open (`App::background_hint_elapsed`), since it would swallow
the Ctrl+B the hint advertises.

No text is entered anywhere on the page, so the hardware cursor is
**hidden** (`ui::cursor_visible` — the permission prompt's rule) while its
*seat* still tracks the highlighted `❯` row (`menu_marker_seat`), so the
cursor's return when the page closes starts somewhere sensible.

## Nothing to export

`/export` on an empty conversation — no item recorded in the transcript on
screen — is a `Nothing to export` toast (`EXPORT_EMPTY_NOTICE`, the
`/compact` `Nothing to compact` rule), and no page opens: a soft rejection
the user needn't keep, never an empty file. The live tail alone never
counts, because a turn records its user message before anything streams,
so an empty history *is* an empty conversation.

## What is exported (`ui::export_text`)

**The transcript on screen, as text — top to bottom.** The Ctrl+O page's
own rows, built by the same `TranscriptCache` walk so the export and the
pager can never disagree, starting where the page starts: the **startup
banner** the terminal opened with — the mascot beside `Alter Zero (v…)`,
the cwd, the `/login   /model   /resume` hint — then one blank row and the
conversation. The banner is kept on purpose: the file is a record of what
the terminal showed, and a transcript that opens the way the session did
says which tool wrote it, which version, and where it ran, without a header
of its own being invented for it. Under it, every message, every tool
call's **full** output (the inline view's `… +N lines` fold is a screen
budget; the file has none), each `Thought for …` cell's whole
chain-of-thought, the compaction markers' summaries, the hook notes, the
task calls as ordinary cells — in the exact order they happened — and,
mid-turn, the live tail: the in-progress reply, the open thinking block,
the running tool and its `⎿ Waiting…` siblings, the queued backlog. (The
pager's `Nothing here yet.` placeholder is never reached: the command
refuses an empty conversation before the page opens.)

**Inside a subagent's session view it is that agent's transcript**, not the
lead's — the `/copy` rule (`docs/copy.md`, `docs/agent-view-streaming.md`):
the export is of the conversation the screen is showing.

**At the terminal's width.** The rows are rendered at the width the
boundary hands in (`term.screen().width`), so what the user sees is what
they get: a table, a code block, a wrapped paragraph all land in the file
exactly as they laid out on screen. (A fixed width would have made the file
a different rendering from the one the user was looking at when they chose
to keep it.)

**Plain.** The styles are dropped — a `Line` is its spans' text joined,
so the banner's gradient mascot lands as its bare block glyphs — and each
row loses the padding a cell carries on screen: a user bubble is padded to
the full width, a timestamp is right-aligned, the banner's rows are padded
past their text, a picture reserves rows of blanks (`docs/images.md`), and
in a text file that padding is noise a diff would show, so every row is
`trim_end`ed. Trailing blank rows go, and the text closes on exactly one
newline. Nothing else is touched: a right-aligned timestamp keeps the
spaces *before* it, a code block keeps its indent, a table keeps its
borders. OSC 8 link carriers and image markers ride `Style` only and vanish
with it (`docs/links.md`).

## The file (`app::export_file_name`)

`conversation-YYYY-MM-DD-HHMMSS.txt` — the date as ISO, the time as one run
of digits, every field zero-padded so the names sort as they were written
— in the **working directory** the session runs in (the footer's cwd, the
permission rules' project key). The stamp is the **local** clock, the
rollout's own rule (`session::rollout_rel_path`); the pure function takes
the date and time as values and the boundary reads `chrono::Local::now()`
(`tui::export`), the `set_clock` pattern.

The file is created with `create_new`, so an export **never overwrites** a
file already there: two exports inside one second — a double Enter — take
the plain name and then `conversation-…-2.txt`, `-3`, … (the function's
`dup` argument: `0` is the plain name, `1` the same name closed by `-2`),
and only a directory that refuses every name is an error. The toast names
the file actually written — `Saved the conversation to
conversation-2026-09-22-090720.txt` — so the user can find it without
guessing which second the clock read.

## The clipboard

The clipboard row goes through `/copy`'s own path
(`clipboard::copy_to_clipboard`, `docs/copy.md`): arboard, with the OSC 52
fallback for a headless, SSH or tmux session, the native lease held for the
app's lifetime. Only the confirmation differs — `Copied the conversation to
clipboard`, worded for what was actually copied, the `/donate` page's
`Copied the BTC address to clipboard` sibling — or a red `Copy failed:
{reason}`. A file write that fails is a red `Export failed: {reason}`.
Never a scrollback bullet (`docs/toast.md`).

## API

- `app::ExportTarget` (`Clipboard` / `File`, `ALL`, `label`) — the two
  rows; `app::ExportPicker` — the open page's state (the highlighted row);
  `app::EXPORT_EMPTY_NOTICE`; `app::export_file_name(date, time, dup)`.
- `App::export_available`, `open_export_picker`, `close_export_picker`,
  `highlighted_export_target`, `on_key_export_picker`.
- `Action::OpenExportPicker` / `CloseExportPicker` / `Export(ExportTarget)`.
- `ui::export_view` — `export_view_lines` (the page builder; its length is
  the reserved height), `export_picker_height`, `render_export_picker`, and
  `export_text` (the plain-text render over `ui::transcript_lines` /
  `ui::agent_transcript_lines`, the pager's own builders).
- `tui::export::Session::export_conversation` — the clipboard write or the
  file write, and the toast.

## Tests

- `app/tests/export.rs` — the palette row (right after `/copy`, the
  description), the `/export` command opening the page (idle, mid-turn,
  inside an agent view) and refusing an empty conversation with the toast,
  the bands abandoned on open, the whole key grammar (wrapping ↑/↓,
  Home/End, Enter and a digit picking **and closing**, digits past the
  rows ignored, Esc/Ctrl+C close, owns-every-key, the Ctrl+B hint clock
  suppressed, no animation), and the file name (the shape, the zero
  padding, the `-2`/`-3` suffixes).
- `ui/tests/export_view.rs` — the framed page (rules, title, blurb, rows,
  the description following the highlight and naming the file's shape and
  the directory — or the fallback before session info is injected — the
  hint), each row read whole as its label, the selection lighting its row,
  width safety, the no-stacked-blanks rule, the height contract, flow
  eligibility, the hidden cursor seated on the `❯`; and the export text —
  the transcript's rows verbatim and in order, opening with the startup
  banner (title row first, cwd, hint, one blank, then the conversation),
  no trailing whitespace, one closing newline, the live tail mid-turn,
  wrapping at the width asked for, and the viewed agent's transcript —
  under its own banner — inside its session view.
- `scripts/smoke.sh` Phase 122 — the page end to end in a real terminal,
  in a throwaway cwd: `/export` on an empty conversation toasts and writes
  nothing; after a settled reply the page opens with both rows and the
  description, ↓ moves the `❯` and swaps the description, Enter writes
  `conversation-YYYY-MM-DD-HHMMSS.txt` into the cwd (the toast names it,
  the file opens with the banner's title row and carries its hint row
  above the message and the reply, no trailing whitespace, one closing
  newline) and the composer comes back; a second
  `/export`'s Enter on the clipboard row lands the same text in tmux's
  paste buffer through the OSC 52 fallback; Esc closes without writing.
