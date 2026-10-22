# Twenty Questions, the desktop app

Language models put twenty yes-or-no questions to a host who is thinking
of something. Press **Play** and watch the players talk, agree and guess
in a group chat, with the secret object and the guesses used at the top.

Color means who is speaking. Each player, Alice to Leo, speaks from the
left in a bubble of its own color, with its name and a square of that
color over it. The host speaks from the right in ink, and the narrator,
centered in italics, tells the start and the end. A guess is its player's
bubble with a border in the player's color, tagged **Guess 7**; the host's
answer is tagged **Answer 7** and quotes the guess it answers. The answer
that names the object, and its guess, are ringed.

The header holds the secret object for the whole game, and at its right
the guesses used, `7 / 20`, over a pip for each guess: ink once used,
pulsing while it waits for its answer, and in the guesser's color for the
one that won. The countdown is under them. Once the game is over, the
header says who won, with **New game**; **File ▸ New Game** (⌘N, Ctrl+N
elsewhere) goes back to the start from anywhere.

**Settings** (⌘,) holds the look, the models and the game: how many
play, the host's model, the secret object (empty, the host picks one) and
the strategy the players play. The settings are kept in `settings.toml`
in the app's config directory, and what probing found in `probes.json` in
its data directory: on macOS, both in
`~/Library/Application Support/io.github.wpm.twenty-questions/`. A model's
API key comes from the OS keychain, under the service
`io.github.wpm.twenty-questions`, then from the environment variable that
names it.

## Development

The page is Leptos, built by Trunk on port 1421, and the shell is
[Tauri](https://tauri.app), in `src-tauri`. Its bundle carries the
`twenty-questions` command line as a sidecar, which
`.github/scripts/sidecar.sh twenty-questions` builds into
`src-tauri/binaries/`. With Trunk and the Tauri CLI
(`cargo install tauri-cli --locked`) installed, from the root of a
checkout:

```sh
.github/scripts/sidecar.sh twenty-questions
cd apps/twenty-questions/gui/src-tauri
cargo tauri dev
```

The icons are made from `src-tauri/icon.svg` with `cargo tauri icon
icon.svg`, run in `src-tauri`; the page's mark is `src-tauri/mark.svg`.

`fixtures/prompted.jsonl` is a game played against LM Studio. CI's Viewer
job builds the page with `build-page.sh` and drives it in headless
Chromium with `e2e/check.mjs`, which plays the recorded game through a
stand-in for the desktop app's backend at ten times its pace, checks the
start, the chat, the header and the Settings window, light and dark, wide
and 420 px narrow, and saves screenshots. From the root of a checkout:

```sh
apps/twenty-questions/gui/build-page.sh
cd apps/twenty-questions/gui/e2e
npm ci
npx playwright install chromium
npm test
```
