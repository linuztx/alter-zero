# vendored crates

## `syntect/` — syntect 5.3.0 + the cyclical-grammar parser fix

Verbatim **syntect 5.3.0** (from crates.io) with the two hunks of
[trishume/syntect#709](https://github.com/trishume/syntect/pull/709) applied:

- `src/parsing/syntax_definition.rs` — `MatchIter::push_include` skips a context
  already on the include stack (a cycle), instead of pushing forever.
- `src/parsing/parser.rs` — `parse_line` bails out of a run of matches that never
  advance the cursor (a non-consuming `set` loop).

### Why

The Svelte grammar bat ships (from `corneliusio/svelte-sublime`) has cyclical
includes:

```
style-common  → style-lang-attribute → style-lang-decider-type → style-common
style-common  → style-lang-attribute → style-lang-decider-lang → style-common
script-common → script-lang-attribute → script-lang-decider-lang → script-common
style-lang-decider-type → script-common → script-lang-attribute → …
```

syntect's include walk had no cycle check, so `MatchIter`'s context stack grew
without bound. Highlighting a **five-byte** line (`  /**`) inside a Svelte diff
pinned a core at 100 % and the TUI never took input again — a resumed session
replays its whole transcript through the highlighter, so the app was wedged
before the first keystroke. `html` / `javascript` / `typescript` / `vue` parse
the same input in < 10 ms; only `svelte` loops.

`vendor/` is the interim home for the fix because **#709 is unmerged and targets
`syntect` master**, whose `parse_line` returns `ParseLineOutput` — the released
5.3.0 API differs, so the branch can't be consumed as-is. Delete `vendor/syntect`
and the `[patch.crates-io]` block in `Cargo.toml` once a syntect release carries
#709.

The regression test is
`highlight::tests::a_cyclical_grammar_does_not_spin_the_parser`, pinned to
`src/highlight/svelte_cycle.txt` (the 81-line prefix that wedges, gaps included —
the lexer state the earlier lines build is what triggers it).
