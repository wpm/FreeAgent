# Social deduction

Social deduction games, played by [free agents](../../README.md), either at
random or by a language model: Werewolf and Avalon.

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

A game is played from a configuration file. Each release archive holds a
`configs/` directory beside the binary, with a table of random players and
a table of prompted players for each game. The shell and PowerShell
installers install the binary alone; take the configurations from the
archive on the [release](https://github.com/wpm/FreeAgent/releases) or from
[`apps/social-deduction/configs/`](configs) in the repository. From the
root of an unpacked archive, or from `apps/social-deduction/` in a
checkout:

```sh
social-deduction play configs/werewolf/random.toml
```

The file's `game` key names the game it describes, `"werewolf"` or
`"avalon"`, and `play` plays that game:

```sh
social-deduction play CONFIG [--log | --no-log]
```

The game is told on standard output as it happens and logged to standard
error as JSON Lines. `--no-log` leaves the log out, and `--log` keeps it;
the last one given wins. A file is checked whole before the game begins: a
missing or unknown `game`, a setting of the other game's or any other key
the game does not know, a TOML syntax error, or settings that do not fit
together stop the program with `invalid configuration file` and the cause,
placed at its line and column wherever TOML can place it.

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

The table seats two werewolves, three villagers, a doctor and a seer unless
the file's counts say otherwise, and each phase waits a minute for its
selections unless the file's limits say otherwise.

Each player plays a *strategy*, dealt to its seat with its role. A strategy
is a *policy*, a kind of player written in Rust, with its parameters:

- `random` selects at once, at random among those it may choose: by night
  the living whose roles it does not know, by day everyone else living.
- `prompted` is a language model, told a system prompt and asked to play.

The configuration file defines named models and named strategies, and
allots each role's seats among the strategies, so a table may mix players:
random werewolves against prompted villagers, or two werewolves with
different prompts. The roles and strategies are dealt together at random:
a seat is as likely as any other to get each role, and each seat of a role
as likely as any other to get each of that role's strategies. The
[configs](configs/werewolf) are a table of random players, which needs no
model, provider or API key and finishes in moments, and a table of one
prompted strategy for everyone:

```sh
social-deduction play configs/werewolf/random.toml
social-deduction play configs/werewolf/prompted.toml
```

A setting comes from the file if it is given there, otherwise from a
default in the code. The log's second entry is
the configuration as it took effect: the version of `social-deduction`
that played the game, the role counts and limits, every
strategy as written, the allotments, each model in play with what was
settled for it, each prompted player's rendered prompt, and the text
blocks as written, so a log says by itself what was played and under which
prompts. The API key is never logged.

### The configuration file

```toml
game = "werewolf"

strategy = "cautious"         # every role not allotted otherwise; random unless it says

[phases]
ratify_limit = "20s"

[models.qwen]
base_url = "http://localhost:1234/v1"
id = "qwen2.5-7b-instruct"

[prompt_parts]
rules = """Werewolf is a game of hidden roles. …"""

[strategies.cautious]
policy = "prompted"
system = """{{ rules }}

You are {{ name }}, a {{ role }}. You are slow to accuse."""

[strategies.bold]
policy = "prompted"
system = """{{ rules }}

You are {{ name }}, a {{ role }}. You trust your instincts and say so."""

[roles.werewolf.strategies]   # one random werewolf and one bold one
random = 1
bold = 1

[roles.doctor]
count = 0
```

Every table may be left out, and a key the file does not know is an error.

- `game`, at the top of the file, before any table: `"werewolf"`. Every
  file has one.
- `[phases]`: `night_limit` and `day_limit`, how long each phase waits for
  the players' selections, a minute each unless they say; and
  `ratify_limit`, how long a day's votes must stand to be ratified, zero
  unless it says, which means a day is not ratified. The ratify limit may
  not be longer than the day limit. Durations are written as `30s` or
  `1m 30s`.
- `[models.<name>]`: one model and how to reach it. `base_url`, the root of
  the provider's OpenAI-compatible API, ending in `/v1`; `id`, the model's
  id exactly as the provider lists it; `api_key_env`, the environment
  variable holding the API key, over the one known for the provider;
  `request_timeout`, how long one request may take, a minute unless it
  says; and `context_window`, the model's context window in tokens.
- `[strategies.<name>]`: a `policy` and that policy's parameters, and only
  those. `random` takes none. `prompted` takes `system`, its system prompt
  template, and `model`, the name of one of the file's models, which may be
  left out when the file defines exactly one. The strategy `random` is
  always defined, and a file may not define it.
- `[roles.werewolf]`, `[roles.villager]`, `[roles.doctor]` and
  `[roles.seer]`: `count`, how many of the role sit at the table;
  `strategy`, the strategy every seat of the role plays that its
  `strategies` table does not allot; and `[roles.<role>.strategies]`, how
  many seats of the role play each strategy.
- `strategy`, at the top of the file: the strategy every role plays that
  is not allotted otherwise.
- `[prompt_parts]`: named pieces of text the templates pull in with
  `{{ name }}`, so that the rules are written once. A part's name is
  letters, digits and underscores.

A role's `strategies` table allots its seats first, and the role's own
`strategy` fills the rest, up to the role's count. These villagers are 2
bold and 3 cautious:

```toml
[roles.villager]
count = 5
strategy = "cautious"

[roles.villager.strategies]
bold = 2
```

A role with a `strategies` table and no `strategy` of its own has as many
seats as the table allots. A role with no table plays its `strategy`,
otherwise the file's `strategy`, otherwise `random`. A count in the file
that the table's seats do not fit is an error
naming the role and both numbers, and so is a strategy allotted that is
not defined. A strategy defined and never allotted is fine.

The key itself is never in the file. `api_key_env` may be left out for a
provider whose variable is known, such as OpenAI's `OPENAI_API_KEY` and
Anthropic's `ANTHROPIC_API_KEY`, and for a local server, which wants none.

A game may play several models, each through the strategies that name it,
so a stronger model can play a weaker one. Here the werewolves play a
Claude model and everyone else a local Qwen model:

```toml
strategy = "local"

[models.qwen]
base_url = "http://localhost:1234/v1"
id = "qwen2.5-7b-instruct"

[models.claude]
base_url = "https://api.anthropic.com/v1"
id = "claude-sonnet-4-5"

[strategies.local]
policy = "prompted"
model = "qwen"
system = "You are {{ name }}, a {{ role }}."

[strategies.smart]
policy = "prompted"
model = "claude"
system = "You are {{ name }}, a {{ role }}."

[roles.werewolf]
strategy = "smart"
```

### Prompts

A prompted player's system prompt is rendered from its strategy's `system`,
a [minijinja](https://docs.rs/minijinja) template with the variables `name`,
the player's seat; `role`, the role it was dealt; and every part in
`[prompt_parts]` by its name. A strategy reads differently by role through
conditionals such as `{% if role == "werewolf" %}`, and players differ by
playing different strategies. Every seat's prompt is rendered for every
(role, strategy) pair at the table whose strategy is prompted before the
game begins, so a template that does not compile, naming the strategy, or
one with a variable it does not have, naming the strategy, the seat and the
role, stops the program at once.

### Prompted players

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
player's utterance, vote or retraction and the environment's tentative
decision, and the model is offered these tools, of which it must call one
and may call several: `speak`, which says something to every other living
player, and never to the environment, which does nothing with speech;
`select`, which votes for a player, and in a ratified day may be called
again to change the vote, while in a day that is not the first vote is
final, as the tool tells the model, and `select` is offered until the
player has voted; in a ratified day, `retract`, which
withdraws the vote, so that the player is undecided again; and `wait`, which says nothing now and asks to be woken after some
seconds unless something wakes the player sooner. The calls are carried out
in the order the model made them. Wake-ups are debounced: a player has at
most one model call in progress and one waiting, however fast the talk
comes, and each call is made from everything the player had heard when it
began. A model may answer from a context that is out of date by the time it
answers; if the phase has ended by then, by night or by day, the player
sends nothing and logs what it withheld. A player ignores talk and votes
from a phase it is not in, and a tentative decision too: it logs the
receipt and neither remembers the message nor wakes for it, so day talk
that arrives by night is forgotten.

The votes are public. Each vote and each retraction goes to every other
living player as well as to the environment. With a ratify limit above zero
the day is ratified: a player's latest vote counts unless it has retracted
it since and is undecided again, and whenever the votes change who they
select, or come to select nobody, the environment tells every living player
so. A day ends once the tentative decision has stood unchanged for the
ratify limit, and the selected player is voted out; or at the day limit,
whatever is standing, and nobody is. A vote or retraction that leaves the
decision as it was leaves the deadline as it was, so a player selected is
voted out one ratify limit after the tentative decision naming them, unless
another follows first. The ratify limit is `ratify_limit` under
`[phases]`, and may not be longer than the day limit. It is zero unless the
file says, and a day whose ratify
limit is zero is not ratified: each player's first vote counts, a
retraction does not, and the day ends when everyone has voted or at its
limit. The account tells what everyone says as they say it, and a player's
story tells it what others said and how they voted. The current day is
told in full, every vote, retraction and tentative decision as it came. A
past day is told by its talk, in order, then a tally of each player alive
that day, in seat order, with the vote that counted or that it did not
vote, then who died. The prompt is the configuration's to write: the
program tells the model only what the tools do and that it may speak,
vote, retract its vote in a ratified day, or wait, so the rules text in the
file should say that by day everyone may talk at any time.

### Before a game begins

A model is in play when a prompted strategy that names it is dealt to a
seat. Only the models in play are set up, one at a time in name order,
since a local server loads each model it is asked for. A model the file
defines and no strategy at the table uses is not contacted, and its key
need not be set. Each model in play is checked before any player is built,
so a model that cannot play fails at launch, and the error names it. Its
key is read, the provider's listing must have its id, and it is then asked
twice exactly as a player is, once as by night and
once as by day: a short prompt, two candidates, and the tools a player has
in that phase, `select` by night and `speak`, `select`, `retract` and
`wait` by day, the tools of a ratified day whatever the ratify limit.
A model that does not answer both requests as a player must stops the
program, and the error names the request that failed and says why in the
provider's own words, such as a server that rejects the request or an
answer that makes no tool call. Any model that passes can play. Each model
in play keeps its own client, tool choice and context window, and each
prompted player asks its strategy's model with them.

A model's context window, in tokens, is settled before the game begins:
from `context_window` in its `[models.<name>]` table if given, otherwise
from the provider's listing of the model, otherwise
from the provider's default in `src/models.toml`, otherwise 4096 tokens, a
size most models can run in. The listing gives it when the model's entry
carries one of the fields named in `src/models.toml`, such as vLLM's
`max_model_len` or OpenRouter's `context_length`, holding a positive whole
number. The provider's default covers a listing that carries none, as
OpenAI's and Anthropic's do: 128000 tokens for OpenAI and 200000 for
Anthropic. A window of 0 stops the program, and so does one with no room
for a story: smaller than the reply, the longest system prompt of the
strategies asking the model, the day's tools and a situation, by estimate.
The account tells each model's window and where it came from, and the
configuration in the log records both.

Each request fits the context window. The window keeps 1024 tokens for the
reply, which the request leaves to the provider's default length, and the
rest is counted by estimate: a token for every 3 characters, rounded up,
and an allowance for the chat template around each message and each tool.
When a player's
whole story would not fit beside its prompt, the tools, where the game
stands and the reply, the player forgets talk, its own and others', oldest
first, until it does, and "You don't remember what was said before this."
stands where the last forgotten line was. Everything else in the story is
kept, and the player still remembers every line for its next request. A
request that does not fit even with no talk is sent as it is, and an
overflow is a failure to play, as any other is. The log's action and
withheld entries carry `forgot`, how many lines of talk the request left
out, whenever it left any out.

Every request makes the model call a tool, unless the model refuses that.
The night's request is made with `tool_choice` set to `"required"`; if the
provider refuses it as a bad request, it is made again with `"auto"`, which
asks the model to call a tool without making it, and both refusals are
given if that is refused too. The game's requests are made the way the
night's was answered, and the account says so, naming the model, when
that is `"auto"`. Under
`"auto"`, an answer that makes no tool call is a failure to play, as any
other answer a player cannot carry out is. Every request ends "Answer by
calling one of the tools."

To play against a local [LM Studio](https://lmstudio.ai) server, download
`qwen2.5-7b-instruct` there and start its server, which listens at
`http://localhost:1234/v1`. `social-deduction models http://localhost:1234/v1 --check`
lists what it serves and marks the models that play when asked, and
[`configs/werewolf/prompted.toml`](configs/werewolf/prompted.toml)
plays it, once its `[models.qwen]` table names the model and server you
have running:

```sh
social-deduction play configs/werewolf/prompted.toml
```

## Avalon

*The Resistance: Avalon* is played by 5 to 10 players on two teams, good
and evil, over at most five quests. Each quest is led by a leader, who
proposes a team of the size the quest needs. Every player votes on the
team in secret, and it goes when a strict majority of all the players
approves. Each member of a team that goes supports or sabotages the quest
in secret, and the quest fails when enough of them sabotage it: one,
except two on the fourth quest with seven or more players. Only how many
sabotaged it is told. `Player 1` leads first, and the lead passes in seat
order after every proposal. Everyone may talk in every phase.

Good wins with three successful quests, unless the Assassin then names
Merlin. Evil wins with three failed quests, five failed proposals in a row,
or Merlin named. A leader who proposes nobody in time fails the proposal,
a player who does not vote does not approve, and a member who does neither
supports.

| Players | Quest 1 | Quest 2 | Quest 3 | Quest 4 | Quest 5 |
|---|---|---|---|---|---|
| 5 | 2 | 3 | 2 | 3 | 3 |
| 6 | 2 | 3 | 4 | 3 | 4 |
| 7 | 2 | 3 | 3 | 4 (2 sabotages) | 4 |
| 8–10 | 3 | 4 | 4 | 5 (2 sabotages) | 5 |

Merlin, Percival and the Servants (loyal servants of Arthur) are good; the
Assassin, Morgana, Mordred, Oberon and the Minions (of Mordred) are evil.
Each player is shown its own role and what it sees of some others:

| Role | Sees |
|---|---|
| Merlin | every evil player but Mordred, as evil |
| Percival | Merlin and Morgana, as Merlin or Morgana |
| Evil, but Oberon | every other evil player but Oberon, as evil |
| Oberon, Servant | nobody |

Only the Assassin names Merlin, and only evil players sabotage. The
environment counts only the choices the rules allow.

A file whose `game` is `"avalon"` plays Avalon, with
[`play`](#installing) as above. The table seats Merlin, the Assassin, two Servants and a Minion unless the
file's counts say otherwise, and each phase waits a minute unless the
file's limits say otherwise. A table seats 5 to 10 players, at most one of each role but
Servant and Minion, and Merlin exactly when it seats an Assassin. The usual
splits between good and evil are 3–2, 4–2, 4–3, 5–3, 6–3 and 6–4 for 5 to
10 players. Under the strategy `random`, a leader proposes
a team drawn at random, a vote approves or rejects with even odds, a good
member supports and an evil one supports or sabotages with even odds, and
the Assassin names one of the players it does not see as evil. The game is
told and logged as Werewolf's is.

The configuration file is shaped like Werewolf's: a top-level `game`,
`"avalon"`, and `strategy`,
`[models.<name>]`, `[prompt_parts]`, `[strategies.<name>]` and each role's table,
with the same allotments and the same precedence, a setting coming from the
file, otherwise the code. The roles' tables are
`[roles.merlin]`, `[roles.percival]`, `[roles.servant]`,
`[roles.assassin]`, `[roles.morgana]`, `[roles.mordred]`,
`[roles.oberon]` and `[roles.minion]`. `[phases]` takes `propose_limit`,
`vote_limit`, `quest_limit` and `assassinate_limit`, a minute each unless
they say. Once the counts settle, the table is checked as above. The log's
second entry is the configuration as it took effect, as in Werewolf.
[`configs/avalon/random.toml`](configs/avalon/random.toml) allots every
role to `random`, with no models:

```sh
social-deduction play configs/avalon/random.toml
```

### Prompted players in Avalon

A prompted strategy may be allotted to any role, beside random ones, with
one model or several, as in Werewolf. Its template has the variables
`name`, `role`, `team` and every part in `[prompt_parts]`. `role` is told as a
sentence names it: "Merlin", "Percival", "a loyal servant of Arthur", "the
Assassin", "Morgana", "Mordred", "Oberon" or "a minion of Mordred"; `team`
is `good` or `evil`. Each
model in play is set up as in Werewolf, and asked four times before the
game, as the leader proposing a team, a voter, an evil member of a team
and the Assassin, each with that player's tools; a model that does not
answer each request well stops the program, naming it and the player it
did not answer as.

Every player may talk in every phase, so a prompted player is woken by its
announcement and by everything another player says in the phase, and
ignores talk from any other. Wake-ups are debounced, as in Werewolf. Each
request is the prompt, the player's story of the game so far, and where
the game stands: the quest and the proposal, the leader, the team's size
and the sabotages that fail it, the team proposed, how long the phase
lasts, and what is asked of the player. The story opens with who the
player is, which side it is on, and what it sees, such as "You are
Player 4, Percival, on the good side. One of Player 1 and Player 3 is Merlin
and the other Morgana.", and tells each
phase, how the one before came out, the player's own choices and all
talk. Every announcement carries the record of every team, vote and
quest, so a story that forgets old talk to fit the context window keeps
every fact of the game.

Every request offers `speak`, which says something to every other player,
and `wait`, which asks to be woken after some seconds. The player the phase
waits on is offered one tool more, whose call goes to the environment
alone:

| Phase | Who | Tool | Takes |
|---|---|---|---|
| Propose | the leader | `propose` | `team`, the seats, as many as the quest needs |
| Vote | every player | `vote` | `approve`, true or false |
| Quest | each member of the team | `perform` | `deed`, `support`, or for an evil player `support` or `sabotage` |
| Assassinate | the Assassin | `select` | `target`, a player it does not see as evil |

The tool stays on offer, and the environment counts the player's first
choice. A response with any call that is not well formed, such as a team of
the wrong size, fails as a whole, and a response that comes after its
phase has ended is withheld, as in Werewolf.
[`configs/avalon/prompted.toml`](configs/avalon/prompted.toml) plays a
game of model players against a local LM Studio server, once its
`[models.qwen]` table names the model and server you have running:

```sh
social-deduction play configs/avalon/prompted.toml
```

```toml
[phases]
vote_limit = "90s"

[strategies.dice]
policy = "random"

[roles.servant]
count = 3

[roles.minion.strategies]   # one dice minion and one random one
dice = 1
random = 1
```

## Models

The `models` command belongs to no game. It asks a provider, named by the
root of its OpenAI-compatible API, for the models it serves and lists them,
one id per line in sorted order. With `--check`, it asks each model listed
to play each game as a player would, Werewolf once as by night and once as
by day, then Avalon once as each player its phases wait on, one model at a
time, and marks with `*` those that answer every request well, with `~`
those that answer them well only when asked to call a tool and not made to
in some game, and with `!` those that do not, with a line at the end saying
so. A model that refuses to be made to call a tool in one game is asked to
call one in the next. Why each `!` model failed goes to standard error, so
the listing stays readable: a line `! id` naming the model as the listing
marks it, then the error's causes, each on its own indented line, the
first naming the game and the next the request it did not answer, so a
provider's words may run to several lines and still be read as one
model's. A model that could not be asked at all, because the provider was
not reached or answered with anything but the model's answer, such as a
missing key or a rate limit, stops the listing with the error, since
nothing it would say about the rest would be true.
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
