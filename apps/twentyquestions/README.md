# twentyquestions

The FreeAgent sample application: Twenty Questions played in a real-time group chat with no turn-taking. Several **Players** share a budget of yes-or-no questions; one **Host** knows the secret. All speech is broadcast — there is no directed speech. Players deliberate openly among themselves to choose the right next question rather than burning through their twenty, and it is the Host's job to know when Players are talking among themselves instead of asking or guessing. The episode ends with a correct guess (win), the budget exhausted (loss), or the episode timeout; the Host's GAME OVER announcement on the public channel is the episode's outcome record in the log.

## The agents

Everything judgment-shaped is the LLM's call; everything countable is code (stochastic vs. programmatic, per DESIGN.md):

- **`Host`** — an `LLMAgent` whose structured decision schema adds a classification (`question` / `guess` / `deliberation` / `other`) and a `guess_correct` verdict to the base speak-or-stay-silent decision. The LLM classifies and judges; code counts the questions, detects the win and the loss, makes the announcement, and signals the environment's inbox with `{"type": "game_over", ...}`. Config: `secret` (default: random from a canned list) and `max_questions` (default 20).
- **`Player`** — the base `LLMAgent` plus a game-specific default system prompt, and *nothing more*: deliberating, addressing the Host to spend a question, guessing, hearing the announcement, and saying goodbye all live in the prompt, not in code. This is the prompts-over-code goal: a basic application defined almost entirely by text.
- **`TwentyQuestionsEnvironment`** — the base environment plus one rule: on the Host's `game_over` inbox signal, initiate shutdown. It holds no game state. The goodbyes happen inside the stopping grace period — closeout timing is the framework's job, not an LLM judgment.

Default prompts live in `prompts.py`; a per-agent `system_prompt` in the episode config overrides them.

## The CLI

This app provides its own command, mounted under the shared `free-agent` root through the `freeagent.apps` entry-point group (`twenty-questions = "twentyquestions.cli:app"`). The app name, the environment class, and the roster (one Host plus the players `alice`, `bob`, `carol`) are defined in `cli.py` — in source, not in configuration. The YAML you pass to `run` carries only per-episode tunables (NATS URL, recorder, episode id, and each component's `config` block); it names no classes. Add a `run` flag or a whole new command by editing `cli.py`.

## Running it

From the repository root, with NATS up:

```sh
# Scripted game with the deterministic fake LLM -- no network, no keys.
# (The fake: model paths in the config are relative to the cwd.)
uv run free-agent twenty-questions run examples/twentyquestions-fake.yml

# A real game (requires ANTHROPIC_API_KEY / OPENAI_API_KEY / GEMINI_API_KEY,
# or FREEAGENT_MODEL set to a litellm model string).
uv run free-agent twenty-questions run examples/twentyquestions.yml
```

Both write the full episode log to Parquet under `out/`. The fake game's canned scripts (`examples/fake/*.yml`) play a complete two-question win — "an octopus" — followed by goodbyes; it is the same configuration the repository's end-to-end test runs.
