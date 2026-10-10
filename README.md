# Free Agent

[![CI](https://github.com/wpm/FreeAgent/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/wpm/FreeAgent/actions/workflows/ci.yml?query=branch%3Amain)
[![codecov](https://codecov.io/gh/wpm/FreeAgent/graph/badge.svg)](https://codecov.io/gh/wpm/FreeAgent)

Episodes in which actors talk to each other by statement, request, and reply.

An **episode** brings a set of actors into being together, decides which of
them can reach which, and runs them on the [Tokio](https://tokio.rs) runtime
until every one of them has stopped. An **actor** is one in the sense of the
[actor model](https://en.wikipedia.org/wiki/Actor_model): it keeps its own
state, takes messages from a mailbox one at a time, and reaches other actors
only by sending them messages. Here it is a [`Behavior`] driven by a mailbox:
the behavior handles statements with [`receive`](Behavior::receive)
and requests with [`answer`](Behavior::answer), and its [`Context`] is how it
reaches the rest of the episode, to send statements, to request and await
replies, to stop other actors, and to log. Everything an actor sends goes to
other actors, never to itself. A typical episode has one environment actor
and several agent actors.

The library lives at the root of the repository and the applications built on
it under `apps/`. For games built on this, see [`social-deduction`](apps/social-deduction).

## Development

```sh
cargo test                                   # unit tests and doctests
cargo doc --no-deps --open                   # the API docs
cargo fmt --check                            # what CI checks, besides the above
cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
cargo llvm-cov --workspace                   # test coverage, which CI reports to Codecov
```
