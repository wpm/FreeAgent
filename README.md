# Free Agent

[![CI](https://github.com/wpm/FreeAgent/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/wpm/FreeAgent/actions/workflows/ci.yml?query=branch%3Amain)
[![codecov](https://codecov.io/gh/wpm/FreeAgent/graph/badge.svg)](https://codecov.io/gh/wpm/FreeAgent)

Episodes in which actors talk to each other by broadcast, request, and reply.

An **episode** is the world in which everything here takes place. It brings a
set of actors into being together, decides which of them can reach which and
which can stop which, and runs them on the [Tokio](https://tokio.rs) runtime
until every one of them has stopped or its patience runs out. No actor starts
until every actor is ready, so an opening move never lands on an actor that
is still initializing.

Within an episode, an **actor** is a [`Behavior`] driven by an inbox, with a
[`Lifecycle`] around it for setup and teardown. The behavior's one method is
the reinforcement-learning one, [`Behavior::policy`]: an observation in,
actions out. What it may do besides answer goes through its [`Context`]:
broadcast a message to the actors it may send to, ask them a question through
[`Context::request`] and collect their replies, leave a note in its own inbox,
stop an actor it may stop, shut itself down, or log an [`Event`].

Logging is out of band. The episode is given one [`Logger`], each actor that
wants one holds a copy, and the other end is drained by [`drain`] or
[`console_log`] once the episode is over.

The library lives at the root of the repository and the applications built on
it under `apps/`. For a game built on this, see [`werewolf`](apps/werewolf).

## Development

```sh
cargo test                                   # unit tests and doctests
cargo doc --no-deps --open                   # the API docs
cargo fmt --check                            # what CI checks, besides the above
cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
cargo llvm-cov --workspace                   # test coverage, which CI reports to Codecov
```

[`Behavior`]: Behavior
[`Behavior::policy`]: Behavior::policy
[`Lifecycle`]: Lifecycle
[`Context`]: Context
[`Context::request`]: Context::request
[`Event`]: Event
[`Logger`]: Logger
[`drain`]: drain
[`console_log`]: console_log
