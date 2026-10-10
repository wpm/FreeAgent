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
apps/social-deduction/src/
  main.rs               # parses the command line and runs the chosen variant
  lib.rs                # declares the game modules
  werewolf.rs           # what every variant shares
  werewolf/
    report.rs           # the Narrator
    uniform_random.rs   # the uniform-random variant
    llm.rs              # the model-played variant
    llm/                # its submodules, if it grows any
```

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
social-deduction werewolf llm --config game.toml
```

The first level names the game, the second the variant. Each level is a
`clap` subcommand enum, so each variant declares its own arguments and its
own `--help`. There is only one game today, but the game level exists from
the start, so that a second game is a new subcommand rather than a breaking
change.

- `werewolf uniform-random` takes the role counts that 0.1.0 took at the top
  level, with the same defaults.
- `werewolf llm` takes `--config`, the path to a TOML file. What goes in the
  file is not decided here.

The subcommand names `uniform-random` and `llm` are working names. Renaming
them is a change to this record's command line, not to its structure.

Every variant keeps 0.1.0's output convention: the game's log goes to
standard error as JSON Lines, and the narration of the game goes to standard
output as it happens.

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

## Deliberately deferred

1. **The contents of the `llm` configuration file.**
2. **A second game,** and with it whether games keep sharing one crate.
3. **Mixed tables,** in which players of different kinds play in one game.
4. **Final names for the variant subcommands.**
