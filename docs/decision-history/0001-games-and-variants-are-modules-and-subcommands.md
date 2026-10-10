# ADR-0001: Games and their variants are modules and subcommands

**Status:** Accepted
**Date:** 2026-10-09
**Deciders:** Bill McNeill

## Context

The `social-deduction` application in `apps/social-deduction` plays one game,
Werewolf, in one way: every player chooses uniformly at random among the
living whose roles it does not know. Version 0.1.0 shipped it as a binary
whose command line is a flat set of role counts:

```sh
social-deduction --werewolves 2 --villagers 3 --doctors 1 --seers 1
```

The crate's `lib.rs` holds everything: the rules every game of Werewolf
shares (`Role`, `Team`, `Phase`, `State`, `Vote` and its two tie-break rules,
`Observation`), the message and log types, the environment that runs a game,
and the `Actor` enum that lets an episode hold both the environment and the
players. `uniform_random.rs` holds the random player. `report.rs` holds the
`Narrator` that tells a game on standard output.

The next piece of work is a Werewolf played by language models. It differs
from the uniform-random game in more than its players:

- **Its protocol differs.** The uniform-random environment asks each awake
  player for a choice with a request and takes the reply. That works because
  a random player answers in microseconds. A model takes seconds, so in the
  model-played game the environment announces a phase with a statement and
  players send their selections back as statements whenever they are ready
  ([ADR-0002](0002-actors-perceive-think-and-act.md)).
- **Its environment differs.** The uniform-random environment runs the whole
  game as a loop inside `Behavior::start`. An environment that waits for
  statements cannot do that, because an actor does not take mail while a
  step is running. The model-played environment is event-driven instead.
- **Its configuration differs.** A model-played game needs a model, its
  provider and key, and prompts for each role. That is too much for command
  line flags, so it comes from a TOML file. The uniform-random game needs
  only role counts.

Both games are Werewolf: the same roles, teams, phases, state and voting
rules. What varies is the environment, the players and the configuration.
We call each such combination a **variant** of the game.

The project has a standing rule against building generality before it is
needed. Splitting games into their own crates, or the game-independent parts
of Werewolf into a library of their own, was considered earlier and
explicitly deferred. Nothing here revisits that.

## Decision

**Within the `social-deduction` crate, each game is a module and each of its
variants a module inside it. The command line is two levels of subcommand,
game and then variant, and each variant takes its own arguments.**

### Layout

```
apps/social-deduction/
  examples/
    werewolf/
      llm/                # example configuration files for `werewolf llm`
  src/
    main.rs               # parses the command line and runs the chosen variant
    lib.rs                # declares the game modules
    model.rs              # the model client, shared by every game and `models`
    tool_models.txt       # models known to make tool calls
    werewolf.rs           # what every variant shares
    werewolf/
      report.rs           # the Narrator
      uniform_random.rs   # the uniform-random variant
      llm.rs              # the model-played variant
      llm/                # its submodules, if it grows any
```

`examples/` holds configuration files that show how a variant is set up,
not Rust examples: Cargo builds only `.rs` files there, so TOML files are left
alone. A test loads every file in it and renders its prompts, so the
examples cannot drift out of date with the configuration they illustrate.

Modules follow the 2018 edition's convention: a module with submodules is a
file named after it beside a directory of the same name, `werewolf.rs`
beside `werewolf/`, never a `mod.rs`.

`werewolf.rs` holds what every variant of Werewolf shares: `Role`,
`Team`, `Phase`, `State`, `Vote` with `RandomTieBreak` and `NoTieBreak`,
`Observation`, the `Message` enum, and the log's `Entry` type.

Each variant's module holds what is particular to it: its environment, its
players, the actor enum its episode holds, and the function that builds and
runs its episode from its arguments.

### One message enum

Every variant of Werewolf shares one `Message` enum, and so does everything
an actor in it sends: messages between actors, and messages handed to an
actor's own think loop, timers among them (ADR-0002). A variant that needs a new
message adds a variant to the enum. Giving each variant, or each kind of
traffic, a type of its own is put off until a second game shows whether that
taxonomy is needed.

Adding to the enum does not change the existing variants. The model-played
variant's messages are new variants of the enum, and `Observation` and
`Action`, which the uniform-random variant uses, stay exactly as they are.

### The uniform-random variant does not change

