# Changelog

Notable changes to Free Agent, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the versions
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). The release
workflow takes a version's notes from here.

## [Unreleased]

### Added

- The `free-agent` library: episodes of actors that talk to each other by
  statement, request, and reply on the Tokio runtime.
- The `social-deduction` application, which plays Werewolf with the roles
  dealt at random.
- `social-deduction werewolf llm`, in which every player is a language
  model set up by a TOML configuration file: the model, how to reach it, and
  the prompt templates each role is given. It checks the file and the model
  before the game begins, and its log opens with the configuration the game
  was played under.
- `social-deduction models`, which lists the models a provider serves,
  marking those known to make tool calls.
- `social-deduction werewolf scripted`, which plays the model-played game
  with no model: the environment announces each phase and waits for
  selections, and every player is a script that selects at once at random.
  It takes the role counts, `--night-limit` and `--day-limit`, with defaults
  in the code and no configuration file.
- An optional think loop for actors: `Think`, `ThinkBuilder` and
  `ActorInit::think`, with `Context::think` to hand a message to it from
  either loop. It runs in a task of its own, so an actor keeps perceiving
  while it thinks.
- Timers: `Context::think_after` hands a message to the think loop once a
  delay has passed. A due timer is thought about ahead of whatever is
  waiting on the think queue.

### Changed

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
