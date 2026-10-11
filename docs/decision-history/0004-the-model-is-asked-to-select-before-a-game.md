# ADR-0004: The model is asked to select before a game

**Status:** Accepted
**Date:** 2026-10-11
**Deciders:** Bill McNeill
**Supersedes:** the tool-call whitelist in
[ADR-0001](0001-games-and-variants-are-modules-and-subcommands.md) and
[ADR-0003](0003-the-model-player.md)

## Context

A model player in the model-played variant of Werewolf
([ADR-0003](0003-the-model-player.md)) makes each selection with one forced
tool call. A model that cannot make one never selects, and a failed thought
is not logged ([ADR-0002](0002-actors-perceive-think-and-act.md)), so such a
game plays silently to a draw. ADR-0001 guarded against that with a
whitelist: `src/models.toml`, compiled into the binary, listed the ids of
the models known to make tool calls, and `werewolf llm` refused any model
not on it before the game began. The `models` command marked the ids on the
list, and a cleverer way of knowing was deferred.

The whitelist answered one question by name: is this model known to make
tool calls. A game needs three things to be true, and the list could tell
none of them. The server must accept the request the player sends, with its
one tool and its `tool_choice`. The model must answer with a call to that
tool. The call's argument must parse into one of the candidates. A game
against a local LM Studio server, with a whitelisted model, played to a
draw because the server rejected the request's shape, and nothing said so.

The list also had to be kept. A new model, or a provider's new name for an
old one, meant an edit and a rebuild before it could be tried, and the
`models` listing could only ever mark what the list already knew.

## Decision

**Before a game begins, the model is asked for one real selection, exactly
as a player would ask, and the program refuses to go on if a candidate does
not come back. The whitelist goes entirely.**

### The probe

`check_selects`, in the model-played variant's module beside `select` and
`selected`, builds a request of the player's shape: a short system prompt,
a one-line user message asking for a selection, and the `select` tool over
two placeholder candidates. It judges the response as a player's is judged,
with `selected`, so no tool call, a call to another tool, arguments that do
not parse, or a target off the list all fail as they would in play. The
probe and the player live together because they must agree on what a good
answer is.

The error names the model and says that it did not select when asked, with
the provider's own error beneath it: that `POST …/chat/completions` was
answered 400 and what the server said, or that the model made no tool call.

### Where it runs

`main.rs` runs the probe after the check that the provider serves the model
and before the deal, where the whitelist check ran, so a failure stops the
program before any actor is built and before anything is printed. It is not
in a player's `initialize`: with one model for the whole table, one call
before the episode proves what seven calls would, and no more. When an
episode may mix models, each player's own initialization is where a
per-model check belongs.

The probe costs one request at startup: under two seconds against a local
model, and one small call against a hosted one.

### The `models` command

The listing keeps saying which models can play, by asking them instead of
by name.

```sh
social-deduction models URL                 # what the provider serves
social-deduction models URL --check         # … and which of them select when asked
social-deduction models URL --check ID ...  # … checking only the ids given
```

Without `--check`, the listing is plain: one id per line, sorted, and no
legend. With `--check`, every id listed is probed, or only the ids given,
each of which must be listed. An id that selects is marked `* `, one that
does not `! `, and one not checked is indented two spaces, so the ids line
up, and a closing line, set off by an empty line, says what the marks mean.
The reason each `!` model failed goes to standard error, one line per model,
in the probe's words, so the listing on standard output stays scannable.

The probes run one at a time, since a local server loads each model it is
asked for. The command exits 0 whether or not every model passed: it is a
listing, not a check that must succeed.

### What leaves

`tool_models` leaves `src/models.toml`, and the functions that read it
leave `src/model.rs` with their tests. The `providers` table stays: a
provider's key variable and headers are still known by base URL. The
examples test no longer asserts that each example's model is listed, since
no list remains to assert against.

## Alternatives considered

### Keeping the whitelist beside the probe

Keep the list as a fast check that needs no network, and probe only models
not on it. Rejected: the list could not tell that a server rejects the
request or that an answer parses, which is what went wrong, and a list the
probe can contradict is a second source of truth to keep.

### The probe in each player's `initialize`

Every player checks its own model when the episode starts. Rejected for now:
every player asks the same model, so seven probes prove no more than one,
and a failure would come after the deal has been logged. This is where a
per-model check goes once a table may mix models.

### A listing that is always checked

Make `models` probe every model it lists. Rejected: a hosted provider lists
many models, and probing each one is a request and a charge the user did not
ask for. Checking is opt-in, and can be narrowed to the ids of interest.

## Consequences

- **A model that cannot play fails at launch, in the provider's words.** A
  request the server rejects, an answer with no tool call, and a target off
  the list are all told apart before anything is printed.
- **Any model that passes can play.** Trying a new model is a change to the
  configuration file or the command line, with no rebuild.
- **Startup costs one request.** A game against a hosted provider makes one
  small call more than it did.
- **The `models` listing says less by default.** A plain listing marks
  nothing; `--check` asks, and each probe is a request to the provider.
- **ADR-0001 and ADR-0003 remain as written.** They record the whitelist as
  it was decided; this record supersedes that part of them and no other.

## Deliberately deferred

1. **A per-model probe in each player's `initialize`,** for a table that
   mixes models.
2. **Stopping a game whose model begins to fail mid-game,** which the probe
   at startup does not see.