The uniform-random variant keeps its request and reply protocol, its
environment's loop inside `start`, and its random player. Its game plays
exactly as it does in 0.1.0. The only change to it is that its actors opt
out of the think loop ADR-0002 introduces.

The first change made under this record is a pure move. The shared rules go
to `werewolf.rs`, the uniform-random environment and player go to
`werewolf/uniform_random.rs`, `report.rs` moves under `werewolf/`, and
behavior and tests are unchanged.

### Command line

```sh
social-deduction werewolf uniform-random [--werewolves N] [--villagers N] [--doctors N] [--seers N]
social-deduction werewolf llm --config game.toml [role counts] [--night-limit 30s] [--day-limit 2m]
social-deduction models --config game.toml
```

The first level names a game, or one of the few commands that belong to no
game, and the second level names the game's variant. Each level is a
`clap` subcommand enum, so each variant declares its own arguments and its
own `--help`. There is only one game today, but the game level exists from
the start, so that a second game is a new subcommand rather than a breaking
change.

- `werewolf uniform-random` takes the role counts that 0.1.0 took at the top
  level, with the same defaults.
- `werewolf llm` takes `--config`, the path to a TOML file, the same
  role-count options as `uniform-random`, and `--night-limit` and
  `--day-limit`. The role counts are one shared `clap` argument group, so
  both variants spell them the same way.
- `models` takes `--config`, asks the provider the file names for its
  `GET /v1/models` listing, and prints each model id, marking those on the
  tool-call whitelist. It plays no game.

The subcommand names `uniform-random` and `llm` are working names. Renaming
them is a change to this record's command line, not to its structure.

Every variant keeps 0.1.0's output convention: the game's log goes to
standard error as JSON Lines, and the narration of the game goes to standard
output as it happens.

### The model-played variant's configuration

**Precedence.** A setting comes from the command line if it is given there,
otherwise from the configuration file, otherwise from a default in the code.
Role counts and phase limits can be given in all three places. Phase limits
are written as durations, such as `30s` or `2m`. The model request timeout,
`request_timeout`, is a duration too, with a default in the code that the
file can override.

**One model for the whole table.** Every model player uses the same model
from the same provider. The file names it once.

**Prompts are templates, written inline in the file.** A user changes how
the game is played by rewriting them. Each model player's system prompt is
rendered from a `minijinja` template with these variables:

- `name`, `role` and `persona`: the player's name, the role it was dealt,
  and its persona, if its seat has one;
- every named text block in the file's `[text]` table, so shared text such
  as the rules is written once and used anywhere.

The template used is the most specific one given: the role's own
`system` override if it has one, otherwise the default `[prompt] system`.

```toml
[text]
rules = """Werewolf is played by …"""
wolf = """You know who the other werewolves are. Win by …"""

[prompt]
system = """{{ rules }}

You are {{ name }}, a {{ role }}.
{% if role == "werewolf" %}{{ wolf }}{% endif %}
{% if persona %}{{ persona }}{% endif %}"""

[roles.seer]
system = """{{ rules }}

You are {{ name }}, the seer. …"""

[personas]
player1 = "You are cautious and slow to accuse."
player4 = "You trust your instincts and say so."
```

One template for everyone is a default that ignores `role`; prompts that
differ by role are conditionals, text blocks or overrides; prompts that
differ by player are personas.

**A persona belongs to a seat, and roles are still dealt at random.** Seats
are named `player1` onward, as in the uniform-random variant, and a persona
is keyed by seat. The deal stays random, so that a persona's results are not
confounded with the strength of the role it happens to hold, and win rates
stay comparable across games. A persona for a seat that does not exist is an
error.

**A template that cannot render stops the program.** Undefined variables are
strict: `{{ rulez }}` is an error, not an empty string. `persona` is always
defined, and empty for a seat without one, so a template can test it. Every seat's prompt
is rendered at startup for every role it could be dealt, before any actor is
built, so a broken template fails at once rather than in the middle of a
game.

**Providers.** Every model is reached through the OpenAI chat completions
protocol: OpenAI itself, Anthropic's OpenAI-compatible endpoint, and a local
LM Studio server among them. A provider is a base URL and the name of the
environment variable that holds its API key.

