# Werewolf

The social deception game, played by [free agents](../../README.md), either at
random or by a language model.

A moderator actor runs the game: it deals the roles, asks the living players
what they want to do, and announces the winner. Each player is an actor too,
and knows only what the moderator has told it.

From the repository root:

```sh
# The paper's random game: sixteen players, four werewolves
cargo run -p werewolf -- apps/werewolf/configs/random.toml

# The same game replayed from a seed
cargo run -p werewolf -- apps/werewolf/configs/random.toml --seed 5

# A game of models; the key comes from the environment
export ANTHROPIC_API_KEY=...
cargo run -p werewolf -- apps/werewolf/configs/llm.toml
```

The output is the record of the game: one JSON object per line for every
request and every reply between the moderator and the players, in the order
they happened, and a last line saying how the game ended:

```json
{"players":16,"werewolves":4,"seed":5,"winner":"Werewolves","rounds":5}
```

The seed is always printed, drawn at random when neither the file nor the
command line gives one, so any game can be replayed from its own last line. A
loop reproducing the paper keeps that line; a display or a training set keeps
them all.

## The game

Play begins with a day and alternates from there.

By day, the living take turns in a shuffled order. On a turn a player may say
something to the village and may nominate one living player to eliminate; the
rest of the village hears each turn as it is taken, and a nomination replaces
the player's earlier one. After a full round, if everyone has nominated and
one player leads, that player is eliminated. Otherwise another round begins,
until the day's time limit, when the current leader goes, with a tie broken at
random and no nominations hanging nobody.

By night, each werewolf names a villager to kill. The plurality dies, with a
tie broken at random.

The villagers win when the last werewolf is dead. The werewolves win when they
are at least as many as the villagers.

## At random

With `kind = "random"`, every player nominates uniformly at random among the
candidates, never itself, and says nothing. This is the game of

> Braverman, Etesami, and Mossel. *Mafia: A theoretical study of players and
> coalitions in a partial information environment.*

With everyone choosing uniformly, a plurality vote with ties broken at random
eliminates a uniformly random candidate, which is the paper's rule, and a
tied day is simply rolled again. The paper's result is that the game is
balanced when the werewolves number about the square root of the players.

## By language model

With `kind = "llm"`, every word and every vote comes from a model speaking the
OpenAI chat completions protocol, which Anthropic's models do through their
compatible endpoint. Each player is one conversation: its role's system prompt,
then everything the moderator tells it. On a turn the model may speak and may
nominate with a `nominate` tool whose only argument is the list of living
candidates; at night it is told to nominate and asked again if it does not.

The API key is read from the environment variable the configuration names. It
is held as a secret that never prints, and goes into the request header and
nowhere else; the game's record contains only what was said in the game.

## Configuration

```toml
players = 6
werewolves = 2
names = ["Ann", "Bob", "Cat", "Dan", "Eve", "Fay"]   # optional
seed = 42                                            # optional

[timing]
day_secs = 300        # the village has this long to agree
patience_secs = 120   # how long to wait for any one answer

[policy]
kind = "llm"          # or "random", which needs nothing else
model = "claude-opus-5-5"
base_url = "https://api.anthropic.com/v1"   # any OpenAI-compatible endpoint
api_key_env = "ANTHROPIC_API_KEY"           # the key itself is never in a file
attempts = 3          # tries for a request that fails on the way to the model
# temperature = 1.0   # optional; current Claude models reject it

[prompts]             # each role's system prompt; sensible defaults
villager = "..."
```

Unknown keys, more werewolves than players, and a list of names that is not
one per player are all errors, as is a missing key. See `configs/` for
complete examples of both kinds of game.

## Not yet

The doctor and the seer have prompts but are never dealt, and the werewolves
do not confer at night. Both are next.
