#!/bin/sh
# Build the page the website deploys at /viewer/: play a random game of
# Avalon and one of Werewolf, fold each log into cues, fold the recorded
# prompted games in fixtures/ too, and build the viewer around them in
# apps/social-deduction-gui/dist. Run from the root of a checkout.
set -eu
log=$(mktemp)
trap 'rm -f "$log"' EXIT
# Built first, so that cargo's own messages stay out of the log on
# standard error.
cargo build --release -p social-deduction --bin social-deduction --example stage
for game in avalon werewolf; do
  cargo run --quiet --release -p social-deduction -- play "apps/social-deduction/configs/$game/random.toml" 2> "$log" > /dev/null
  cargo run --quiet --release -p social-deduction --example stage < "$log" > "apps/social-deduction-gui/game/$game.json"
  cargo run --quiet --release -p social-deduction --example stage < "apps/social-deduction-gui/fixtures/$game-prompted.jsonl" > "apps/social-deduction-gui/game/$game-prompted.json"
done
cd apps/social-deduction-gui
trunk build --release --public-url /FreeAgent/viewer/
