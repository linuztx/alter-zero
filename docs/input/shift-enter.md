# Shift+Enter inserts a newline — Design

Date: 2026-06-15

## Goal

Pressing **Shift+Enter** in the composer inserts a newline at the cursor (growing
the input box) instead of submitting — the way openai/codex lets you compose a
multi-line message — with a plain Enter still submitting. The feature must also
degrade gracefully on terminals that can't report a modified Enter.

## What codex does (ported from `/tmp/codex/codex-rs/tui`)

- **It turns on keyboard enhancement.** `tui.rs::set_modes` calls
  `keyboard_modes::enable_keyboard_enhancement`, which pushes the kitty keyboard
  protocol flags with crossterm's `PushKeyboardEnhancementFlags(
  DISAMBIGUATE_ESCAPE_CODES | REPORT_EVENT_TYPES | REPORT_ALTERNATE_KEYS)`. This
  is the load-bearing piece: without it a terminal sends Shift+Enter as a bare
  `\r` — **byte-identical to Enter** — so it can't be distinguished. With
  `DISAMBIGUATE_ESCAPE_CODES`, Shift+Enter arrives as `CSI 13;2u`, which crossterm
  parses to `KeyEvent{ code: Enter, modifiers: SHIFT }`.
- **It pops the flags on the way out** (`restore_keyboard_enhancement_stack`), and
  does a stronger reset on process exit so the parent shell never inherits enhanced
  key reporting.
- **It binds three keys to "insert newline"** (`keymap.rs`, `editor.insert_newline`):
  `Ctrl+J`, `Shift+Enter`, and `Alt+Enter`. `Ctrl+J` is the **universal fallback** —
  in raw mode every terminal delivers it without any enhancement.
- It has an env escape hatch (`CODEX_TUI_DISABLE_KEYBOARD_ENHANCEMENT`) and
  auto-disables enhancement for VS Code under WSL, where the protocol misbehaves.
- It deliberately avoids crossterm's blocking `supports_keyboard_enhancement()`
  probe (up to 2s on terminals that never answer) in favour of its own 100ms one.

## Why this was *mostly* already here

`App::on_key_conversation` already mapped `Enter + (ALT | SHIFT)` to
`self.input.insert_newline()` with passing unit tests — but it was **dead code on
most terminals**, because we never enabled keyboard enhancement, so the terminal
never actually reported the SHIFT modifier on Enter. The two missing pieces were
the terminal-side enablement and the `Ctrl+J` fallback.

## The mapping onto this codebase

### Key handling (`App::on_key_conversation`) — pure, unit-tested

Three keys insert a newline; a plain Enter submits:

- `Enter` with `ALT` **or** `SHIFT` → `insert_newline()` (already present).
- `Ctrl+J` → `insert_newline()` (**new**). In raw mode the byte `0x0A` parses to
  `Char('j') + CONTROL` on every terminal (crossterm `parse.rs`: `\n` only maps to
  Enter when *not* in raw mode; otherwise it falls into the `0x01..=0x1A` →
  `Ctrl+<letter>` arm). So `Ctrl+J` works as the newline key with **no** keyboard
  enhancement — the reliable fallback. With enhancement on it arrives as the same
  `Char('j') + CONTROL` via `CSI 106;5u`, so the one arm covers both paths.
- `Enter` (no modifiers) → submit / queue / run-shell, exactly as before.

`Alt+Enter` already worked on most terminals (it sends `ESC` + `\r` →
`KeyEvent{Enter, ALT}`); `Ctrl+J` covers the rest. The arm sits next to the
existing Enter arms in `on_key_conversation`, so all newline-insertion logic reads
together.

### Terminal enablement (`term.rs::init` / `restore` / panic hook) — I/O boundary

`InlineViewport::init` pushes the kitty flag **after** the synchronous cursor
query and **before** the `EventStream` is created:

```rust
PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
```

Two invariants make this safe:

1. **Invariant 1 (single stdin reader).** A *push* is fire-and-forget — the
   terminal sends **no reply** to it — so it adds no second stdin reader and can't
   steal the cursor-position (DSR) reply. (It's also issued after `get_cursor_position`,
   so the cursor query runs in its original pristine context.)
2. We push **only `DISAMBIGUATE_ESCAPE_CODES`** — the single flag that disambiguates
   Shift+Enter from Enter. We don't need codex's `REPORT_EVENT_TYPES` (key-release
   events; the loop already filters to `KeyEventKind::Press`) or
   `REPORT_ALTERNATE_KEYS` (base-layout keys).

`restore()` pops the flag (while still in raw mode) and the panic hook pops it too,
so neither a clean quit nor a panic leaves the shell with enhanced key reporting.
Both only pop when `init` actually pushed (the `keyboard_enhanced` field / the
captured bool), keeping the push/pop stack balanced.

We **skip** crossterm's `supports_keyboard_enhancement()` probe on purpose — it
blocks up to 2s on terminals that never answer (the reason codex wrote its own).
An unconditional push is simply ignored by terminals that don't support the
protocol, so there's nothing to gain by paying that latency.

### Escape hatch (`ALTER_ZERO_DISABLE_KEYBOARD_ENHANCEMENT`)

A truthy value (`1`/`true`/`yes`, case-insensitive) keeps enhancement **off**, for
the rare terminal where the kitty protocol misbehaves (codex's
`CODEX_TUI_DISABLE_KEYBOARD_ENHANCEMENT`). The parsing is a pure
`keyboard_enhancement_disabled(Option<&str>) -> bool` helper in `term.rs`, so it's
unit-tested even though the surrounding terminal I/O is only smoke-covered.
`Ctrl+J` and `Alt+Enter` still insert newlines when the hatch is set — only
Shift+Enter goes dark (it can't be reported without the protocol).

## Testing

- `app` (unit): `Ctrl+J` inserts a newline instead of submitting, including
  mid-stream; the existing `shift_enter_inserts_a_newline_too` /
  `alt_enter_inserts_a_newline_instead_of_submitting` /
  `plain_enter_submits_a_multi_line_message_intact` tests still hold.
- `term` (unit): `keyboard_enhancement_disabled` is off by default / for
  unrecognised values and on for the truthy set.
- `src/tui/` (smoke): Phase 2 already drives `Alt+Enter` (`M-Enter`) to grow the
  box with keyboard enhancement now on (so it also guards that enabling the
  protocol didn't break ordinary key delivery — plain Enter, Esc, Ctrl+C all still
  work across every phase). Phase 23 drives `Ctrl+J` through the real
  terminal+crossterm path: the box grows to show both composed lines.
