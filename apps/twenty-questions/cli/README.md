# Twenty Questions

Twenty Questions, played by [free agents](../../../README.md): a host holds a
secret object, and language-model players talk among themselves and put
yes-or-no guesses to it until one names the object, the twenty guesses run
out, or the time does. Everyone talks at once, with no turns. The host is a
model too, and runs the game: it picks the object when the configuration
names none, answers each guess in the order it came, and ends the game. A
guess that repeats one already counted, as when two players put the same
guess at once, is not counted and costs no guess.

## Installing

[Download Twenty Questions](https://wpm.github.io/FreeAgent/twenty-questions.html#platforms)
for macOS, Windows or Linux. Each installer gives you the desktop app and
the `twenty-questions` command line:

- **macOS:** open `Twenty-Questions.dmg`, drag the app to Applications,
  then choose **Twenty Questions ▸ Install Command-Line Tool…**.
- **Windows:** `Twenty-Questions-setup.exe` puts `twenty-questions` on your
  PATH, in terminals opened after installing.
- **Linux:** the `.deb` and `.rpm`, for x86_64 and arm64, install
  `/usr/bin/twenty-questions`.

### Servers and CI

The command line comes with every desktop installer: on the PATH with
the `.deb`, the `.rpm` and the Windows setup, and from the macOS app's
**Twenty Questions ▸ Install Command-Line Tool…** menu.

Without the desktop app or its webview libraries, the command line is an
archive for each platform on the latest `twenty-questions-v*` release on
the [Releases page](https://github.com/wpm/FreeAgent/releases):
`twenty-questions-<target>.tar.gz`, or `.zip` for Windows, each with a
`.sha256` beside it. It holds the binary, the example configurations, this
README and the licenses.

With a Rust toolchain, `cargo install --git https://github.com/wpm/FreeAgent twenty-questions`
builds it from source.

## Playing

A game is played from a configuration file. The binary carries the example
configurations. `twenty-questions configs` lists them, each with the first
sentence of its opening comment, and `twenty-questions configs --write .`
writes them into the current directory, overwriting a file already there
only when given `--force`:

```sh
twenty-questions configs --write .
twenty-questions play prompted.toml
```

`prompted.toml` plays against a local [LM Studio](https://lmstudio.ai) server
serving `qwen2.5-7b-instruct-1m`. `ollama.toml` plays the same game against a
local [Ollama](https://ollama.com) server serving `qwen2.5:7b`, and
`mixed.toml` seats the players on LM Studio and the host on Claude Haiku,
which answers guesses more truly. The same files are in
[`configs/`](configs). A configuration file that is missing says how to write
out the examples.

```sh
twenty-questions play CONFIG [--episodes N] [--log | --no-log]
```

The game is told on standard output as it happens and logged to standard
error as JSON Lines. `--no-log` leaves the log out, and `--log` keeps it;
the last one given wins. `--episodes N` plays N games one after another, for
training data. The models are set up once, and every episode deals the
strategies to the seats afresh and, when the host picks the object, picks a
new one. Each episode logs under an episode ID of its own, in one JSON Lines
stream, and its account follows the one before it after a blank line.

Every model in play is asked to play before the first game begins, as a
player, as the host answering a guess, and as the host picking an object, so
one that cannot is refused at launch, in its provider's words. Before that,
the file is checked whole: a key the game does not know, a TOML syntax
error, or settings that do not fit together stop the program with `invalid
configuration file` and the cause; and every prompt is rendered, so a
template that does not render stops it too.

## The configuration file

The [example](configs/prompted.toml) says what each setting does.

```toml
format = 1
guesses = 20
time_limit = "10m"

[players]
count = 4
strategy = "everyone"
strategies = { skeptic = 1 }

[host]
temperature = 1.2
categories = ["an animal", "a musical instrument"]
pick = "Pick a secret object for a game of Twenty Questions: {{ category }}."
system = "{{ rules }} You are the host. The secret object is {{ object }}."

[models.qwen]
base_url = "http://localhost:1234/v1"
id = "qwen2.5-7b-instruct-1m"
context_window = 3072

[prompt_parts]
rules = "Twenty Questions: a host holds a secret object, …"

[strategies.everyone]
system = "{{ rules }} You are {{ name }}, a player, with {{ others }}."

[strategies.skeptic]
system = "{{ rules }} You are {{ name }}. You doubt every easy answer."
```

A key the file does not know is an error, and so is every setting out of
range, naming its key.

- `format`: the configuration format the file is written in, `1`. A file
  without one is format 1, and a file of a newer format is refused.
- `guesses`: how many guesses the players share, 1 to 20, and 20 unless it
  says.
- `time_limit`: how long the game lasts, at least `30s`, and `10m` unless it
  says. Durations are written as `30s` or `1m 30s`.
- `[players]`: `count`, how many play, 1 to 12, and 4 unless it says, seated
  in the order Alice, Bob, Carol, Dave, Eve, Frank, Grace, Heidi, Ian, Judy,
  Kevin and Leo; `strategies`, how many seats play each strategy; and
  `strategy`, the strategy every other seat plays, which may be left out
  when the file defines exactly one.
- `[host]`: `system`, the host's system prompt template; `object`, the
  secret object, which the host picks when it is left out; `pick`, the
  template the host is asked to pick an object with; `categories`, of which
  the pick is asked for one, drawn at random; `temperature`, the temperature
  the pick is asked at, from 0 to 2, and 1.2 unless it says; and `model`, the
  name of the model the host asks, which may be left out when the file
  defines exactly one. A host needs an `object` or a `pick`.
- `[models.<name>]`: one model and how to reach it. `base_url`, the root of
  the provider's OpenAI-compatible API, ending in `/v1`; `id`, the model's id
  exactly as the provider lists it; `api_key_env`, the environment variable
  holding the API key, over the one known for the provider;
  `request_timeout`, how long one request may take, a minute unless it says;
  and `context_window`, the model's context window in tokens, over what the
  provider reports.
- `[strategies.<name>]`: a prompted player. `system`, its system prompt
  template, and `model`, the name of one of the file's models, which may be
  left out when the file defines exactly one.
- `[prompt_parts]`: named pieces of text the templates pull in with
  `{{ name }}`, so that the rules are written once.

A player's prompt is rendered with `name`, the player's own name, `others`,
everyone else's, and `guesses`; the host's with `object` and `players`; and
the pick with `category`. Only the host's prompt knows the object.

## The log

Each episode's log opens with a `Header`, naming the log's `format`, the
`version` of `twenty-questions` that wrote it and the `game`,
`"twenty-questions"`, and closes with `Ended`: `"won"`, `"stopped"`, or
`{"failed": error}` with the error and its causes. Between them:

- `Start`: every player's strategy, the guesses and the time limit.
- `Configuration`: the game as it took effect: the version, the guesses and
  the time limit, the host's settings and every strategy as written, the
  allotments, each model in play with what was settled for it, each
  player's rendered prompt, and the prompt parts.
- `Chosen`: the object the host picked, and the category it was asked for.
- `Action` and `Observation`: each message, logged whole by its sender and
  as a receipt by each receiver. A message is speech, a `Guess`, or the
  host's `Answer`, which says which guess it answers, whether it names the
  object, and how many guesses are left.
- `Thinking`, `Thought` and `Usage`: each model call going out, coming back,
  and the usage its provider reported.
- `Failed`: an actor that failed to act, with the error.
- `Uncounted`: a guess that came after the last one.
- `Repeated`: a guess repeating one counted already, as when two players put
  the same guess at once. It is neither counted nor answered.
- `End`: the side that won, `players` or `host`, and how: `named`,
  `out_of_guesses` or `out_of_time`; the guesses used, the object, and how
  long the game took.

A log carries no secrets, so it is safe to share. Every API key the run
loaded is written `[redacted]` wherever a provider echoes it, and every URL
is written without its userinfo and query.

## Models

```sh
twenty-questions models URL [--api-key-env VAR] [--check [ID ...]]
```

`models` asks a provider, named by the root of its OpenAI-compatible API,
for the models it serves and lists them, one id per line in sorted order.
With `--check`, it asks each model listed to play Twenty Questions, as a
player, as the host answering a guess, and as the host picking an object,
one model at a time, and marks with `*` those that answer every request
well, with `~` those that answer them well only when asked to call a tool and
not made to, and with `!` those that do not, with a line at the end saying
so. Why each `!` model failed goes to standard error: a line `! id`, then
the error's causes, each on its own indented line. A model that could not be
asked at all, because the provider was not reached or answered with anything
but the model's answer, stops the listing with the error. `--check` followed
by ids asks only those, each of which must be listed; the ids end with `--`
when the base URL follows them. `models --help` tables the providers known
and the environment variable each one's key is read from:

```sh
twenty-questions models http://localhost:1234/v1
twenty-questions models http://localhost:11434/v1 --check
twenty-questions models --check qwen2.5:7b -- http://localhost:11434/v1
twenty-questions models https://example.com/v1 --api-key-env EXAMPLE_API_KEY
```

The design is [ADR-0013](../../../docs/decision-history/0013-twenty-questions-the-host-is-the-environment-and-a-player.md).
