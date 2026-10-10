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
every player chooses at random:

```sh
social-deduction werewolf uniform-random [--werewolves N] [--villagers N] [--doctors N] [--seers N]
```

The table seats two werewolves, three villagers, a doctor and a seer unless
the counts say otherwise. The game is told on standard output as it happens
and logged to standard error as JSON Lines.

In `llm` every player is a language model, set up by a TOML configuration
file that names the model, how to reach it, and the prompts each role is
given. The [examples](examples/werewolf/llm) show how one is written.

```sh
social-deduction werewolf llm --config game.toml [role counts] [--night-limit 30s] [--day-limit 2m]
```

A setting comes from the command line if it is given there, otherwise from
the file, otherwise from a default in the code. The file names the
environment variable that holds the provider's API key, never the key
itself. For now `llm` checks its configuration, prints the settings that
took effect, and says the game is not playable yet.
