# Shift+Tab thinking modes (reasoning effort)

Reasoning-capable models ("thinking" models) accept a **reasoning effort** —
how much chain-of-thought to spend before answering. This document covers how
the TUI detects that capability per model, the **Shift+Tab** cycle that
switches the mode, where the mode shows (the footer, beside the model name),
how it rides the request, and how it persists.

```
  ❯ ask something…
────────────────────────────────────────────────
  qwen3-5-35b-a3b medium · ~/alter-zero            (footer: {model} {mode} · {cwd})
```

Pressing Shift+Tab cycles `off → low → medium → high → off …` (whatever the
model offers), raises a transient `Thinking: high` toast (`docs/toast.md`),
and the next request carries the new effort. On a model with no reasoning at
all, Shift+Tab raises `{model} does not support thinking` instead — the key is
never silently dead. The default for a reasoning model is **medium** (where
offered).

## Detection: what `/v1/models` says

Support is parsed per record by the pure `llm::models::reasoning_support_of`
into [`llm::reasoning::ReasoningSupport`] — `efforts` (the accepted ladder,
canonically ordered), `can_disable`, `default_effort`. Both provider shapes
are sniffed **per record**, so a custom OpenAI-compatible provider using
either works unconfigured:

- **OpenRouter** — a `reasoning` object on the record:
  `{"mandatory": bool, "supported_efforts": ["low", …] | null,
  "default_effort": "medium"}`. A null/absent `supported_efforts` means "all
  accepted" (the default `low/medium/high` ladder is offered); an offered
  `"none"` maps to the Off switch, not a ladder rung; `mandatory: true` drops
  Off (e.g. kimi-k3, whose only mode is `max`). Records without the object
  but with `"reasoning"`/`"reasoning_effort"` in `supported_parameters` count
  too.
- **Venice / Agent Zero** — `model_spec.capabilities.supportsReasoning` (+
  `supportsReasoningEffort`). With effort support the default ladder is
  offered; without it the model is an **on/off-only reasoner** (`efforts`
  empty) — it thinks at its own default, and the cycle is just `off ↔ on`.

A model advertising neither shape has no thinking: no footer mode, and
Shift+Tab explains.

## The mode, the cycle, and the wire

[`llm::reasoning::ThinkingMode`] is `Off`, `On`, or `Effort(level)` with the
canonical ladder `minimal → low → medium → high → xhigh → max`
(`ReasoningEffort`). `ReasoningSupport::modes()` builds the cycle — `Off`
(when disableable) then the model's efforts, or `[Off, On]` for an
effort-less reasoner — and `next_mode` steps it, wrapping; a stale persisted
mode the model no longer offers steps to `default_mode()` (medium →
provider-reported default → the ladder's middle → `On`).

On the wire (`openai::build_payload`, applied **after** the provider
`extra_body` merge so the user's choice wins over a file-configured static):

| mode | request |
| --- | --- |
| `Effort(e)` | `"reasoning": {"effort": "e"}` |
| `Off` | `"reasoning": {"enabled": false}` |
| `On` | nothing — the model's own default |

**The Venice quirk:** live testing showed Venice (through the Agent Zero
proxy) *ignores* `reasoning.enabled` — its hybrid reasoners keep thinking.
The toggle it honours is `venice_parameters.disable_thinking`. So when the
payload carries a `venice_parameters` table (the Venice-family marker — the
built-in `a0_venice` provider always has one, for
`include_venice_system_prompt`), the builder syncs `disable_thinking` to
`mode == Off`, preserving the table's other keys; providers without the table
(OpenRouter) keep a clean payload. `providers.toml` no longer pins
`disable_thinking = true` statically — the mode owns it (previously thinking
was force-disabled for every Agent Zero model).

## Key handling

Legacy terminals report Shift+Tab as `BackTab` (`ESC[Z`); with the kitty
keyboard protocol it can arrive as `Tab`+`SHIFT` — `App::on_key_conversation`
binds **both** (the Shift+Enter pattern, `docs/shift-enter.md`), the shifted
arm sitting before every plain-Tab arm so it never queues (`docs/queue.md`) or
palette-completes. Cycling works **mid-turn** — like `/model`, it only rebinds
the *next* turn's backend, never the streaming one. While the `/model` picker,
`/login` flow, or Ctrl+R search own the keys, Shift+Tab is inert; the
`/resume` picker keeps its own Tab/BackTab toolbar binding. The `?` shortcuts
band lists `shift+tab to cycle thinking` (`docs/shortcuts.md`).

The pure `App::cycle_thinking` advances `App::thinking`
(`ThinkingState { support, mode }`) and returns `Action::SetThinking(mode)`;
the loop rebuilds the backend with the mode in its `ModelConfig`, persists,
and presents the `Thinking: {mode}` toast (presented by the loop so the
boundary's expiry timer arms — `docs/toast.md`).

## Where the support state comes from

- **`/model` switch** — the picked `ModelEntry` already carries `reasoning`
  (parsed from the same `/v1/models` fetch that listed it), riding
  `Action::SelectModel` so a successful switch seeds the cycle at the model's
  default mode with no refetch.
- **Startup, saved selection** — `config.json` persists a `thinking` blob
  (`llm::settings::ThinkingSettings`: the mode + efforts + `can_disable`, or
  the `supported: false` marker for a known non-reasoner) beside the saved
  provider/model, so startup seeds `App::set_thinking` straight from the file.
- **Startup, support unknown** — a real backend whose saved settings carry no
  blob (an env-selected model, or the first run since this feature) spawns a
  one-shot background **probe**: `fetch_models` for the active provider on a
  worker thread (a dedicated channel — the `/model` picker's parallel fetches
  can't confuse it), the active model's record read out of the result. The
  probe seeds the state, rebinds the backend so the default mode rides the
  next request, and persists — but **only onto the recorded selection**: an
  env-overridden model never writes `config.json` (env always wins, never
  sticks), so that combination just re-probes next run. A failed probe leaves
  support unknown silently (background bookkeeping, not a user action).

`/clear` and `/resume` never touch the state (the model didn't change); a
switch to a non-reasoning model clears it (`App::set_thinking(None)`), which
blanks the footer mode.

## Testing

The mode/support/cycle logic, the models-record parse (both provider shapes),
the payload mapping (including the Venice `disable_thinking` sync), the
settings round-trip (including stale-blob degradation), the Shift+Tab
grammar (BackTab and Tab+SHIFT, mid-turn, picker-inert, unsupported-toast),
and the footer rendering are all unit-tested in their modules. The boundary
wiring (probe, persistence, backend rebinds) is exercised against the real
providers — `tests/live_openrouter.rs`'s ignored live tests plus manual tmux
runs (`docs/llm.md`'s testing notes).

## Known limitations (v1)

- Venice enumerates no per-model effort list (just the boolean), so
  effort-capable Venice models get the safe `low/medium/high` ladder;
  `minimal`/`xhigh`/`max` are not offered there even where a model might
  accept them. OpenRouter models use their exact advertised list.
- The probe only covers the startup model; a model switched to via
  `ALTER_ZERO_MODEL` mid-flight (impossible — env is read once) needs no more
  than that.
- `ThinkingMode::On` sends nothing, so an effort-less reasoner's actual
  thinking depends on the model's own default (Venice's Claude models think
  by default; `off` is the reliable lever there via `disable_thinking`).
