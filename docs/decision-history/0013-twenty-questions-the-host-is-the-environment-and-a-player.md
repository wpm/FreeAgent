# ADR-0013: Twenty Questions: the host is the environment and a player

**Status:** Accepted
**Date:** 2026-10-21
**Deciders:** Bill McNeill
**Depends on:** [ADR-0002](0002-actors-perceive-think-and-act.md),
[ADR-0003](0003-the-model-player.md),
[ADR-0005](0005-actors-talk-by-sending-messages-alone.md),
[ADR-0006](0006-players-talk-by-day.md),
[ADR-0007](0007-a-tool-call-is-required-where-the-model-allows-it.md),
[ADR-0011](0011-speech-is-never-cut-off-at-a-phase-end.md),
[ADR-0012](0012-applications-share-crates-each-game-is-its-own.md)

## Context

Twenty Questions is Free Agent's second application, in
`apps/twenty-questions`. A host holds a secret object, and model players
put yes-or-no questions to it until one names the object. By
[ADR-0012](0012-applications-share-crates-each-game-is-its-own.md), it
reaches its models through `free-agent-model` and shares its seats, example
format and log redaction through `free-agent-game`; everything typed on its
own messages and log is its own, copied from Social Deduction and edited.

Social Deduction's games are built around phases. An environment that
never calls a model announces each phase, players act in it, and the
environment closes it ([ADR-0011](0011-speech-is-never-cut-off-at-a-phase-end.md)).
Twenty Questions has no phases and no turns: everyone may talk at any time,
and one move counts, a guess. And the actor that runs the game, which
answers every guess and decides when it is won, has to be a model, since
only a model can judge a question about an object it was not told in
advance.

## Decision

**The host is one actor that is both the episode's environment and a model
player. It numbers, counts and answers guesses, picks the object when it
has to, and ends the episode. Everyone else is a prompted player, and
everyone talks to everyone at once.**

### The rules

- **The table.** A host, `Host`, and 1 to 12 players, named in seat order
  Alice, Bob, Carol, Dave, Eve, Frank, Grace, Heidi, Ian, Judy, Kevin and
  Leo.
- **The secret object.** The configuration names it, or the host picks one
  as the game begins, asked with a pick prompt of its own at a high
  temperature (1.2 unless configured), for a category drawn at random from
  the configuration's `categories`, under a system prompt of its own.
- **Everyone talks at once.** Whatever anyone says goes to everyone else,
  the host included.
- **A guess** is the one move that counts: a `Guess` message, a yes-or-no
  question put formally to the host. The players share twenty, numbered in
  the order the host receives them. A guess after the last is logged as
  `Uncounted`, and neither counted nor answered.
