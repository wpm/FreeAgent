#!/bin/sh
# Build the page the screen check drives: fold the recorded prompted game
# in fixtures/ into the cues the stand-in backend plays, in
# e2e/prompted.json, and build the page in apps/twenty-questions/gui/dist.
# Run from the root of a checkout.
set -eu
cargo run --quiet --release -p twenty-questions --example stage < apps/twenty-questions/gui/fixtures/prompted.jsonl > apps/twenty-questions/gui/e2e/prompted.json
cd apps/twenty-questions/gui
trunk build --release
