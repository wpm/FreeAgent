# Changelog

Notable changes to Free Agent, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the versions
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). The release
workflow takes a version's notes from here.

## [Unreleased]

### Added

- A viewer for social deduction games, in the browser: the game as a group
  chat beside a live Avalon board, built with Leptos and Rich-Chat. Every
  push to `main` plays a random game of Avalon and deploys it to
  <https://wpm.github.io/FreeAgent/viewer/>.
- The viewer as a desktop app, built with Tauri: choose a game, press
  **Play**, and watch it live, with **Stop**.
  Each game's log is saved, and API keys come from the OS keychain, then
  the environment.
- Installers for the desktop app on each release: a `.dmg` for macOS, a
  setup `.exe` and an `.msi` for Windows, and an `.AppImage`, a `.deb` and
  an `.rpm` for Linux. The website links to the newest of each.
- The desktop app's settings live in a standard Settings window (⌘, on
  macOS, Ctrl+, elsewhere), stored as they change, with each field's error
  beneath it. Both games share one list of models, grouped by provider:
  each provider's card holds its key and the models it offers that pass
  `models --check`, most capable first, checked in the background and kept
  in `probes.json`. Local servers are found at launch, and any other
  OpenAI-compatible provider can be added. API keys sit with their
  provider, in the OS keychain. A provider whose models can't be checked,
  as when its account has no credits left, says why on its card.
  The main window opens on a start state with **Werewolf** or **Avalon**
  and **Play**, and **File** imports and exports configuration files that
  `social-deduction play` plays the same way.
- `social_deduction::settings`, the desktop app's settings, which make
  the configuration a game is played from; `social_deduction::catalog`,
  the providers, listings and probes behind its Models tab; and
  `social_deduction::model::check`, the probe `models --check` runs.
- `social_deduction::live`, which plays a prepared game in the background,
  saving its log and folding it into cues as it happens.
- `social_deduction::stage`, which folds a game's log into the viewer's
  cues, and the `stage` example, which folds a log file.
- The release archives hold the example configurations in `configs/`,
  beside the binary.
- The `free-agent` library: episodes of actors that talk to each other by
  sending messages on the Tokio runtime.
- `Episode::stopper`, a `Stopper` that ends a running episode from outside
  it, think loops included. A stopped episode returns `Ok(())`.
- The `social-deduction` application, which plays Werewolf with the roles
  dealt at random.
- `social-deduction werewolf llm`, in which every player is a language
  model set up by a TOML configuration file: the model, how to reach it, and
  the prompt templates each role is given. It checks the file, and asks the
  model to play as a player would, once as by night and once as by day,
  before the game begins, and its log opens with the configuration the game
  was played under. A model that refuses to be made to call a tool is asked
  to call one instead.
- The model's context window, settled before a model-played game begins
  from `context_window` in the model's table, the provider's listing of
  the model, the provider's default in `src/models.toml`
  (128000 tokens for OpenAI, 200000 for Anthropic), or a default of 4096
  tokens, and told and logged with where it came from. A window with no
  room for a story is refused.
- A model player's request fits the context window: the window keeps 1024
  tokens for the reply, and when the story would not fit beside the rest,
  by estimate, the player forgets talk, oldest first, and is told "You
  don't remember what was said before this." The log's action and withheld
  entries carry `forgot`, how many lines of talk were left out, when any
  were.
- `social-deduction models`, which lists the models a provider serves.
  `--check` asks each model to play as by night and as by day and marks
  those that answer both requests well, and those that do only when not
  made to call a tool.
- `--night-limit` and `--day-limit` on `social-deduction werewolf`, how
  long each phase waits for the players' selections, with defaults in the
  code.
- `--log` and `--no-log` on `social-deduction werewolf`, saying whether the game
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
- In `social-deduction werewolf llm` the day's vote may be ratified: with
  a ratify limit above zero, a player may change its vote while the day is
  open and its latest vote counts, and the environment sends every living
  player a `Tentative` whenever the votes change whom they select. A day
  ends one ratify limit after a `Tentative` naming a player, unless another
  follows first, voting that player out, or at the day limit with nobody
  voted out. A vote that leaves the decision as it was leaves the deadline
  as it was. The ratify limit is `ratify_limit` under `[phases]` or
  `--ratify-limit`, and may not be longer than the day limit. It is zero by
  default, and a day whose ratify limit is zero is not ratified: each
  player's first vote counts, and the day ends when everyone has voted or
  at the day limit. A model player's `select` tool says which vote counts:
  the latest in a ratified day, and otherwise the first, which is final,
  and in a day that is not ratified it is offered until the player has
  voted.
  The log records how every day ended as `Voted`, the
  start entry carries the ratify limit, `"0s"` for a day that is not
  ratified, as does the configuration, and the narration tells the ratify
  limit when it is above zero, the votes as they come, each tentative
  decision, and whether the votes stood or the day ran out.
- The votes are public: by day every player's vote, a uniform-random
  player's as well as a model player's, goes to every other living player
  as well as to the environment, and each of them logs observing it.