- **A repeated guess is not counted.** A guess whose question, trimmed and
  in any case, repeats one counted already, as when two players put the
  same guess at once, is logged as `Repeated`, and neither counted nor
  answered, so it costs no guess. Each player tells only the first of the
  two it hears, and takes the host's answer to either as the answer to
  both. *Amended 2026-10-21 (#466):* two players often put the same guess
  at the same moment, each before hearing the other's, and both were
  counted and answered alike; in one game four of the twenty guesses went
  to repeats.
- **The host answers every guess**, one at a time in the order received, in
  its own words, with one of two tools: `answer(text)` for a guess that does
  not name the object, and `named(text)` for one that does.
- **A guess that says the object's name wins**, whichever tool the host
  calls: the guess, lowercased, holds the object's name, lowercased and
  without a leading *a*, *an* or *the* or a measure such as *pair of*, as
  words of their own, singular or plural. *Scissors?* says *a pair of
  scissors*; *Is it a category?* does not say *a cat*. A guess that holds
  some word of the name, of three letters or more, wins when the host
  calls `named`, as *Is it a cast iron pan?* for *a cast iron skillet*. A
  guess the host calls `named` that holds no such word is answered *Yes.*
  and does not win. *Amended 2026-10-21 (#466):* the host called *Is it
  furniture?* a loveseat and *Is it a flower?* a tulip, and two of ten
  games were won that way.
- **The host keeps its secret.** A line of the host's, an answer or a word,
  that gives the object away before the game is won is not sent: one that
  says its name, or the name's last word, its head noun, unless the guess
  it answers said that word already. *Scissors are made of metal* gives
  away *a pair of scissors*; *Yes, it is a tower* answers *Is it a tower?*
  about *the Eiffel Tower*. An answer that gives it away after a leading
  *Yes* or *No* is cut to that word; any other such line is logged as
  `Failed`, and the host is asked again. *Amended 2026-10-21 (#466):* the
  host kept saying *No, a rose bush is not …* and failed the episode on
  the third try, when its *No* was right.
- **The end.** A guess answered as naming the object wins for the players.
  The last guess answered without it, or the time limit (10 minutes unless
  configured), wins for the host. A guess still waiting when the game ends
  is not answered. Either way the host logs `End`, with the object, and stops
  every player.

The two code-side rules about the object's name are there because small
models are unreliable judges of their own secret: by hand,
`qwen2.5-7b-instruct-1m` judged 13 of 14 guesses right, and Ollama's
`qwen2.5:7b` missed exact names.

### Messages

```rust
pub enum Message {
    Begin  { n, view: View },                                 // host → every player, once
    Say    { from, n, text },                                 // anyone → everyone else
    Guess  { from, n, text },                                 // player → everyone else
    Answer { n, guess, asked_by, text, named, left },         // host → every player
    Over,                                                     // host → itself, at the time limit
    Wake,                                                     // an actor → itself
    Remind { count },                                         // a player → itself
}
```

Every message an actor sends to others is numbered by its sender from 1,
logged once by the sender as an `Action` and by each receiver as an
`Observation`, as [ADR-0006](0006-players-talk-by-day.md) has it. There is
no phase, so nothing is tagged with one, nothing is withheld, and nothing
closes.

### The host

Two loops over one mind, never locked across a model call
([ADR-0002](0002-actors-perceive-think-and-act.md)).

- **Perceiving**, it counts each `Guess` while guesses remain and queues
  it, and keeps each line of speech it hears. A counted guess or a line of
  speech wakes the think loop, debounced as a player's is.
- **Thinking**, a guess waiting comes first: the host is asked to answer
  the oldest, shown the guess alone and the guess put about the object
  itself, its *it* replaced by the object (*Is a unicycle alive?*),
  offered `answer` and `named`. With none
  waiting, it is asked whether to speak only when a player spoke to it by
  name, or, once until the next guess, when nobody has put a guess for two
  minutes; otherwise the speech it heard is let go unasked. Asked, it is
  shown that speech and how long since the last guess, offered `speak` and
  a `wait` that says nothing, and told to ask "Is that your guess?" of a
  question put to it in speech, or to urge a quiet table to guess.
- **A failure is asked again.** An answer, or the pick, that fails or is not
  well formed is logged as `Failed` and asked again 10 seconds later, so
  that a busy server has time to clear, up to three times in all; then the
  episode fails, with the last error, which the host's
  clean-up returns. A word that fails is asked again up to as many times,
  and then the speech it was about is dropped.
- **The time limit** is a timer, `Over`, which ends an idle game; an answer
  or a word that comes back after the limit ends the game instead of being
  sent.

### The players

A prompted player, copied from Social Deduction's and edited. Two loops over
one mind, woken by every message it hears, at most one model call in flight
and one waiting, a `wait(seconds)` that sets its own reminder, and a story
told in the second person that forgets old talk, oldest first, to fit the
context window. Guesses and answers are never forgotten: a guess's own line
may be forgotten once it is answered, since the answer's line repeats the
question.

Each request ends with where the game stands: the guesses waiting for the
host's answer, how many are left, how much time is left, and what the
player may do. It offers `speak`, `wait`, and `guess` when the player may
guess: while guesses are left, none is waiting, and someone has spoken
since the last answer, so that a guess is proposed before it is made. A
guess the model makes that was put already fails the answer, and the next
request tells the player it was not put. A guess made while another came
in during the call is not put, so one guess waits at a time. A player
whose decision fails, its model unreached or its answer not well formed,
tries again after 10 seconds unless something wakes it sooner.

### The log

JSON Lines of `free_agent::Event`, as Social Deduction's, written by a
`Writer` copied from its own, each episode opening with a `Header` naming
`twenty-questions` and closing with how it `Ended`, keys redacted.

| Entry | Logged by | Records |
|---|---|---|
| `Start` | host | each player's strategy, the guesses and the time limit |
| `Configuration` | host | the configuration played, models and the players' rendered prompts included, keys never |
| `Chosen` | host | the object and the category drawn, when the host picked it |
| `Action`, `Observation` | each actor | every message, sent and received |
| `Thinking`, `Thought`, `Usage`, `Failed` | each actor | each model call, and each failure to act |
| `Uncounted` | host | a guess after the last |
| `Repeated` | host | a guess repeating one counted already |
| `End` | host | who won and how, the guesses answered, the object, how long it took |

### Configuration and startup

A TOML file, `deny_unknown_fields`, with `guesses`, `time_limit`,
`[players]` (`count`, `strategy`, `strategies`), `[host]` (`model`,
`object`, `temperature`, `categories`, `pick`, `system`), `[models.*]`,
`[prompt_parts]` and `[strategies.*]`, each strategy a prompted policy with
`system` and `model`. Each episode shuffles the strategies over the seats.
Before the first actor is built, the file is checked, every prompt rendered,
and each model in play listed, its context window settled, and probed as a
player, as the host answering a guess, and as the host picking, under
`"required"` and then `"auto"` ([ADR-0007](0007-a-tool-call-is-required-where-the-model-allows-it.md)).

## What watching games changed

Games against LM Studio's `qwen2.5-7b-instruct-1m` were watched while this
was built.

- **The context window.** LM Studio loads a model with 8192 tokens shared by
  4 requests at once, and refused requests past 2048 with "Context size has
  been exceeded". The example configuration sets `context_window`, and its
  prompts are short.
- **Answers without a story.** Shown the story of earlier answers, the host
  repeated them ("No, it is not alive" to every guess). Shown the guess
  alone, it answers each on its own.
- **The host speaks rarely.** Shown the whole story and asked whether to
  speak after every line of talk, the host answered guesses in plain
  speech, echoed the players, and asked "Is that your guess?" of players
  proposing guesses to one another. It is now asked only when spoken to by
  name or when the table has gone quiet, and shown only the speech since it
  last decided.
- **The secret is a head noun too.** Picking *a pair of scissors*, the host
  said "Scissors are typically made of metal", which the whole name did not
  match.
- **Players propose before they guess.** Woken together, every player
  guessed at once and all twenty guesses went in under a minute. Offering
  `guess` only once someone has spoken since the last answer and none is
  waiting, and not putting a guess that crossed another, turned the bursts
  into proposals. Saying a crossed guess as a proposal instead was tried,
  and made three players parrot the same line.

Tuning against `qwen2.5-7b-instruct-1m`, Ollama's `qwen2.5:7b` and
`llama3.2:3b` (#466) changed more.

- **The guess about the object itself.** On 72 test guesses with known
  answers, the host answered 59 right shown *Is it alive?* about a
  unicycle, and 66 shown also *Is a unicycle alive?*. Asking it to think
  first in an argument the players never see did no better, and telling it
  to add no hints, or that a similar thing is answered No, did worse (61
  and 62). An object
  picked without an article is asked about as *a* or *an* one: about *the
  raven*, read as Poe's poem, it answered 6 of 14 right; about *a raven*,
  11.
- **A short host prompt.** The host's instructions for speaking cost it
  answers: told only the rules and the object, it answered 12 of 12
  guesses about living things right, against 10 with them, and 67 of the
  72 against 66. The instructions for speaking are in the request that
  asks it to speak.
- **Repeated guesses.** Two players put the same guess at once, and both
  were counted: four of twenty guesses went to repeats in one game.
- **Retries.** A busy server timed out all four players' first calls, and
  nothing woke them again; a player refused a repeated guess in silence
  tried it again for five minutes.
- **Spoken to by name.** Players speak of *the host's answers* in nearly
  every line, and the host asked "Is that your guess?" nine times in one
  game; it is now asked only when called *host*.
- **Three players with parts.** Four players parroted one another, all
  woken by the same answer; three players and the host fit LM Studio's four
  slots. A scout, a keeper and a closer each have a part to play.

## Alternatives considered

### A separate environment and a host player

An environment like Social Deduction's announcing one, which never calls a
model, and a host player beside it. Rejected: the environment would ask the
host for every answer and wait on it, two actors doing one job, with the
judgment of when the game is won split between them.

### One `answer` tool with a `correct` flag

Tried by hand: the model marked *Is it found in a kitchen?* correct for a
teapot and answered *No* to *Is it a teapot?*. Two tools, one of them for
naming, are judged far better.

### Answering in turns

Asking players one at a time would stop guesses crossing, but Twenty
Questions, like Social Deduction's talk, has no turns: the rules leave
coordination to the players, and the player's policy helps them.

## Consequences

- An episode ends only through the host: it logs the end, sends the winner
  and stops every player. A host that fails stops everyone and fails the
  episode.
- A host's model call in flight at the time limit holds the end until it
  returns, by up to the request timeout.
- The example's three players and the host fill a local server's four
  parallel slots; more players take turns at them, and a server that
  answers one request at a time, as Ollama does by default, makes every
  actor wait for the others.

## Deliberately deferred

1. Playing many episodes, checking models and writing out the examples
   (#461).
2. Tuning the prompts against real models (#466).
3. Factoring the prompted player, the log writer and the story shared with
   Social Deduction's copies.
