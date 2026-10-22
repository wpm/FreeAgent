# ADR-0012: Applications share crates; each game is its own

**Status:** Accepted
**Date:** 2026-10-21
**Deciders:** Bill McNeill
**Depends on:** [ADR-0010](0010-avalon-is-a-second-game-built-by-copying-werewolf.md)

## Context

Free Agent has one application, Social Deduction, in
`apps/social-deduction`: a command line, a desktop app, a web frontend
and the types the frontend shares with the native code. It plays two
games, Werewolf and Avalon, built by the rule of
[ADR-0010](0010-avalon-is-a-second-game-built-by-copying-werewolf.md):
whatever compiles unchanged for both games moves to a module they share,
and whatever needs an edit is copied.

The second application is Twenty Questions, in `apps/twenty-questions`. A
host holds a secret object and model players put guesses to it. Its
players are language models, as Social Deduction's are, so it reaches,
lists, probes and ranks models the same way, keeps API keys the same way,
and has the same Settings window and Models page. Its game, its messages,
its log and its screen are its own.

A great deal of what Social Deduction has would compile unchanged in
Twenty Questions: thousands of lines of model and settings code, and the
look of the desktop app. Two copies of it would drift apart, and the two
apps would come to look different.

## Decision

**ADR-0010's rule applies one level up, from games to applications. An
item that would compile unchanged in both applications moves to a crate
under `crates/`, and both import it from there. An item that needs any
edit is copied into the new application and edited.** Nothing in
`apps/twenty-questions` imports from `apps/social-deduction`, or the
other way round.

### The shared crates

Five crates, each `publish = false`, each a member of the workspace:

| Crate | Folder | Builds for | Holds |
|---|---|---|---|
| `free-agent-model` | `crates/model` | native | reaching, listing, probing and ranking models; API keys |
| `free-agent-game` | `crates/game` | native | seats, strategies, example configurations, log redaction, forgetting talk |
| `free-agent-gui-types` | `crates/gui-types` | native and WASM | what the shared native code and the shared frontend send each other |
| `free-agent-gui` | `crates/gui` | WASM | the shared look: tokens, chrome, Settings window, Models page |
| `free-agent-desktop` | `crates/desktop` | native, with Tauri | the shared desktop shell: keychain, model catalog, updater, command-line tool, menus, dialogs |

The workspace's members pick up every crate under `crates/`. Its default
members name the native ones, since `crates/gui` builds for the browser
and `crates/desktop` needs the system's webview, as an application's
frontend and desktop shell do.

### What stays in each game

Everything typed on a game's own messages, log entries, views or rules:
the actors, the prompted player, the narrator, the log's writer and
reader, the cues, the board. These are copied into Twenty Questions and
edited, so that comparing the two applications file by file shows the
seams for a later refactoring.

### The probe is the app's

A model is probed by asking it to play, and what it is asked to play is
the app's. The shared crate runs the probe the app gives it: Social
Deduction's asks a model to play Werewolf and then Avalon, and Twenty
Questions' will ask it to play Twenty Questions. The shared crate gives
every probe the two pieces it is built from: settling the tool choice a
model is asked under, and saying what came of the requests, naming the
game.

### Test support

The shared crates' fakes, such as the fake model and the canned providers
a test talks to, sit behind a `test-support` feature that each app turns
on in its `[dev-dependencies]`. A canned provider that plays answers as a
canned player the app passes it, since what a player answers is the
app's.

## Alternatives considered

### Copy everything into Twenty Questions

Twenty Questions would then be independent of Social Deduction with no
move at all. It would also carry thousands of lines of model and settings
code that would compile unchanged, and two looks that would drift apart.
Rejected.

### A generic "talking game" framework first

A framework for any game whose players talk would let both applications
share their actors, players and logs. With one application, its shape
would be a guess. Rejected for the reason ADR-0010 gave: two working games
show the seams; one game and a guess do not.

### Import from `apps/social-deduction`

Twenty Questions could use Social Deduction's general items where they
are. That would make one application depend on another, and the
dependency would have to be undone later. Rejected.

## Consequences

- Social Deduction plays, logs, probes and looks as before, on the shared
  crates.
- Each shared crate is held to the workspace's coverage, with its tests
  beside it.
- The shared crates are never published and are never stamped with a
  release's version. An app passes what needs its version, such as the
  version its stored probes were found by.
- Each change to an item one application copied from the other has to be
  weighed for the other until they are merged.

## Deliberately deferred

1. Factoring the prompted player, the log writer and the cue machinery
   once both applications work.
2. Sharing API keys and models between the two apps.

## Notes

- **2026-10-22:** Twenty Questions' probe asks a model to play Twenty
  Questions. The coverage CI holds is the native shared crates':
  `free-agent-model`, `free-agent-game` and `free-agent-gui-types`.
  `free-agent-gui` and `free-agent-desktop` are tested in CI's Viewer and
  Desktop jobs instead, as each app's frontend and desktop shell are.
