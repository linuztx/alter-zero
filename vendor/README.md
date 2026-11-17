# vendored crates

## `syntect/` — syntect 5.3.0 with a parser loop guard

**syntect 5.3.0** as published on crates.io, with the two hunks of
[trishume/syntect#709](https://github.com/trishume/syntect/pull/709) applied.
`Cargo.toml` points at it through `[patch.crates-io]`.

- `src/parsing/parser.rs` — the fix. `parse_line` counts the matches in a row
  that leave the cursor where it was, and after 32 of them steps one character
  on (or ends the line), the same recovery syntect already uses for the one
  loop it does detect.
- `src/parsing/syntax_definition.rs` — `MatchIter::push_include` skips a
  context already on the include stack. No grammar two-face bundles has an
  include cycle, so this never fires today; it stays because #709 carries it
  and a grammar update could add one.

### Why

syntect 5.3.0 catches one kind of loop: a non-consuming pop at the stack depth
of the non-consuming push before it. A grammar can also keep matching
lookaheads that consume nothing in other ways (a `set`, or a push and a pop at
different depths), and then `parse_line` never returns. Highlighting runs on
the UI loop, so the TUI pinned a core and stopped reading keys. On `--resume`
that happened before the first keystroke, because the transcript cache
highlights every file cell when it warms. Ordinary Svelte is enough:

```svelte
<p>{count /* first
   second */}</p>
```

So is the line `:s@/` in Scala, from Sublime's Scala syntax tests. Both are
pinned by `highlight::tests::matches_that_never_advance_do_not_spin_the_parser`.

The guard only steps in after 32 matches in a row that consume nothing. Over
bat's syntax-test samples and this repository's own sources (855 files, 151
grammars) it changed nothing: the scopes are byte-identical with and without
the patch.

### Why vendored

#709 is unmerged and targets syntect master, whose `parse_line` returns
`ParseLineOutput` rather than 5.3.0's `Vec`, so the branch can't be used with
the released API. Delete `vendor/syntect`, the `[patch.crates-io]` block and
the `[profile.release.package.syntect]` override in `Cargo.toml` once a syntect
release carries the fix.

The vendored crate is a path dependency, so `Cargo.lock` no longer checksums
it. Review a change here like first-party code.

### Checking this copy

Everything but the two files above is byte-identical to the published crate:

```sh
curl -sSfL -o syntect-5.3.0.crate https://static.crates.io/crates/syntect/syntect-5.3.0.crate
sha256sum syntect-5.3.0.crate  # 656b45c05d95a5704399aeef6bd0ddec7b2b3531b7c9e900abbf7c4d2190c925
tar -xzf syntect-5.3.0.crate
diff -rq syntect-5.3.0/src vendor/syntect/src  # only parser.rs and syntax_definition.rs
for f in Cargo.toml LICENSE.txt Readme.md; do cmp syntect-5.3.0/$f vendor/syntect/$f; done
```

The published crate also carries tests, examples, benches and assets that are
not vendored. Its `Cargo.toml` still lists them as targets, which is harmless:
the vendored crate is not a workspace member, so nothing builds them.

## `crossterm/` — crossterm 0.29.0 that reads a typed burst whole

**crossterm 0.29.0** as published on crates.io, with one fix to its unix
event source. `Cargo.toml` points at it through `[patch.crates-io]`.

- `src/event/source/unix/mio.rs` — the fix. A read of the terminal that fills
  the 1024-byte buffer is noted (`tty_unread`), and `try_read` reads what
  such a read left behind (`read_unread`) before it waits on the terminal
  again.
- `src/terminal/sys/file_descriptor.rs` — `FileDesc::has_input`, the
  `FIONREAD` check that fix reads behind, for both of the crate's descriptor
  flavours (`rustix`, which this build uses, and `libc`).
- `src/terminal/sys/unix.rs` — a pair of redundant parentheses removed from
  `window_size`. Nothing about behaviour: a path dependency's warnings are
  printed on every clean build, a registry crate's are not.

Each change is marked `alter-zero patch` where it is made.

### Why

The event source waits for input with mio's `Poll`, which is edge-triggered:
it reports the terminal readable when input *arrives*, not while input
*remains*. On that report it reads 1024 bytes and returns the first event
parsed, without reading on. The events of that one read are handed out by the
calls that follow; when they run out it polls again, and the bytes a full
read left in the terminal are not news, so nothing is reported until another
key arrives.

So more than a kilobyte typed in one piece — a paste into a terminal with no
bracketed paste, a dictation tool's text, typeahead a stalled connection
delivers at once — reached the composer 1024 bytes per key press. A bracketed
paste never showed it: the parser returns no event until the paste's end, so
that read loop does run on.

The patched source reads on only while the terminal says it holds input. The
descriptor is not a non-blocking one (crossterm reads stdin as it finds it),
so a read with nothing to return would wait there for the next key — on the
loop thread, where the stream polls with a zero timeout.

`scripts/smoke.sh` Phase 159 types 3,200 bytes in one write and reads the
composer's last words; `examples/latency_probe.rs`'s `burst` scenario times
the same thing (`docs/input-latency.md`).

### Why vendored

0.29.0 is the latest release, and crossterm's master reads the terminal the
same way. Delete `vendor/crossterm`, its line in `[patch.crates-io]` and the
`[profile.release.package.crossterm]` override in `Cargo.toml` once a
crossterm release reads a burst whole — and check that ratatui's own
crossterm requirement moves with it, since the patch replaces the one crate
both depend on.

The vendored crate is a path dependency, so `Cargo.lock` no longer checksums
it. Review a change here like first-party code.

### Checking this copy

Everything but the three files above is byte-identical to the published
crate:

```sh
curl -sSfL -o crossterm-0.29.0.crate https://static.crates.io/crates/crossterm/crossterm-0.29.0.crate
sha256sum crossterm-0.29.0.crate  # d8b9f2e4c67f833b660cdb0a3523065869fb35570177239812ed4c905aeff87b
tar -xzf crossterm-0.29.0.crate
diff -rq crossterm-0.29.0/src vendor/crossterm/src  # only the three files above
for f in Cargo.toml LICENSE README.md; do cmp crossterm-0.29.0/$f vendor/crossterm/$f; done
```

The published crate also carries examples and docs that are not vendored. Its
`Cargo.toml` still lists the examples as targets, which is harmless: the
vendored crate is not a workspace member, so nothing builds them.
