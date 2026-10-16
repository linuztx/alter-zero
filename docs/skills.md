# The `Skill` tool

Claude Code's skills, ported: folders of authored markdown the model can pull
into the conversation on demand. A skill is a *prompt* — domain knowledge,
a checklist, a workflow — that costs nothing until it is needed, because only
its one-line description sits in the context until the model asks for the body.

```
● Skill(dataviz)
  ⎿  Successfully loaded skill
```

That cell is the whole visible surface. What the *model* reads is the skill's
entire `SKILL.md` body; what the *user* sees is one green line saying it
loaded. The split is the point — a 400-line skill would otherwise dump itself
into the transcript every time it is used. Once loaded, the model reuses the
instructions while they remain in its context; repeated calls return a short
reminder instead of another copy of the body.

## On disk

A skill is a **directory** holding a `SKILL.md`, exactly the reference's
layout (`loadSkillsDir.ts` supports the directory form only, and so do we):

```
<root>/<skill-name>/SKILL.md
```

`SKILL.md` is YAML frontmatter over a markdown body:

```markdown
---
name: dataviz
description: Use when creating any chart, graph, plot or dashboard. Covers
  palette choice, axis and legend rules, and the stat-tile layout.
---

# Data visualization

Read `references/palette.md` before choosing colors…
```

`description` is required — it is the only thing the model sees before
invoking, so a skill without one could never be chosen (both references treat
a missing description as a load error, and so do we). `name` is optional and
defaults to the directory name; when present it must match `^[a-z0-9]+(-[a-z0-9]+)*$`
and be ≤ 64 characters, so a name is always safe to print, to match against a
tool argument, and to type after a `/`.

Anything else in the frontmatter is **ignored, not rejected** — a skill
authored for Claude Code carrying `allowed-tools:`, `model:`, `version:` or
`when_to_use:` loads here unchanged, minus the behaviour we don't implement.
That is deliberate: the ecosystem's skills are the reason to have skills at
all, and refusing to parse a file over a key we don't use would lock them out.
(`when_to_use` is the one such key we *do* read — the reference appends it to
the description in the listing, and it is pure prose, so it costs nothing.)

### Roots

In precedence order — the **first** root to claim a name wins, so a project
can shadow a personal skill of the same name, and a subdirectory can shadow
its own project:

| # | Root | Scope |
|---|------|-------|
| 1 | `<cwd>/.alter-zero/skills` | directory |
| 2 | `<cwd>/.claude/skills` | directory (compat) |
| 3 | `<project root>/.alter-zero/skills` | project |
| 4 | `<project root>/.claude/skills` | project (compat) |
| 5 | `{config_home}/skills` | personal (`~/.alter-zero/skills`) |
| 6 | `~/.claude/skills` | personal (compat) |

Rows 3–4 are the **`.git` walk-up** — `project_doc::find_project_root`, the
same discovery `AGENTS.md` uses — and they are skipped entirely when the
project root *is* the cwd, which is the common case: launched at the repo
root, the walk reads the same two directories it always did. They exist
because launching in `repo/src` must still find `repo/.claude/skills`; a
skill is a property of the project, not of whichever directory you happened
to start in. The cwd keeps precedence over the project root for the same
reason project keeps it over personal: the more specific root wins.

The personal root hangs off the **config home**, so it follows
`ALTER_ZERO_CONFIG_DIR` exactly like `hooks.json`, `permissions.json`,
`settings.json` and `config.json` do — one rule for where per-user state
lives, rather than a second one reading `$HOME` directly.

`ALTER_ZERO_SKILLS_DIR` **replaces** the whole list rather than adding to it.
That is the convention every other `*_DIR`/`*_FILE` override here follows
(`ALTER_ZERO_CHECKPOINTS_DIR`, `ALTER_ZERO_HOOKS_FILE`), and it is the only
shape that can make a test run hermetic: a merely-prepended root still leaves
the developer's own `~/.claude/skills` in every session, which is exactly how
`smoke.sh` Phase 36 broke on the first full run — a real personal skill's
listing displaced the context rows that phase asserts on. The suite now points
the variable at an empty temp dir.

The `.claude/` rows are the pragmatic half of the design: skills are portable
markdown with no tool-specific behaviour in them, the ecosystem writes them to
`.claude/skills`, and reading that directory costs one `read_dir` that usually
returns `NotFound`. A user who has already written skills for Claude Code gets
them here for free.

The project rows sit deliberately **outside** the `/trust` gate that guards
the project's hooks and MCP files (`docs/project-config.md`): a `SKILL.md`
is inert markdown until the model explicitly loads it, and any command in a
loaded body still meets the permission gate like every other tool call —
there is nothing here that executes on discovery.

### Built-in skills

Four skills ship with the binary: **`skill-creator`**, **`jina-reader`**,
**`web-search`** and **`yt-dlp`**. All use the same seed and discovery path
described below.

#### `skill-creator`

**`skill-creator`** teaches this
format — the frontmatter contract, where to write the folder, how to word a
description that actually triggers, and how to update an existing skill
without clobbering it.

It exists because the format is *ours*. A model asked to "write me a skill"
without it writes something plausible — a lone `my-skill.md` at a root, an
`allowed-tools:` line it expects to be honoured, a description that reads like
a persona — and every one of those fails silently: the walk only looks for
`<root>/<name>/SKILL.md`, and a skill that is never listed is a skill that is
never chosen. The rules are cheap to state and impossible to guess.

