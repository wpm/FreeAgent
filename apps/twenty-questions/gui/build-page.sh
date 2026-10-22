#!/bin/sh
# Build the web page: fold each recorded game in fixtures/ into the cues
# the page opens, game/<name>.json, and build the page around them in
# apps/twenty-questions/gui/dist, to be served from the public URL given,
# `/` by default. The website deploys it at
# /FreeAgent/twenty-questions/viewer/. Run from the root of a checkout:
#
#   apps/twenty-questions/gui/build-page.sh [public-url]
set -eu
public_url=${1:-/}
cargo build --quiet --release -p twenty-questions --example stage
for log in apps/twenty-questions/gui/fixtures/*.jsonl; do
  name=$(basename "$log" .jsonl)
  cargo run --quiet --release -p twenty-questions --example stage < "$log" > "apps/twenty-questions/gui/game/$name.json"
done
cd apps/twenty-questions/gui
trunk build --release --public-url "$public_url"
