#!/bin/sh
# Build the page the website deploys at /viewer/: play a random game of
# Avalon, fold its log into cues, and build the viewer around them in
# apps/social-deduction-gui/dist. Run from the root of a checkout.
set -eu
log=$(mktemp)
trap 'rm -f "$log"' EXIT
# Built first, so that cargo's own messages stay out of the log on
# standard error.
cargo build --release -p social-deduction --bin social-deduction --example stage
cargo run --quiet --release -p social-deduction -- play apps/social-deduction/configs/avalon/random.toml 2> "$log" > /dev/null
cargo run --quiet --release -p social-deduction --example stage < "$log" > apps/social-deduction-gui/cues.json
cd apps/social-deduction-gui
trunk build --release --public-url /FreeAgent/viewer/
