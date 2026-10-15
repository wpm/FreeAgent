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

The desktop app plays a game live. Its main window opens on a start
state: choose **Werewolf** or **Avalon**, read the line that sums up its
settings, and press **Play**. A game whose settings have an error names
the first one under **Play**, linked to its field in Settings. The game
plays with **Stop**, and once it is over, **New game** and **Show log
file** appear. **File ▸ New Game** (⌘N, Ctrl+N elsewhere) returns to the
start state. The game last played is chosen at launch.

**Settings…** (⌘, on macOS, **Edit ▸ Preferences…** and Ctrl+, elsewhere)
opens the Settings window. Every change is stored as it is made and
applies to the next game. Each field's error shows in red beneath it, and
hovering over anything, or focusing it, shows what it is and what changing
it does.

- **General**: the folder game logs are saved in, with **Change…** and
  **Show in Finder** (**Show in Explorer** elsewhere), and the
  appearance: System, Light or Dark. System keeps Werewolf dark by night
  and light by day.
- **Models**: a card for each provider, holding its base URL and key,
  **Your models** from it, and **Available**: its models that play and
  aren't added yet, most capable first, each with its context window and
  **Add**. Adding one asks for its name, filled in with the model id. A
  name is edited in place, and renaming updates the strategies that use
  it. A context window left empty is the provider's figure; a number
  typed overrides it, and **Reset** goes back. A model that stops playing
  stays under **Your models** with "No longer plays" and the strategies
  that use it. The key line names the variable and where its key comes
  from, the environment or the keychain, which takes priority, and stores
  or removes it there. **Add provider…** adds any other OpenAI-compatible
  service by name, base URL and key variable. Hovering over or focusing
  anything shows what it does.
- **Werewolf** and **Avalon**: the role counts beside the phase limits
  (`45s`, `2m`), the strategies two across, who plays what, with **Split
  seats…** to share a role among strategies, and the prompt parts: named
  pieces of text a system prompt pulls in with `{{ name }}`, each listed
  with how many strategies use it. On a narrow window it is one column.
  **Restore Defaults…** brings back the game's example `prompted`
  configuration, leaving the models as they are.

The app looks for a local server at `http://localhost:<port>/v1` on ports
1234, 11434, 8080 and 8000 (LM Studio's, Ollama's, llama.cpp's and
vLLM's), and each one that answers is a provider, **Local
(localhost:1234)**. Models are checked in the background with the probe
`social-deduction models --check` runs: every model at the first launch
and after each update, a model a provider lists for the first time, and a
provider's models again when its key changes. A remote provider's are
checked four at a time; a local server's one at a time, since it loads
each model it is asked about, and never while a game plays. A line at the
top of the Models tab says what is being checked. A model that can't be
asked, as when the provider doesn't accept the key, is asked again when
Settings next opens, and its provider's card says why in red, the
provider's own words in its help. When the account is the reason, such as
no credits left, the provider's other models wait for the next try too. A
provider none of whose models play says why most of them didn't, and its
help gives each model's reason. A provider's models are
ranked by the families in `src/models.toml`, newer first within a family,
then by the parameter count in the id, then newest first.

**File ▸ Import Configuration…** reads a configuration file. Its game's
settings replace that game's, and its models join the shared ones; a model
whose name is taken by another is added as `<name>-2`, with the
strategies that ask it to match. **File ▸ Export Configuration…** writes a
game's configuration file, with its models, and `social-deduction play`
plays it as **Play** does.

The settings are kept in `settings.toml` in the app's config directory,
and what probing found in `probes.json` in its data directory: on macOS,
both in `~/Library/Application Support/io.github.wpm.social-deduction/`.
At the first launch, a `last.toml` an earlier version kept there is
imported into its game.

It is a [Tauri](https://tauri.app) shell, in `src-tauri`, around the same
page. To run it in development, with Trunk and the Tauri CLI
(`cargo install tauri-cli --locked`) installed:

```sh
cd apps/social-deduction-gui/src-tauri
cargo tauri dev
```

On Linux, Tauri needs the WebKitGTK development packages:
`libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libayatana-appindicator3-dev`.

Each game's log is saved as JSON Lines in the logs folder, by default
`games/` in the app's data directory, named for when the game started (in
UTC) and which game it is. **Show log file** reveals it once the game is
over.

A model's API key comes from the OS keychain, then from the environment
variable that names it, such as `OPENAI_API_KEY`. Each key is a keychain
entry whose service is `io.github.wpm.social-deduction` and whose account
is the variable's name.

## The screen check

`fixtures/` holds the logs of two prompted games played against a local
model, with talk, thinking and late speech, which random players never
produce. `#avalon-prompted` and `#werewolf-prompted` open them. CI's Viewer
job builds the page with `build-page.sh` and drives it in headless
Chromium with `e2e/check.mjs`, which checks what each game shows, opens
the desktop app's start state and Settings window against a stand-in
backend, and saves screenshots. To run it locally, from the root of a checkout:

```sh
apps/social-deduction-gui/build-page.sh
cd apps/social-deduction-gui/e2e
npm ci
npx playwright install chromium
npm test
```
