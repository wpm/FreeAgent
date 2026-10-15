# Social deduction viewer

The viewer shows one Werewolf or Avalon game in God mode: a group chat
beside a live board, with evil players speaking from the left, good players
from the right, and the narrator in the center. Votes and selections flash
onto the board as they land. It is a [Leptos](https://leptos.dev) page built
on [Rich-Chat](https://github.com/wpm/Rich-Chat), drawn from the cues a
game's log folds into. Werewolf's window is dark by night and light by day.
Every push to `main` deploys a freshly played random game of each,
[Avalon](https://wpm.github.io/FreeAgent/viewer/#avalon) and
[Werewolf](https://wpm.github.io/FreeAgent/viewer/#werewolf).

## Watching a game locally

With the `wasm32-unknown-unknown` target and [Trunk](https://trunkrs.dev)
installed, from the root of a checkout:

```sh
cargo build -p social-deduction --bin social-deduction --example stage
cargo run -q -p social-deduction -- play apps/social-deduction/configs/avalon/random.toml 2> avalon.jsonl > /dev/null
cargo run -q -p social-deduction --example stage < avalon.jsonl > apps/social-deduction-gui/game/avalon.json
cargo run -q -p social-deduction -- play apps/social-deduction/configs/werewolf/random.toml 2> werewolf.jsonl > /dev/null
cargo run -q -p social-deduction --example stage < werewolf.jsonl > apps/social-deduction-gui/game/werewolf.json
for game in avalon werewolf; do
  cargo run -q -p social-deduction --example stage < apps/social-deduction-gui/fixtures/$game-prompted.jsonl > apps/social-deduction-gui/game/$game-prompted.json
done
cd apps/social-deduction-gui
trunk serve
```

The page opens on the finished Avalon game, and `#werewolf` opens the
Werewolf one. **Play** replays it from the start, a
cue every quarter second, and a hash ending in `-fast` replays ten times
as fast.

## The desktop app

The desktop app plays a game live. Its settings screen is the
configuration file as a form: the players, the phase limits, the
strategies, who plays what, the models and the text blocks, with a **TOML**
tab over the whole file. **Open** and **Save** read and write `.toml`
files, and keep their comments and order. Pick a preset or open a file,
press **Run**, and watch the game as it happens, with **Stop**. **Run**
prepares the file as `social-deduction play` does, and an error that
names a line offers to show it in the TOML tab. The configuration last run
is kept as `last.toml` in the app's config directory and opens at the
start. It is a
[Tauri](https://tauri.app) shell, in `src-tauri`, around the same page. To
run it in development, with Trunk and the Tauri CLI
(`cargo install tauri-cli --locked`) installed:

```sh
cd apps/social-deduction-gui/src-tauri
cargo tauri dev
```

On Linux, Tauri needs the WebKitGTK development packages:
`libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libayatana-appindicator3-dev`.

Each game's log is saved as JSON Lines in the app's data directory, under
`games/`, named for when the game started (in UTC) and which game it is:
on macOS, `~/Library/Application Support/io.github.wpm.social-deduction/games/`.
**Show log file** reveals it once the game is over.

A model's API key comes from the OS keychain, then from the environment
variable that names it, such as `OPENAI_API_KEY`. The Keys section lists
each variable the models need and where its key is, and stores or removes
it in the keychain. Each key is a keychain entry whose service is
`io.github.wpm.social-deduction` and whose account is the variable's name.
A model's **Check** asks it to play, as `social-deduction models --check`
does.

## The screen check

`fixtures/` holds the logs of two prompted games played against a local
model, with talk, thinking and Werewolf phases closing while players
finish speaking, which random players never produce. `#avalon-prompted` and `#werewolf-prompted` open them. CI's Viewer
job builds the page with `build-page.sh` and drives it in headless
Chromium with `e2e/check.mjs`, which checks what each game shows and saves
screenshots. To run it locally, from the root of a checkout:

```sh
apps/social-deduction-gui/build-page.sh
cd apps/social-deduction-gui/e2e
npm ci
npx playwright install chromium
npm test
```