- In `social-deduction werewolf llm` the players talk by day, with no
  turns. A player is woken by every message it hears by day, debounced so
  that it has at most one model call in progress and one waiting, and the
  model chooses with one required call among `speak`, which goes to every
  other living player and not to the environment, `select`, and `wait`,
  which asks to be woken later; it may call several at once. A response
  that comes back after its phase has ended, by night or by day, is
  withheld and logged as `Withheld`. A player ignores talk, votes and
  tentative decisions from a phase it is not in: it logs the receipt and
  neither remembers the message nor wakes for it. The log has `Say`
  messages, the narration tells what each player says while its phase
  lasts, and a player's story tells it what others said. A day's failure is narrated as a failure to play. The
  night plays as before.
- A Werewolf game may play several models, so a stronger model can play a
  weaker one: each model in play is set up and probed before the game, one
  at a time in name order, its context window checked against the
  strategies that name it, and each prompted player asks its strategy's
  model. The narration tells each model's context window, and each model
  that does not accept a required tool call, by name.
- In `social-deduction werewolf llm` a player may retract its vote by day
  with the `retract` tool, and is undecided again. A `Retract` goes where a
  vote goes, to the environment and every other living player, and is
  handled like a change of vote: one that changes the tentative decision is
  announced with a `Tentative` and starts or stops the ratify timer. A
  retraction counts in a day with a ratify limit above zero, from a player
  with a vote standing, and a model player is offered `retract` only in
  such a day. The narration tells "player4 withdraws their vote and is
  undecided again.", and a retraction that does not count as "player4
  withdraws their vote, but the withdrawal does not count."
- `social-deduction avalon`, which plays *The Resistance: Avalon* with
  random players, any valid table of 5 to 10 players and all eight roles,
  from the command line: `--merlin`, `--percival`, `--servants`,
  `--assassin`, `--morgana`, `--mordred`, `--oberon` and `--minions`, and
  `--propose-limit`, `--vote-limit`, `--quest-limit` and
  `--assassinate-limit`. The environment counts only the choices the rules
  allow. The game is told on standard output and logged to standard error,
  each phase ending with `Proposed`, `Tallied`, `Quested` or
  `Assassinated`.
- `social-deduction avalon [CONFIG]` takes a configuration file shaped
  like Werewolf's, with Avalon's eight `[roles.<role>]` tables and
  `[phases]` limits, and deals roles and strategies together at random.
  The log records the configuration right after the deal, and
  `examples/avalon/random.toml` plays the game of no file.
- Prompted players play Avalon, beside random ones, with any number of
  models. Each is told its role and its side, and a prompt template has
  `team`, `good` or `evil`, beside `name` and `role`. They talk in every phase with `speak` and `wait`, and the player
  a phase waits on chooses with `propose`, `vote`, `perform` or `select`.
  Each model in play is asked four times before the game, as the leader, a
  voter, an evil member of a team and the Assassin, and a model that does
  not answer each well stops the program, naming it.
  `examples/avalon/prompted.toml` plays a game of model players against a
  local model.
- The `social-deduction-stage` crate: the cues a game's viewer is drawn
  from, built natively and for `wasm32-unknown-unknown`.
- A prompted player logs `Thinking` as each model call goes out and
  `Thought` as it comes back, answered or failed with its error, so a log
  shows how long every call took and whose call is out. A call cut off by
  the player's stopping ends at its death, at the game's end, or at the
  end of the log. The narration leaves both out.
- `social_deduction::run::prepare` makes a game ready from a
  configuration's text, with its API keys looked up in the `Keys` the
  caller gives, and the episode it builds logs to the caller's logger.
  `config::parse` settles a configuration's text, and the command line
  looks keys up in the process `Environment`.
- The log's `Configuration` entry names the version of `social-deduction`
  that played the game, in its `version` field.

### Changed

- A configuration file's `[text]` is `[prompt_parts]`, and a part's name is
  letters, digits, underscores and spaces, pulled in with its spaces as
  underscores (`"Seer only"` is `{{ Seer_only }}`). The log's configuration calls them
  `prompt_parts` too.
- Each game's narrator tells its story in lines that say what they are:
  a phase heading, a player's vote or selection, a player's utterance with
  who spoke, a blank line, or anything else told. The console prints them
  as before.
- `social-deduction models --check` asks each model to play Werewolf, then
  Avalon, and marks it as playing when it plays both. A model that refuses
  to be made to call a tool in Werewolf is asked to call one in Avalon. The
  reason a model did not play names the model once, then the game and the
  request it did not answer.
- A provider's error gives the provider's own words first when it
  classifies them with a code, and reads that code: an account out of
  credits is told to add credits, and only a rate limit is told to try
  again shortly.
- Werewolf lists three or more players as Avalon does, "a, b and c": a
  pack of werewolves, the doctors protecting one player, and the dead.
- **Breaking:** `social-deduction play CONFIG` replaces
  `social-deduction werewolf CONFIG` and `social-deduction avalon CONFIG`,
  and every configuration file names its game with `game = "werewolf"` or
  `game = "avalon"`. A file is checked whole before the game begins, and
  anything malformed in it, a setting of the other game's among them, is an
  `invalid configuration file` error naming the file and the cause.
