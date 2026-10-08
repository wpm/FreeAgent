# Free Agent

[![CI](https://github.com/wpm/FreeAgent/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/wpm/FreeAgent/actions/workflows/ci.yml?query=branch%3Amain)
[![codecov](https://codecov.io/gh/wpm/FreeAgent/graph/badge.svg)](https://codecov.io/gh/wpm/FreeAgent)

Episodes in which actors talk to each other by broadcast, request, and reply.

An **episode** brings a set of actors into being together, decides which of
them can reach which, and runs them on the [Tokio](https://tokio.rs) runtime
until every one of them has stopped. An **actor** is a [`Behavior`] driven by
an inbox: the behavior's [`policy`](Behavior::policy) takes an observation in
and gives actions out, and its [`Context`] is how it reaches the rest of the
episode, to broadcast, to request and await replies, to stop other actors, and
to log. The vocabulary is reinforcement learning's, and a typical episode has
one environment actor and several agent actors.

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
