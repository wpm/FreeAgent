# Twenty Questions

Twenty Questions played in a real-time group chat with no turn-taking. Several **Players** share a budget of yes-or-no questions; one **Host** knows the secret. All speech is broadcast — nobody is addressed privately. The Players deliberate openly about which question is worth spending next rather than burning through their twenty, and the Host has to tell the difference between the Players talking among themselves and actually asking or guessing. The episode ends with a correct guess (a win), the budget running out (a loss), or a timeout; the Host announces the result on the public channel, and that announcement is the episode's outcome record.

## The agents

The split throughout is judgment versus bookkeeping: the LLM makes the judgment calls, and plain logic handles anything countable.

- **Host** — knows the secret, decides whether each thing said was a question, a guess, or just deliberation, and judges whether a guess is correct. The question budget, the win, and the loss are tracked mechanically, and the Host makes the final announcement.
- **Players** — defined almost entirely by their instructions rather than by code: how they deliberate, when they spend a question, when they guess, and how they sign off all come from what they are told, not from special-case logic.
- **The environment** — holds no game state. It ends the episode when the Host signals the game is over, and its wind-down grace period is what lets the Players say their goodbyes before everything stops.

## Running it

From the repository root, with NATS up:

```sh
# Scripted game with the deterministic fake LLM -- no network, no keys.
uv run free-agent twenty-questions run examples/twentyquestions-fake.yml

# A real game (requires ANTHROPIC_API_KEY / OPENAI_API_KEY / GEMINI_API_KEY,
# or FREEAGENT_MODEL set to a litellm model string).
uv run free-agent twenty-questions run examples/twentyquestions.yml
```

Add `--parquet-log PATH` (a path that must not already exist) to record the episode's full message log to a Parquet file:

```sh
uv run free-agent twenty-questions run examples/twentyquestions-fake.yml \
  --parquet-log out/twentyquestions-fake.parquet
```

The fake game plays a complete, scripted two-question win — "an octopus" — followed by goodbyes; it is the same game the repository's end-to-end test runs.