Authored in `prompts/skills/<name>/` and `include_str!`'d beside every
other markdown this crate carries, then **written into `{config_home}/skills`
at startup when the file is absent** — the agent definitions' rule
(`docs/subagents.md`), for the agent definitions' reasons: a default that is a
real file on disk can be read, edited and diffed, and a release that improves
it ships the improvement. `every_authored_built_in_file_is_embedded` keeps the
directory and the embedded list in step: a file authored under
`prompts/skills/` but left out of `BUILTIN_SKILL_FILES` fails the test suite
instead of quietly missing from every install (a script's `test_*` unit tests
stay out on purpose). The seed runs *before* the walk, so the session that
installed the app can already use it. An edited copy is never overwritten; a
deleted file comes back next launch, so the off-switch is `/skills` (which
persists) rather than `rm -rf`.

One difference from the agent definitions: `ALTER_ZERO_SKILLS_DIR` is **never
seeded into**. That variable replaces the root list — it says "these are the
skills, and only these" — and a built-in skill is a convenience the session
works without, where a built-in *agent type* has to resolve because
`general-purpose` is the `agent` schema's default. So the override is honoured
literally: nothing of ours is written into a directory the user curates, and a
hermetic run (`smoke.sh`) keeps the empty root it made.

#### Why `skill-creator` is two files

The skill is a directory holding `SKILL.md` **and** `reference.md`, and the
split is forced by the loader itself. A body is rendered through the
`${…SKILL_DIR}` expansion before the model sees it — so a body that
*documents* those tokens has them rewritten out from under it. The first live
run of an earlier draft is the evidence: the sentence naming both
`${…SKILL_DIR}` spellings arrived as the same absolute path twice, explaining
nothing. (`$ARGUMENTS` was caught by the very same trap in that run, reaching
the model as `` `create commit-style` ``; retiring the `args` parameter is
what makes that token safe in a body today, and the skill-dir pair the only
one left that is not.)

Detail that has to survive verbatim therefore lives in a sibling file the
model **reads** — which is also the multi-file pattern the skill teaches, so
the built-in demonstrates it rather than only describing it.
`no_built_in_body_carries_a_placeholder_the_loader_would_eat` keeps a body
from re-acquiring one: it renders every built-in and requires the body back
byte-for-byte.

#### `jina-reader`

**`jina-reader`** teaches the agent to fetch public webpages and PDFs through
`https://r.jina.ai/` using curl. Mention `$jina-reader` with a URL, or ask to
read a webpage and let its description select it. No Jina API key is required
for basic access; an existing `JINA_API_KEY` can provide optional authentication.
The skill requires curl, not a new dependency or a native web-fetch tool.

Its single `prompts/skills/jina-reader/SKILL.md` covers extracted Markdown,
plain text, rendered HTML, JSON envelopes, screenshot URLs, selectors,
client-rendered pages, hash routes and PDFs. It separates response formats
from JSON envelopes and screenshot links from image bytes, and instructs the
agent to inspect responses before summarizing and cite the original URL.

Loading it performs **no request**. Subsequent curl calls use the ordinary
shell tool and permission gate. Reader is a third-party service: the skill
forbids sending private/internal URLs, credentials, signed links, cookies or
sensitive content without explicit authorization. Retrieved content is
untrusted data, not instructions. Optional keys must never be logged or sent
to screenshot download hosts; failures and rate limits require bounded,
non-automatic retries rather than an assumption that HTTP success means
complete extraction.

Like `skill-creator`, it is editable at `{config_home}/skills/jina-reader/`,
never overwrites an existing file, respects project-root precedence and can
be disabled per project through `/skills`. An `ALTER_ZERO_SKILLS_DIR` override
receives no built-ins. Offline regression tests exercise the actual
seed → discovery → listing → tool-load path and verify that local edits
survive reseeding and deleted files are restored.

#### `web-search`

**`web-search`** gives the agent web search without a native search tool:
four standard-library Python scripts in its `scripts/` directory, each
reaching one provider's free tier, so no key is needed and nothing is
installed. Mention `$web-search`, or ask to search the web and let the
description select it. Where `jina-reader` reads a URL you already have, this
one finds the URLs (and can read them too).

| Script | Reaches | Free tier | Optional key |
| --- | --- | --- | --- |
| `exa_search.py` | Exa's hosted MCP server, `https://mcp.exa.ai/mcp`: `web_search_exa`, `web_fetch_exa` | a daily quota per address; a shared cloud address may get none | `EXA_API_KEY` |
| `parallel_search.py` | Parallel's Search MCP, `https://search.parallel.ai/mcp`: `web_search`, `web_fetch` | rate limited per `session_id` | `PARALLEL_API_KEY` |
| `tavily_search.py` | Tavily's hosted MCP, `https://mcp.tavily.com/mcp/`: `tavily_search`, `tavily_extract` | a monthly quota per address | `TAVILY_API_KEY` |
| `jina_web_search.py` | Jina Reader (`r.jina.ai`) fetching a search engine's own results page | about 20 requests a minute | `JINA_API_KEY` |

