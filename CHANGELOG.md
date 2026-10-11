# Changelog

Notable changes to Free Agent, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the versions
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). The release
workflow takes a version's notes from here.

## [Unreleased]

### Added

- The `free-agent` library: episodes of actors that talk to each other by
  sending messages on the Tokio runtime.
- The `social-deduction` application, which plays Werewolf with the roles
  dealt at random.
- `social-deduction werewolf llm`, in which every player is a language
  model set up by a TOML configuration file: the model, how to reach it, and
  the prompt templates each role is given. It checks the file, and asks the
  model for a selection as a player would, before the game begins, and its
  log opens with the configuration the game was played under.
- `social-deduction models`, which lists the models a provider serves.
  `--check` asks each model for a selection and marks those that give one.
- `--night-limit` and `--day-limit` on `social-deduction werewolf
  uniform-random`, how long each phase waits for the players' selections,
  with defaults in the code.
- `--log` and `--no-log` on every Werewolf variant, saying whether the game
  is logged to standard error. It is unless `--no-log` says otherwise, and
  the last one given wins.
- The log records each night's hunt: whom the werewolves elected and which
  doctors protected them. The narration tells it before who dies, so a
  night in which nobody dies says why.
- The log's start entry carries how long a night and a day wait for the
  players, and the narration tells the limits after the deal and, as each
  phase ends, how long it took.
- The log's end entry carries how long the whole game took, from the deal
  to the end, and the narration tells it after who won.
- An optional think loop for actors: `Think`, `ThinkBuilder` and
  `ActorInit::think`, with `Context::think` to hand a message to it from
  either loop. It runs in a task of its own, so an actor keeps perceiving
  while it thinks.
- Timers: `Context::think_after` hands a message to the think loop once a
  delay has passed. A due timer is thought about ahead of whatever is
  waiting on the think queue.
- A model player's failure to select, whether the model was not reached
  or answered with something that is not a selection, is logged as
  `Failed` with the phase and the error, and narrated as it happens. The
  player has not selected, and the game goes on.

### Changed

- Actors talk by sending messages alone: `Context::request`,
  `Behavior::answer` and the reply are gone, so a behavior handles
  everything in `receive`. The uniform-random variant plays the announcing
  environment as the model-played variant does: it announces each phase
  and waits for selections, and every player selects at once at random. Its
  log has `Announce` and `Select` messages in `Sent` and `Received`
  entries, with no `Observation`, `Action` or `Replied`.
- The command line is `social-deduction werewolf uniform-random`, a game
  and then its variant, each a subcommand with its own `--help`. The role
  counts `--werewolves`, `--villagers`, `--doctors` and `--seers` belong to
  the variant; the 0.1.0 top-level flags are gone.
- By day a player may vote against anyone living but itself, whatever it
  knows them to be, so a seer can vote out a werewolf it has found. By
  night it still chooses among those whose roles it does not know.
- A selection that arrives after the next phase has been announced is told
  as too late to count, for the phase it was for, instead of as a deed of
  the phase that followed.

[Unreleased]: https://github.com/wpm/FreeAgent/compare/main...HEAD
