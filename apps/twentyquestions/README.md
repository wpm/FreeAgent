# Twenty Questions

A sample FreeAgent game in which a team of AI **Players** tries to guess a
secret that an AI **Host** is keeping — the classic parlour game of Twenty
Questions, but played by language-model agents in a live group chat.

## How the game works

The Host thinks of a secret thing. The Players have to identify it by asking
yes-or-no questions, and they share a single budget of **twenty questions**
between them all.

What makes this version different from the schoolyard game is that everyone is
in one **real-time chat room with no turn-taking**. Anyone can speak at any
moment, nobody is addressed privately, and staying silent is itself a move. So
the Players don't just fire off questions — they talk among themselves first,
proposing a question, refining it, and agreeing it's worth spending before they
commit. The Host listens to all of it and has to tell the difference between
the Players merely thinking out loud and actually putting a question or a guess
to them.

### The rules

- **The budget is shared and scarce.** All the Players draw from the same pool
  of twenty yes-or-no questions. Only a sentence *addressed to the Host* —
  "Host, is it bigger than a person?" — spends one. Chatter among the Players
  costs nothing.
- **The Host answers truthfully.** Every question gets a yes or no and a
  running count ("That was question 4 of your 20."). The Host never lies, never
  reveals the secret, and never volunteers hints.
- **A guess is just a question addressed to the Host that names the thing.**
  Close wording and synonyms count, so "is it an octopus?" wins if the secret
  is *an octopus*.
- **The game ends three ways:** the Players name the secret (a **win**), they
  run out of questions (a **loss**), or the episode times out. The Host's
  public game-over announcement is the official record of the outcome.

For the engine internals — the agents, the prompts, and how the Host and
Players are built — see the [engine README](engine/README.md).

## Running a game

There are two pieces:

- The **engine** plays a game and broadcasts every message. This is all you
  need to run a game and see its result.
- The **viewer** is an optional web app that renders a game as a live chat
  transcript in your browser, so you can watch it unfold.

### From the command line (the engine)

From the repository root, with NATS running:

```sh
# A scripted demo game using a deterministic fake model -- no network, no keys.
uv run free-agent twenty-questions run examples/twentyquestions-fake.yml

# A real game (needs an API key -- ANTHROPIC_API_KEY, OPENAI_API_KEY, or
# GEMINI_API_KEY -- or FREEAGENT_MODEL set to a litellm model string).
uv run free-agent twenty-questions run examples/twentyquestions.yml
```

The demo plays out a complete two-question win — the secret is "an octopus" —
followed by the Players saying their goodbyes. Add `--parquet-log PATH` (to a
file that doesn't yet exist) to save the full transcript for replaying later.

### Watching one game in the browser (the viewer)

Most people want to *watch* a single game from start to finish. Open three
terminals from the repository root:

```sh
# Terminal 1 -- start NATS (the message bus the engine and viewer share):
docker compose -f docker/nats/docker-compose.yml up -d

# Terminal 2 -- run one game. It prints "episode_id=<id>" when it finishes;
# set "episode_id:" in the YAML beforehand if you want to know the id up front.
uv run free-agent twenty-questions run examples/twentyquestions-fake.yml

# Terminal 3 -- start the viewer and open it pointed at that episode:
pnpm --filter twentyquestions-viewer run dev
#   then open http://localhost:5173/?episode=<id>
```

The viewer is read-only — it just watches — and reconnects on its own, so you
can open it *before* the game starts and it will light up the moment the
episode begins. The transcript shows the whole conversation: the Players
deliberating, the questions they spend, the Host's answers, and finally the
Host's game-over announcement. Because it always reads the episode from the
beginning, you see the complete game even if you join late.

You can also watch a **recorded** game: run with `--parquet-log` to save it,
then replay the file onto NATS and point the viewer at it — the replay looks
exactly like the live game. See the [viewer README](viewer/README.md) for the
full set of options, including how to share a game by URL.