A key is read from its environment variable and sent only to its own
provider, as a bearer token (`x-api-key` for Exa's REST API); the body asks
for a `/secrets` placeholder in front of the command and forbids passing a
key as an argument, printing it, or tracing with `set -x`.

**The three MCP clients share `mcp_http.py`**: one bare, stateless JSON-RPC
`tools/call` POST over streamable HTTP (no `initialize`, no session), the
answer decoded whether it arrives as one SSE event (Exa, Tavily) or as plain
JSON (Parallel), and a failure classified: a 429, a JSON-RPC error that says
it is a rate limit (Parallel answers its free-tier limit with HTTP 200), Exa's
`_meta["ai.exa/rateLimited"]` flag or Tavily's cap document is **rate
limited**, printed as `rate limited (retry in 15m):` over the server's own
message, word for word; a 401 **needs a key**, and anything else is an error
carrying the server's text. Those errors are all that goes to stderr: a line
per request (endpoint, status, size, time) cost the agent tokens on every
search and filled the collapsed tool cell's few rows. Each script adds only
its provider's request shape and result rendering. What the live services taught them, recorded as
tests against the live response shapes:

- **Cloudflare in front of `mcp.exa.ai` refuses urllib's default
  `Python-urllib/…` user agent** (error 1010), so every request names itself,
  and a JSON body that is not a JSON-RPC response (that refusal) is an error
  instead of empty output and exit 0.
- **Tavily answers a keyless client with a 401 OAuth challenge** unless the
  request carries its documented `X-Tavily-Access-Mode: keyless` header, and
  it reports failures inside HTTP 200, `isError: false` results: a bad key as
  an `error` document, the keyless cap as `{code, message, window,
  retry_after_seconds, next_actions}`. Both are failures, never results.
- **A spent quota offers ways around itself**: x402 pay-per-call, signing up,
  bonus credits for answering survey questions. The scripts take none of
  them, and the body tells the agent never to either. The message naming
  them still reaches the agent unchanged, so it reads the limit as the
  provider states it. Exa's REST API is the same story without MCP
  (`--rest`: HTTP 402 under x402, $0.007 a search), which is why the body
  never mentions that flag.
- **Everything a provider returns is printed, uncut.** Parallel answers a
  search with ten results, about 16 KB of excerpts, and its `web_fetch` with
  no objective sent python.org's downloads page back as one 70,105-character
  excerpt; all of it reaches the agent, so it sees the whole context the
  provider returned. A URL Parallel could not read comes back in the
  document's `errors` rather than its results, and is listed last as a
  `Failed:` line, as Tavily's are.

**The Jina script searches the engines that survived testing.** More than
thirty engines and variants were fetched through Jina Reader without a key,
from a datacenter address, and each survivor was then run against many
distinct queries:

| Engine | How it is read | Pages |
| --- | --- | --- |
| `duckduckgo` (default) | `html.duckduckgo.com` result markup | its "Next" form, replayed as a GET with its `vqd` token |
| `yahoo` | `div.algo` blocks, the `/RU=` redirect unwrapped | its `a.next` link |
| `wikipedia` | the MediaWiki search API's JSON | `sroffset` |
| `github` | the repository search page's embedded JSON | `p` |
| `arxiv` | `li.arxiv-result` markup, relevance order | `start` |
| `hackernews` | the Algolia search API's JSON | `page` |

Google, Bing, Brave, Startpage, Mojeek, Ecosia, Yandex, Qwant, AOL, Ask and
others were left out: they answer with bot checks or script-only pages, or
Jina refuses them anonymously. YouTube's results page answered too, but the
script deliberately does not search it. `--time` maps to each engine's own
filter (DuckDuckGo's `df`, Yahoo's `btf`, GitHub's `pushed:>`, Hacker News's
`created_at_i`); Wikipedia and arXiv say they ignore it. Three findings shape its request: the **whole target URL is quoted into
Jina's path**, since Jina reads its own query string as its options (Hacker
News's `page=0` came back as Jina's "page must be >= 1") and percent-decodes
the path once; **no `X-Engine` header**, since Jina's `curl` engine decoded
entities inside Wikipedia's JSON; and **Yahoo needs `fr=fp-tts`**, without
which it mostly answered with its temporary-problems page. A cached bot-check
page is retried once past Jina's cache, a 429 waits for its `Retry-After`
once (giving up past 20 seconds), Jina's intermittent anonymous refusal and
network failures retry once, and nothing retries twice. A page an engine no
longer lays out as expected is an error ("does not recognise"), never an empty
result.

**Which script to use was measured, not assumed.** For one ordinary question
("latest stable rust release version") Exa returned the newest release with
its date in under 2 KB, Parallel's top hit was a release behind in 13.5 KB of
excerpts (a documentation search ran to 39 KB of them), and Jina's DuckDuckGo
page was 3 KB of one-line snippets. So the body sends most searches to Exa,
research questions worth several phrasings to Parallel, link lists and
Wikipedia, GitHub, arXiv or Hacker News to Jina, and names a fallback order
(Exa, Parallel, Tavily, Jina) for when a free tier refuses: each quota is
independent, and a shared address can spend one, as the development
container's did Tavily's monthly cap.

