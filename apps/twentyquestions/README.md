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
uv run free-agent twenty-questions run apps/twentyquestions/examples/twentyquestions-fake.yml

# A real game (needs an API key -- ANTHROPIC_API_KEY, OPENAI_API_KEY, or
# GEMINI_API_KEY -- or FREEAGENT_MODEL set to a litellm model string).
uv run free-agent twenty-questions run apps/twentyquestions/examples/twentyquestions.yml
```

The demo plays out a complete two-question win — the secret is "an octopus" —
followed by the Players saying their goodbyes. Add `--parquet-log PATH` (to a
file that doesn't yet exist) to save the full transcript for replaying later.

### Watching one game in the browser (the viewer)

The viewer talks **only to the episode service** — never NATS directly
(ADR-0003/0004) — so watching a game means running the **backend** (the service
+ NATS) and then the **viewer** as a separate host process. Open two terminals
from the repository root:

```sh
# Terminal 1 -- bring up the backend (NATS + the episode service):
docker compose -f docker/compose.yml up --build       # REST API on :8000

# Terminal 2 -- start the viewer (a separate host process):
pnpm install
pnpm --filter twentyquestions-viewer run dev          # http://localhost:5173
```

In the browser, the viewer talks to the service at `http://localhost:8000` by
default (change it in **Settings**). Set your model and provider API key in
Settings (kept in the browser, never on the bus), then **start a game** from the
Twenty Questions composer and watch the transcript stream live.

The viewer is read-only once a game is running — it just watches — and reads each
episode from the beginning, so you see the complete game even if you join late:
the Players deliberating, the questions they spend, the Host's answers, and the
Host's game-over announcement. A sealed (finished) game **replays through the
exact same view**.

Prefer the command line? You can still run a game with the engine CLI (the
section above) against the same NATS the backend uses; it appears in the viewer's
episode list like any other. See the [viewer README](viewer/README.md) for the
full set of options, including how to share a game by URL.