**API keys come from the environment, never the file.** The file names the
variable (`api_key_env = "OPENAI_API_KEY"`); the key is read from it once at
startup into a `secrecy::SecretString` and exposed only where a request is
built, so it never appears in `Debug` output, the log or an error message.
A variable that is not set is an error at startup.

**A configured model must pass two checks before the game starts:**

1. **Its provider serves it.** The model's id appears in the provider's
   `GET /v1/models` listing. This catches a misspelled id and a local model
   that is not loaded.
2. **It is known to make tool calls.** A model player makes its selection
   with a tool call, and a model that cannot make one would never select,
   silently, since a failed thought is not logged
   ([ADR-0002](0002-actors-perceive-think-and-act.md)). So the id must be
   on a whitelist of models known to make tool calls.

Either failure stops the program before any actor is built.

**The whitelist is a text file compiled into the binary.** It is not
particular to Werewolf, since `models` reads it too, so it lives at the root
of the crate's source, `src/tool_models.txt`, and is read with
`include_str!`: one model id per line, exactly as `/v1/models` lists it,
with blank lines and lines starting with `#` ignored. An id is matched
exactly, across every provider. Adding a model is an edit to the file and a
rebuild, which is acceptable while this is a command line program with one
user. A test checks that the file parses and is not empty.

## Alternatives considered

### A binary per variant

`werewolf-random`, `werewolf-llm`, and so on. Each would have its own
arguments without nested subcommands. Rejected: every new variant would need
a new release artifact and a new name to install, and the variants share
almost all of their code, so a single binary that dispatches costs nothing.

### Flat flags with a `--variant` switch

Keep one level of command line and choose the variant with a flag. Rejected
because the variants take different arguments. With a flag, either every
variant's arguments sit in one flat list, most of them meaningless for any
given variant, or the program checks combinations by hand that subcommands
check for free.

### The variant chosen in the configuration file

Every variant reads a TOML file that says which variant it is. Rejected
because the uniform-random game needs nothing a file would hold, and making
the simplest game the hardest to run is backwards.

### Discovering tool support

Ask each model at startup whether it can make tool calls, with a small
probe request, rather than keeping a list. Put off in favor of the
whitelist, which is simpler and good enough for one user; something cleverer
comes later.

### A fixed deal for per-player prompts

Let the file assign each seat both its role and its prompt. Useful for a
scripted scenario, but it removes the random deal that makes games
comparable. Rejected for personas, which vary players without fixing roles.

### Plain prompts with a shared preamble

Shared text prepended to one prompt per role. Simpler, but it cannot vary
prompts by player or put shared text anywhere but the start. Rejected for
templates.

### The whitelist in the configuration file

Easy to edit without a rebuild, but a list anyone can add to at run time is
not a whitelist. Rejected.

### A crate per game or per variant

Rejected for now, as before: it is the generality the project has decided to
earn rather than anticipate. Modules can become crates when the boundaries
have proven themselves.

### Per-variant message types

Each variant, or each kind of traffic within an actor, gets its own message
type. Rejected for now as premature taxonomy. One enum per game is the
simplest thing that works, and a refactor is cheap once there is evidence
for a better split.

## Consequences

- **The 0.1.0 command line breaks.** `social-deduction --werewolves 2` stops
  working; `social-deduction werewolf uniform-random --werewolves 2` replaces
  it. The README and CHANGELOG say so in the release that makes the change.
- **A game's shared rules have one home.** Both Werewolf environments resolve
  nights and days with the same `State`, so they cannot drift apart on the
  rules.
- **Variants of one game can be compared.** Two variants that share `State`,
  `Vote` and `Entry` write logs of the same shape, which is what an
  experiment comparing a model-played game with a uniform-random baseline
  needs.
- **The `Message` enum grows with every variant.** A variant's players can be
  sent messages that only another variant uses. Actors in Werewolf cooperate
  with the rules, so this is accepted.

- **A new model needs a rebuild.** Until the whitelist moves out of the
  binary, trying a model that is not on it means editing
  `src/tool_models.txt` and building again. `models` shows which of a
  provider's models are on it.

## Deliberately deferred

1. **Models that differ by role or by player,** prompts kept in files of
   their own, and a fixed deal.
2. **A second game,** and with it whether games keep sharing one crate.
3. **Mixed tables,** in which players of different kinds play in one game.
4. **Final names for the variant subcommands.**
5. **A cleverer way to know which models make tool calls,** probably when
   this stops being a command line program.