**Parallel asks for one `session_id` per conversation** ("Do NOT change it
between turns"), because its free tier rate-limits on it. Nothing hands the
agent's shell a session identifier, so the script mints one per run unless
`PARALLEL_SESSION_ID` is set, and the body asks the agent to pick one value
and reuse it for the conversation rather than sidestep the limit.

The scripts are seeded beside the `SKILL.md` as
`{config_home}/skills/web-search/scripts/` — written like every seeded file,
without the executable bit, so the body runs each with `python3`, building
the path from the `Base directory` header since a built-in body cannot use
the `${…SKILL_DIR}` placeholder. Three of them import `mcp_http.py` from
their own directory, so each puts that directory on `sys.path` itself:
Python does it by default, but not in isolated mode (`-I`) or under
`PYTHONSAFEPATH`, and an agent may well run a script with `-I`. They also set
`sys.dont_write_bytecode`, since that import would otherwise leave a
`__pycache__` folder in the user's skill directory. Their unit
tests stay in the repository beside them and are not seeded: recorded
response shapes, trimmed copies of each engine's pages, a fake fetcher, no
network (`python3 -m unittest discover -s prompts/skills/web-search/scripts
-p 'test_*.py'`, run by CI). `JINA_WEB_SEARCH_LIVE=1` adds one real search
per Jina engine, and `--list` on an MCP script prints the provider's current
tool schema: the ways to re-verify a provider when it starts failing.

Loading the skill makes no request; each search is an ordinary shell call
through the permission gate, and the body keeps searches small and forbids
searching for secrets, private data or internal host names, since every query
reaches a third party. Like the other built-ins its seeded copy is editable,
a deleted file comes back, project skills shadow it, `/skills` disables it,
and an `ALTER_ZERO_SKILLS_DIR` override is never seeded. Smoke Phase 102
checks that the first launch seeds every file, that each seeded script
starts, and that a deleted script is restored.

#### `yt-dlp`

**`yt-dlp`** guides downloads of authorized online videos and playlists,
subtitles/transcripts and audio extraction with the external `yt-dlp` CLI.
Mention `$yt-dlp` or ask to save media; its listing covers format/quality
selection and access troubleshooting. The single
`prompts/skills/yt-dlp/SKILL.md` checks for `yt-dlp`, `ffmpeg` and `ffprobe`
before promising conversions, defaults to a single item unless a playlist was
requested, and inspects formats and subtitle languages when needed. Its recipes
cover playlist archives, resolution ceilings, MP4 preferences, audio extraction
and captions; the agent checks the actual output files before reporting success.

The skill **does not install or bundle** `yt-dlp`, `ffmpeg` or `ffprobe`, and
loading it does not download anything. Later CLI calls use the ordinary shell
and permission gate. Browser cookies require asking first; credentials must
not be logged or copied into the download directory. It does not claim to
bypass DRM or access controls. Like the other built-ins, its seeded copy is
editable, project skills can shadow it, `/skills` can disable it, and an
`ALTER_ZERO_SKILLS_DIR` override is never seeded.

### The walk re-runs every turn

Discovery runs at startup **and at every turn start** —
`Session::rescan_skills`, right beside the `AGENTS.md` refresh and for the
same reason. A startup-only walk could not notice a skill that had just been
*added*, including one the agent had written for you one turn earlier. The
repeated walk keeps discovery and descriptions current. The tool reads the
body on its first successful load into the agent's context, so edits before
that load are observed; after loading, it reuses the instructions already in
context until that body is removed. Discovery does not reload bodies into
the conversation.

The cost is four to six `read_dir`s and one small read per skill, against a
turn that is about to make a network request. Two things follow from the new
set beyond the registry itself, and both matter:

- **The listing is re-rendered**, so this turn's context carries it. A
  listing that *changed* does invalidate the prompt cache behind it — but a
  listing that changed is one where the answer genuinely differs, and an
  unchanged listing renders byte-identically, so the steady state is
  unaffected.
- **The backend is rebuilt only when the tool set changes.** The `skill` spec
  is decided at `with_skills` time, so the *first* skill appearing (or the
  last one going away) has to re-attach; `ModelSession::refresh_skills`
  compares its recorded `skills_attached` against the live verdict and does
  nothing on the turns — nearly all of them — where the answer is the same.
  Rebuilding unconditionally would re-derive the whole backend every turn for
  nothing.

The `/settings` **Skills** row's availability is re-derived too, since "did
anything load" can now flip mid-session.

Discovery never fails a session: an unreadable root is skipped, and a
`SKILL.md` that won't parse is collected as a [`SkillError`] and surfaced as a
one-row toast naming the file and the reason
(`Skill /p/.claude/skills/broken/SKILL.md: missing YAML frontmatter delimited
by ---`, `(+N more)` when there are others), never a panic and never
silence — going quiet is what makes "my skill isn't being used" unanswerable.
Because the walk now repeats, so would the toast: [`unreported_errors`] keeps
it to **once per file**, and the reported set is re-seeded from each walk, so
a file that is fixed and broken again reports again.

## The listing

Every turn, the derived context leads with one `<system-reminder>` whose
sections are the project doc (AGENTS.md, `docs/project-doc.md`), then the
skill listing, then the subagent types the `Agent` tool can launch
(`docs/subagents.md`; any section may be absent — the wrapper is
`reminder::reminder_message`, the skills section is `skills::skill_section`,
and `skills::listing_message` is that section wrapped alone, which is what a
launched subagent gets):

```
<system-reminder>
Use the following contexts and instructions:

Codebase and user instructions are shown below. Be sure to adhere to these instructions. IMPORTANT: …

Contents of /work/my-app/AGENTS.md (project instructions, checked into the codebase):

…

The following skills are available for use with the Skill tool:

- commit: Create a git commit with staged changes
- dataviz: Use when creating any chart, graph, plot or dashboard…

Available agent types for the Agent tool:

- general-purpose: General-purpose agent for researching complex questions… (Tools: *)
- explore: Read-only search agent for broad fan-out searches… (Tools: Bash, Read, Skill, mcp__*)
</system-reminder>
```

It is a **leading fragment**, not history: the two listing sections are
rendered at the boundary into `App::listings` (`subagents::listing_sections`)
and `context::context_messages_full` wraps them, behind the instructions
section, into the one block in front of the conversation — so Ctrl+D shows
it, `App::estimate_context_tokens` counts it, and it survives a `/compact` at
the front exactly like the project doc. It never enters `history`, so it
cannot be backtracked past or recorded twice.

Its position is *after* the AGENTS.md section and before everything else,
which is a prompt-cache decision: every section is re-rendered per turn, and
one that moves invalidates every token behind it.

The listing is budgeted like the reference's `formatCommandsWithinBudget`:
each entry's description is capped at [`MAX_LISTING_DESC_CHARS`] (250), and if
the whole listing still exceeds [`listing_budget`] — 1% of the model's context
window, in characters, the reference's `SKILL_BUDGET_CONTEXT_PERCENT` — the
descriptions are trimmed to an even share, degrading to names-only rather than
dropping skills. A skill you can't see is a skill you can't invoke; a skill
with a short description is merely a worse match.

## The tool

Offered as `skill` (lowercase on the wire like every other tool here,
displayed as `Skill`) **only when at least one skill loaded** — an empty
listing means the tool has nothing to do, and both references omit it too.

```json
{"skill": "dataviz"}
```

`skill` is the whole schema. The reference's optional `args` string is
**deliberately absent** — see [Why there are no arguments](#why-there-are-no-arguments)
below.

The body the model receives is prefixed with its own directory so relative
references resolve:

```
Base directory for this skill: /home/u/.claude/skills/dataviz

# Data visualization
…
```

`${ALTER_ZERO_SKILL_DIR}` / `${CLAUDE_SKILL_DIR}` in the body expand to that
same directory (the second spelling for ecosystem compatibility). That
expansion is the **only** rewrite the loader performs; everything else in a
body reaches the model exactly as its author wrote it.

### Load once while the instructions remain in context

The tool description briefly tells the model when to load a skill, recognizes
`$<name>` mentions, points to the `<system-reminder>` for valid names, and says
to load each skill only once while its instructions remain in context.
The executor enforces that rule too: a repeated call succeeds with only:

```text
Skill dataviz is already loaded. Follow its instructions; do not load it again.
```

It neither rereads `SKILL.md` nor appends its body again. This saves the body
tokens even when a model ignores the description. The normal name and enabled
skill checks still run first, and a failed read or parse does not mark a skill
as loaded, so a repaired file can be retried.

`LoadedSkills` belongs to one agent's running turn. It is seeded from that
agent's incoming messages, pairing assistant `skill` call IDs with tool
results containing a rendered body and matching its directory header to the
current registry. Matching accepts raw and secret-redacted headers, so normal
result masking cannot bypass deduplication. A user quoting a skill, a failed
call, or an already-loaded reminder does not establish a load. Directory
identity keeps aliases and hook rewrites from causing a second body load.
Successful loads update the set
immediately, covering repeats within one batch and later rounds.

Rebuilding this set from the actual context also covers later turns, resumed
sessions, backend rebuilds, and subagent continuations without a separate
persistent cache. Each subagent uses its own messages, so a lead's load cannot
suppress the subagent's first load. If compaction, `/clear`, or backtracking
removes the body, the skill can be loaded again. A process-wide or
session-lifetime cache would incorrectly block that recovery; prompt guidance
alone would still allow duplicate bodies.

If a secret is renamed, removed, or changed after masking a directory header,
that older header may no longer identify the skill. A fresh load is then
allowed rather than suppressing a different skill on uncertain evidence;
subsequent calls recognize the new result normally.

### Why there are no arguments

The reference's schema carries an optional `args` string, substituted into the
body for `$ARGUMENTS` and for `$1`…`$9` split on whitespace, with a trailing
`Arguments: {args}` line appended when the body named no placeholder. All of
it is gone. Three things were wrong with it:

- **It rewrote the body's own prose.** The substitution pass ran over the
  whole text — prose, code fences, examples alike — so a skill could not
  *document* the tokens it was written to use. That is not a hypothetical: the
  built-in `skill-creator` had to be split into two files over it, because a
  live run read `` `create commit-style` `` where its `SKILL.md` said
  `$ARGUMENTS`. A body that means `$ARGUMENTS` literally now says so.
- **Nothing supplied it.** A skill is loaded from the model's own tool call or
  from a `$name` mention (`docs/skill-mentions.md`), and a mention is plain
  text carrying no parameter. The field existed for the model to fill in
  freehand — an invented value, substituted into instructions the user wrote,
  with no user in the loop to see it happen.
- **It cost a decision on every call.** A parameter in the schema is one the
  model weighs and fills; the tokens are spent whether or not the skill has
  any use for them.

What a skill needs to vary per run belongs in its body — *ask the user which
branch to review* — where the instruction is visible, rather than in a
parameter the model quietly guesses at.

A call recorded before the change still replays cleanly: the executor's
`SkillArgs` ignores unknown fields (serde's default), so an old rollout's
`{"skill": "x", "args": "y"}` loads `x` and drops the rest.

### Why the result is a two-text split

`ToolOutcome::context` already exists for exactly this shape — "the cell shows
one thing, the model reads another" — built for `AskUserQuestion`
(`docs/ask.md`). A skill call reuses it wholesale rather than growing a
parallel mechanism:

- `output` = `Successfully loaded skill` → the committed cell's `⎿` row;
- `context` = the whole rendered body → what the tool call returns to the model.

Everything downstream is inherited and needs no new code: the green
`StreamEvent::ToolAnswered` path, `ToolCall::context_output` recording,
`context::context_messages` replaying the body as the `tool` result on every
later turn, the rollout round-trip so a `/resume` restores it, and Ctrl+D
showing what was actually sent. The reference achieves the same thing by
injecting a separate user message after the tool result; we don't need one,
because a `tool`-role result is already a place a long text can live here.

**Ctrl+O keeps the one line too.** The transcript is the record of what
happened; Ctrl+D is the record of what was *sent*. That is already the rule for
every other two-text call here — a permission-rejected `write` expands to its
red `User rejected …` display, never the stop-and-wait text the model read —
and it is the reference's behaviour as well. It also keeps a 100 KiB skill body
out of the transcript render cache, which is rebuilt per commit.

The replayed call carries `{"skill": "<name>"}`: history stores the one-line
summary, and for this tool the summary *is* the name. Since `skill` is now the
whole schema, that replay is **exact** — nothing is lost — where it used to be
lossy for the optional `args` (a provider that validates the schema rejects
the `skill({})` an unmapped tool would have replayed, which is why the
reconstruction exists at all).

The body is capped at [`SKILL_BODY_MAX_BYTES`] (100 KiB — the reference's
`maxResultSizeChars`), truncated with a marker rather than refused.

A call naming an unknown skill resolves **red and recoverable** with the
available names listed, so the model can correct itself in the same turn
rather than ending the round on an error.

### What it deliberately does not do

The reference's `context: fork` (run the skill in a subagent), `allowed-tools`
(widen the permission set for the skill's duration), `model:`/`effort:`
overrides, hooks-in-frontmatter, and `!`-shell interpolation in the body are
all **parsed-and-ignored**. Each is a permission or a control-flow decision
wearing a markdown file's clothes, and this TUI already has explicit,
user-visible seams for all of them (`/settings`, Shift+Tab, `docs/permissions.md`,
`docs/agent-tool.md`). A skill here is *text*, and text cannot escalate.

That is also why a skill invocation raises **no permission prompt**: nothing
runs. The body it loads is subject to every gate it then tries to pass
through — a skill telling the model to run `rm -rf` still meets the permission
modal at the `bash` call.

## `$<skill-name>`

The user-invocation shorthand is the `$` mention, and it needs no code of its
own. A submitted message carrying `$haiku-writer about tmux` is an ordinary
user turn; the **Skill tool's own description** tells the model a `$<name>`
mention is a request to run that skill, and the listing tells it which names
exist — so it answers with a `skill` call naming it. The rest of the line
needs no parameter to carry it: it is already in the user's message, right
there in the same context as the body the call loads. (The guidance lives on
the tool because it rides every request the
tool does — the retired `prompts/tools.md` note and the listing's old closing
sentence each said it a second time, per turn.) Verified end to end against a
live model:

```
❯ $haiku-writer about tmux
● Skill(haiku-writer)
  ⎿  Successfully loaded skill
● Grid of terminals, …
```

The user-invoked and model-invoked paths therefore converge on one
implementation rather than two. The composer's **`$` skill-mention picker**
(`docs/skill-mentions.md`) feeds the same rails: typing `$` anywhere in a
message fuzzy-completes the discovered skills' names and Tab/Enter insert
the `$<name>` mention.

What is **not** implemented is palette *autocomplete*: typing `/` lists only
the built-in commands, not the discovered skills. `app::COMMANDS` is a `const`
array and `matching_commands` hands out `&'static SlashCommand`, so mixing in
runtime-discovered entries means making that surface owned — a refactor
reaching `CommandMenu`, `ui::menu`, and their tests, for discovery alone.
The `$` picker covers the discovery gap from the mention side — every skill
name is completable there — and nothing here forecloses the palette half.

## Turning one skill off: the `/skills` menu

`/skills` opens the **fifth composer-replacing inline picker**. Its list is
deliberately the `/settings` menu's twin rather than a new shape — same frame,
same `❯` type-to-search, same aligned `{label}  {value}` column with the same
two-tone colouring, same `(n/total)` counter, same Enter/Space grammar — under
the header the `/hooks` and `/mcp` pages open on:

```
  Skills
  3 skills · 2 enabled

  ❯

→ commit-helper        ✔ enabled
  haiku-writer         ◯ disabled
  startup-hook-skill   ✔ enabled
  (1/3)

  Write a git commit message in this project's house style…

  Type to search · Enter/Space to enable/disable · Esc to cancel
```

Only the rows differ: one per discovered skill instead of one per knob, with
the skill's **own description** as the line under the list — so the picker
doubles as the browser that answers "what is this skill actually for?", which
is the `/hooks` menu's value in the shape of the `/settings` menu.

The header and the value column answer "what is on?" before a row is read:

- **The title** wears the pages' one title dress (`header::page_title_line` —
  bold, in the active theme's banner gradient), so `/skills` reads as one of
  the composer-replacing pages rather than a bare list.
- **The count line** — `3 skills · 2 enabled`, dim — counts what is
  *installed*, never what the search shows: the `(n/total)` counter already
  follows the query, so a header that shrank with it would say the same thing
  twice. The enabled count is taken over the discovered skills
  (`SkillsMenu::enabled_count`), never as `skills - disabled`, because the
  off-set keeps names that are not installed here (see *Where it persists*).
  An empty catalog reads `0 skills` alone.
- **Each value leads with a mark** — `✔ enabled`, `◯ disabled` — the `/login`
  rows' own pair, so the two lists that answer "is this on?" answer it with
  one glyph. Only the check is coloured (the `/model` picker's green), the
  circle stays dim, and the word keeps the `/settings` two-tone. When a row is
  too narrow for the whole value the word `…`-cuts and the mark keeps its
  seat — the `/mcp` rows' rule, since the mark *is* the state.

Two notes the `/settings` menu has no need of:

- a **session-off note** in the header, under the count, when the `/settings`
  **Skills** row is off, so browsing and toggling with the master switch down
  never reads as "my toggles do nothing". It says what state the whole catalog
  is in, so it belongs to the header rather than between the search line and
  the rows it filters. It costs a row only when shown and it wraps, which is
  why the search line has no fixed row: the caret's seat is found in the built
  page (`model_view::view_prompt_row`, `/login`'s rule). It is never shown
  over an empty catalog — with nothing installed that row is unavailable, so
  the session always reads as off there, and "turn them on in /settings" is
  advice the row refuses;
- an empty list that **names the roots** rather than just saying "none" —
  `No skills found. Add one at:` over `~/.claude/skills/<name>/SKILL.md` — since
  "why isn't my skill here?" is the only question an empty list ever raises.

### A terminal too short for the page

The whole page is thirteen rows of chrome plus the rows and the description:
twenty for six skills. A shorter terminal gets a **compact** page instead —
the `/theme` picker's (`docs/theme.md`), over this list — and codex's order of
what yields: *"optional gaps and previews yield first"*, while a header stays
for as long as it fits (`selection_picker_layout.rs`). Here the description
yields first — cut to one row on a terminal just short of the wrapped one,
then gone with the `(n/total)` counter:

```
──────────────────────────────────────────────────────────

  Skills
  6 skills · 6 enabled

  ❯
  ↑ 1 more above
  dataviz         ✔ enabled
→ haiku-writer    ✔ enabled
  jina-reader     ✔ enabled
  ↓ 2 more below
  Type to search · Enter/Space to enable/disable · Esc to…

──────────────────────────────────────────────────────────
```

and on a terminal too short for that too, the header follows them — the
title, the count and the session-off note together, since the note is part
of the header:

```
──────────────────────────────────────────────────────────

  ❯
  ↑ 1 more above
  dataviz         ✔ enabled
→ haiku-writer    ✔ enabled
  jina-reader     ✔ enabled
  ↓ 2 more below
  Type to search · Enter/Space to enable/disable · Esc to…

──────────────────────────────────────────────────────────
```

The search line, `COMPACT_LIST_ROWS` (3, the whole family's compact window)
rows and the hint always stay. Before this the page bottom-anchored on a short
terminal like every framed view, which kept the description, the hint and the
bottom of the list on screen and flowed the search line and the top of the
list into scrollback — so the row Enter would toggle could be off screen
(`a_short_terminal_keeps_the_skill_choices_on_screen`). The overflow rows are
codex's reserved arrow slots, as on `/theme`: the `↑` directly under the
search line, the `↓` directly over the hint, each blank when nothing is hidden
that way, so the page is one height at every selection; and the value column
is sized off **every** listed row rather than the three in view, so it holds
still as ↑/↓ scroll the window
(`the_compact_page_holds_still_across_selections`). A list with nothing to
choose from — a search matching nothing, an empty catalog — has no window to
compact, so only its header yields.

The choice is the page builder's: `skills_view_lines` takes the rows the page
may have and builds the first of four pages that fits them
(`first_page_that_fits`) — whole, whole with the description cut to one row,
compact with the header, compact without it. The cut step is there because
descriptions wrap to different heights: the whole page's height moves with
the selection, so a terminal at the edge, deciding on that height alone,
flipped between the whole and the compact page as ↑/↓ crossed a long
description. Cut to one row it is one height at every selection, and the
list shrinks only on a terminal too short for that
(`a_long_description_is_cut_to_one_row_before_the_list_shrinks`). Those rows
are the terminal's (a
composer-replacing view keeps its rows and the streaming strip above it gives
way, `view_split`), and every reader of the page — the reserved height, the
paint, the cursor seat, the flow — passes either that height or the region
already reserved for the page, which always builds the same page. The cursor
seat already finds the search line in the built page (`view_prompt_row`), so
it follows the header away for free. On a terminal shorter still the compact
page sheds what is left one row at a time and never its search line
(`docs/view-flow.md` *Below the compact page*) — the gaps, the `↑`/`↓` pair,
the rules, the rows down to the highlighted skill, the hint — so the page
fits at every height and nothing flows; an empty list puts its placeholder
in the list's place and sheds the same way. `smoke.sh` Phase 77b drives all
three pages and a toggle on the compact one, Phase 138 the menu on 7 rows.

### What a toggle changes

Disabling has to hold in **two** places, and does:

- the skill leaves the `<system-reminder>` listing, so the model never chooses
  it;
- `SkillRegistry::find` refuses it, so a model that remembers the name from an
  earlier turn gets the recoverable "unknown skill" error instead of loading
  something the user turned off.

`SkillRegistry::snapshot` still returns it — the menu must show a disabled
skill, or you could never turn it back on. That is why the registry answers
two different questions: `is_empty` ("was anything **found**?", the
`/settings` row's availability) and `has_enabled` ("is anything **on**?",
whether the `skill` tool is offered at all). Turning every skill off withdraws
the tool exactly as having none installed does — the same rule, since a tool
that can only answer "unknown skill" is worse than no tool. That withdrawal is
also why a toggle rebuilds the backend: the registry handle is shared, so the
executor and the listing see the change immediately, but the **tool set** is
decided when `with_skills` runs.

### Where it persists

`{config_home}/skills.json`, `permissions.json`'s shape exactly:

```json
{ "projects": { "/abs/cwd": { "disabled": ["haiku-writer"] } } }
```

Per **project**, because skill relevance is project-specific: a `dataviz`
skill earns its listing tokens in an analytics repo and not in a kernel
driver. The session-wide switch already exists as the `/settings` **Skills**
row, so this file is the finer scope rather than a second copy of the same
decision. Writes are a read-modify-write (another project's entry written
meanwhile survives) and best-effort (a read-only home never kills the TUI); an
empty set **drops** the project's entry, so re-enabling the last skill leaves
no residue and the file stays a diff from "everything on".

A disabled name that isn't installed here is **kept** rather than pruned: the
same file serves a checkout on another machine where that skill does exist,
and pruning on load would silently re-enable it there.

### Verified end to end

Against a live model, with three skills on disk:

| step | observed |
|---|---|
| disable `haiku-writer`, ask for a haiku | no `Skill(…)` cell, and the reply **lost the `— alter-zero` signature** the skill body mandates — the body never reached the model |
| Ctrl+D | the `<system-reminder>` lists the other two only |
| force `skill(haiku-writer)` while others are on | red `⎿ Unknown skill: haiku-writer. Available skills: commit-helper.` — recoverable, and the "available" list excludes every disabled one |
| disable **all** | no listing at all, and the tool is withdrawn (`⎿ unknown tool: skill`) |
| restart | the menu opens with the same skills off |
| re-enable | the skill loads again and the signature is back |

The built-in `skill-creator` has its own two live runs (`tests/live_openrouter.rs`,
on the `a0_venice` key):

| test | observed |
|---|---|
| `live_the_built_in_skill_creator_writes_a_skill_this_crate_can_load` | the prompt never names the skill — the **description alone** earns the load — and the `commit-style/SKILL.md` the model then writes is one this crate's own walk discovers and parser accepts |
| `live_the_skill_creator_sends_the_model_to_its_reference_file` | asked for a skill that takes an argument, the model follows the body's pointer, `Read`s `reference.md`, and writes a body using a live `$1` — the indirection works on a real model, not just on paper |

The second is the one worth keeping: the reference file only earns its
existence if a model actually opens it.

The signature is the useful probe: it is *in the skill body and nowhere else*,
so its presence proves the body reached the model and its absence proves it
didn't — better evidence than the cell, which only shows what the TUI drew.

## Settings

`/settings` gains a **Skills** row (`SettingKey::Skills`): on by default,
`false` drops the tool and the listing at the next turn — the row reports
`false (unavailable)` when no skill loaded, since there is nothing to turn on.
`ALTER_ZERO_SKILLS=0` seeds it off for the session. It is the session-wide
switch; `/skills` is the per-skill one.

`/settings` → **Tools** = `false` withdraws the whole tool set, `skill`
included — so it withdraws the listing too. That is
`SessionSettings::skills_offered`, the one gate the listing and the tool set
share, and it is deliberately *not* `skills_active`: the **Skills** row keeps
its own value (Tools must not rewrite it), while what reaches the wire is the
conjunction. Gating the listing on the Skills row alone left a
`<system-reminder>` naming a `skill` tool the request never carried — a dead
end the model spends a round hunting for. That is one invariant with three
enforcement points, and this is the third: the rescan's conditional rebuild
keeps the tool set current, `skills_offered` keeps the listing and the tool
set on one gate, and `subagent_skill_reminder` (below) carries the pair onto
the subagent surface. The `/compact` summarization turn runs on the
tools-free backend and so carries no listing either.

Because a `skill` call goes through the ordinary tool loop, it also meets the
lifecycle hooks: `PreToolUse` can block or rewrite one, `PostToolUse` sees the
load. Claude Code's spelling is aliased, so `{"matcher": "Skill"}` selects it
here unchanged (`hooks::claude_code_alias`, `docs/hooks.md`).

## Module layout

| Where | What |
|---|---|
| `src/skills.rs` | **pure**: `SkillMetadata`, frontmatter parse, name validation, listing + budget, body render, the `SkillRegistry` handle |
| `prompts/skills/` | embedded and seeded built-ins: `skill-creator` (`SKILL.md` + `reference.md`), `web-search` (`SKILL.md` + five `scripts/*.py`, their unit tests beside them unseeded), `jina-reader` and `yt-dlp` (one `SKILL.md` each) |
| `src/llm/skill.rs` | root resolution, the `read_dir` walk, the built-in seed (`seed_builtin_skills`), context-derived `LoadedSkills`, and the tool executor (`run_skill_tool`) |
| `src/llm/tools.rs` | `skill_spec()`, `display_name`, `summarize_call` |
| `src/llm/backend.rs` | `with_skills` — the `with_tasks` pattern |
| `src/context.rs` | the leading listing fragment |
| `src/tui/bootstrap.rs` | the built-in seed, then discovery at startup, and the failure toast |
| `src/tui/models.rs` | `with_skills` on every backend rebuild, `sync_skill_listing` |
| `src/app/skills_menu.rs` | the `/skills` picker's state, rows and key map |
| `src/ui/skills_view.rs` | the `/skills` picker's rendering and geometry |
| `src/tui/settings.rs` | opening the menu, and applying a toggle |

Subagents carry the tool too (`SubagentConfig::skills`): a side agent benefits
from an authored `SKILL.md` exactly as the lead does, and loading one is pure
text, so it needs no gate of its own.

They carry the **listing** with it — `subagent_skill_reminder`, pushed onto
the agent's message list right after its launch prompt and ahead of any
`SubagentStart` hook note, so it reads as part of the briefing. A subagent
starts on a fresh context, so the lead's `<system-reminder>` never reaches it;
with the spec but no roster it would have to guess a name and read the real
ones back out of the "unknown skill" error, and the spec's own description
would be lying to it ("the available skills are listed in a system-reminder
message in the conversation"). This is the same invariant `skills_offered`
enforces for the lead, applied to the other surface: the tool and the listing
travel together, or neither does. Its budget is the default 8 000 characters —
`SubagentConfig` carries no context window, and a roster is small.

The offline `DummyAi` carries a `skills` scenario (cue `skill`) whose cell and
context entry are built by [`SKILL_LOADED_DISPLAY`] and [`render_skill_body`] —
the very constants and formatter `run_skill_tool` uses — so the offline demo is
byte-for-byte the live one, the rule every scripted demo follows. `smoke.sh`
Phase 76 drives it, and gets its hermetic (skill-free) session for every other
phase from `ALTER_ZERO_SKILLS_DIR` pointed at an empty temp dir.
