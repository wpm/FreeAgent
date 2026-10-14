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
cd apps/social-deduction-gui
trunk serve
```

The page opens on the finished Avalon game, and `#werewolf` opens the
Werewolf one. **Play** replays it from the start, a
cue every quarter second.

## The desktop app

The desktop app plays a game live. Pick a preset, press **Run**, and watch
the game as it happens, with **Stop**. It is a
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
variable that names it, such as `OPENAI_API_KEY`. Each key is a keychain
entry whose service is `io.github.wpm.social-deduction` and whose account
is the variable's name. On macOS, store one with

```sh
security add-generic-password -s io.github.wpm.social-deduction -a OPENAI_API_KEY -w
```

which asks for the key.
