# The idle text layout — the code a session keeps in memory is the code it runs

A session at rest — launched, nothing typed, no MCP server — is the state
this app spends most of its life in, and its resident memory there is the
number `docs/memory.md` exists to keep small. Measuring what the plugin
system (`docs/plugins.md`) costs there turned up something that had nothing
to do with plugins: **most of an idle session's memory is the binary's own
code, an idle session runs about a megabyte and a half of it, and eight to
ten megabytes of it were resident anyway** — a figure that moved by a
megabyte whenever a change shifted where the linker put things, the plugin
system's own code included, none of which runs in a session without a
plugin. This page is that measurement, the fix, and what the fix leaves.

The fix is one linker script. The functions an idle session runs are listed
in `layout/idle-functions.txt`; `build.rs` has the linker put them side by
side in a section of their own, `.text.idle`, ahead of the rest of the code.
At rest, a configured session of the shipped binary goes from
**13.96 MB to 8.02 MB** just installed, and
from **15.86 MB to 8.90 MB** once it has been
read back from disk.

## The measurement

Everything here ran on one x86_64 Linux machine (4 cores, 15 GB, kernel
6.18, ext4, rustc 1.94.1 linking with its bundled `rust-lld`), with
`scripts/text_layout.sh`, which is checked in so the numbers can be taken
again:

- **A session at rest, in the state sessions are really in.** A list
  profiled on an empty config home works there — the figure falls by five
  megabytes — and on a config home that has been used it falls by two. A
  session with a provider signed in to runs a different
  startup from the offline demo's — a real backend, an HTTP client, the
  key store, half a dozen state files' readers — and those functions,
  unlisted, were as scattered as ever. So the script makes the state it
  measures, by driving the binary under test through a short session of
  its own, and no file in it is in a format the script knows:
  - **a configured session** — the state nearly every session is in: a
    provider with a key and a model chosen from its listing, a turn taken,
    a theme, a setting and a permission mode changed, in a git checkout
    with an `AGENTS.md`;
  - **a first run** — an empty config home and the offline demo backend;
  - **with plugins** — the configured session with a plugin on that brings
    a skill, a command, an agent and a hook, the hook trusted in the review.

  Nothing leaves the machine: the provider is a stub on the loopback, which
  also stands in for the telemetry collector and the release page, so the
  first launch's daily ping and update check land there and the launches
  that are measured find both done for the day, as a session that is not
  the day's first does. Each launch is in a 120×40 pane of a private tmux
  server, in an environment holding `PATH`, `TERM` and nothing of the
  user's. `measure` waits for the banner, then eight seconds, reads `VmRSS`,
  `RssAnon` and `RssFile` from `/proc/{pid}/status` and the resident part
  of the binary's executable mapping from `/proc/{pid}/smaps`, and reports
  the median of five launches.
