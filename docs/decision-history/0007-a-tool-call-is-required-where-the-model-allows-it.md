# ADR-0007: A tool call is required where the model allows it

**Status:** Accepted
**Date:** 2026-10-12
**Deciders:** Bill McNeill
**Depends on:** [ADR-0003](0003-the-model-player.md),
[ADR-0004](0004-the-model-is-asked-to-select-before-a-game.md),
[ADR-0006](0006-players-talk-by-day.md)
**Amends:** [ADR-0003](0003-the-model-player.md),
[ADR-0004](0004-the-model-is-asked-to-select-before-a-game.md),
[ADR-0006](0006-players-talk-by-day.md)

## Context

A model player answers every request with tool calls: by night one `select`
call, by day any number of `speak`, `select` and `wait` calls
([ADR-0003](0003-the-model-player.md), [ADR-0006](0006-players-talk-by-day.md)).
Every request sets `tool_choice` to `"required"`, so the model must call a
tool and cannot answer with prose instead. Before a game begins, the model
is asked once as by night and once as by day, and the program refuses to go
on unless both answers come back right
([ADR-0004](0004-the-model-is-asked-to-select-before-a-game.md)).

Some models do not accept a required tool call. Against Anthropic's
OpenAI-compatible API, `claude-opus-5-5` refuses the night's probe:

```
Error: claude-opus-5-5 did not answer a night's request

Caused by:
    POST https://api.anthropic.com/v1/chat/completions was answered 400 Bad Request.
    The provider said: tool_choice: type "tool" and "any" are not supported for this model.
```

The same provider accepts a required call from other models, so this is a
property of the model, not of the provider. Such a model accepts
`tool_choice: "auto"`, under which it may call a tool or answer in prose.
The probe caught the refusal at launch, as it is meant to; what is missing
is a way to play with such a model at all.

## Decision

**Every request requires a tool call, unless the model refuses that, and
then every request allows one. The probe finds out which, before the game
begins.**

### The probe settles the tool choice

The night's probe is asked with `tool_choice: "required"`. If the provider
refuses that request as bad, answering 400, the probe asks again with
`tool_choice: "auto"`. The first tool choice the night's request is
answered under is the game's: the day's probe is asked under it, and so is
every request every player makes.

Only a 400 sends the probe to `"auto"`, since a 400 is the provider
refusing the request's shape. Any other failure, such as an unreachable
provider, a key it does not accept, or an answer with no tool call, fails
the probe as it does today, without a second request. The probe reads the
status alone, never the provider's words, so it works the same against any
provider.

If the night's request is refused under both tool choices, the probe fails,
and the error gives both refusals in the provider's words.

### Under `"auto"`, prose is a failure

A model allowed to answer in prose sometimes will. Such an answer is judged
as it is today: a response with no tool call is not well formed, so the
player logs a `Failed`, carries nothing out, and the game goes on.

So that the model knows a tool call is the only answer that counts, the
situation that ends every request, and each probe's request, adds
"Answer by calling one of the tools." under either tool choice, so the
request reads the same whichever the game uses.

### What is said and logged

- The `Configuration` entry records `tool_choice`, `required` or `auto`.
- When the game uses `"auto"`, startup says so: "claude-opus-5-5 does not
  accept a required tool call, so its players are asked to call one."
- `models URL --check` marks a model that played under `"required"` with
  `* `, one that played only under `"auto"` with `~ `, and one that did not
  play with `! `. The closing line becomes "* marks a model that played
  when asked, ~ one that played when asked to call a tool but not made to,
  and ! one that did not."

## Alternatives considered

### Always `"auto"`

Ask every model to call a tool and require it of none. Rejected: a required
call is the guarantee that a model answers with a tool call, and small local
models in particular drift into prose without it. A model that accepts the
guarantee keeps it.

### A tool choice per provider

Record in `src/models.toml` which providers refuse a required call.
Rejected: the refusal is the model's, and the same provider accepts a
required call from other models. A list of models would be the whitelist
that ADR-0004 removed.

### Reading the provider's error

Fall back to `"auto"` only when the provider's message says the tool choice
is not supported. Rejected: every provider words it differently, and
matching their words ties the program to each of them. A 400 under
`"required"` followed by a good answer under `"auto"` says the same thing
without reading anything.

### A setting

Let the configuration file or the command line name the tool choice.
Deferred: the probe finds it, and nothing yet calls for overriding what the
probe finds.

## Consequences

- **A model that refuses a required tool call can play.** Its players are
  asked to call a tool, and an answer in prose is a `Failed`, as any other
  malformed answer is.
- **Startup costs one more request** for such a model, and none for any
  other.
- **A 400 for another reason costs one more request** before the probe
  fails, and the error gives both answers.
- **ADR-0003 is amended:** selecting is a tool call that is required where
  the model allows it, and asked for otherwise.
- **ADR-0004 is amended:** the probe settles the tool choice as well as
  checking the model can play, and `models --check` marks the tool choice a
  model played under.
- **ADR-0006 is amended:** a day's request requires a call where the model
  allows it, and asks for one otherwise.

## Deliberately deferred

1. **A setting for the tool choice,** in the configuration file or on the
   command line.
2. **Anthropic's own Messages API,** in place of its OpenAI-compatible one.
