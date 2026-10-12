# Social deduction

Social deduction games, played by [free agents](../../README.md), either at
random or by a language model. The first of them is Werewolf.

## Installing

Each [release](https://github.com/wpm/FreeAgent/releases) carries a built
`social-deduction` binary for macOS, Linux, and Windows, and an installer
that picks the right one:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/wpm/FreeAgent/releases/latest/download/social-deduction-installer.sh | sh
```

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/wpm/FreeAgent/releases/latest/download/social-deduction-installer.ps1 | iex"
```

With a Rust toolchain, `cargo install --git https://github.com/wpm/FreeAgent social-deduction`
builds it from source.

## Werewolf

A few of the players are secretly werewolves; the rest are villagers, among
them a doctor and a seer. Play alternates between night and day, starting
with night. By night the werewolves, who know one another, choose a villager
to kill, the doctor chooses one player to save from that kill, and the seer
learns one player's side. By day everyone votes and the player with the most
votes is eliminated. Werewolves know who the other werewolves are, the seer
knows what it has discovered, and everyone else knows only their own role and
who is still alive. The villagers win when the last werewolf is dead. The
werewolves win when they are at least as many as the villagers.

The command line names the game, then its variant. In `uniform-random`
the environment announces each phase to every living player and waits for
the awake players' selections, as it does for language models, and every
player awake in the phase selects at once, at random among those it may
choose: by night the living whose roles it does not know, by day everyone
else living. A player with nothing to do in the phase sleeps. It plays the
model-played game with no model, provider, API key or configuration file,
so a game finishes in moments.

```sh
social-deduction werewolf uniform-random [--werewolves N] [--villagers N] [--doctors N] [--seers N] [--night-limit 30s] [--day-limit 2m]
```

The table seats two werewolves, three villagers, a doctor and a seer unless
the counts say otherwise, and each phase waits a minute for its selections
unless a limit says otherwise. The game is told on standard output as it
happens and logged to standard error as JSON Lines. Every variant takes
`--no-log`, which leaves the log out, and `--log`, which keeps it; the last
one given wins.

In `llm` every player is a language model, set up by a TOML configuration
file that names the model, how to reach it, and the prompts each role is
given. The [examples](examples/werewolf/llm) show how one is written.

```sh
social-deduction werewolf llm game.toml [role counts] [limits] [--ratify-limit 20s] [--model-base-url URL] [--model-id ID]
```

A setting comes from the command line if it is given there, otherwise from
the file, otherwise from a default in the code. The model and its provider
may be given on the command line too, for trying another without editing
the file. The game is told and logged as the other variants are, and the
log's second entry is the configuration as it took effect: every setting,
each player's rendered prompt, and the templates and text blocks as
written, so a log says by itself what was played and under which prompts.
The API key is never logged.

Every living player is told each phase as it begins, awake in it or not,
since only the environment knows when a phase ends. A player with nothing
to do in the phase sleeps: it remembers the announcement, makes no model
call, and is woken by nothing until the day, and its story says "You
sleep." Each time a player is woken, it is told its prompt and the story of
the game so far from its own point of view, what others said and how they
voted included, and where the game stands, and the model answers with tool
calls. By night a werewolf, the doctor or the seer is woken once, when the
night is announced to it, and selects one of the living with one forced
`select` call, which goes to the environment alone; the first selection
counts, and the night ends when everyone awake has selected or at its
limit. A player whose model fails,
because it was not reached or answered with something that is not well
formed, has done nothing, and the failure is logged with the phase and the
error and told as it happens; the game goes on, and the player is not told
of it.

By day the players talk, with no turns. Every living player is woken when
the day is announced to it and again by every message it hears, another
player's utterance or vote and the environment's tentative decision, and
the model is offered three tools, of which it must call one and may call
several: `speak`, which says something to every other living player, and
never to the environment, which does nothing with speech; `select`, which
votes for a player, and may be called again to change the vote; and `wait`,
which says nothing now and asks to be woken after some seconds unless
something wakes the player sooner. The calls are carried out in the order
the model made them. Wake-ups are debounced: a player has at most one model
call in progress and one waiting, however fast the talk comes, and each
call is made from everything the player had heard when it began. A model
may answer from a context that is out of date by the time it answers; if
the phase has ended by then, by night or by day, the player sends nothing
and logs what it withheld. A player ignores talk and votes from a phase it
is not in, and a tentative decision too: it logs the receipt and neither
remembers the message nor wakes for it, so day talk that arrives by night
is forgotten.

The votes are public and ratified. Each vote goes to every other living
player as well as to the environment, a player's latest vote counts, and
whenever the votes change who they select, or come to select nobody, the
environment tells every living player so. A day ends once the votes have
stood unchanged for the ratify limit, and the selected player is voted
out; or at the day limit, whatever is standing, and nobody is. The ratify
limit is `ratify_limit` under `[phases]`, or `--ratify-limit` on the
command line, twenty seconds unless either says, and may not be longer
than the day limit. The account tells what everyone says as they say it,
and a player's story tells it what others said and how they voted. The
prompt is the configuration's to write: the program tells the model only
what the three tools do and that it may speak, vote, or wait, so the rules
text in the file should say that by day everyone may talk at any time.

### The configuration file

Every table but `[model]` may be left out, and a key the file does not know
is an error.

- `[model]`: `base_url`, the root of the provider's OpenAI-compatible API,
  ending in `/v1`; `id`, the model's id exactly as the provider lists it;
  `api_key_env`, the environment variable holding the API key, over the one
  known for the provider; and `request_timeout`, how long one request may
  take, a minute unless it says.
- `[phases]`: `night_limit` and `day_limit`, how long each phase waits for
  the players' selections, a minute each unless they say; and
  `ratify_limit`, how long a day's votes must stand to be ratified, twenty
  seconds unless it says. Durations are written as `30s` or `1m 30s`.
- `[roles.werewolf]`, `[roles.villager]`, `[roles.doctor]` and
  `[roles.seer]`: `count`, how many of the role sit at the table, and
  `system`, the role's own system prompt template over the default.
- `[prompt]`: `system`, the system prompt template every role falls back on.
- `[text]`: named blocks of text for the templates to use, so that the rules
  are written once.
- `[personas]`: what a seat, `player1` onward, is like, for the seats that
  are anyone in particular.

The key itself is never in the file. `api_key_env` may be left out for a
provider whose variable is known, such as OpenAI's `OPENAI_API_KEY` and
Anthropic's `ANTHROPIC_API_KEY`, and for a local server, which wants none.

### Prompts

Each player's system prompt is rendered from a
[minijinja](https://docs.rs/minijinja) template: the role's own `system`
when `[roles.<role>]` has one, otherwise `[prompt] system`. A template has
the variables `name`, the player's seat; `role`, the role it was dealt;
`persona`, its seat's persona, empty for a seat without one; and every block
in `[text]` by its name. One template for everyone ignores `role`; prompts
that differ by role are conditionals, text blocks or a role's own template;
prompts that differ by player are personas, which belong to a seat while
the roles are still dealt at random. Every seat's prompt is rendered for
every role at the table before the game begins, so a template that does not
compile, a variable it does not have, a role at the table with no template,
or a persona for a seat that is not there stops the program at once.

### Before a game begins

The model is checked before any player is built, so a model that cannot
play fails at launch. The provider's listing must have the model's id, and
the model is then asked twice exactly as a player is, once as by night and
once as by day: a short prompt, two candidates, and the tools a player has
in that phase, `select` by night and `speak`, `select` and `wait` by day.
A model that does not answer both requests as a player must stops the
program, and the error names the request that failed and says why in the
provider's own words, such as a server that rejects the request or an
answer that makes no tool call. Any model that passes can play.

To play against a local [LM Studio](https://lmstudio.ai) server, download
`qwen2.5-7b-instruct` there and start its server, which listens at
`http://localhost:1234/v1`. `social-deduction models http://localhost:1234/v1 --check`
lists what it serves and marks the models that play when asked, and
[`examples/werewolf/llm/minimal.toml`](examples/werewolf/llm/minimal.toml)
plays it:

```sh
social-deduction werewolf llm examples/werewolf/llm/minimal.toml
```

## Models

The `models` command belongs to no game. It asks a provider, named by the
root of its OpenAI-compatible API, for the models it serves and lists them,
one id per line in sorted order. With `--check`, it asks each model listed
to play as a player would, once as by night and once as by day, one model
at a time, and marks with `*` those that answer both requests well and
with `!` those that do not, with a line at the end saying so. Why each `!`
model failed goes to standard error, so the listing stays readable: a line
`! id` naming the model as the listing marks it, then the error's causes,
each on its own indented line, so a provider's words may run to several
lines and still be read as one model's. A model that could not be asked at
all, because the provider was not reached or answered with anything but the
model's answer, such as a missing key or a rate limit, stops the listing
with the error, since nothing it would say about the rest would be true.
`--check` followed by ids asks only those, each of which must be listed,
and leaves the rest unmarked; the ids end with `--` when the base URL
follows them. A provider whose API key variable is known is read with it;
any other provider that wants a key is told the environment variable
holding it. `models --help` tables the providers known:

```sh
social-deduction models http://localhost:1234/v1
social-deduction models http://localhost:1234/v1 --check
social-deduction models https://api.openai.com/v1 --check gpt-5.5 gpt-5.5-mini
social-deduction models --check gpt-5.5 -- https://api.openai.com/v1
social-deduction models https://example.com/v1 --api-key-env EXAMPLE_API_KEY
```