- **In both states a binary's file can be in.** How much of a mapped file
  is resident depends on how it reached the page cache. A fault maps the
  whole block of cache the page sits in (and, where blocks are small, the
  sixteen pages around it — the kernel's fault-around), and a file **read
  from disk** sits in larger blocks than one **just written**. A binary
  just installed is in the second state and the same binary after a reboot
  is in the first, so `measure` takes both: once on a fresh copy, once after
  dropping that copy's cache and reading it back. They differ by a megabyte
  or two, which is why two builds measured in different cache states cannot
  be compared at all.
- **What an idle session runs.** `scripts/text_layout.sh profile` runs the
  same launches under valgrind's callgrind — the configured one for 150
  seconds, long enough for the banner mascot's nap
  (`docs/mascot-animation.md`) — and lists every function that executed.
- **Whose pages are resident.** The present bit of every page of the
  executable mapping (`/proc/{pid}/pagemap`), joined to the symbol table.

## Where an idle session's memory is

The shipped binary (`dist` profile, `docs/binary-size.md`) laid out the
linker's default way — `ALTER_ZERO_TEXT_LAYOUT=0` — in a configured session:

| A configured session at rest, read from disk | Resident |
|---|---|
| The binary's code | 10.44 MB |
| The rest of what is file-backed — the binary's constants, relocations and unwind tables, the shared C library and its kin | 4.31 MB |
| Anonymous — the heap, the stacks, the data the loader relocates | 1.11 MB |
| **Total** | **15.86 MB** |

Most of it is the binary's code. And callgrind's answer to how much code a
configured session needs at rest, out of the binary's 12.3 MB of it:
**874 functions, 1.43 MB**. (A first run needs 746 functions and 1.18 MB;
the `release` build, which inlines less, 1,614 functions and 1.25 MB out of
14.1 MB.)

## Why eight megabytes are resident for the one and a half that run

Memory is resident by the page, and the kernel maps more than the page that
faulted: the sixteen pages around it at least, the whole cache block where
blocks are large. So what decides how much code is resident is not how much
runs but **how scattered it is** — and the linker's default order is the
order its inputs arrive in, crate by crate and codegen unit by codegen
unit, which scatters what an idle session runs across all twelve megabytes.
Nearly every 64 kB window of the code has something in it that ran once at
startup, so nearly every window is resident.

Two consequences follow, and the second was measured before the cause was
understood:

- **Code is resident whether it runs or not.** Nearly all of the code a
  session at rest never runs — the providers it is not signed in to, the
  pickers it has not opened, the plugin system with no plugin installed —
  sits between functions that did run once, so its pages come in with
  theirs.
- **The figure moves with any change.** The plugin system added 0.6 MB of
  code to the `release` binary (13.5 to 14.1 MB), none of which a session
  with no plugin runs, and a configured session's idle figure rose by
  0.55 MB just written and 0.95 MB read from disk (14.75 to 15.30, 16.11
  to 17.06 MB). That was not a cost of the code added. It was where the
  linker happened to put the next thing.

That second one is why this is fixed here rather than worked around per
feature: a budget measured in resident megabytes cannot be kept against a
layout that hands out a megabyte at random.

## The layout

rustc gives every function an input section of its own —
`.text.{mangled name}`, `.text.unlikely.{mangled name}` for one marked cold
— so a linker script can gather functions by name. The one `build.rs`
writes (`layout/script.rs` is the generator) is short:

```
SECTIONS
{
  .text.idle :
  {
    /* a configured session */
    *(
      ".text._ZN4core3fmt5write17h*" ".text.unlikely._ZN4core3fmt5write17h*"
      …
    )
    /* a first run */
    *( … )
    /* with plugins */
    *( … )
  }
}
INSERT BEFORE .text;
```

`INSERT` makes it an addition to the linker's default script rather than a
replacement, so everything not named is laid out exactly as it was. GNU ld
and lld both read it. A pattern that matches nothing is not an error, and
the cold twin of each pattern matters: an idle session runs plenty of
functions marked cold (a vector's first growth is one), and the first
version of the list, without it, left 67 of them where they were.

**The list** is `layout/idle-functions.txt`, written by
`scripts/text_layout.sh profile` from the two builds people run — `release`
and `dist`, whose function names differ where one inlined what the other
did not — as the union of what each executed. A legacy-mangled Rust name
ends in a hash, `17h{16 hex}E`, which changes with the compiler, the crate's
version and the crate an instance was made in; the list keeps the path and
leaves the hash to a `*`, so one list serves every build until the code it
names is renamed.

**The list is in groups, and a group is one run of code.** A linker places
the sections one input statement matches together, so each `[group]` of the
list is a statement of its own, in the list's order: `[a configured
session]` first — everything such a session runs, 1,349 functions — then
`[a first run]`, the 18 only the offline demo adds, then `[with plugins]`,
the 129 a session with a plugin on adds. A session keeps resident the
groups it runs: a configured one the first, one with plugins the first and
the last, and neither pays for the other's. That is also how a new
always-on feature should join the list — as a group, profiled in a scenario
of its own in the script — rather than by widening the first.

**The build script** asks for the script on the `alter-zero` binary's link
(`cargo::rustc-link-arg-bins`: `-T {script}`, never on a test or an
example), and stands aside where it does not apply:

| Build | Laid out? |
|---|---|
| `cargo build --release`, `--profile dist`, a Linux target | yes |
| a `dev` build (`cargo build`, `cargo test`) | no — the script costs a second or two per link, for a binary nobody leaves idling |
| macOS | no — another object format; its linker orders by exact symbol names, which change with every build |
| `-fuse-ld=gold` / `mold` in the flags | no, with a warning — gold has no `INSERT`, mold reads no `SECTIONS` |
| `ALTER_ZERO_TEXT_LAYOUT=0` | no; `=1` lays a `dev` build out too |

The binary behaves the same either way. Only where its functions sit
differs.

## What it bought

`scripts/text_layout.sh measure`, the median of five launches (three for a
first run), megabytes resident at rest — just written, then read from disk —
and, after them, how much of that is the binary's code.

**A configured session**, the state nearly every session is in:

| | Just written | Read from disk | Of which its code |
|---|---|---|---|
| `release`, `main` before the plugin system | 14.75 | 16.11 | 9.33 / 10.06 |
| `release`, with the plugin system, default order | 15.30 | 17.06 | 9.99 / 11.07 |
| `release`, with the plugin system, laid out | **9.38** | **10.22** | 4.03 / 4.32 |
| `dist`, with the plugin system, default order | 13.96 | 15.86 | 9.00 / 10.44 |
| `dist`, with the plugin system, laid out | **8.02** | **8.90** | 3.07 / 3.30 |

**A first run** — an empty config home, the offline demo:

| | Just written | Read from disk | Of which its code |
|---|---|---|---|
| `release`, `main` before the plugin system | 14.20 | 15.73 | 9.09 / 10.11 |
| `release`, with the plugin system, default order | 14.49 | 16.57 | 9.45 / 10.88 |
| `release`, with the plugin system, laid out | **8.91** | **9.98** | 3.84 / 4.13 |
| `dist`, with the plugin system, default order | 12.88 | 15.18 | 8.09 / 9.94 |
| `dist`, with the plugin system, laid out | **7.84** | **8.51** | 3.09 / 3.30 |

**With a plugin on** (a skill, a command, an agent and a trusted hook; no
`main` row, which has no plugins):

| | Just written | Read from disk | Of which its code |
|---|---|---|---|
| `release`, default order | 15.60 | 17.22 | 10.09 / 11.02 |
| `release`, laid out | **9.65** | **10.44** | 4.15 / 4.32 |
| `dist`, default order | 14.39 | 16.42 | 9.32 / 10.76 |
| `dist`, laid out | **8.29** | **9.08** | 3.13 / 3.31 |

`.text.idle` is 3.1 MB in the `dist` binary and 4.0 MB in the `release`
one, so the code resident at rest is now about the section, where it was
most of the binary. The anonymous part is the same in every row of a table:
the layout moves no data.

The "default order" rows are what the plugin system would have shipped as:
its code is in them, unlaid, and the figure is above `main`'s by whatever
the linker's order happened to cost that day.

## What it costs

- **Link time.** Rebuilding the `release` binary after an edit to
  `src/main.rs`: 1.07 s without the script, 1.84 s with it (rustc's bundled
  `rust-lld`). In a `dev` build, which has more functions left to match,
  1.35 s and 2.64 s — the reason `dev` builds are left alone. The script
  has one statement per group rather than per function: a linker walks its
  input sections once per statement, so all the patterns of a group share
  one walk.
- **Binary size.** 8 bytes in `release`, 72 in `dist`.
- **Nothing at run time.** The same functions, in a different order. If
  anything the ones that run together now share pages and TLB entries.

## What it leaves

**Look-alikes come along.** The legacy mangling spells every instance of a
generic function the same but for the hash — `RawVec<T,A>::grow_one` is one
name for hundreds of functions — so the pattern for the instance an idle
session runs takes the rest with it. In the `dist` binary the functions a
configured session ran are 1.43 MB and their patterns take 2.73 MB (1.25 MB
and 3.71 MB in `release`, where every crate keeps its own copy of each
instance): half of `.text.idle` and more is code that did not run and is
resident anyway. The `v0` mangling names an instance by its type arguments,
which would let the list name exactly what ran. It was weighed and left: it
is a compiler flag for every crate (a `.cargo/config.toml` that any
`RUSTFLAGS` in the environment silently replaces — the AUR build sets one),
and its names are longer, in a symbol table this binary deliberately keeps
(`docs/binary-size.md`).

**It is one profile of three states.** A session with MCP servers running,
with hooks of its own, or signed in to a provider whose wire format has a
startup step of its own runs functions no scenario here ran; they sit in
the ordinary code, as everything did before. The scenarios are the
script's (`scenario_configured` and its kin); one more is one more
function there and one more group in the list.

**A stale list degrades, it does not break.** A listed function that no
longer exists matches nothing. A function that newly runs at startup and is
not listed sits in the ordinary code and costs what any scattered function
costs — a 64 kB window — until the list is taken again, so the figure creeps
back toward the old one a window at a time rather than falling off a cliff.
Take the list again at each release and after a toolchain bump:

```bash
scripts/text_layout.sh profile            # builds release and dist, profiles both, rewrites the list
cargo build --release                     # links with it
scripts/text_layout.sh measure target/release/alter-zero
```

What cannot degrade quietly is the mechanism: `tests/text_layout.rs` checks
the list's form and the generator in every run of the gate, and, where the
binary under test was laid out, that `.text.idle` is really in it, ahead of
`.text`, executable and holding a plausible share of the code. CI's smoke
job forces the layout onto its `dev` build (`ALTER_ZERO_TEXT_LAYOUT=1`) so
that check runs on every pull request and the suite drives a binary linked
the way a shipped one is. A toolchain that mangled names another way would
show up there as a near-empty section.

**The rest of what is resident stays.** After the layout a configured
session of the `dist` binary is, read from disk, 3.30 MB
of code, 4.49 MB of other file-backed pages and
1.11 MB anonymous. Two parts of the binary's file were
looked at:

- `.rela.dyn` (0.47 MB; 0.78 MB in `release`) is read whole by the dynamic
  loader at startup, and `.data.rel.ro` (0.35 MB; 0.53 MB) is written by it
  — the price of a position-independent executable. Packed relative
  relocations (`-z pack-relative-relocs`) would shrink the first to a few
  tens of kilobytes, but a binary linked with them needs glibc 2.36 and the
  release is built against 2.35 on purpose (`docs/release.md`). A
  fixed-address executable would need neither section, and would give up
  address-space randomisation for the program's own code. Both refused.
- `.rodata` has no stable names to order by.

## Regenerating, and looking

```bash
scripts/text_layout.sh profile [--idle SECS] [--no-build] [--names DIR]
scripts/text_layout.sh measure [--runs N] [--first-run | --plugins] BINARY…
scripts/text_layout.sh measure --config-home DIR [--env KEY=VALUE]… BINARY…   # your own config home, copied
scripts/text_layout.sh section BINARY…    # .text.idle beside the rest of the code
ALTER_ZERO_TEXT_LAYOUT=0 cargo build --release   # the same build without it, to compare
ALTER_ZERO_TEXT_LAYOUT=1 cargo test --test text_layout   # the binary check, on a dev build
```

`profile` needs valgrind; everything needs tmux, python3 and binutils. A
`profile` is six sessions under callgrind, about a quarter of an hour once
the two binaries are built.
