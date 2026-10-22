# Free Agent

[![CI](https://github.com/wpm/FreeAgent/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/wpm/FreeAgent/actions/workflows/ci.yml?query=branch%3Amain)
[![codecov](https://codecov.io/gh/wpm/FreeAgent/graph/badge.svg)](https://codecov.io/gh/wpm/FreeAgent)

Episodes in which actors talk to each other by sending messages.

An **episode** brings a set of actors into being together, decides which of
them can reach which, and runs them on the [Tokio](https://tokio.rs) runtime
until every one of them has stopped. An **actor** is one in the sense of the
[actor model](https://en.wikipedia.org/wiki/Actor_model): it keeps its own
state, takes messages from a mailbox one at a time, and reaches other actors
only by sending them messages. An actor perceives, thinks, and acts. It
perceives in its message loop, a [`Behavior`] driven by a mailbox, which
hands each message to [`receive`](Behavior::receive). It thinks in an optional second loop, a
[`Think`] in a task of its own that takes what [`Context::think`] hands it
one message at a time, so slow work such as a model call never holds up
perceiving. A timer set with [`Context::think_after`] is a delayed thought,
handled when it falls due ahead of whatever is waiting to be thought about.
It acts through the [`Context`] each loop holds, which is how it reaches the
rest of the episode, to send messages, to stop other actors, and to log. Everything an actor sends goes to
other actors, never to itself. A typical episode has one environment actor
and several agent actors.

The library lives at the root of the repository and the applications built on
it under `apps/`, one folder each. An application's folder holds whichever of
these it has: `cli/`, its command line; `gui/`, its web frontend, built for
WASM; `gui/src-tauri/`, its desktop shell; and `gui-types/`, the types and
helpers its native code shares with its frontend, which build for WASM too.
The workspace picks these up by their folders. Crates shared by more than one
application go in `crates/`.

For games built on this, see [`social-deduction`](apps/social-deduction/cli)
and [`twenty-questions`](apps/twenty-questions/cli). Each is a desktop app and
a command line, installed together from its page on the
[website](https://wpm.github.io/FreeAgent/#apps), and each README's Servers
and CI section installs the command line alone.

## Development

```sh
cargo test                                   # unit tests and doctests
cargo doc --no-deps --open                   # the API docs
cargo fmt --check                            # what CI checks, besides the above
cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
cargo llvm-cov --workspace                   # test coverage, which CI reports to Codecov
```
