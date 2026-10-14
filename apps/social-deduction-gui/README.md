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
cargo run -q -p social-deduction --example stage < avalon.jsonl > apps/social-deduction-gui/avalon.json
cargo run -q -p social-deduction -- play apps/social-deduction/configs/werewolf/random.toml 2> werewolf.jsonl > /dev/null
cargo run -q -p social-deduction --example stage < werewolf.jsonl > apps/social-deduction-gui/werewolf.json
cd apps/social-deduction-gui
trunk serve
```

The page opens on the finished Avalon game, and `#werewolf` opens the
Werewolf one. **Play** replays it from the start, a
cue every quarter second.
