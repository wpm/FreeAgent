# Twenty Questions

Twenty Questions, played by [free agents](../../../README.md): a host holds a
secret object, and language-model players talk among themselves and put
yes-or-no guesses to it until one names the object, the twenty guesses run
out, or the time does. Everyone talks at once, with no turns. The host is a
model too, and runs the game: it picks the object when the configuration
names none, answers each guess in the order it came, and ends the game.

## Playing

A game is played from a configuration file:

```sh
cargo run -p twenty-questions -- play apps/twenty-questions/cli/configs/prompted.toml
```

plays the example against a local [LM Studio](https://lmstudio.ai) server
serving `qwen2.5-7b-instruct-1m`, tells the game on standard output as it
happens, and logs it to standard error as JSON Lines. Every model in play is
asked to play before the game begins, so one that cannot is refused at
launch, in its provider's words.

The [example](configs/prompted.toml) says what each setting does: how many
players and which strategy each plays, the guesses and the time limit, the
host's object or how it picks one, the models, and the prompts.

The design is [ADR-0013](../../../docs/decision-history/0013-twenty-questions-the-host-is-the-environment-and-a-player.md).