- **Breaking:** A game is played from a configuration file, which is
  required: `social-deduction werewolf CONFIG` and
  `social-deduction avalon CONFIG`, each with `--log` or `--no-log`. The
  role-count flags (`--werewolves`, `--villagers`, `--doctors`, `--seers`,
  `--merlin`, `--percival`, `--servants`, `--assassin`, `--morgana`,
  `--mordred`, `--oberon`, `--minions`) and the phase-limit flags
  (`--night-limit`, `--day-limit`, `--ratify-limit`, `--propose-limit`,
  `--vote-limit`, `--quest-limit`, `--assassinate-limit`) are gone: each
  setting is written under `[roles.<role>] count` or `[phases]` in the
  file, and a setting comes from the file, otherwise from the code. The
  example configurations moved from `examples/` to `configs/`.
- Seats are named `Player 1` onward, capitalized with a space before the
  number. Players are told in seat order: alphabetically by name, whatever
  its case, then by number, so `Player 2` comes before `Player 10`.
- Actors talk by sending messages alone: `Context::request`,
  `Behavior::answer` and the reply are gone, so a behavior handles
  everything in `receive`. The uniform-random variant plays the announcing
  environment as the model-played variant does: it announces each phase
  and waits for selections, and every player selects at once at random. Its
  log has `Announce` and `Select` messages and no `Replied` entries.
- The log is written in actions and observations by every actor. Each
  message an actor sends to others is an `Action` of its sender, logged
  whole, once, by the sender, with everyone it was sent to, and an
  `Observation` of each receiver, logged by the receiver as a receipt
  naming the message by its sender and number and the phase it arrived in.
  Every actor numbers what it sends from 1. `Sent` and `Received` are gone.
  A model player's `Action` says how many history entries the model was
  shown when it made the call that produced the message. Announcements
  carry the phase's limit, and the model player's request tells it: "It is
  day 2, which lasts 1m." `Hunted` names its phase. The struct a player is
  shown is `View`, and the players with the most votes are `most_votes`.
- The command line is `social-deduction werewolf uniform-random`, a game
  and then its variant, each a subcommand with its own `--help`. The role
  counts `--werewolves`, `--villagers`, `--doctors` and `--seers` belong to
  the variant; the 0.1.0 top-level flags are gone.
- By day a player may vote against anyone living but itself, whatever it
  knows them to be, so a seer can vote out a werewolf it has found. By
  night it still chooses among those whose roles it does not know.
- A message that arrives after its phase has ended, in Werewolf or Avalon,
  stays in the log and out of the narration, instead of being told as a
  deed of the phase that followed.
- Every living player is announced every phase, awake in it or not, in
  name order, so a player knows when the day has ended. A model player
  announced a phase it sleeps through remembers the announcement and makes
  no call: by night a villager is woken by nothing, talk from the day is
  observed and ignored, and a day's `wait` reminder no longer counts once
  the night is announced. A uniform-random player sleeps through such
  a phase too, so its games play as before. A player's story tells a phase
  it sleeps through as the phase line and "You sleep."
- A model player's story tells a past day more briefly: its talk, in
  order, then a tally of each player alive that day, in seat order, with
  the vote that counted or that it did not vote, then the day's outcome.
  The current phase is told in full, every vote and tentative decision
  included. The omniscient account reads as before.
- Every Werewolf episode holds one kind of actor, `werewolf::Actor`: the
  environment, a random player or a prompted player. `werewolf::game`
  builds every episode, with each seat's player built for it, so a table
  may seat players of both policies. The player modules are named after
  their policies: `werewolf::uniform_random` is `werewolf::random`, and
  `werewolf::llm` is `werewolf::prompted`.
- **Breaking:** Werewolf is one game, played by one command,
  `social-deduction werewolf [CONFIG]`, whose players are dealt
  strategies. `werewolf uniform-random` and `werewolf llm` are gone.
  Without a file every player is random, as in 0.1.0's game. The
  configuration file defines named models under `[models.<name>]` and
  named strategies under `[strategies.<name>]`, each a policy, `random` or
  `prompted`, with its parameters, a prompted one its `system` template and
  its `model`; and allots each role's seats among the strategies by a
  `[roles.<role>.strategies]` table, with the role's own `strategy`
  filling the seats it leaves; by a role's `strategy`; or by the file's
  top-level `strategy`, otherwise `random`. Roles and strategies are dealt
  together at random. Only the models in play are set up, and a game may
  play several, each with its own tool choice and context window. The `[model]`, `[prompt]` and `[personas]`
  tables, each role's `system` template and the `persona` variable are
  gone, and so are the command line's `--model-base-url`, `--model-id` and
  `--context-window`: a model is changed in its `[models.<name>]` table,
  where its context window may be given too. The log's `Start` carries
  every player's strategy and no longer a variation, `Configuration` is
  logged in every game and records the strategies, the allotments and each
  model in play, and the narration tells each player's strategy with its
  role and each model's context window by name. The examples are
  `examples/werewolf/random.toml` and `examples/werewolf/prompted.toml`.

[Unreleased]: https://github.com/wpm/FreeAgent/compare/main...HEAD
