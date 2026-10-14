# Social deduction viewer

The viewer shows one Werewolf or Avalon game in God mode: a group chat
beside a live board, with evil players speaking from the left, good players
from the right, and the narrator in the center. Votes and selections flash
onto the board as they land. It is a [Leptos](https://leptos.dev) page built
on [Rich-Chat](https://github.com/wpm/Rich-Chat), drawn from the cues a
game's log folds into. Every push to `main` deploys a freshly played random
game of Avalon to <https://wpm.github.io/FreeAgent/viewer/>.

## Watching a game locally

With the `wasm32-unknown-unknown` target and [Trunk](https://trunkrs.dev)
installed, from the root of a checkout:

```sh
cargo build -p social-deduction --bin social-deduction --example stage
cargo run -q -p social-deduction -- play apps/social-deduction/configs/avalon/random.toml 2> game.jsonl > /dev/null
cargo run -q -p social-deduction --example stage < game.jsonl > apps/social-deduction-gui/cues.json
cd apps/social-deduction-gui
trunk serve
```

The page opens on the finished game. **Play** replays it from the start, a
cue every quarter second.
